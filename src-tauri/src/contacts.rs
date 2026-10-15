//! Recipient autocomplete: GNOME address books (evolution-data-server)
//! first, then addresses seen in the mail cache. EDS is connected lazily,
//! retried at most once a minute when unavailable, and results are cached
//! for 30 s per query.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use neutron_core::gnome::Eds;
use neutron_core::model::ContactSuggestion;
use serde::Serialize;
use tauri::State;

use crate::commands::Res;
use crate::AppState;

const CACHE_TTL: Duration = Duration::from_secs(30);
const RETRY_AFTER: Duration = Duration::from_secs(60);
const LIMIT: usize = 8;

#[derive(Default)]
pub struct Contacts {
    inner: tokio::sync::Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    eds: Option<Eds>,
    last_attempt: Option<Instant>,
    last_error: Option<String>,
    cache: HashMap<(i64, String), (Instant, Vec<ContactSuggestion>)>,
}

impl Inner {
    async fn ensure_eds(&mut self) {
        if self.eds.is_some() || self.last_attempt.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
            return;
        }
        self.last_attempt = Some(Instant::now());
        match Eds::connect().await {
            Ok(eds) => {
                tracing::info!(books = eds.book_count(), "GNOME contacts (EDS) connected");
                self.eds = Some(eds);
                self.last_error = None;
            }
            Err(e) => {
                tracing::info!("GNOME contacts unavailable, using mail history only: {e}");
                self.last_error = Some(e.to_string());
            }
        }
    }
}

#[tauri::command]
pub async fn suggest_contacts(
    state: State<'_, AppState>,
    account_id: i64,
    query: String,
) -> Res<Vec<ContactSuggestion>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    tracing::debug!(len = q.len(), "contacts: suggest");
    let mut inner = state.contacts.inner.lock().await;
    if let Some((t, hits)) = inner.cache.get(&(account_id, q.clone())) {
        if t.elapsed() < CACHE_TTL {
            return Ok(hits.clone());
        }
    }
    inner.ensure_eds().await;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut eds_failed = false;
    if let Some(eds) = inner.eds.as_ref() {
        match eds.search(&q, LIMIT).await {
            Ok(hits) => {
                tracing::debug!(hits = hits.len(), "contacts: EDS results");
                for h in hits {
                    if seen.insert(h.email.to_lowercase()) {
                        out.push(ContactSuggestion {
                            name: h.name,
                            email: h.email,
                            source: "contacts",
                        });
                    }
                }
            }
            Err(e) => {
                // EDS went away (e.g. the session restarted): reconnect later.
                tracing::info!("contact search failed: {e}");
                eds_failed = true;
            }
        }
    }
    if eds_failed {
        inner.eds = None;
    }
    // Mail history ranks below address-book contacts.
    for (name, email) in state.store.address_history(account_id, &q, LIMIT as i64)? {
        if out.len() >= LIMIT {
            break;
        }
        if seen.insert(email.to_lowercase()) {
            out.push(ContactSuggestion {
                name,
                email,
                source: "history",
            });
        }
    }
    out.truncate(LIMIT);
    if inner.cache.len() > 500 {
        inner.cache.clear();
    }
    inner
        .cache
        .insert((account_id, q), (Instant::now(), out.clone()));
    Ok(out)
}

#[derive(Serialize)]
pub struct ContactsStatus {
    eds_connected: bool,
    books: usize,
    error: Option<String>,
}

#[tauri::command]
pub async fn contacts_status(state: State<'_, AppState>) -> Res<ContactsStatus> {
    let mut inner = state.contacts.inner.lock().await;
    inner.ensure_eds().await;
    Ok(ContactsStatus {
        eds_connected: inner.eds.is_some(),
        books: inner.eds.as_ref().map_or(0, |e| e.book_count()),
        error: inner.last_error.clone(),
    })
}
