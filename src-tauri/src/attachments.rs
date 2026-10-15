//! Attachment commands. Native dialogs are driven from Rust: the webview
//! is granted *no* dialog or filesystem permissions, never sees or sends
//! file paths for reading, and can only refer to picked files by token.

use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;

use neutron_core::attachments::{is_risky, mime_for, safe_filename, MAX_ATTACHMENT_BYTES};
use neutron_core::model::OutgoingAttachment;
use neutron_core::{parse, Error};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::commands::Res;
use crate::AppState;

pub struct PickedFile {
    path: PathBuf,
    name: String,
}

#[derive(Serialize)]
pub struct PickedAttachment {
    token: String,
    name: String,
    size: u64,
    mime: &'static str,
}

#[derive(Serialize)]
pub struct PickResult {
    files: Vec<PickedAttachment>,
    /// Files that were rejected (too large, unreadable), with the reason.
    rejected: Vec<String>,
}

fn token() -> String {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    let mut h2 = std::collections::hash_map::RandomState::new().build_hasher();
    h2.write_u64(h.finish());
    format!("{:016x}{:016x}", h.finish(), h2.finish())
}

/// Native multi-file picker for outgoing attachments.
#[tauri::command]
pub async fn pick_attachments(app: AppHandle, state: State<'_, AppState>) -> Res<PickResult> {
    let dialog_app = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("Attach files")
            .blocking_pick_files()
    })
    .await
    .map_err(|e| Error::Invalid(format!("file picker failed: {e}")))?;
    let mut result = PickResult {
        files: Vec::new(),
        rejected: Vec::new(),
    };
    for fp in picked.unwrap_or_default() {
        let Ok(path) = fp.into_path() else { continue };
        let name = safe_filename(
            &path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        match std::fs::metadata(&path) {
            Ok(m) if !m.is_file() => result.rejected.push(format!("{name}: not a regular file")),
            Ok(m) if m.len() > MAX_ATTACHMENT_BYTES => result.rejected.push(format!(
                "{name}: too large ({} MB, max {} MB)",
                m.len() / 1024 / 1024,
                MAX_ATTACHMENT_BYTES / 1024 / 1024
            )),
            Ok(m) => {
                let t = token();
                result.files.push(PickedAttachment {
                    token: t.clone(),
                    name: name.clone(),
                    size: m.len(),
                    mime: mime_for(&name),
                });
                state
                    .picked
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(t, PickedFile { path, name });
            }
            Err(e) => result.rejected.push(format!("{name}: {e}")),
        }
    }
    Ok(result)
}

/// Read a picked file for sending (size re-checked: the file may have
/// changed since it was picked).
pub fn read_picked(state: &AppState, token: &str) -> Res<OutgoingAttachment> {
    let (path, name) = {
        let map = state.picked.lock().unwrap_or_else(|p| p.into_inner());
        let f = map
            .get(token)
            .ok_or_else(|| Error::NotFound("picked file".into()))?;
        (f.path.clone(), f.name.clone())
    };
    let meta = std::fs::metadata(&path)?;
    if meta.len() > MAX_ATTACHMENT_BYTES {
        return Err(Error::Invalid(format!("{name} is too large")));
    }
    let data = std::fs::read(&path)?;
    Ok(OutgoingAttachment {
        mime: mime_for(&name).to_string(),
        name,
        data,
    })
}

pub fn forget(state: &AppState, token: &str) {
    state
        .picked
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(token);
}

fn load(state: &AppState, message_id: i64, index: usize) -> Res<OutgoingAttachment> {
    let raw = state.store.raw(message_id)?;
    parse::attachment_content(&raw, index).ok_or_else(|| Error::NotFound("attachment".into()))
}

/// Save a received attachment via the native save dialog. The suggested
/// name is sanitized; the user picks the location.
#[tauri::command]
pub async fn save_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    index: usize,
) -> Res<Option<String>> {
    let att = load(&state, message_id, index)?;
    let name = safe_filename(&att.name);
    let dialog_app = app.clone();
    let target = tauri::async_runtime::spawn_blocking(move || {
        dialog_app
            .dialog()
            .file()
            .set_title("Save attachment")
            .set_file_name(&name)
            .blocking_save_file()
    })
    .await
    .map_err(|e| Error::Invalid(format!("save dialog failed: {e}")))?;
    let Some(target) = target else {
        return Ok(None);
    };
    let path = target
        .into_path()
        .map_err(|e| Error::Invalid(format!("bad path: {e}")))?;
    std::fs::write(&path, &att.data)?;
    tracing::info!(bytes = att.data.len(), "attachment saved");
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[derive(Serialize)]
pub struct OpenResult {
    /// "opened" or "needs_confirmation".
    status: &'static str,
    reason: Option<String>,
}

/// Open a received attachment with the default application. It is written
/// to a private (0700) per-open directory under the app cache with a
/// sanitized name. Executables/scripts require `confirmed = true` (the UI
/// shows a warning first).
#[tauri::command]
pub async fn open_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
    index: usize,
    confirmed: bool,
) -> Res<OpenResult> {
    let att = load(&state, message_id, index)?;
    let name = safe_filename(&att.name);
    if is_risky(&name, &att.mime, &att.data) && !confirmed {
        return Ok(OpenResult {
            status: "needs_confirmation",
            reason: Some(format!(
                "\"{name}\" is a program or script. Opening it can run code on your computer. Only continue if you trust the sender and expected this file."
            )),
        });
    }
    let base = app
        .path()
        .app_cache_dir()
        .map_err(|e| Error::Invalid(format!("cache dir: {e}")))?
        .join("open-attachments");
    std::fs::create_dir_all(&base)?;
    crate::restrict_dir(&base);
    let dir = base.join(token());
    std::fs::create_dir(&dir)?;
    crate::restrict_dir(&dir);
    let path = dir.join(&name);
    // The joined path must stay inside `dir` (defence in depth; the name
    // has no separators after sanitizing).
    if path.parent() != Some(dir.as_path()) {
        return Err(Error::Invalid("unsafe attachment name".into()));
    }
    write_private(&path, &att.data)?;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| Error::Invalid(format!("could not open attachment: {e}")))?;
    tracing::info!(
        risky_confirmed = confirmed,
        "attachment opened with default app"
    );
    Ok(OpenResult {
        status: "opened",
        reason: None,
    })
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, data)
}
