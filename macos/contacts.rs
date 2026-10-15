//! macOS preview: suggestions come only from the mail cache.
//! No Apple Contacts framework, permission, or system address book access.

use neutron_core::model::ContactSuggestion;
use serde::Serialize;
use tauri::State;

use crate::{commands::Res, AppState};

#[derive(Default)]
pub struct Contacts {
    _private: (),
}

#[tauri::command]
pub async fn suggest_contacts(
    state: State<'_, AppState>,
    account_id: i64,
    query: String,
) -> Res<Vec<ContactSuggestion>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    Ok(state
        .store
        .address_history(account_id, &query, 8)?
        .into_iter()
        .map(|(name, email)| ContactSuggestion {
            name,
            email,
            source: "history",
        })
        .collect())
}

#[derive(Serialize)]
pub struct ContactsStatus {
    eds_connected: bool,
    books: usize,
    error: Option<String>,
}

#[tauri::command]
pub async fn contacts_status() -> Res<ContactsStatus> {
    Ok(ContactsStatus {
        eds_connected: false,
        books: 0,
        error: None,
    })
}
