//! Send later: a message is built when it is scheduled (attachments
//! included, so the files may change or go away afterwards) and kept in the
//! local cache until its time. A background task sends what is due; mail
//! whose time passed while the app was closed goes out at the next start.
//! Failed sends are retried a few times, then kept as "failed" until the
//! user sends, edits (back to Drafts) or deletes them.

use std::time::Duration;

use neutron_core::{
    imap,
    model::{AttachmentRef, OutgoingMessage},
    smtp,
    store::{NewScheduled, Scheduled},
    Error,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{
    after_send, delete_draft, delete_drafts_by_message_id, resolve_attachments, validate_outgoing,
    Res, SendResult,
};
use crate::AppState;

/// How often due messages are looked for.
const TICK: Duration = Duration::from_secs(20);
/// Tries before a message is marked failed; waits double each time.
const MAX_ATTEMPTS: i64 = 5;
/// Furthest a message can be scheduled.
const MAX_AHEAD_SECS: i64 = 366 * 86_400;

/// `mail://scheduled`: something changed in the list (`status` "changed"),
/// or a message was sent / will be retried / failed.
#[derive(Clone, Serialize)]
struct ScheduledEvent {
    id: i64,
    status: &'static str,
    subject: String,
    recipients: String,
    error: Option<String>,
    result: Option<SendResult>,
}

fn changed(app: &AppHandle, id: i64) {
    emit(
        app,
        ScheduledEvent {
            id,
            status: "changed",
            subject: String::new(),
            recipients: String::new(),
            error: None,
            result: None,
        },
    );
}

fn emit(app: &AppHandle, e: ScheduledEvent) {
    if let Err(err) = app.emit("mail://scheduled", e) {
        tracing::warn!("emit failed: {err}");
    }
}

/// Short "Name, other@x +2" label for the list.
fn recipients_label(m: &OutgoingMessage) -> String {
    let all: Vec<&String> = m.to.iter().chain(&m.cc).chain(&m.bcc).collect();
    let first = all
        .first()
        .map(|a| {
            let a = a.trim();
            match a.find('<') {
                Some(i) if i > 0 => a[..i].trim().trim_matches('"').to_string(),
                _ => a.trim_matches(['<', '>']).to_string(),
            }
        })
        .unwrap_or_default();
    match all.len() {
        0 | 1 => first,
        n => format!("{first} +{}", n - 1),
    }
}

#[tauri::command]
pub fn schedule_send(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    message: OutgoingMessage,
    replace_draft: Option<i64>,
    replace_message_id: Option<String>,
    send_at: i64,
) -> Res<Scheduled> {
    let now = chrono::Utc::now().timestamp();
    if send_at < now - 60 || send_at > now + MAX_AHEAD_SECS {
        return Err(Error::Invalid(
            "pick a time between now and a year from now".into(),
        ));
    }
    validate_outgoing(&state, account_id, &message)?;
    let account = state.store.account(account_id)?;
    let identities = state.store.account_settings(account_id)?.identities;
    let sender = smtp::with_sender(&account.config, &identities, message.from.as_deref())?;
    let files = resolve_attachments(&state, account_id, &message.attachments)?;
    // Built like a draft (Bcc header kept, so "Edit" can put it back in
    // Drafts intact); the header is dropped when it is sent.
    let built = smtp::build(&sender, &message, &files, true)?;
    let env_from = built
        .envelope
        .from()
        .map(|a| a.to_string())
        .unwrap_or_else(|| sender.email.clone());
    let env_to: Vec<String> = built.envelope.to().iter().map(|a| a.to_string()).collect();
    let id = state.store.add_scheduled(&NewScheduled {
        account_id,
        send_at: send_at.max(now),
        subject: message.subject.trim(),
        recipients: &recipients_label(&message),
        env_from: &env_from,
        env_to: &env_to,
        raw: &built.raw,
        replace_draft,
        replace_message_id: replace_message_id.as_deref().filter(|m| !m.is_empty()),
    })?;
    for t in &message.attachments {
        if let AttachmentRef::File { token } = t {
            crate::attachments::forget(&state, token);
        }
    }
    tracing::info!(account = account_id, "message scheduled");
    changed(&app, id);
    state
        .store
        .scheduled()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| Error::NotFound(format!("scheduled message {id}")))
}

#[tauri::command]
pub fn list_scheduled(state: State<'_, AppState>) -> Res<Vec<Scheduled>> {
    state.store.scheduled()
}

/// Send a scheduled (or failed) message right away.
#[tauri::command]
pub fn send_scheduled_now(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<()> {
    if !state.store.claim_scheduled(id)? {
        return Err(Error::Invalid("this message is already being sent".into()));
    }
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move { send_one(&app2, id).await });
    changed(&app, id);
    Ok(())
}

/// Change the time of a waiting (or failed) message.
#[tauri::command]
pub fn reschedule(app: AppHandle, state: State<'_, AppState>, id: i64, send_at: i64) -> Res<()> {
    let now = chrono::Utc::now().timestamp();
    if send_at < now - 60 || send_at > now + MAX_AHEAD_SECS {
        return Err(Error::Invalid(
            "pick a time between now and a year from now".into(),
        ));
    }
    if !state.store.claim_scheduled(id)? {
        return Err(Error::Invalid("this message is already being sent".into()));
    }
    state.store.release_scheduled(id, Some(send_at.max(now)))?;
    changed(&app, id);
    Ok(())
}

/// Don't send: the message goes back to Drafts (replacing the draft it was
/// written from), or is deleted when `keep` is false.
#[tauri::command]
pub async fn cancel_scheduled(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    keep: bool,
) -> Res<()> {
    if !state.store.claim_scheduled(id)? {
        return Err(Error::Invalid("this message is already being sent".into()));
    }
    let res = async {
        if keep {
            let p = state.store.scheduled_payload(id)?;
            let account = state.store.account(p.account_id)?;
            let drafts = state
                .store
                .folder_by_role(account.id, "drafts")?
                .ok_or_else(|| Error::Invalid("this account has no Drafts folder".into()))?;
            let password = crate::providers::credential(&state, account.id).await?;
            // Stored like a draft (Bcc kept), so it goes back as it was.
            let mut s = imap::connect(&account.config, &password).await?;
            let r = imap::append_with_flags(&mut s, &drafts.name, &p.raw, "(\\Seen \\Draft)").await;
            drop(s.logout().await);
            r?;
            if let Some(d) = p.replace_draft {
                let _ = delete_draft(&state, &account, &password, d).await;
            }
            if let Some(mid) = p.replace_message_id.as_deref() {
                let _ = delete_drafts_by_message_id(&state, &account, &password, mid).await;
            }
        }
        state.store.delete_scheduled(id)
    }
    .await;
    if let Err(e) = res {
        let _ = state.store.release_scheduled(id, None);
        return Err(e);
    }
    changed(&app, id);
    Ok(())
}

/// Start the background sender (once, at startup).
pub fn start(app: &AppHandle) {
    let state = app.state::<AppState>();
    match state.store.reset_sending_scheduled() {
        Ok(0) => {}
        Ok(n) => tracing::warn!(
            messages = n,
            "scheduled sends interrupted earlier; retrying"
        ),
        Err(e) => tracing::warn!("scheduled sends: {e}"),
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let now = chrono::Utc::now().timestamp();
            let due = app.state::<AppState>().store.claim_due_scheduled(now);
            match due {
                Ok(ids) => {
                    for id in ids {
                        send_one(&app, id).await;
                    }
                }
                Err(e) => tracing::warn!("scheduled sends: {e}"),
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Send one claimed message, then report it.
async fn send_one(app: &AppHandle, id: i64) {
    let state = app.state::<AppState>();
    let p = match state.store.scheduled_payload(id) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("scheduled message {id}: {e}");
            return;
        }
    };
    let res = async {
        let account = state.store.account(p.account_id)?;
        let password = crate::providers::credential(&state, account.id).await?;
        let raw = smtp::for_sending_at(&p.raw, chrono::Utc::now().timestamp());
        smtp::send_raw(&account.config, &password, &p.env_from, &p.env_to, &raw).await?;
        // Sent: from here on, never send it again.
        state.store.delete_scheduled(id)?;
        tracing::info!(account = account.id, "scheduled message sent");
        after_send(
            &state,
            &account,
            &password,
            &raw,
            p.replace_draft,
            p.replace_message_id.as_deref(),
        )
        .await
    }
    .await;
    let event = |status, error, result| ScheduledEvent {
        id,
        status,
        subject: p.subject.clone(),
        recipients: p.recipients.clone(),
        error,
        result,
    };
    match res {
        Ok(r) => emit(app, event("sent", None, Some(r))),
        Err(e) => {
            let attempts = p.attempts + 1;
            let retry = (attempts < MAX_ATTEMPTS)
                .then(|| chrono::Utc::now().timestamp() + 60 * (1 << attempts));
            tracing::warn!(attempts, "scheduled message failed: {e}");
            if let Err(db) = state.store.scheduled_failed(id, &e.to_string(), retry) {
                tracing::warn!("scheduled message {id}: {db}");
            }
            let status = if retry.is_some() { "retry" } else { "failed" };
            emit(app, event(status, Some(e.to_string()), None));
        }
    }
}
