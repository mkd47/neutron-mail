//! Update notice: is a newer release on GitHub? (Off in settings: never asked.)

use neutron_core::update::{self, Release};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::commands::Res;

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Res<Option<Release>> {
    let current = app.package_info().version.to_string();
    update::check(&current).await
}

/// Open the release page (notes and downloads) in the browser.
#[tauri::command]
pub fn open_release_page(app: AppHandle, url: String) -> Res<()> {
    if !url.starts_with("https://github.com/mkd47/neutron-mail/releases/") {
        return Err(neutron_core::Error::Invalid("not a release page".into()));
    }
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| neutron_core::Error::Invalid(format!("could not open the browser: {e}")))
}
