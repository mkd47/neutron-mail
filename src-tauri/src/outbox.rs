//! Undo send: a sent message waits a few seconds in this queue before it
//! actually leaves, so it can still be cancelled ("Undo") or released at
//! once ("Send now"). The queue lives in the backend, not the window: if
//! the app is quit during the countdown, pending messages are sent right
//! away instead of being lost (see [`flush_on_exit`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use neutron_core::model::OutgoingMessage;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{oneshot, Notify};

use crate::commands::{deliver, validate_outgoing, Res, SendResult};
use crate::AppState;

/// Longest undo window the UI may ask for.
const MAX_DELAY_SECS: u64 = 60;

enum Command {
    Cancel,
    SendNow,
}

#[derive(Default)]
pub struct Outbox {
    next: AtomicU64,
    /// Messages still waiting for their delay, by queue id.
    waiting: Mutex<HashMap<u64, oneshot::Sender<Command>>>,
    /// Waiting plus currently sending; the app only exits at 0.
    in_flight: AtomicUsize,
    idle: Notify,
}

impl Outbox {
    fn waiting(&self) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Command>>> {
        self.waiting.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn signal(&self, id: u64, cmd: Command) -> bool {
        match self.waiting().remove(&id) {
            Some(tx) => tx.send(cmd).is_ok(),
            None => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) == 0
    }
}

/// Outcome of a queued send, emitted as `mail://send-result`.
#[derive(Clone, Serialize)]
struct SendEvent {
    id: u64,
    account_id: i64,
    /// "sent", "failed" or "cancelled".
    status: &'static str,
    result: Option<SendResult>,
    error: Option<String>,
}

/// Queue a message to be sent after `delay_secs` (0 sends right away, still
/// through the queue). The message is checked first, so invalid addresses
/// or oversized attachments are reported at once, in the composer.
#[tauri::command]
pub async fn queue_send(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: i64,
    message: OutgoingMessage,
    replace_draft: Option<i64>,
    replace_message_id: Option<String>,
    delay_secs: u64,
) -> Res<u64> {
    validate_outgoing(&state, account_id, &message)?;
    let outbox = &state.outbox;
    let id = outbox.next.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = oneshot::channel();
    outbox.waiting().insert(id, tx);
    outbox.in_flight.fetch_add(1, Ordering::SeqCst);
    let delay = Duration::from_secs(delay_secs.min(MAX_DELAY_SECS));
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app2.state::<AppState>();
        let cancelled = tokio::select! {
            _ = tokio::time::sleep(delay) => false,
            cmd = rx => matches!(cmd, Ok(Command::Cancel)),
        };
        // From here on Undo is no longer possible.
        state.outbox.waiting().remove(&id);
        let event = if cancelled {
            tracing::info!(account = account_id, "queued message cancelled");
            SendEvent {
                id,
                account_id,
                status: "cancelled",
                result: None,
                error: None,
            }
        } else {
            match deliver(
                &state,
                account_id,
                &message,
                replace_draft,
                replace_message_id.as_deref(),
            )
            .await
            {
                Ok(r) => SendEvent {
                    id,
                    account_id,
                    status: "sent",
                    result: Some(r),
                    error: None,
                },
                Err(e) => {
                    tracing::warn!(account = account_id, "queued message failed: {e}");
                    SendEvent {
                        id,
                        account_id,
                        status: "failed",
                        result: None,
                        error: Some(e.to_string()),
                    }
                }
            }
        };
        if let Err(e) = app2.emit("mail://send-result", event) {
            tracing::warn!("emit failed: {e}");
        }
        if state.outbox.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            state.outbox.idle.notify_waiters();
        }
    });
    Ok(id)
}

/// Undo: true if the message had not started sending (it never leaves).
#[tauri::command]
pub fn cancel_send(state: State<'_, AppState>, id: u64) -> bool {
    state.outbox.signal(id, Command::Cancel)
}

/// Skip the rest of the countdown.
#[tauri::command]
pub fn send_now(state: State<'_, AppState>, id: u64) -> bool {
    state.outbox.signal(id, Command::SendNow)
}

/// Send everything still waiting, then exit once nothing is in flight
/// (at most a minute). Called when the app is asked to quit.
pub fn flush_on_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let ids: Vec<u64> = state.outbox.waiting().keys().copied().collect();
    tracing::info!(count = ids.len(), "sending queued messages before exit");
    for id in ids {
        state.outbox.signal(id, Command::SendNow);
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _ = tokio::time::timeout(Duration::from_secs(60), async {
            while !state.outbox.is_empty() {
                let notified = state.outbox.idle.notified();
                if state.outbox.is_empty() {
                    break;
                }
                notified.await;
            }
        })
        .await;
        app.exit(0);
    });
}
