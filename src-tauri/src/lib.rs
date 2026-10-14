//! Tauri shell: thin command layer over `neutron-core` plus a background
//! watcher per account (IMAP IDLE when supported, polling otherwise).

mod commands;
mod watcher;

use std::{collections::HashMap, sync::Arc};

use neutron_core::{secrets::SecretStore, store::Store};
use tauri::Manager;

pub struct AppState {
    pub store: Arc<Store>,
    pub secrets: Arc<SecretStore>,
    /// Serializes syncs so the watcher and manual "sync now" never race on
    /// the same cache.
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    pub watchers: std::sync::Mutex<HashMap<i64, tauri::async_runtime::JoinHandle<()>>>,
}

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
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            restrict_dir(&dir);
            let store = Arc::new(Store::open(&dir.join("cache.sqlite3"))?);
            let secrets = Arc::new(SecretStore::detect());
            tracing::info!(backend = ?secrets.backend(), "secret storage selected");
            let state = AppState {
                store,
                secrets,
                sync_lock: Arc::new(tokio::sync::Mutex::new(())),
                watchers: std::sync::Mutex::new(HashMap::new()),
            };
            let accounts = state.store.accounts()?;
            app.manage(state);
            for a in accounts {
                watcher::start(app.handle(), a.id);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::list_accounts,
            commands::add_account,
            commands::remove_account,
            commands::set_password,
            commands::sync_account,
            commands::list_folders,
            commands::list_messages,
            commands::get_message,
            commands::search,
            commands::send_message,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Neutron Mail");
}

/// The data dir holds the mail cache and WebKit storage: owner-only access.
fn restrict_dir(dir: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            tracing::warn!("could not restrict data directory permissions: {e}");
        }
    }
}
