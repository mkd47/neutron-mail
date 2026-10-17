//! Messages as files: save one as .eml, and import .eml / mbox files
//! (Thunderbird, Apple Mail, Gmail Takeout) into a folder on the server.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use neutron_core::{attachments::safe_filename, imap, mbox, Error};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::Res;
use crate::AppState;

/// Save the open message, exactly as received, to a .eml file.
#[tauri::command]
pub async fn save_eml(
    app: AppHandle,
    state: State<'_, AppState>,
    message_id: i64,
) -> Res<Option<String>> {
    let raw = state.store.raw(message_id)?;
    let subject = Some(neutron_core::parse::parse(&raw).subject)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "message".into());
    let stem: String = subject.chars().take(80).collect();
    let name = format!("{}.eml", safe_filename(&stem));
    let target = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Save message")
            .set_file_name(&name)
            .add_filter("Email message", &["eml"])
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
    std::fs::write(&path, &raw)?;
    tracing::info!(bytes = raw.len(), "message saved as .eml");
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[derive(Serialize)]
pub struct ImportResult {
    imported: usize,
    /// Messages over the size limit, or that the server refused.
    skipped: usize,
    /// First server error, if any message was refused.
    error: Option<String>,
}

#[derive(Clone, Serialize)]
struct ImportProgress {
    folder_id: i64,
    imported: usize,
}

/// Pick .eml / mbox files and APPEND every message in them to the folder
/// (marked read). The folder syncs afterwards as usual.
#[tauri::command]
pub async fn import_messages(
    app: AppHandle,
    state: State<'_, AppState>,
    folder_id: i64,
) -> Res<Option<ImportResult>> {
    let folder = state.store.folder(folder_id)?;
    let account = state.store.account(folder.account_id)?;
    let dialog = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        dialog
            .dialog()
            .file()
            .set_title("Import messages")
            .add_filter("Mail files (.eml, mbox)", &["eml", "mbox", "mbx", "txt"])
            .add_filter("All files", &["*"])
            .blocking_pick_files()
    })
    .await
    .map_err(|e| Error::Invalid(format!("file dialog failed: {e}")))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let paths: Vec<PathBuf> = picked
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect();
    let password = crate::providers::credential(&state, account.id).await?;
    let mut session = imap::connect(&account.config, &password).await?;
    let mut result = ImportResult {
        imported: 0,
        skipped: 0,
        error: None,
    };
    let mut failures = 0;
    let outcome = async {
        for path in &paths {
            // Read in a blocking thread, hand messages over one by one.
            let (tx, mut rx) = tokio::sync::mpsc::channel::<std::io::Result<mbox::Item>>(4);
            let p = path.clone();
            let reader = tauri::async_runtime::spawn_blocking(move || {
                let file = match std::fs::File::open(&p) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return;
                    }
                };
                let mut input = BufReader::with_capacity(1 << 16, file);
                let is_mbox = match input.fill_buf() {
                    Ok(start) => mbox::looks_like_mbox(start),
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        return;
                    }
                };
                if !is_mbox {
                    // A single message (.eml): never split, whatever it contains.
                    let _ = tx.blocking_send(mbox::single(input));
                    return;
                }
                for item in mbox::Reader::new(input) {
                    if tx.blocking_send(item).is_err() {
                        break;
                    }
                }
            });
            while let Some(item) = rx.recv().await {
                match item.map_err(Error::from)? {
                    None => result.skipped += 1,
                    Some(raw) => {
                        let mut res = imap::append(&mut session, &folder.name, &raw, true).await;
                        if res.is_err() {
                            // The connection may have dropped: reconnect once, try again.
                            if let Ok(fresh) = imap::connect(&account.config, &password).await {
                                session = fresh;
                                res = imap::append(&mut session, &folder.name, &raw, true).await;
                            }
                        }
                        match res {
                            Ok(()) => {
                                result.imported += 1;
                                failures = 0;
                                if result.imported.is_multiple_of(25) {
                                    let _ = app.emit(
                                        "mail://import-progress",
                                        ImportProgress {
                                            folder_id,
                                            imported: result.imported,
                                        },
                                    );
                                }
                            }
                            Err(e) => {
                                result.skipped += 1;
                                failures += 1;
                                result.error.get_or_insert(e.to_string());
                                // Several in a row: the server is gone or refusing.
                                if failures >= 5 {
                                    return Err(Error::Invalid(format!(
                                        "import stopped after {} messages: {e}",
                                        result.imported
                                    )));
                                }
                            }
                        }
                    }
                }
            }
            let _ = reader.await;
        }
        Ok::<_, Error>(())
    }
    .await;
    drop(session.logout().await);
    outcome?;
    tracing::info!(
        imported = result.imported,
        skipped = result.skipped,
        "messages imported"
    );
    Ok(Some(result))
}
