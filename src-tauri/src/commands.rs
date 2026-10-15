use std::collections::HashMap;

use neutron_core::{
    compose, imap, links,
    model::{
        Account, AccountConfig, AttachmentRef, ComposeDraft, Folder, MessageDetail, MessageSummary,
        OutgoingAttachment, OutgoingMessage, SearchHit, SyncReport,
    },
    remote, sanitize,
    secrets::Backend,
    smtp, sync, Error,
};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::{attachments, watcher, AppState};

pub type Res<T> = Result<T, Error>;

/// Raw source shown by "View source" is capped (the UI renders it as text).
const MAX_SOURCE_BYTES: usize = 10 * 1024 * 1024;

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
    /// Remote images referenced by the message (fetchable on request).
    remote_images_available: usize,
    /// Remote images that could not be loaded (blocked host, not an image, too large…).
    remote_images_failed: usize,
    /// The user chose "always load images from this sender".
    sender_trusted: bool,
}

#[derive(Serialize)]
pub struct SendResult {
    /// "saved", "no_sent_folder" or "failed".
    sent_copy: &'static str,
    sent_copy_error: Option<String>,
    /// Set when a resumed draft could not be removed from Drafts.
    draft_cleanup_error: Option<String>,
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
///
/// Remote images are fetched (by the core, never by the webview) only if
/// `load_remote` is set or the sender is on the account's allow-list.
#[tauri::command]
pub async fn get_message(
    state: State<'_, AppState>,
    message_id: i64,
    dark: bool,
    load_remote: bool,
) -> Res<MessageView> {
    let account_id = state.store.message_account(message_id)?;
    let (mut detail, urls) = state.store.render_message(message_id, &HashMap::new())?;
    let sender_trusted = !detail.summary.from_addr.is_empty()
        && state
            .store
            .is_image_sender_trusted(account_id, &detail.summary.from_addr)?;
    let mut failed = 0;
    if (load_remote || sender_trusted) && !urls.is_empty() {
        let report = remote::fetch_images(&urls, remote::allow_private_from_env()).await?;
        failed = report.failed;
        tracing::info!(
            loaded = report.images.len(),
            failed,
            "remote images fetched on request"
        );
        detail = state.store.render_message(message_id, &report.images)?.0;
    }
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
    Ok(MessageView {
        remote_images_available: urls.len(),
        remote_images_failed: failed,
        sender_trusted,
        detail,
        document,
    })
}

#[tauri::command]
pub fn set_image_sender_trust(
    state: State<'_, AppState>,
    account_id: i64,
    email: String,
    trusted: bool,
) -> Res<()> {
    if trusted {
        state.store.trust_image_sender(account_id, &email)
    } else {
        state.store.untrust_image_sender(account_id, &email)
    }
}

/// Raw RFC 5322 source for "View source". Returned as text and rendered
/// as text by the UI (escaped), never as HTML.
#[tauri::command]
pub fn message_source(state: State<'_, AppState>, message_id: i64) -> Res<String> {
    let raw = state.store.raw(message_id)?;
    let shown = &raw[..raw.len().min(MAX_SOURCE_BYTES)];
    let mut s = String::from_utf8_lossy(shown).into_owned();
    if raw.len() > MAX_SOURCE_BYTES {
        s.push_str(&format!("\n\n[… truncated, {} bytes total]", raw.len()));
    }
    Ok(s)
}

#[tauri::command]
pub fn search(state: State<'_, AppState>, account_id: i64, query: String) -> Res<Vec<SearchHit>> {
    state.store.search(account_id, &query, 100)
}

#[tauri::command]
pub fn compose_prefill(
    state: State<'_, AppState>,
    message_id: i64,
    mode: compose::Mode,
) -> Res<ComposeDraft> {
    let account = state
        .store
        .account(state.store.message_account(message_id)?)?;
    let raw = state.store.raw(message_id)?;
    Ok(compose::prepare(
        &raw,
        message_id,
        mode,
        &account.config.email,
    ))
}

#[tauri::command]
pub fn analyze_link(url: String) -> Res<links::LinkInfo> {
    links::analyze(&url)
}

/// Resolve composer attachment references to content. File tokens come
/// from the native picker; message attachments must belong to the same
/// account.
fn resolve_attachments(
    state: &AppState,
    account_id: i64,
    refs: &[AttachmentRef],
) -> Res<Vec<OutgoingAttachment>> {
    let mut out = Vec::with_capacity(refs.len());
    for r in refs {
        match r {
            AttachmentRef::File { token } => out.push(attachments::read_picked(state, token)?),
            AttachmentRef::Message { message_id, index } => {
                if state.store.message_account(*message_id)? != account_id {
                    return Err(Error::Invalid(
                        "attachment belongs to another account".into(),
                    ));
                }
                let raw = state.store.raw(*message_id)?;
                out.push(
                    neutron_core::parse::attachment_content(&raw, *index)
                        .ok_or_else(|| Error::NotFound("attachment".into()))?,
                );
            }
        }
    }
    Ok(out)
}

/// Remove a resumed draft from the server and the cache.
async fn delete_draft(
    state: &AppState,
    account: &Account,
    password: &str,
    draft_id: i64,
) -> Res<()> {
    let (acc, folder, uid) = state.store.message_location(draft_id)?;
    if acc != account.id {
        return Err(Error::Invalid("draft belongs to another account".into()));
    }
    let folder_id = state.store.message_detail(draft_id)?.summary.folder_id;
    let mut s = imap::connect(&account.config, password).await?;
    imap::delete_uid(&mut s, &folder, uid).await?;
    drop(s.logout().await);
    state.store.remove_uids(folder_id, &[uid])?;
    Ok(())
}

/// Sends via SMTP, then APPENDs a copy to the account's Sent folder (if
/// one is known). The result tells the UI whether the Sent copy worked; a
/// failure there does not mean the message was not sent.
#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    account_id: i64,
    message: OutgoingMessage,
    replace_draft: Option<i64>,
) -> Res<SendResult> {
    let account = state.store.account(account_id)?;
    let password = state.secrets.get(account_id)?;
    let files = resolve_attachments(&state, account_id, &message.attachments)?;
    let raw = smtp::send(&account.config, &password, &message, &files).await?;
    tracing::info!(
        account = account_id,
        recipients = message.to.len() + message.cc.len() + message.bcc.len(),
        attachments = files.len(),
        "message sent"
    );
    let mut result = SendResult {
        sent_copy: "no_sent_folder",
        sent_copy_error: None,
        draft_cleanup_error: None,
    };
    if let Some(sent) = state.store.folder_by_role(account_id, "sent")? {
        let res = async {
            let mut s = imap::connect(&account.config, &password).await?;
            imap::append(&mut s, &sent.name, &raw, true).await?;
            drop(s.logout().await);
            Ok::<_, Error>(())
        }
        .await;
        match res {
            Ok(()) => result.sent_copy = "saved",
            Err(e) => {
                tracing::warn!(account = account_id, "sent, but copy to Sent failed: {e}");
                result.sent_copy = "failed";
                result.sent_copy_error = Some(e.to_string());
            }
        }
    }
    if let Some(draft) = replace_draft {
        if let Err(e) = delete_draft(&state, &account, &password, draft).await {
            result.draft_cleanup_error = Some(e.to_string());
        }
    }
    for t in message.attachments.iter() {
        if let AttachmentRef::File { token } = t {
            attachments::forget(&state, token);
        }
    }
    Ok(result)
}

/// Save to the Drafts folder (APPEND with \Draft). When resuming a draft,
/// the previous version is removed after the new one is stored.
#[tauri::command]
pub async fn save_draft(
    state: State<'_, AppState>,
    account_id: i64,
    message: OutgoingMessage,
    replace_draft: Option<i64>,
) -> Res<()> {
    let account = state.store.account(account_id)?;
    let password = state.secrets.get(account_id)?;
    let drafts = state
        .store
        .folder_by_role(account_id, "drafts")?
        .ok_or_else(|| Error::Invalid("this account has no Drafts folder".into()))?;
    let files = resolve_attachments(&state, account_id, &message.attachments)?;
    let built = smtp::build(&account.config, &message, &files, true)?;
    let mut s = imap::connect(&account.config, &password).await?;
    imap::append_with_flags(&mut s, &drafts.name, &built.raw, "(\\Seen \\Draft)").await?;
    drop(s.logout().await);
    if let Some(old) = replace_draft {
        delete_draft(&state, &account, &password, old).await?;
    }
    Ok(())
}
