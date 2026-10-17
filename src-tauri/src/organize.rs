//! Folder management, moving/copying mail, and rules (local + Sieve).

use neutron_core::{
    folders::{self, DeleteMode, Transfer},
    imap, managesieve,
    model::Folder,
    rules::{self, MessageFacts, Plan, Rule, SieveExport},
    rules_exec::{self, RunReport},
    store::SieveSettings,
    Error,
};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::{commands::Res, AppState};

async fn session(state: &AppState, account_id: i64) -> Res<(imap::Session, String)> {
    let account = state.store.account(account_id)?;
    let password = crate::providers::credential(state, account_id).await?;
    let s = imap::connect(&account.config, &password).await?;
    Ok((s, password))
}

async fn done(mut s: imap::Session) {
    drop(s.logout().await);
}

/// Re-sync in the background after a change (errors only logged).
fn resync_later(app: &AppHandle, account_id: i64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let r = crate::commands::run_sync(&app, &state, account_id).await;
        if let Err(e) = &r {
            tracing::warn!(account = account_id, "resync after change failed: {e}");
        }
        let (report, error) = match r {
            Ok(rep) => (Some(rep), None),
            Err(e) => (None, Some(e.to_string())),
        };
        crate::watcher::emit(
            &app,
            crate::watcher::SyncEvent {
                account_id,
                report,
                error,
            },
        );
    });
}

#[tauri::command]
pub async fn create_folder(
    state: State<'_, AppState>,
    account_id: i64,
    parent_id: Option<i64>,
    name: String,
) -> Res<Folder> {
    let (mut s, _) = session(&state, account_id).await?;
    let r = {
        let _g = state.account_sync_lock(account_id).lock_owned().await;
        folders::create(&state.store, &mut s, account_id, parent_id, name.trim()).await
    };
    done(s).await;
    let wire = r?;
    state
        .store
        .folders(account_id)?
        .into_iter()
        .find(|f| f.name == wire)
        .ok_or_else(|| Error::Invalid("folder created but not listed".into()))
}

/// Rename and/or move a folder. `to_root` moves it to the top level;
/// otherwise `new_parent_id` (if set) is the new parent.
#[tauri::command]
pub async fn rename_folder(
    state: State<'_, AppState>,
    folder_id: i64,
    name: Option<String>,
    new_parent_id: Option<i64>,
    to_root: bool,
) -> Res<Folder> {
    let f = state.store.folder(folder_id)?;
    let parent = if to_root {
        Some(None)
    } else {
        new_parent_id.map(Some)
    };
    let (mut s, _) = session(&state, f.account_id).await?;
    let r = {
        let _g = state.account_sync_lock(f.account_id).lock_owned().await;
        let leaf = name.as_deref().map(str::trim);
        folders::rename(&state.store, &mut s, folder_id, leaf, parent).await
    };
    done(s).await;
    r?;
    state.store.folder(folder_id)
}

#[tauri::command]
pub async fn delete_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    folder_id: i64,
    mode: DeleteMode,
) -> Res<usize> {
    let f = state.store.folder(folder_id)?;
    let (mut s, _) = session(&state, f.account_id).await?;
    let r = {
        let _g = state.account_sync_lock(f.account_id).lock_owned().await;
        folders::delete(&state.store, &mut s, folder_id, mode).await
    };
    done(s).await;
    if r.is_ok() && mode == DeleteMode::MoveToTrash {
        resync_later(&app, f.account_id);
    }
    r
}

#[tauri::command]
pub async fn set_folder_subscribed(
    state: State<'_, AppState>,
    folder_id: i64,
    subscribed: bool,
) -> Res<()> {
    let f = state.store.folder(folder_id)?;
    if !subscribed && folders::is_protected(&f) {
        return Err(Error::Invalid("special folders stay subscribed".into()));
    }
    let (mut s, _) = session(&state, f.account_id).await?;
    let r = imap::set_subscribed(&mut s, &f.name, subscribed).await;
    done(s).await;
    r?;
    state.store.set_folder_subscribed(folder_id, subscribed)
}

/// Move (or copy) messages to a folder. The result can be passed to
/// `undo_transfer`.
#[tauri::command]
pub async fn move_messages(
    app: AppHandle,
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
    target_folder_id: i64,
    copy: bool,
) -> Res<Transfer> {
    if message_ids.is_empty() || message_ids.len() > 5000 {
        return Err(Error::Invalid("select 1–5000 messages".into()));
    }
    let target = state.store.folder(target_folder_id)?;
    let (mut s, _) = session(&state, target.account_id).await?;
    let r = {
        let _g = state
            .account_sync_lock(target.account_id)
            .lock_owned()
            .await;
        folders::transfer(&state.store, &mut s, &message_ids, target_folder_id, copy).await
    };
    done(s).await;
    if r.is_ok() {
        resync_later(&app, target.account_id);
    }
    r
}

/// Star/unstar or mark read/unread, on the server and in the cache.
#[tauri::command]
pub async fn set_message_flag(
    app: AppHandle,
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
    flag: folders::MessageFlag,
    on: bool,
) -> Res<usize> {
    if message_ids.is_empty() || message_ids.len() > 5000 {
        return Err(Error::Invalid("select 1–5000 messages".into()));
    }
    let account_id = state.store.message_account(message_ids[0])?;
    let (mut s, _) = session(&state, account_id).await?;
    let r = {
        let _g = state.account_sync_lock(account_id).lock_owned().await;
        folders::set_flag(&state.store, &mut s, &message_ids, flag, on).await
    };
    done(s).await;
    let (_, n) = r?;
    resync_later(&app, account_id);
    Ok(n)
}

/// Delete messages for good (used in Trash, or with Shift+Delete). The UI
/// asks for confirmation first; this cannot be undone.
#[tauri::command]
pub async fn delete_messages(
    app: AppHandle,
    state: State<'_, AppState>,
    message_ids: Vec<i64>,
) -> Res<usize> {
    if message_ids.is_empty() || message_ids.len() > 5000 {
        return Err(Error::Invalid("select 1–5000 messages".into()));
    }
    let account_id = state.store.message_account(message_ids[0])?;
    let (mut s, _) = session(&state, account_id).await?;
    let r = {
        let _g = state.account_sync_lock(account_id).lock_owned().await;
        folders::delete_permanently(&state.store, &mut s, &message_ids).await
    };
    done(s).await;
    let (_, n) = r?;
    tracing::info!(
        account = account_id,
        count = n,
        "messages deleted permanently"
    );
    resync_later(&app, account_id);
    Ok(n)
}

#[tauri::command]
pub async fn undo_transfer(
    app: AppHandle,
    state: State<'_, AppState>,
    transfer: Transfer,
) -> Res<usize> {
    // The target must belong to the account named in the transfer.
    let target = state.store.folder(transfer.target_folder_id)?;
    if target.account_id != transfer.account_id || transfer.items.len() > 5000 {
        return Err(Error::Invalid("bad undo request".into()));
    }
    let (mut s, _) = session(&state, transfer.account_id).await?;
    let r = {
        let _g = state
            .account_sync_lock(transfer.account_id)
            .lock_owned()
            .await;
        folders::undo(&state.store, &mut s, &transfer).await
    };
    done(s).await;
    resync_later(&app, transfer.account_id);
    r
}

#[tauri::command]
pub fn list_rules(state: State<'_, AppState>) -> Res<Vec<Rule>> {
    state.store.rules()
}

#[tauri::command]
pub fn save_rules(state: State<'_, AppState>, rules: Vec<Rule>) -> Res<Vec<Rule>> {
    state.store.save_rules(&rules)
}

#[derive(Serialize)]
pub struct RuleTest {
    matched: bool,
    /// Per condition.
    conditions: Vec<bool>,
    /// What this rule alone would do to the message (guards applied).
    plan: Plan,
}

/// Test one (possibly unsaved) rule against a cached message. Nothing is
/// changed or sent.
#[tauri::command]
pub fn test_rule(state: State<'_, AppState>, rule: Rule, message_id: i64) -> Res<RuleTest> {
    rules::validate(&rule)?;
    let (account_id, folder_id, _, _) = state
        .store
        .message_refs(&[message_id])?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("message not found".into()))?;
    let account = state.store.account(account_id)?;
    let raw = state.store.raw(message_id)?;
    let facts = MessageFacts::from_raw(&raw, &account.config.email);
    let (matched, conditions) = rules::explain(&rule, &facts)?;
    let source = state.store.folder(folder_id)?.display;
    let mut r = rule.clone();
    r.enabled = true;
    let plan = rules::plan([&r], &facts, &source)?;
    Ok(RuleTest {
        matched,
        conditions,
        plan,
    })
}

/// "Run now": apply the local rules to every cached message of a folder.
#[tauri::command]
pub async fn run_rules_now(
    app: AppHandle,
    state: State<'_, AppState>,
    folder_id: i64,
) -> Res<RunReport> {
    let f = state.store.folder(folder_id)?;
    let account = state.store.account(f.account_id)?;
    let rules = rules_exec::local_rules(&state.store, f.account_id)?;
    let ids = state.store.message_ids(folder_id)?;
    let (mut s, password) = session(&state, f.account_id).await?;
    let r = {
        let _g = state.account_sync_lock(f.account_id).lock_owned().await;
        rules_exec::run(
            &state.store,
            &mut s,
            &account.config,
            &password,
            f.account_id,
            folder_id,
            &ids,
            &rules,
            false,
        )
        .await
    };
    done(s).await;
    resync_later(&app, f.account_id);
    r
}

#[derive(Serialize)]
pub struct SieveStatus {
    settings: SieveSettings,
    preview: SieveExport,
    oauth: bool,
}

pub(crate) fn export(state: &AppState, account_id: i64) -> Res<SieveExport> {
    let account = state.store.account(account_id)?;
    let trash = state
        .store
        .folder_by_role(account_id, "trash")?
        .map(|f| f.display)
        .unwrap_or_else(|| "Trash".into());
    // The automatic reply rides in the same script when it runs on the server.
    let reply = state.store.auto_reply(account_id)?;
    let vacation = (reply.enabled && reply.on_server).then(|| {
        let own: Vec<String> = std::iter::once(account.config.email.clone())
            .chain(
                state
                    .store
                    .account_settings(account_id)
                    .map(|s| s.identities)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|i| i.email),
            )
            .collect();
        neutron_core::autoreply::sieve_block(&reply, &own)
    });
    Ok(rules::to_sieve_with(
        &state.store.rules()?,
        &account.config.email,
        &trash,
        vacation,
    ))
}

/// Upload and activate our script (rules + automatic reply) on `port`.
pub(crate) async fn upload_script(
    state: &AppState,
    account_id: i64,
    port: u16,
) -> Res<SieveExport> {
    let account = state.store.account(account_id)?;
    let ex = export(state, account_id)?;
    let password = crate::providers::credential(state, account_id).await?;
    let mut ms = managesieve::Client::connect(&account.config, port, &password).await?;
    let r = async {
        ms.check_script(&ex.script).await?;
        ms.put_script(managesieve::SCRIPT_NAME, &ex.script).await?;
        ms.set_active(managesieve::SCRIPT_NAME).await
    }
    .await;
    ms.logout().await;
    r?;
    state
        .store
        .set_sieve_settings(account_id, SieveSettings { port, active: true })?;
    tracing::info!(
        account = account_id,
        rules = ex.exported.len(),
        "sieve script activated"
    );
    Ok(ex)
}

/// Deactivate and remove our script.
pub(crate) async fn remove_script(state: &AppState, account_id: i64) -> Res<()> {
    let account = state.store.account(account_id)?;
    let settings = state.store.sieve_settings(account_id)?;
    let password = crate::providers::credential(state, account_id).await?;
    let mut ms = managesieve::Client::connect(&account.config, settings.port, &password).await?;
    let r = async {
        let ours = ms
            .list_scripts()
            .await?
            .into_iter()
            .find(|(n, _)| n == managesieve::SCRIPT_NAME);
        if let Some((_, active)) = ours {
            if active {
                ms.set_active("").await?;
            }
            ms.delete_script(managesieve::SCRIPT_NAME).await?;
        }
        Ok::<_, Error>(())
    }
    .await;
    ms.logout().await;
    r?;
    state.store.set_sieve_settings(
        account_id,
        SieveSettings {
            port: settings.port,
            active: false,
        },
    )
}

#[tauri::command]
pub fn sieve_status(state: State<'_, AppState>, account_id: i64) -> Res<SieveStatus> {
    Ok(SieveStatus {
        settings: state.store.sieve_settings(account_id)?,
        preview: export(&state, account_id)?,
        oauth: state.store.account(account_id)?.config.auth.is_oauth(),
    })
}

/// Check, upload and activate the server-side rules as a Sieve script.
#[tauri::command]
pub async fn sieve_upload(
    state: State<'_, AppState>,
    account_id: i64,
    port: u16,
) -> Res<SieveExport> {
    let preview = export(&state, account_id)?;
    let reply = state.store.auto_reply(account_id)?;
    if preview.exported.is_empty() && !(reply.enabled && reply.on_server) {
        return Err(Error::Invalid(
            "no enabled rule is set to run on the server".into(),
        ));
    }
    upload_script(&state, account_id, port).await
}

/// Deactivate and remove our script; server-side rules run locally again.
#[tauri::command]
pub async fn sieve_disable(state: State<'_, AppState>, account_id: i64) -> Res<()> {
    remove_script(&state, account_id).await
}
