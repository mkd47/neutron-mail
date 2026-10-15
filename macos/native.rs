//! macOS-only native notifications and Dock badge.

use neutron_core::{Error, Result};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

pub async fn notify(app: &AppHandle, title: &str, body: &str, _replace: u32) -> Result<u32> {
    app.notification()
        .request_permission()
        .map_err(|e| Error::Invalid(format!("notification permission: {e}")))?;
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| Error::Invalid(format!("notification: {e}")))?;
    Ok(0)
}

pub async fn launcher_badge(app: &AppHandle, count: i64) -> Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_badge_count((count > 0).then_some(count))
            .map_err(|e| Error::Invalid(format!("Dock badge: {e}")))?;
    }
    Ok(())
}
