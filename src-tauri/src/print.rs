//! Printing one message. The main window can't print just the message
//! (the body is a sandboxed iframe inside the app UI), so "Print" opens a
//! small preview window that shows only the message and starts the print
//! dialog. The page is served from a custom `neutron-print://` scheme:
//! an escaped header plus the already-sanitized body under the reading
//! pane's CSP (no scripts, no network; remote images stay blocked). The
//! window has no capability, so it cannot call any app command, and it
//! never navigates anywhere.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use neutron_core::sanitize;
use tauri::{
    http, webview::PageLoadEvent, AppHandle, Manager, State, UriSchemeContext, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};

use crate::{commands::Res, AppState};

pub const SCHEME: &str = "neutron-print";

#[derive(Default)]
pub struct PrintJobs {
    next: AtomicU64,
    /// Printable page per open print window, by key.
    pages: Mutex<HashMap<String, String>>,
}

impl PrintJobs {
    fn pages(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.pages.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Serves `neutron-print://localhost/<key>`.
pub fn protocol(
    ctx: UriSchemeContext<'_, tauri::Wry>,
    req: http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    let key = req.uri().path().trim_start_matches('/');
    let page = ctx
        .app_handle()
        .state::<AppState>()
        .print
        .pages()
        .get(key)
        .cloned();
    let (status, body) = match page {
        Some(p) => (200, p.into_bytes()),
        None => (404, b"Not found".to_vec()),
    };
    http::Response::builder()
        .status(status)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", sanitize::MESSAGE_CSP)
        .body(body)
        .unwrap_or_default()
}

#[tauri::command]
pub async fn print_message(app: AppHandle, state: State<'_, AppState>, message_id: i64) -> Res<()> {
    let detail = state.store.message_detail(message_id)?;
    let date = chrono::DateTime::from_timestamp(detail.summary.date, 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a, %-d %b %Y %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    let page = sanitize::print_document(&detail, &date, detail.html.as_deref());
    let n = state.print.next.fetch_add(1, Ordering::Relaxed);
    let key = n.to_string();
    state.print.pages().insert(key.clone(), page);
    let url = format!("{SCHEME}://localhost/{key}")
        .parse()
        .map_err(|_| neutron_core::Error::Invalid("print URL".into()))?;
    let subject = if detail.summary.subject.is_empty() {
        "(no subject)".to_string()
    } else {
        detail.summary.subject.chars().take(80).collect()
    };
    let window =
        WebviewWindowBuilder::new(&app, format!("print-{n}"), WebviewUrl::CustomProtocol(url))
            .title(format!("Print – {subject}"))
            .inner_size(820.0, 900.0)
            .on_navigation(|u| u.scheme() == SCHEME)
            .on_page_load(|w, payload| {
                if payload.event() == PageLoadEvent::Finished {
                    if let Err(e) = w.print() {
                        tracing::warn!("print dialog failed: {e}");
                    }
                }
            })
            .build()
            .map_err(|e| neutron_core::Error::Invalid(format!("print window: {e}")))?;
    let handle = app.clone();
    window.on_window_event(move |e| {
        if matches!(e, WindowEvent::Destroyed) {
            handle.state::<AppState>().print.pages().remove(&key);
        }
    });
    Ok(())
}
