//! Notes and To-do commands (local storage in the encrypted cache) and the
//! reminder loop (desktop notifications).

use neutron_core::pim::{
    EmailLink, Note, NoteFolder, NoteInput, NoteQuery, NoteSummary, Tag, Task, TaskCounts,
    TaskInput, TaskList, TaskView,
};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::{commands::Res, AppState};

#[tauri::command]
pub fn note_folders(state: State<'_, AppState>) -> Res<Vec<NoteFolder>> {
    state.store.note_folders()
}
#[tauri::command]
pub fn create_note_folder(state: State<'_, AppState>, name: String) -> Res<i64> {
    state.store.create_note_folder(&name)
}
#[tauri::command]
pub fn rename_note_folder(state: State<'_, AppState>, id: i64, name: String) -> Res<()> {
    state.store.rename_note_folder(id, &name)
}
#[tauri::command]
pub fn delete_note_folder(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store.delete_note_folder(id)
}
#[tauri::command]
pub fn note_tags(state: State<'_, AppState>) -> Res<Vec<Tag>> {
    state.store.tags()
}
#[tauri::command]
pub fn list_notes(state: State<'_, AppState>, query: NoteQuery) -> Res<Vec<NoteSummary>> {
    state.store.notes(&query)
}
#[tauri::command]
pub fn get_note(state: State<'_, AppState>, id: i64) -> Res<Note> {
    state.store.note(id)
}
#[tauri::command]
pub fn save_note(state: State<'_, AppState>, note: NoteInput) -> Res<Note> {
    state.store.save_note(&note)
}
#[tauri::command]
pub fn delete_note(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store.delete_note(id)
}
/// "Save to Notes": a new note with the message's subject and text, linked
/// to the message.
#[tauri::command]
pub fn note_from_message(
    state: State<'_, AppState>,
    message_id: i64,
    folder_id: Option<i64>,
) -> Res<Note> {
    let link = state.store.email_link_for(message_id)?;
    let detail = state.store.message_detail(message_id)?;
    let text: String = detail.text.chars().take(20_000).collect();
    let html = format!(
        "<p><b>From:</b> {}</p>{}",
        neutron_core::sanitize::escape_html(&link.sender),
        neutron_core::sanitize::text_to_html(&text)
    );
    let title = if link.subject.trim().is_empty() {
        "(no subject)".to_string()
    } else {
        link.subject.chars().take(300).collect()
    };
    let note = state.store.save_note(&NoteInput {
        id: None,
        folder_id,
        title,
        html,
        tags: vec![],
        pinned: false,
    })?;
    state.store.link_email("note", note.id, &link)?;
    state.store.note(note.id)
}
#[tauri::command]
pub fn link_note_email(state: State<'_, AppState>, note_id: i64, message_id: i64) -> Res<Note> {
    let link = state.store.email_link_for(message_id)?;
    state.store.link_email("note", note_id, &link)?;
    state.store.note(note_id)
}
#[tauri::command]
pub fn unlink_note_email(
    state: State<'_, AppState>,
    note_id: i64,
    message_id: String,
) -> Res<Note> {
    state.store.unlink_email("note", note_id, &message_id)?;
    state.store.note(note_id)
}
/// Link data for a cached message (prefills "Add to To-do").
#[tauri::command]
pub fn email_link(state: State<'_, AppState>, message_id: i64) -> Res<EmailLink> {
    state.store.email_link_for(message_id)
}
/// The cached message a link points to (None if it isn't cached any more).
#[tauri::command]
pub fn resolve_email_link(state: State<'_, AppState>, link: EmailLink) -> Res<Option<i64>> {
    state.store.resolve_email(&link)
}

#[tauri::command]
pub fn flagged_emails(state: State<'_, AppState>) -> Res<Vec<neutron_core::model::MessageSummary>> {
    state.store.flagged_messages(200)
}
#[tauri::command]
pub fn task_lists(state: State<'_, AppState>) -> Res<Vec<TaskList>> {
    state.store.task_lists()
}
#[tauri::command]
pub fn create_task_list(state: State<'_, AppState>, name: String, color: String) -> Res<i64> {
    state.store.create_task_list(&name, &color)
}
#[tauri::command]
pub fn rename_task_list(state: State<'_, AppState>, id: i64, name: String) -> Res<()> {
    state.store.rename_task_list(id, &name)
}
#[tauri::command]
pub fn delete_task_list(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store.delete_task_list(id)
}
#[tauri::command]
pub fn list_tasks(state: State<'_, AppState>, view: TaskView) -> Res<Vec<Task>> {
    state.store.tasks(view)
}
#[tauri::command]
pub fn task_counts(state: State<'_, AppState>, day_end: i64) -> Res<TaskCounts> {
    state.store.task_counts(day_end)
}
#[tauri::command]
pub fn save_task(state: State<'_, AppState>, task: TaskInput) -> Res<Task> {
    state.store.save_task(&task)
}
#[tauri::command]
pub fn set_task_completed(state: State<'_, AppState>, id: i64, done: bool) -> Res<Task> {
    state.store.set_task_completed(id, done)
}
#[tauri::command]
pub fn delete_task(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store.delete_task(id)
}

/// Every 20 s: fire due reminders as desktop notifications and tell the UI.
pub fn start_reminders(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let due = app.state::<AppState>().store.take_due_reminders(now);
            match due {
                Ok(tasks) => {
                    for t in tasks {
                        let body = t
                            .email
                            .as_ref()
                            .map(|e| format!("From email: {}", e.subject))
                            .unwrap_or_default();
                        if let Err(e) =
                            neutron_core::gnome::notify(&format!("Reminder: {}", t.title), &body, 0)
                                .await
                        {
                            tracing::warn!("reminder notification failed: {e}");
                        }
                        let _ = app.emit("pim://reminder", &t);
                    }
                }
                Err(e) => tracing::warn!("reminder check failed: {e}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
    });
}
