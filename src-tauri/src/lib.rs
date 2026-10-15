//! Tauri shell: thin command layer over `neutron-core`, a background
//! watcher per account (IMAP IDLE when supported, polling otherwise), and
//! link interception for the message viewer.

mod attachments;
mod calendar;
mod commands;
mod contacts;
mod providers;
mod watcher;

use std::{collections::HashMap, sync::Arc};

use neutron_core::{links, secrets::SecretStore, store::Store};
use tauri::{Emitter, Manager};

pub struct AppState {
    pub store: Arc<Store>,
    pub secrets: Arc<SecretStore>,
    /// Serializes syncs so the watcher and manual "sync now" never race on
    /// the same cache.
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    pub watchers: std::sync::Mutex<HashMap<i64, tauri::async_runtime::JoinHandle<()>>>,
    /// Files chosen in the native picker, by opaque token. The UI only ever
    /// sees tokens, so it cannot make the backend read arbitrary paths.
    pub picked: std::sync::Mutex<HashMap<String, attachments::PickedFile>>,
    pub contacts: contacts::Contacts,
    pub calendar: calendar::Calendar,
    pub cache: CacheStatus,
}

/// How the mail cache is stored, shown in the UI.
#[derive(Clone, serde::Serialize)]
pub struct CacheStatus {
    /// "encrypted" (SQLCipher, key in the keyring) or "memory" (no keyring:
    /// nothing is written to disk, the cache lasts for this session only).
    pub mode: &'static str,
    /// One-off information for the user (migration done, unreadable cache…).
    pub notice: Option<String>,
}

/// Open the encrypted cache, migrating a plaintext one; without a usable
/// keyring keep everything in memory and leave existing files untouched.
fn open_cache(dir: &std::path::Path, secrets: &SecretStore) -> (Store, CacheStatus) {
    let path = dir.join("cache.sqlite3");
    let reason = match secrets.cache_key() {
        Ok(Some(key)) => match Store::open_encrypted(&path, &key) {
            Ok((store, report)) => {
                let notice = if let Some(aside) = report.unreadable_moved_to {
                    Some(format!(
                        "The mail cache could not be decrypted (its key is missing from the keyring), so a new one was created. Mail is downloaded again; accounts must be added again. The old file was kept at {}.",
                        aside.display()
                    ))
                } else if report.migrated {
                    Some(
                        "Your mail cache is now encrypted (key stored in the system keyring)."
                            .into(),
                    )
                } else {
                    None
                };
                tracing::info!(migrated = report.migrated, "mail cache: encrypted");
                return (
                    store,
                    CacheStatus {
                        mode: "encrypted",
                        notice,
                    },
                );
            }
            Err(e) => format!("the encrypted cache could not be opened: {e}"),
        },
        Ok(None) => "no system keyring is available".to_string(),
        Err(e) => format!("the cache key could not be read from the keyring: {e}"),
    };
    tracing::warn!("mail cache kept in memory only: {reason}");
    let store = Store::open_in_memory().expect("in-memory SQLite");
    let notice = Some(format!(
        "Your mail cache is kept in memory only because {reason}. Nothing is written to disk; accounts and mail must be set up again next time."
    ));
    (
        store,
        CacheStatus {
            mode: "memory",
            notice,
        },
    )
}

/// Origins of the app's own UI. Any other navigation of the webview or of
/// the message iframe is a link click and is intercepted.
/// The Vite dev server origin is only trusted in debug builds.
#[cfg(debug_assertions)]
const APP_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "http://127.0.0.1:1420",
];
#[cfg(not(debug_assertions))]
const APP_ORIGINS: &[&str] = &["tauri://localhost", "http://tauri.localhost"];

pub fn run() {
    // Log level from NEUTRON_LOG (default: info). Nothing logs passwords or
    // message bodies; see docs/AUDIT.md.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("NEUTRON_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            restrict_dir(&dir);
            let secrets = Arc::new(SecretStore::detect());
            tracing::info!(backend = ?secrets.backend(), "secret storage selected");
            let (store, cache) = open_cache(&dir, &secrets);
            let store = Arc::new(store);
            let state = AppState {
                store,
                secrets,
                sync_lock: Arc::new(tokio::sync::Mutex::new(())),
                watchers: std::sync::Mutex::new(HashMap::new()),
                picked: std::sync::Mutex::new(HashMap::new()),
                contacts: contacts::Contacts::default(),
                calendar: calendar::Calendar::default(),
                cache,
            };
            let accounts = state.store.accounts()?;
            app.manage(state);
            attachments::clear_opened(app.handle());
            create_main_window(app.handle())?;
            for a in accounts {
                watcher::start(app.handle(), a.id);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_accounts,
            commands::add_account,
            providers::list_providers,
            providers::detect_provider,
            providers::autoconfig_lookup,
            providers::oauth_sign_in,
            providers::oauth_configured,
            commands::remove_account,
            commands::set_password,
            commands::sync_account,
            commands::list_folders,
            commands::list_messages,
            commands::get_message,
            commands::set_image_sender_trust,
            commands::update_account,
            commands::get_account_settings,
            commands::set_account_settings,
            commands::list_image_senders,
            commands::new_draft,
            commands::message_source,
            commands::search,
            commands::compose_prefill,
            commands::send_message,
            commands::save_draft,
            commands::analyze_link,
            attachments::pick_attachments,
            attachments::save_attachment,
            attachments::open_attachment,
            contacts::suggest_contacts,
            contacts::contacts_status,
            commands::sanitize_compose_html,
            calendar::upcoming_events,
            commands::set_unread_badge,
            calendar::get_invite,
            calendar::respond_invite,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Neutron Mail");
}

/// The main window is created here (not from tauri.conf.json) so that a
/// navigation handler is attached before any content loads.
fn create_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    let cfg = app
        .config()
        .app
        .windows
        .first()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("main window config".into()))?;
    let handle = app.clone();
    let window = tauri::WebviewWindowBuilder::from_config(app, &cfg)?
        .on_navigation(move |url| {
            let s = url.as_str();
            tracing::debug!(url_len = s.len(), "navigation request");
            if links::is_internal_navigation(s, APP_ORIGINS) {
                return true;
            }
            // A link clicked in a message (the iframe tried to navigate).
            // Never navigate; hand the link to the UI for confirmation.
            let payload = match links::analyze(s) {
                Ok(info) => serde_json_value(&info),
                Err(e) => {
                    tracing::info!("blocked link with disallowed scheme");
                    serde_json::json!({ "error": e.to_string(), "original": s })
                }
            };
            if let Err(e) = handle.emit("mail://link", payload) {
                tracing::warn!("emit link failed: {e}");
            }
            false
        })
        .build()?;
    hover_status(&window);
    Ok(())
}

fn serde_json_value<T: serde::Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

/// Show the real target of a hovered link (like a browser status bar).
/// WebKitGTK reports hit-test results for subframes too, so this works for
/// links inside the sandboxed message iframe without any script in it.
#[cfg(target_os = "linux")]
fn hover_status(window: &tauri::WebviewWindow) {
    let handle = window.app_handle().clone();
    let res = window.with_webview(move |wv| {
        use webkit2gtk::{HitTestResultExt, WebViewExt};
        wv.inner().connect_mouse_target_changed(move |_, hit, _| {
            let uri = hit.link_uri().map(|u| u.to_string());
            let _ = handle.emit("mail://hover", uri);
        });
    });
    if let Err(e) = res {
        tracing::warn!("link hover status unavailable: {e}");
    }
}

#[cfg(not(target_os = "linux"))]
fn hover_status(_window: &tauri::WebviewWindow) {}

/// The data dir holds the mail cache and WebKit storage: owner-only access.
pub(crate) fn restrict_dir(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            tracing::warn!("could not restrict directory permissions: {e}");
        }
    }
}
