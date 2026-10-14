//! Background new-mail watcher, one task per account, spawned on
//! `tauri::async_runtime`. Uses IMAP IDLE on INBOX when the server supports
//! it (re-issued every 25 min per RFC 2177), otherwise polls every 5 min.
//! Every wake-up runs a full (incremental) account sync and emits
//! `mail://synced` to the UI.

use std::time::Duration;

use neutron_core::{imap, model::SyncReport};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{commands::run_sync, AppState};

const IDLE_MAX: Duration = Duration::from_secs(25 * 60);
const POLL_EVERY: Duration = Duration::from_secs(5 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Clone, Serialize)]
struct SyncEvent {
    account_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<SyncReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub fn start(app: &AppHandle, account_id: i64) {
    let state = app.state::<AppState>();
    let app2 = app.clone();
    let handle = tauri::async_runtime::spawn(async move { watch(app2, account_id).await });
    if let Some(old) = state
        .watchers
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(account_id, handle)
    {
        old.abort();
    };
}

pub fn stop(state: &AppState, account_id: i64) {
    if let Some(h) = state
        .watchers
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&account_id)
    {
        h.abort();
    };
}

fn emit(app: &AppHandle, ev: SyncEvent) {
    if let Err(e) = app.emit("mail://synced", ev) {
        tracing::warn!("emit failed: {e}");
    }
}

async fn watch(app: AppHandle, account_id: i64) {
    loop {
        let state = app.state::<AppState>();
        // 1. Sync, report to the UI.
        match run_sync(&state, account_id).await {
            Ok(report) => emit(
                &app,
                SyncEvent {
                    account_id,
                    report: Some(report),
                    error: None,
                },
            ),
            Err(e) => {
                tracing::warn!(account = account_id, "background sync failed: {e}");
                emit(
                    &app,
                    SyncEvent {
                        account_id,
                        report: None,
                        error: Some(e.to_string()),
                    },
                );
                tokio::time::sleep(RETRY_AFTER * 5).await;
                continue;
            }
        }
        // 2. Wait for a change: IDLE if possible, else a poll interval.
        if let Err(e) = wait_for_change(&state, account_id).await {
            tracing::warn!(
                account = account_id,
                "IDLE failed, falling back to a delay: {e}"
            );
            tokio::time::sleep(RETRY_AFTER).await;
        }
    }
}

async fn wait_for_change(state: &AppState, account_id: i64) -> neutron_core::Result<()> {
    let account = state.store.account(account_id)?;
    let password = state.secrets.get(account_id)?;
    let mut session = imap::connect(&account.config, &password).await?;
    if !imap::supports_idle(&mut session).await? {
        drop(session.logout().await);
        tokio::time::sleep(POLL_EVERY).await;
        return Ok(());
    }
    let (mut session, changed) = imap::idle_inbox(session, IDLE_MAX).await?;
    tracing::debug!(account = account_id, changed, "IDLE returned");
    drop(session.logout().await);
    Ok(())
}
