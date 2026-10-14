//! GNOME integration interfaces (stubs).
//!
//! The research report recommends libebook/libecal (evolution-data-server)
//! on a dedicated glib main-loop thread, and GNOME Online Accounts over
//! D-Bus. These traits define the boundary the rest of the app codes
//! against; the prototype ships only no-op implementations.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContactHit {
    pub name: String,
    pub email: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub start: i64,
    pub end: i64,
    pub calendar: String,
}

/// Address-book access (future: EDS `EBookClient` + `EBookQuery`).
pub trait ContactsProvider: Send + Sync {
    fn autocomplete(&self, prefix: &str, limit: usize) -> crate::Result<Vec<ContactHit>>;
    fn add_contact_vcard(&self, vcard: &str) -> crate::Result<String>;
}

/// Calendar access (future: EDS `ECalClient`, `occur-in-time-range?`).
pub trait CalendarProvider: Send + Sync {
    fn events_in_range(&self, start: i64, end: i64) -> crate::Result<Vec<CalendarEvent>>;
    fn create_event_ics(&self, ics: &str) -> crate::Result<String>;
}

/// No-op implementation used until EDS integration lands.
#[derive(Debug, Default)]
pub struct Unavailable;

impl ContactsProvider for Unavailable {
    fn autocomplete(&self, _prefix: &str, _limit: usize) -> crate::Result<Vec<ContactHit>> {
        Ok(Vec::new())
    }
    fn add_contact_vcard(&self, _vcard: &str) -> crate::Result<String> {
        Err(crate::Error::Invalid(
            "GNOME Contacts integration is not implemented yet".into(),
        ))
    }
}

impl CalendarProvider for Unavailable {
    fn events_in_range(&self, _start: i64, _end: i64) -> crate::Result<Vec<CalendarEvent>> {
        Ok(Vec::new())
    }
    fn create_event_ics(&self, _ics: &str) -> crate::Result<String> {
        Err(crate::Error::Invalid(
            "GNOME Calendar integration is not implemented yet".into(),
        ))
    }
}
