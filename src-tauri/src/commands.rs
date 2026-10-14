use neutron_core::{
    imap,
    model::{
        Account, AccountConfig, Folder, MessageDetail, MessageSummary, OutgoingMessage, SearchHit,
        SyncReport,
    },
    sanitize,
    secrets::Backend,
    smtp, sync, Error,
};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::{watcher, AppState};

type Res<T> = Result<T, Error>;

#[derive(Serialize)]
pub struct AppInfo {
    secret_backend: Backend,
    version: &'static str,
}

#[derive(Serialize)]
pub struct MessageView {
    #[serde(flatten)]
    detail: MessageDetail,
    /// Complete HTML document (sanitized body + CSP meta) for a sandboxed
    /// iframe's `srcdoc`, or `None` for text-only messages.
    document: Option<String>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        secret_backend: state.secrets.backend(),
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Res<Vec<Account>> {
    state.store.accounts()
}

/// Verifies the IMAP login *before* persisting anything, then stores the
/// config in SQLite and the password in the secret store.
#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AccountConfig,
    password: String,
) -> Res<Account> {
    config.validate()?;
    let mut session = imap::connect(&config, &password).await?;
    drop(session.logout().await);
    let id = state.store.add_account(&config)?;
    if let Err(e) = state.secrets.set(id, &password) {
        let _ = state.store.delete_account(id);
        return Err(e);
    }
    tracing::info!(account = id, "account added");
    watcher::start(&app, id);
    state.store.account(id)
}

#[tauri::command]
pub fn remove_account(state: State<'_, AppState>, account_id: i64) -> Res<()> {
    watcher::stop(&state, account_id);
    let _ = state.secrets.delete(account_id);
    state.store.delete_account(account_id)
}

/// Re-enter a password (needed after restart when only the in-memory
/// fallback is available).
#[tauri::command]
pub fn set_password(state: State<'_, AppState>, account_id: i64, password: String) -> Res<()> {
    state.store.account(account_id)?;
    state.secrets.set(account_id, &password)
}

#[tauri::command]
pub async fn sync_account(state: State<'_, AppState>, account_id: i64) -> Res<SyncReport> {
    run_sync(&state, account_id).await
}

pub async fn run_sync(state: &AppState, account_id: i64) -> Res<SyncReport> {
    let account = state.store.account(account_id)?;
    let password = state.secrets.get(account_id)?;
    let _guard = state.sync_lock.lock().await;
    sync::sync_account(&state.store, account_id, &account.config, &password).await
}

#[tauri::command]
pub fn list_folders(state: State<'_, AppState>, account_id: i64) -> Res<Vec<Folder>> {
    state.store.folders(account_id)
}

#[tauri::command]
pub fn list_messages(
    state: State<'_, AppState>,
    folder_id: i64,
    offset: i64,
    limit: i64,
) -> Res<Vec<MessageSummary>> {
    state
        .store
        .messages(folder_id, offset.max(0), limit.clamp(1, 500))
}

/// Returns the message from the cache and marks it read (locally at once,
/// on the server in the background; a server failure is only logged and the
/// next sync reconciles the flag).
#[tauri::command]
pub async fn get_message(
    state: State<'_, AppState>,
    message_id: i64,
    dark: bool,
) -> Res<MessageView> {
    let detail = state.store.message_detail(message_id)?;
    let document = detail
        .html
        .as_deref()
        .map(|h| sanitize::wrap_document(h, dark));
    if !detail.summary.seen {
        state.store.mark_seen_local(message_id)?;
        let (account_id, folder, uid) = state.store.message_location(message_id)?;
        let account = state.store.account(account_id)?;
        if let Ok(password) = state.secrets.get(account_id) {
            tauri::async_runtime::spawn(async move {
                let res = async {
                    let mut s = imap::connect(&account.config, &password).await?;
                    imap::mark_seen(&mut s, &folder, uid).await?;
                    drop(s.logout().await);
                    Ok::<_, Error>(())
                }
                .await;
                if let Err(e) = res {
                    tracing::warn!(account = account_id, "could not set \\Seen on server: {e}");
                }
            });
        }
    }
    Ok(MessageView { detail, document })
}

#[tauri::command]
pub fn search(state: State<'_, AppState>, account_id: i64, query: String) -> Res<Vec<SearchHit>> {
    state.store.search(account_id, &query, 100)
}

/// Sends via SMTP, then APPENDs a copy to the account's Sent folder (if one
/// is known). The send is the operation that matters: an APPEND failure is
/// logged, not reported as a send failure.
#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    account_id: i64,
    message: OutgoingMessage,
) -> Res<()> {
    let account = state.store.account(account_id)?;
    let password = state.secrets.get(account_id)?;
    let raw = smtp::send(&account.config, &password, &message).await?;
    tracing::info!(
        account = account_id,
        recipients = message.to.len() + message.cc.len(),
        "message sent"
    );
    let sent = state
        .store
        .folders(account_id)?
        .into_iter()
        .find(|f| f.role.as_deref() == Some("sent"));
    if let Some(sent) = sent {
        let res = async {
            let mut s = imap::connect(&account.config, &password).await?;
            imap::append(&mut s, &sent.name, &raw, true).await?;
            drop(s.logout().await);
            Ok::<_, Error>(())
        }
        .await;
        if let Err(e) = res {
            tracing::warn!(account = account_id, "sent, but copy to Sent failed: {e}");
        }
    }
    Ok(())
}
