//! GNOME integration.
//!
//! **Contacts (real):** evolution-data-server over D-Bus with zbus. The
//! session's address books are discovered via the EDS source registry
//! (`org.gnome.evolution.dataserver.Sources5`), opened through the address
//! book factory (`…AddressBook10`), and queried with an EBookQuery
//! S-expression `(contains "x-evolution-any-field" "<q>")`, which matches
//! names, nicknames and emails. The D-Bus connection stays open because EDS
//! closes a book when the client that opened it disconnects. Every call
//! has a timeout; if EDS is missing, the caller falls back to the address
//! history in the mail cache.
//!
//! **Calendar (real, v0.4):** the same registry lists `[Calendar]` sources;
//! each is opened through the calendar factory (`…Calendar8`) and queried
//! with `(occur-in-time-range? …)`. Read-only, except that an accepted
//! invitation can be added to the local "Personal" calendar with
//! `CreateObjects`.

use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::{Error, Result};

const SOURCES_BUS: &str = "org.gnome.evolution.dataserver.Sources5";
const SOURCES_PATH: &str = "/org/gnome/evolution/dataserver/SourceManager";
const SOURCE_IFACE: &str = "org.gnome.evolution.dataserver.Source";
const BOOK_FACTORY_BUS: &str = "org.gnome.evolution.dataserver.AddressBook10";
const BOOK_FACTORY_PATH: &str = "/org/gnome/evolution/dataserver/AddressBookFactory";
const BOOK_FACTORY_IFACE: &str = "org.gnome.evolution.dataserver.AddressBookFactory";
const BOOK_IFACE: &str = "org.gnome.evolution.dataserver.AddressBook";
/// UID of the local "Personal" address book every EDS installation has.
pub const SYSTEM_BOOK: &str = "system-address-book";
const CALL_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContactHit {
    pub name: String,
    pub email: String,
    /// Address book display name.
    pub book: String,
}

struct Book {
    uid: String,
    name: String,
    bus: String,
    path: OwnedObjectPath,
}

/// Open connection to the user's EDS address books.
pub struct Eds {
    conn: zbus::Connection,
    books: Vec<Book>,
}

fn dbus_err(e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("contacts (EDS): {e}"))
}

async fn timed<T>(f: impl std::future::Future<Output = zbus::Result<T>>) -> Result<T> {
    tokio::time::timeout(CALL_TIMEOUT, f)
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(dbus_err)
}

/// `key=value` from a section of an EDS source keyfile.
fn keyfile_value<'a>(data: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut in_section = false;
    for line in data.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_section = l == format!("[{section}]");
        } else if in_section {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim());
                }
            }
        }
    }
    None
}

impl Eds {
    /// Connect to the session bus and open every enabled address book.
    pub async fn connect() -> Result<Self> {
        let conn = timed(zbus::Connection::session()).await?;
        let reply = timed(conn.call_method(
            Some(SOURCES_BUS),
            SOURCES_PATH,
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        ))
        .await?;
        type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
        let objects: Objects = reply.body().deserialize().map_err(dbus_err)?;
        let mut sources: Vec<(String, String)> = objects
            .values()
            .filter_map(|ifaces| {
                let props = ifaces.get(SOURCE_IFACE)?;
                let uid = String::try_from(props.get("UID")?.try_clone().ok()?).ok()?;
                let data = String::try_from(props.get("Data")?.try_clone().ok()?).ok()?;
                let is_book = data.lines().any(|l| l.trim() == "[Address Book]");
                let enabled = keyfile_value(&data, "Data Source", "Enabled") != Some("false");
                let name = keyfile_value(&data, "Data Source", "DisplayName")
                    .unwrap_or(&uid)
                    .to_string();
                (is_book && enabled).then_some((uid, name))
            })
            .collect();
        // Local book first, then the rest in a stable order.
        sources.sort_by_key(|(uid, name)| (uid != SYSTEM_BOOK, name.clone()));

        let mut books = Vec::new();
        let mut problems = Vec::new();
        if sources.is_empty() {
            problems.push(format!(
                "no enabled address book among {} sources",
                objects.len()
            ));
        }
        for (uid, name) in sources {
            match Self::open_book(&conn, &uid).await {
                Ok((path, bus)) => books.push(Book {
                    uid,
                    name,
                    bus,
                    path,
                }),
                // A remote book that needs credentials or network must not
                // break autocomplete for the others.
                Err(e) => {
                    tracing::info!(book = %uid, "address book not available: {e}");
                    problems.push(format!("{uid}: {e}"));
                }
            }
        }
        if books.is_empty() {
            return Err(Error::Invalid(format!(
                "contacts (EDS): no address book could be opened ({})",
                problems.join("; ")
            )));
        }
        Ok(Self { conn, books })
    }

    async fn open_book(conn: &zbus::Connection, uid: &str) -> Result<(OwnedObjectPath, String)> {
        let reply = timed(conn.call_method(
            Some(BOOK_FACTORY_BUS),
            BOOK_FACTORY_PATH,
            Some(BOOK_FACTORY_IFACE),
            "OpenAddressBook",
            &(uid,),
        ))
        .await?;
        // EDS returns the object path as a plain string (signature "ss").
        let (path, bus): (String, String) = reply.body().deserialize().map_err(dbus_err)?;
        let path = OwnedObjectPath::try_from(path).map_err(dbus_err)?;
        timed(conn.call_method(
            Some(bus.as_str()),
            path.as_str(),
            Some(BOOK_IFACE),
            "Open",
            &(),
        ))
        .await?;
        Ok((path, bus))
    }

    pub fn book_count(&self) -> usize {
        self.books.len()
    }

    /// Contacts whose any field contains `query`, one hit per email
    /// address, from all open books (local book first).
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<ContactHit>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let sexp = format!(
            "(contains \"x-evolution-any-field\" \"{}\")",
            sexp_escape(q)
        );
        let mut hits = Vec::new();
        for b in &self.books {
            let reply = match timed(self.conn.call_method(
                Some(b.bus.as_str()),
                b.path.as_str(),
                Some(BOOK_IFACE),
                "GetContactList",
                &(sexp.as_str(),),
            ))
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::info!(book = %b.uid, "contact query failed: {e}");
                    continue;
                }
            };
            let vcards: Vec<String> = reply.body().deserialize().map_err(dbus_err)?;
            for v in vcards {
                let c = parse_vcard(&v);
                for email in c.emails {
                    hits.push(ContactHit {
                        name: c.name.clone(),
                        email,
                        book: b.name.clone(),
                    });
                }
            }
            if hits.len() >= limit {
                break;
            }
        }
        hits.truncate(limit);
        Ok(hits)
    }

    /// Whether any address book has a contact with exactly this email.
    pub async fn has_email(&self, email: &str) -> Result<bool> {
        let email = email.trim();
        Ok(self
            .search(email, 50)
            .await?
            .iter()
            .any(|h| h.email.trim().eq_ignore_ascii_case(email)))
    }

    /// Store a vCard in the local ("Personal") address book. Returns the
    /// new contact UID.
    pub async fn add_contact_vcard(&self, vcard: &str) -> Result<String> {
        let b = self
            .books
            .iter()
            .find(|b| b.uid == SYSTEM_BOOK)
            .ok_or_else(|| {
                Error::Invalid("contacts (EDS): local address book not available".into())
            })?;
        let reply = timed(self.conn.call_method(
            Some(b.bus.as_str()),
            b.path.as_str(),
            Some(BOOK_IFACE),
            "CreateContacts",
            &(vec![vcard], 0u32),
        ))
        .await?;
        let uids: Vec<String> = reply.body().deserialize().map_err(dbus_err)?;
        uids.into_iter()
            .next()
            .ok_or_else(|| dbus_err("no UID returned"))
    }
}

/// vCard 3.0 for a new contact with one email address. Names and
/// addresses come from received mail, so both are validated and escaped
/// (RFC 2426 TEXT: backslash, comma, semicolon, newline); control
/// characters are dropped so no extra properties can be injected.
pub fn contact_vcard(name: &str, email: &str) -> Result<String> {
    let email = email.trim();
    if email.is_empty()
        || email.len() > 320
        || !email.contains('@')
        || email.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(Error::Invalid(format!(
            "\"{email}\" is not an email address"
        )));
    }
    let clean = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect::<String>()
            .trim()
            .to_string()
    };
    let esc = |s: &str| -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                ',' => out.push_str("\\,"),
                ';' => out.push_str("\\;"),
                _ => out.push(c),
            }
        }
        out
    };
    let name = clean(name);
    let shown = if name.is_empty() {
        email.to_string()
    } else {
        name.clone()
    };
    // Structured name: "Given Family" -> N:Family;Given.
    let (given, family) = match name.rsplit_once(' ') {
        Some((g, f)) => (g.trim(), f.trim()),
        None => (name.as_str(), ""),
    };
    Ok(format!(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:{}\r\nN:{};{};;;\r\nEMAIL;TYPE=INTERNET:{}\r\nEND:VCARD\r\n",
        esc(&shown),
        esc(family),
        esc(given),
        esc(email)
    ))
}

/// Escape a string for an EBookQuery S-expression literal.
pub fn sexp_escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(100)
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            c => vec![c],
        })
        .collect()
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct VCardContact {
    pub name: String,
    pub emails: Vec<String>,
}

/// Minimal vCard 3/4 reader: FN (or N) and EMAIL properties.
pub fn parse_vcard(v: &str) -> VCardContact {
    // Unfold continuation lines (RFC 6350 §3.2).
    let unfolded = v
        .replace("\r\n ", "")
        .replace("\r\n\t", "")
        .replace("\n ", "")
        .replace("\n\t", "");
    let mut c = VCardContact::default();
    let mut n_name = String::new();
    for line in unfolded.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let prop = key.split(';').next().unwrap_or("").to_ascii_uppercase();
        // Strip a group prefix like "item1.EMAIL".
        let prop = prop.rsplit('.').next().unwrap_or("").to_string();
        let value = unescape(value.trim());
        match prop.as_str() {
            "FN" => c.name = value,
            "N" => {
                let parts: Vec<&str> = value.split(';').collect();
                n_name = format!(
                    "{} {}",
                    parts.get(1).unwrap_or(&""),
                    parts.first().unwrap_or(&"")
                )
                .trim()
                .to_string();
            }
            "EMAIL" if value.contains('@') => c.emails.push(value),
            _ => {}
        }
    }
    if c.name.is_empty() {
        c.name = n_name;
    }
    c
}

fn unescape(s: &str) -> String {
    s.replace("\\n", " ")
        .replace("\\N", " ")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

const CAL_FACTORY_BUS: &str = "org.gnome.evolution.dataserver.Calendar8";
const CAL_FACTORY_PATH: &str = "/org/gnome/evolution/dataserver/CalendarFactory";
const CAL_FACTORY_IFACE: &str = "org.gnome.evolution.dataserver.CalendarFactory";
const CAL_IFACE: &str = "org.gnome.evolution.dataserver.Calendar";
/// UID of the local "Personal" calendar.
pub const SYSTEM_CALENDAR: &str = "system-calendar";

/// One occurrence of an event, for the agenda.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    /// Calendar display name.
    pub calendar: String,
    pub color: String,
}

async fn registry_sources(
    conn: &zbus::Connection,
    section: &str,
) -> Result<Vec<(String, String, String)>> {
    let reply = timed(conn.call_method(
        Some(SOURCES_BUS),
        SOURCES_PATH,
        Some("org.freedesktop.DBus.ObjectManager"),
        "GetManagedObjects",
        &(),
    ))
    .await?;
    type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;
    let objects: Objects = reply.body().deserialize().map_err(dbus_err)?;
    let header = format!("[{section}]");
    let mut out: Vec<(String, String, String)> = objects
        .values()
        .filter_map(|ifaces| {
            let props = ifaces.get(SOURCE_IFACE)?;
            let uid = String::try_from(props.get("UID")?.try_clone().ok()?).ok()?;
            let data = String::try_from(props.get("Data")?.try_clone().ok()?).ok()?;
            let has = data.lines().any(|l| l.trim() == header);
            let enabled = keyfile_value(&data, "Data Source", "Enabled") != Some("false")
                && keyfile_value(&data, section, "Selected") != Some("false");
            let name = keyfile_value(&data, "Data Source", "DisplayName")
                .unwrap_or(&uid)
                .to_string();
            let color = keyfile_value(&data, section, "Color")
                .filter(|c| {
                    c.len() <= 9
                        && c.starts_with('#')
                        && c[1..].chars().all(|x| x.is_ascii_hexdigit())
                })
                .unwrap_or("")
                .to_string();
            (has && enabled).then_some((uid, name, color))
        })
        .collect();
    out.sort_by_key(|(uid, name, _)| (uid != SYSTEM_CALENDAR, name.clone()));
    Ok(out)
}

struct Cal {
    uid: String,
    name: String,
    color: String,
    bus: String,
    path: OwnedObjectPath,
}

/// Open connection to the user's EDS calendars.
pub struct EdsCalendar {
    conn: zbus::Connection,
    cals: Vec<Cal>,
}

fn make_time(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%SZ")
        .to_string()
}

impl EdsCalendar {
    pub async fn connect() -> Result<Self> {
        let conn = timed(zbus::Connection::session()).await?;
        let sources = registry_sources(&conn, "Calendar").await?;
        let mut cals = Vec::new();
        let mut problems = Vec::new();
        for (uid, name, color) in sources {
            match Self::open(&conn, &uid).await {
                Ok((path, bus)) => cals.push(Cal {
                    uid,
                    name,
                    color,
                    bus,
                    path,
                }),
                Err(e) => {
                    tracing::info!(calendar = %uid, "calendar not available: {e}");
                    problems.push(format!("{uid}: {e}"));
                }
            }
        }
        if cals.is_empty() {
            return Err(Error::Invalid(format!(
                "calendar (EDS): no calendar could be opened ({})",
                problems.join("; ")
            )));
        }
        Ok(Self { conn, cals })
    }

    async fn open(conn: &zbus::Connection, uid: &str) -> Result<(OwnedObjectPath, String)> {
        let reply = timed(conn.call_method(
            Some(CAL_FACTORY_BUS),
            CAL_FACTORY_PATH,
            Some(CAL_FACTORY_IFACE),
            "OpenCalendar",
            &(uid,),
        ))
        .await?;
        let (path, bus): (String, String) = reply.body().deserialize().map_err(dbus_err)?;
        let path = OwnedObjectPath::try_from(path).map_err(dbus_err)?;
        timed(conn.call_method(
            Some(bus.as_str()),
            path.as_str(),
            Some(CAL_IFACE),
            "Open",
            &(),
        ))
        .await?;
        Ok((path, bus))
    }

    pub fn calendar_count(&self) -> usize {
        self.cals.len()
    }

    /// Event occurrences overlapping `[from, to)` from all calendars,
    /// sorted by start time.
    pub async fn events_in_range(&self, from: i64, to: i64) -> Result<Vec<CalendarEvent>> {
        let sexp = format!(
            "(occur-in-time-range? (make-time \"{}\") (make-time \"{}\"))",
            make_time(from),
            make_time(to)
        );
        let mut out = Vec::new();
        for c in &self.cals {
            let reply = match timed(self.conn.call_method(
                Some(c.bus.as_str()),
                c.path.as_str(),
                Some(CAL_IFACE),
                "GetObjectList",
                &(sexp.as_str(),),
            ))
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::info!(calendar = %c.uid, "calendar query failed: {e}");
                    continue;
                }
            };
            let objects: Vec<String> = reply.body().deserialize().map_err(dbus_err)?;
            // Detached occurrences (RECURRENCE-ID) replace the series' instance.
            let mut parsed: Vec<crate::ical::Event> = objects
                .iter()
                .filter_map(|o| crate::ical::parse(o).ok())
                .flat_map(|cal| cal.events)
                .collect();
            parsed.sort_by_key(|e| e.recurrence_id.is_some());
            let overrides: Vec<(String, i64)> = parsed
                .iter()
                .filter_map(|e| {
                    let r = e.recurrence_id.as_ref()?;
                    Some((e.uid.clone(), crate::ical::parse_time(r, &[])?.0))
                })
                .collect();
            for e in &parsed {
                if e.status == "CANCELLED" {
                    continue;
                }
                for (s, en) in crate::ical::occurrences(e, from, to) {
                    if e.recurrence_id.is_none()
                        && overrides.iter().any(|(u, t)| *u == e.uid && *t == s)
                    {
                        continue;
                    }
                    out.push(CalendarEvent {
                        uid: e.uid.clone(),
                        summary: e.summary.clone(),
                        location: e.location.clone(),
                        start: s,
                        end: en,
                        all_day: e.all_day,
                        calendar: c.name.clone(),
                        color: c.color.clone(),
                    });
                }
            }
        }
        out.sort_by_key(|e| (e.start, e.end));
        Ok(out)
    }

    /// Add a VEVENT to the local calendar. If an event with the same UID
    /// exists it is replaced (an updated invitation).
    pub async fn add_event(&self, vevent: &str) -> Result<String> {
        let c = self
            .cals
            .iter()
            .find(|c| c.uid == SYSTEM_CALENDAR)
            .or_else(|| self.cals.first())
            .ok_or_else(|| Error::Invalid("calendar (EDS): no calendar".into()))?;
        let uid = crate::ical::parse(vevent)?
            .events
            .first()
            .map(|e| e.uid.clone())
            .ok_or_else(|| Error::Invalid("calendar (EDS): no event".into()))?;
        let exists = timed(self.conn.call_method(
            Some(c.bus.as_str()),
            c.path.as_str(),
            Some(CAL_IFACE),
            "GetObject",
            &(uid.as_str(), ""),
        ))
        .await
        .is_ok();
        if exists {
            timed(self.conn.call_method(
                Some(c.bus.as_str()),
                c.path.as_str(),
                Some(CAL_IFACE),
                "ModifyObjects",
                &(vec![vevent], "all", 0u32),
            ))
            .await?;
            return Ok(uid);
        }
        let reply = timed(self.conn.call_method(
            Some(c.bus.as_str()),
            c.path.as_str(),
            Some(CAL_IFACE),
            "CreateObjects",
            &(vec![vevent], 0u32),
        ))
        .await?;
        let uids: Vec<String> = reply.body().deserialize().map_err(dbus_err)?;
        Ok(uids.into_iter().next().unwrap_or(uid))
    }
}

/// Escape text for the notification body (GNOME Shell interprets a small
/// markup subset).
pub fn notification_escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(300)
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Show a desktop notification via `org.freedesktop.Notifications`
/// (GNOME Shell). `replaces` lets one bubble be updated instead of stacking.
pub async fn notify(summary: &str, body: &str, replaces: u32) -> Result<u32> {
    let conn = timed(zbus::Connection::session()).await?;
    let mut hints: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", zbus::zvariant::Value::from("Neutron Mail"));
    hints.insert("category", zbus::zvariant::Value::from("email.arrived"));
    let reply = timed(conn.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "Neutron Mail",
            replaces,
            "mail-unread",
            notification_escape(summary),
            notification_escape(body),
            Vec::<&str>::new(),
            hints,
            -1i32,
        ),
    ))
    .await?;
    reply.body().deserialize::<u32>().map_err(dbus_err)
}

/// Unread count on the dock icon via the `com.canonical.Unity.LauncherEntry`
/// signal (shown by Dash to Dock / Ubuntu Dock; plain GNOME Shell ignores it).
pub async fn launcher_badge(desktop_file: &str, count: i64) -> Result<()> {
    let conn = timed(zbus::Connection::session()).await?;
    let mut props: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    props.insert("count", zbus::zvariant::Value::from(count.max(0)));
    props.insert("count-visible", zbus::zvariant::Value::from(count > 0));
    timed(conn.emit_signal(
        None::<&str>,
        "/dev/neutronmail/NeutronMail",
        "com.canonical.Unity.LauncherEntry",
        "Update",
        &(format!("application://{desktop_file}"), props),
    ))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_contact_vcards_are_escaped_and_round_trip() {
        let v = contact_vcard("Ayşe Yılmaz", "ayse@example.test").unwrap();
        assert!(v.contains("FN:Ayşe Yılmaz\r\n") && v.contains("N:Yılmaz;Ayşe;;;\r\n"));
        let c = parse_vcard(&v);
        assert_eq!(c.emails, vec!["ayse@example.test".to_string()]);
        // A hostile display name cannot add properties or break fields.
        let v = contact_vcard("Evil\r\nEMAIL:x@attacker.test;a,b\\", "v@example.test").unwrap();
        assert_eq!(v.lines().filter(|l| l.starts_with("EMAIL")).count(), 1);
        assert_eq!(parse_vcard(&v).emails, vec!["v@example.test".to_string()]);
        assert!(v.contains("FN:EvilEMAIL:x@attacker.test\\;a\\,b\\\\\r\n"));
        assert!(contact_vcard("", "not-an-address").is_err());
        assert!(contact_vcard("", "a b@example.test").is_err());
        assert!(contact_vcard("", "a@example.test\r\nTEL:1").is_err());
        assert!(contact_vcard("", "solo@example.test")
            .unwrap()
            .contains("FN:solo@example.test\r\n"));
    }

    #[test]
    fn vcard_parsing() {
        let v = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Yilmaz;Ayse;;;\r\nFN:Ayse Yilmaz\r\nEMAIL;TYPE=WORK:ayse@exam\r\n ple.test\r\nitem1.EMAIL:ayse.private@example.test\r\nTEL:123\r\nEND:VCARD\r\n";
        let c = parse_vcard(v);
        assert_eq!(c.name, "Ayse Yilmaz");
        assert_eq!(
            c.emails,
            vec!["ayse@example.test", "ayse.private@example.test"]
        );
        let n_only = parse_vcard("BEGIN:VCARD\nN:Doe;Jane\nEMAIL:j@d.test\nEND:VCARD");
        assert_eq!(n_only.name, "Jane Doe");
    }

    #[test]
    fn notification_text_is_escaped() {
        assert_eq!(
            notification_escape("<b>Hi</b> & \u{7}bye"),
            "&lt;b&gt;Hi&lt;/b&gt; &amp; bye"
        );
        assert_eq!(notification_escape(&"x".repeat(500)).len(), 300);
    }

    #[test]
    fn sexp_escaping() {
        assert_eq!(sexp_escape(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(sexp_escape("x\u{0}\ny"), "xy");
    }

    #[test]
    fn keyfile() {
        let d = "\n[Data Source]\nDisplayName=Personal\nEnabled=true\n[Address Book]\nBackendName=local\n";
        assert_eq!(
            keyfile_value(d, "Data Source", "DisplayName"),
            Some("Personal")
        );
        assert_eq!(
            keyfile_value(d, "Address Book", "BackendName"),
            Some("local")
        );
    }
}
