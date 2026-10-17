//! Templates ("quick replies"): saved snippets the composer can insert.
//! They are kept in the local cache database and shared by all accounts.

use neutron_core::store::Template;
use tauri::State;

use crate::commands::Res;
use crate::AppState;

#[tauri::command]
pub fn list_templates(state: State<'_, AppState>) -> Res<Vec<Template>> {
    state.store.templates()
}

#[tauri::command]
pub fn save_template(
    state: State<'_, AppState>,
    id: Option<i64>,
    name: String,
    html: String,
) -> Res<Template> {
    state.store.save_template(id, &name, &html)
}

#[tauri::command]
pub fn delete_template(state: State<'_, AppState>, id: i64) -> Res<()> {
    state.store.delete_template(id)
}
