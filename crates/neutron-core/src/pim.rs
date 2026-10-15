//! Notes and To-do, stored locally in the (SQLCipher) cache database.
//!
//! The schema is laid out for later sync without implementing it yet:
//! every note, folder, list and task has a stable random `uid` (the future
//! IMAP-note `X-Uniform-Type-Identifier`/Message-ID or the CalDAV VTODO
//! `UID`), a `modified` time, a `sync_state` ("local" today; later
//! "synced"/"dirty"), `remote_ref`/`etag` columns for the server copy, and
//! deletions are tombstones (`deleted = 1`) so a future sync can propagate
//! them. Task priority uses the iCalendar scale (RFC 5545: 1 high, 5
//! medium, 9 low, 0 none) and subtasks use `parent_id` (VTODO RELATED-TO).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::store::Store;
use crate::{Error, Result};

pub const MAX_NOTE_HTML: usize = 1024 * 1024;
pub const MAX_TITLE: usize = 300;
pub const MAX_TASK_NOTES: usize = 20_000;
pub const MAX_NAME: usize = 80;
pub const MAX_TAGS: usize = 20;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS note_folders (
  id INTEGER PRIMARY KEY,
  uid TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0,
  modified INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  sync_state TEXT NOT NULL DEFAULT 'local',
  remote_ref TEXT,
  etag TEXT
);
CREATE TABLE IF NOT EXISTS notes (
  id INTEGER PRIMARY KEY,
  uid TEXT NOT NULL UNIQUE,
  folder_id INTEGER REFERENCES note_folders(id) ON DELETE SET NULL,
  title TEXT NOT NULL DEFAULT '',
  html TEXT NOT NULL DEFAULT '',
  text TEXT NOT NULL DEFAULT '',
  pinned INTEGER NOT NULL DEFAULT 0,
  created INTEGER NOT NULL,
  modified INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  sync_state TEXT NOT NULL DEFAULT 'local',
  remote_ref TEXT,
  etag TEXT
);
CREATE INDEX IF NOT EXISTS notes_modified ON notes(deleted, modified DESC);
CREATE TABLE IF NOT EXISTS note_tags (
  note_id INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  tag TEXT NOT NULL,
  PRIMARY KEY (note_id, tag)
);
CREATE TABLE IF NOT EXISTS tag_colors (
  tag TEXT PRIMARY KEY,
  color TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS email_links (
  id INTEGER PRIMARY KEY,
  owner TEXT NOT NULL CHECK (owner IN ('note', 'task')),
  owner_id INTEGER NOT NULL,
  account_id INTEGER,
  message_id TEXT NOT NULL,
  subject TEXT NOT NULL DEFAULT '',
  sender TEXT NOT NULL DEFAULT '',
  date INTEGER NOT NULL DEFAULT 0,
  UNIQUE (owner, owner_id, message_id)
);
CREATE TABLE IF NOT EXISTS task_lists (
  id INTEGER PRIMARY KEY,
  uid TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  color TEXT NOT NULL DEFAULT '#8b5cf6',
  position INTEGER NOT NULL DEFAULT 0,
  modified INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  sync_state TEXT NOT NULL DEFAULT 'local',
  remote_ref TEXT,
  ctag TEXT
);
CREATE TABLE IF NOT EXISTS tasks (
  id INTEGER PRIMARY KEY,
  uid TEXT NOT NULL UNIQUE,
  list_id INTEGER REFERENCES task_lists(id) ON DELETE SET NULL,
  parent_id INTEGER REFERENCES tasks(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  notes TEXT NOT NULL DEFAULT '',
  due INTEGER,
  due_time INTEGER NOT NULL DEFAULT 0,
  reminder INTEGER,
  reminded INTEGER NOT NULL DEFAULT 0,
  priority INTEGER NOT NULL DEFAULT 0,
  completed INTEGER,
  position INTEGER NOT NULL DEFAULT 0,
  created INTEGER NOT NULL,
  modified INTEGER NOT NULL,
  deleted INTEGER NOT NULL DEFAULT 0,
  sync_state TEXT NOT NULL DEFAULT 'local',
  remote_ref TEXT,
  etag TEXT
);
CREATE INDEX IF NOT EXISTS tasks_due ON tasks(deleted, completed, due);
"#;

pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    // First run: the default lists and note folders.
    let empty: bool = conn.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM task_lists) AND NOT EXISTS (SELECT 1 FROM note_folders)",
        [],
        |r| r.get(0),
    )?;
    if empty {
        let now = now();
        for (i, (name, color)) in [("Work", "#8b5cf6"), ("Personal", "#3b82f6")]
            .iter()
            .enumerate()
        {
            conn.execute(
                "INSERT INTO task_lists (uid, name, color, position, modified) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_uid(), name, color, i as i64, now],
            )?;
        }
        for (i, name) in ["Personal", "Work", "Ideas"].iter().enumerate() {
            conn.execute(
                "INSERT INTO note_folders (uid, name, position, modified) VALUES (?1, ?2, ?3, ?4)",
                params![new_uid(), name, i as i64, now],
            )?;
        }
    }
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Random RFC 4122 v4 UUID (usable as a VTODO UID).
pub fn new_uid() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS random source");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn clean_name(s: &str, what: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() || s.chars().count() > MAX_NAME || s.chars().any(char::is_control) {
        return Err(Error::Invalid(format!(
            "{what} names must be 1–{MAX_NAME} characters without control characters"
        )));
    }
    Ok(s.to_string())
}

fn clean_title(s: &str) -> Result<String> {
    let s: String = s
        .trim()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if s.chars().count() > MAX_TITLE {
        return Err(Error::Invalid(format!(
            "titles are limited to {MAX_TITLE} characters"
        )));
    }
    Ok(s)
}

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit())
}

/// Note bodies come from a contenteditable in the app: allow only basic
/// formatting, links (http/https/mailto), and checklists
/// (`<ul data-checklist>` with `<li data-checked="true|false">`). No
/// images, styles, classes or scripts; relative URLs are dropped.
pub fn sanitize_note_html(input: &str) -> String {
    let schemes: HashSet<&str> = ["http", "https", "mailto"].into_iter().collect();
    let mut attrs: HashMap<&str, HashSet<&str>> = HashMap::new();
    attrs.insert("a", ["href"].into_iter().collect());
    attrs.insert("ul", ["data-checklist"].into_iter().collect());
    attrs.insert("li", ["data-checked"].into_iter().collect());
    attrs.insert("ol", ["start"].into_iter().collect());
    let tags: HashSet<&str> = [
        "p",
        "br",
        "div",
        "h1",
        "h2",
        "h3",
        "b",
        "strong",
        "i",
        "em",
        "u",
        "s",
        "strike",
        "a",
        "ul",
        "ol",
        "li",
        "blockquote",
        "code",
        "pre",
        "hr",
    ]
    .into_iter()
    .collect();
    ammonia::Builder::default()
        .tags(tags)
        .url_schemes(schemes)
        .link_rel(Some("noopener noreferrer nofollow"))
        .url_relative(ammonia::UrlRelative::Deny)
        .tag_attributes(attrs)
        .generic_attributes(HashSet::new())
        .attribute_filter(|el, attr, value| match (el, attr) {
            ("li", "data-checked") => Some(Cow::Borrowed(if value == "true" {
                "true"
            } else {
                "false"
            })),
            ("ul", "data-checklist") => Some(Cow::Borrowed("")),
            _ => Some(Cow::Borrowed(value)),
        })
        .clean(input)
        .to_string()
}

// ---- types -----------------------------------------------------------------

/// A link to an email, by Message-ID (stable across folders and moves).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailLink {
    pub account_id: Option<i64>,
    pub message_id: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub sender: String,
    #[serde(default)]
    pub date: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteFolder {
    pub id: i64,
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub name: String,
    pub color: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteSummary {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub title: String,
    pub snippet: String,
    pub modified: i64,
    pub pinned: bool,
    pub tags: Vec<String>,
    pub linked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: i64,
    pub uid: String,
    pub folder_id: Option<i64>,
    pub title: String,
    pub html: String,
    pub created: i64,
    pub modified: i64,
    pub pinned: bool,
    pub tags: Vec<String>,
    pub links: Vec<EmailLink>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NoteInput {
    pub id: Option<i64>,
    pub folder_id: Option<i64>,
    pub title: String,
    pub html: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NoteQuery {
    pub folder_id: Option<i64>,
    pub tag: Option<String>,
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskList {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub open: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Task {
    pub id: i64,
    pub uid: String,
    pub list_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub title: String,
    pub notes: String,
    pub due: Option<i64>,
    pub due_time: bool,
    pub reminder: Option<i64>,
    pub priority: u8,
    pub completed: Option<i64>,
    pub created: i64,
    pub modified: i64,
    pub email: Option<EmailLink>,
    pub subtasks: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskInput {
    pub id: Option<i64>,
    pub list_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub notes: String,
    pub due: Option<i64>,
    #[serde(default)]
    pub due_time: bool,
    pub reminder: Option<i64>,
    #[serde(default)]
    pub priority: u8,
    pub email: Option<EmailLink>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskView {
    /// Open tasks due before `day_end` (today and overdue).
    Today {
        day_end: i64,
    },
    /// Open tasks due after `day_end`.
    Upcoming {
        day_end: i64,
    },
    /// Open tasks created from an email.
    FromEmail,
    Completed,
    List {
        id: i64,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskCounts {
    pub today: i64,
    pub upcoming: i64,
    pub from_email: i64,
    pub completed: i64,
}

fn validate_link(l: &EmailLink) -> Result<EmailLink> {
    let mid = l.message_id.trim();
    if mid.is_empty() || mid.len() > 998 || mid.chars().any(char::is_control) {
        return Err(Error::Invalid("bad Message-ID".into()));
    }
    let cut = |s: &str, n: usize| -> String {
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(n)
            .collect()
    };
    Ok(EmailLink {
        account_id: l.account_id,
        message_id: mid.to_string(),
        subject: cut(&l.subject, 500),
        sender: cut(&l.sender, 300),
        date: l.date,
    })
}

const PRIORITIES: [u8; 4] = [0, 1, 5, 9];

impl Store {
    // ---- note folders and tags ---------------------------------------------

    pub fn note_folders(&self) -> Result<Vec<NoteFolder>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT f.id, f.name, (SELECT count(*) FROM notes n WHERE n.folder_id = f.id AND n.deleted = 0)
             FROM note_folders f WHERE f.deleted = 0 ORDER BY f.position, f.name",
        )?;
        let v = st
            .query_map([], |r| {
                Ok(NoteFolder {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    count: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn create_note_folder(&self, name: &str) -> Result<i64> {
        let name = clean_name(name, "folder")?;
        let db = self.db();
        db.execute(
            "INSERT INTO note_folders (uid, name, position, modified)
             VALUES (?1, ?2, (SELECT coalesce(max(position), 0) + 1 FROM note_folders), ?3)",
            params![new_uid(), name, now()],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn rename_note_folder(&self, id: i64, name: &str) -> Result<()> {
        let name = clean_name(name, "folder")?;
        self.db().execute(
            "UPDATE note_folders SET name = ?2, modified = ?3, sync_state = CASE sync_state WHEN 'local' THEN 'local' ELSE 'dirty' END WHERE id = ?1",
            params![id, name, now()],
        )?;
        Ok(())
    }

    /// Deletes the folder; its notes stay (unfiled).
    pub fn delete_note_folder(&self, id: i64) -> Result<()> {
        let db = self.db();
        db.execute(
            "UPDATE notes SET folder_id = NULL, modified = ?2 WHERE folder_id = ?1",
            params![id, now()],
        )?;
        db.execute(
            "UPDATE note_folders SET deleted = 1, modified = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    pub fn tags(&self) -> Result<Vec<Tag>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT t.tag, coalesce(c.color, ''), count(*) FROM note_tags t JOIN notes n ON n.id = t.note_id AND n.deleted = 0
             LEFT JOIN tag_colors c ON c.tag = t.tag GROUP BY t.tag ORDER BY t.tag",
        )?;
        let palette = [
            "#8b5cf6", "#3b82f6", "#22c55e", "#eab308", "#ef4444", "#14b8a6",
        ];
        let v: Vec<Tag> = st
            .query_map([], |r| {
                Ok(Tag {
                    name: r.get(0)?,
                    color: r.get(1)?,
                    count: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v.into_iter()
            .enumerate()
            .map(|(i, mut t)| {
                if t.color.is_empty() {
                    t.color = palette[i % palette.len()].into();
                }
                t
            })
            .collect())
    }

    pub fn set_tag_color(&self, tag: &str, color: &str) -> Result<()> {
        if !valid_color(color) {
            return Err(Error::Invalid("color must be #rrggbb".into()));
        }
        self.db().execute(
            "INSERT INTO tag_colors (tag, color) VALUES (?1, ?2) ON CONFLICT(tag) DO UPDATE SET color = excluded.color",
            params![tag, color],
        )?;
        Ok(())
    }

    // ---- notes ---------------------------------------------------------------

    pub fn notes(&self, q: &NoteQuery) -> Result<Vec<NoteSummary>> {
        let db = self.db();
        let like = format!(
            "%{}%",
            q.query
                .trim()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut st = db.prepare(
            "SELECT n.id, n.folder_id, n.title, substr(n.text, 1, 160), n.modified, n.pinned,
                    EXISTS (SELECT 1 FROM email_links l WHERE l.owner = 'note' AND l.owner_id = n.id)
             FROM notes n
             WHERE n.deleted = 0
               AND (?1 IS NULL OR n.folder_id = ?1)
               AND (?2 IS NULL OR EXISTS (SELECT 1 FROM note_tags t WHERE t.note_id = n.id AND t.tag = ?2))
               AND (?3 = '%%' OR n.title LIKE ?3 ESCAPE '\\' OR n.text LIKE ?3 ESCAPE '\\')
             ORDER BY n.pinned DESC, n.modified DESC LIMIT 1000",
        )?;
        let mut v: Vec<NoteSummary> = st
            .query_map(params![q.folder_id, q.tag, like], |r| {
                Ok(NoteSummary {
                    id: r.get(0)?,
                    folder_id: r.get(1)?,
                    title: r.get(2)?,
                    snippet: r.get(3)?,
                    modified: r.get(4)?,
                    pinned: r.get(5)?,
                    tags: Vec::new(),
                    linked: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut tq = db.prepare("SELECT tag FROM note_tags WHERE note_id = ?1 ORDER BY tag")?;
        for n in &mut v {
            n.tags = tq
                .query_map([n.id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
        }
        Ok(v)
    }

    fn links(db: &Connection, owner: &str, id: i64) -> Result<Vec<EmailLink>> {
        let mut st = db.prepare(
            "SELECT account_id, message_id, subject, sender, date FROM email_links WHERE owner = ?1 AND owner_id = ?2 ORDER BY id",
        )?;
        let v = st
            .query_map(params![owner, id], |r| {
                Ok(EmailLink {
                    account_id: r.get(0)?,
                    message_id: r.get(1)?,
                    subject: r.get(2)?,
                    sender: r.get(3)?,
                    date: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn note(&self, id: i64) -> Result<Note> {
        let db = self.db();
        let mut n = db
            .query_row(
                "SELECT id, uid, folder_id, title, html, created, modified, pinned FROM notes WHERE id = ?1 AND deleted = 0",
                [id],
                |r| {
                    Ok(Note {
                        id: r.get(0)?,
                        uid: r.get(1)?,
                        folder_id: r.get(2)?,
                        title: r.get(3)?,
                        html: r.get(4)?,
                        created: r.get(5)?,
                        modified: r.get(6)?,
                        pinned: r.get(7)?,
                        tags: Vec::new(),
                        links: Vec::new(),
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound("note".into()))?;
        let mut tq = db.prepare("SELECT tag FROM note_tags WHERE note_id = ?1 ORDER BY tag")?;
        n.tags = tq
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        n.links = Self::links(&db, "note", id)?;
        Ok(n)
    }

    /// Create (id = None) or update a note. The HTML is sanitized here.
    pub fn save_note(&self, input: &NoteInput) -> Result<Note> {
        if input.html.len() > MAX_NOTE_HTML {
            return Err(Error::Invalid("note is too large (1 MB max)".into()));
        }
        let title = clean_title(&input.title)?;
        let html = sanitize_note_html(&input.html);
        let text = crate::sanitize::html_to_text(&html);
        let mut tags: Vec<String> = Vec::new();
        for t in &input.tags {
            let t = clean_name(t.trim_start_matches('#'), "tag")?;
            if !tags.contains(&t) {
                tags.push(t);
            }
        }
        if tags.len() > MAX_TAGS {
            return Err(Error::Invalid(format!("at most {MAX_TAGS} tags")));
        }
        let id = {
            let mut db = self.db();
            let tx = db.transaction()?;
            let t = now();
            let id = match input.id {
                Some(id) => {
                    let n = tx.execute(
                        "UPDATE notes SET folder_id = ?2, title = ?3, html = ?4, text = ?5, pinned = ?6, modified = ?7,
                           sync_state = CASE sync_state WHEN 'local' THEN 'local' ELSE 'dirty' END
                         WHERE id = ?1 AND deleted = 0",
                        params![id, input.folder_id, title, html, text, input.pinned, t],
                    )?;
                    if n == 0 {
                        return Err(Error::NotFound("note".into()));
                    }
                    id
                }
                None => {
                    tx.execute(
                        "INSERT INTO notes (uid, folder_id, title, html, text, pinned, created, modified) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                        params![new_uid(), input.folder_id, title, html, text, input.pinned, t],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            tx.execute("DELETE FROM note_tags WHERE note_id = ?1", [id])?;
            for tag in &tags {
                tx.execute(
                    "INSERT INTO note_tags (note_id, tag) VALUES (?1, ?2)",
                    params![id, tag],
                )?;
            }
            tx.commit()?;
            id
        };
        self.note(id)
    }

    /// Tombstone (kept for a future sync to propagate the deletion).
    pub fn delete_note(&self, id: i64) -> Result<()> {
        let db = self.db();
        db.execute(
            "UPDATE notes SET deleted = 1, html = '', text = '', title = '', modified = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        db.execute("DELETE FROM note_tags WHERE note_id = ?1", [id])?;
        db.execute(
            "DELETE FROM email_links WHERE owner = 'note' AND owner_id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn link_email(&self, owner: &str, owner_id: i64, link: &EmailLink) -> Result<()> {
        if owner != "note" && owner != "task" {
            return Err(Error::Invalid("bad link owner".into()));
        }
        let l = validate_link(link)?;
        self.db().execute(
            "INSERT OR IGNORE INTO email_links (owner, owner_id, account_id, message_id, subject, sender, date)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![owner, owner_id, l.account_id, l.message_id, l.subject, l.sender, l.date],
        )?;
        Ok(())
    }

    pub fn unlink_email(&self, owner: &str, owner_id: i64, message_id: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM email_links WHERE owner = ?1 AND owner_id = ?2 AND message_id = ?3",
            params![owner, owner_id, message_id],
        )?;
        Ok(())
    }

    /// The cached message for a link (any folder), if it is still cached.
    pub fn resolve_email(&self, link: &EmailLink) -> Result<Option<i64>> {
        let db = self.db();
        Ok(db
            .query_row(
                "SELECT m.id FROM messages m JOIN folders f ON f.id = m.folder_id
                 WHERE m.message_id = ?1 AND (?2 IS NULL OR f.account_id = ?2)
                 ORDER BY (f.role = 'trash'), m.id DESC LIMIT 1",
                params![link.message_id, link.account_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Link data for a cached message (for "Add to To-do" / "Save to Notes").
    pub fn email_link_for(&self, message_id: i64) -> Result<EmailLink> {
        let db = self.db();
        let (account_id, mid, subject, from_name, from_addr, date): (
            i64,
            String,
            String,
            String,
            String,
            i64,
        ) = db
            .query_row(
                "SELECT f.account_id, m.message_id, m.subject, m.from_name, m.from_addr, m.date
                 FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
                [message_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound("message".into()))?;
        if mid.trim().is_empty() {
            return Err(Error::Invalid(
                "this message has no Message-ID, so it can't be linked".into(),
            ));
        }
        Ok(EmailLink {
            account_id: Some(account_id),
            message_id: mid,
            subject,
            sender: if from_name.is_empty() {
                from_addr
            } else {
                from_name
            },
            date,
        })
    }

    // ---- task lists ----------------------------------------------------------

    pub fn task_lists(&self) -> Result<Vec<TaskList>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT l.id, l.name, l.color,
               (SELECT count(*) FROM tasks t WHERE t.list_id = l.id AND t.deleted = 0 AND t.completed IS NULL AND t.parent_id IS NULL)
             FROM task_lists l WHERE l.deleted = 0 ORDER BY l.position, l.name",
        )?;
        let v = st
            .query_map([], |r| {
                Ok(TaskList {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    color: r.get(2)?,
                    open: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn create_task_list(&self, name: &str, color: &str) -> Result<i64> {
        let name = clean_name(name, "list")?;
        let color = if valid_color(color) { color } else { "#8b5cf6" };
        let db = self.db();
        db.execute(
            "INSERT INTO task_lists (uid, name, color, position, modified)
             VALUES (?1, ?2, ?3, (SELECT coalesce(max(position), 0) + 1 FROM task_lists), ?4)",
            params![new_uid(), name, color, now()],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn rename_task_list(&self, id: i64, name: &str) -> Result<()> {
        let name = clean_name(name, "list")?;
        self.db().execute(
            "UPDATE task_lists SET name = ?2, modified = ?3 WHERE id = ?1",
            params![id, name, now()],
        )?;
        Ok(())
    }

    /// Deletes the list and (tombstones) its tasks.
    pub fn delete_task_list(&self, id: i64) -> Result<()> {
        let db = self.db();
        let t = now();
        db.execute(
            "UPDATE tasks SET deleted = 1, modified = ?2 WHERE list_id = ?1",
            params![id, t],
        )?;
        db.execute(
            "UPDATE task_lists SET deleted = 1, modified = ?2 WHERE id = ?1",
            params![id, t],
        )?;
        Ok(())
    }

    // ---- tasks ---------------------------------------------------------------

    fn task_rows(db: &Connection, where_: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<Task>> {
        let sql = format!(
            "SELECT t.id, t.uid, t.list_id, t.parent_id, t.title, t.notes, t.due, t.due_time, t.reminder, t.priority,
                    t.completed, t.created, t.modified
             FROM tasks t WHERE t.deleted = 0 AND {where_}"
        );
        let mut st = db.prepare(&sql)?;
        let mut v: Vec<Task> = st
            .query_map(p, |r| {
                Ok(Task {
                    id: r.get(0)?,
                    uid: r.get(1)?,
                    list_id: r.get(2)?,
                    parent_id: r.get(3)?,
                    title: r.get(4)?,
                    notes: r.get(5)?,
                    due: r.get(6)?,
                    due_time: r.get(7)?,
                    reminder: r.get(8)?,
                    priority: r.get(9)?,
                    completed: r.get(10)?,
                    created: r.get(11)?,
                    modified: r.get(12)?,
                    email: None,
                    subtasks: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        for t in &mut v {
            t.email = Self::links(db, "task", t.id)?.into_iter().next();
        }
        Ok(v)
    }

    fn with_subtasks(db: &Connection, mut v: Vec<Task>) -> Result<Vec<Task>> {
        for t in &mut v {
            t.subtasks =
                Self::task_rows(db, "t.parent_id = ?1 ORDER BY t.position, t.id", &[&t.id])?;
        }
        Ok(v)
    }

    pub fn tasks(&self, view: TaskView) -> Result<Vec<Task>> {
        let db = self.db();
        let open = "t.parent_id IS NULL AND t.completed IS NULL";
        let order = "ORDER BY t.due IS NULL, t.due, CASE t.priority WHEN 0 THEN 10 ELSE t.priority END, t.position, t.id";
        let v = match view {
            TaskView::Today { day_end } => Self::task_rows(
                &db,
                &format!("{open} AND t.due IS NOT NULL AND t.due < ?1 {order}"),
                &[&day_end],
            )?,
            TaskView::Upcoming { day_end } => Self::task_rows(
                &db,
                &format!("{open} AND t.due >= ?1 {order}"),
                &[&day_end],
            )?,
            TaskView::FromEmail => Self::task_rows(
                &db,
                &format!("{open} AND EXISTS (SELECT 1 FROM email_links l WHERE l.owner = 'task' AND l.owner_id = t.id) {order}"),
                &[],
            )?,
            TaskView::Completed => Self::task_rows(
                &db,
                "t.parent_id IS NULL AND t.completed IS NOT NULL ORDER BY t.completed DESC LIMIT 500",
                &[],
            )?,
            TaskView::List { id } => Self::task_rows(
                &db,
                "t.parent_id IS NULL AND t.list_id = ?1 ORDER BY t.completed IS NOT NULL, t.completed DESC, t.due IS NULL, t.due, t.position, t.id",
                &[&id],
            )?,
        };
        Self::with_subtasks(&db, v)
    }

    pub fn task(&self, id: i64) -> Result<Task> {
        let db = self.db();
        let v = Self::task_rows(&db, "t.id = ?1", &[&id])?;
        Self::with_subtasks(&db, v)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::NotFound("task".into()))
    }

    pub fn task_counts(&self, day_end: i64) -> Result<TaskCounts> {
        let db = self.db();
        let c = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> Result<i64> {
            Ok(db.query_row(
                &format!("SELECT count(*) FROM tasks t WHERE t.deleted = 0 AND t.parent_id IS NULL AND {sql}"),
                p,
                |r| r.get(0),
            )?)
        };
        Ok(TaskCounts {
            today: c("t.completed IS NULL AND t.due IS NOT NULL AND t.due < ?1", &[&day_end])?,
            upcoming: c("t.completed IS NULL AND t.due >= ?1", &[&day_end])?,
            from_email: c("t.completed IS NULL AND EXISTS (SELECT 1 FROM email_links l WHERE l.owner = 'task' AND l.owner_id = t.id)", &[])?,
            completed: c("t.completed IS NOT NULL", &[])?,
        })
    }

    pub fn save_task(&self, input: &TaskInput) -> Result<Task> {
        let title = clean_title(&input.title)?;
        if title.is_empty() {
            return Err(Error::Invalid("a task needs a title".into()));
        }
        if input.notes.len() > MAX_TASK_NOTES {
            return Err(Error::Invalid(
                "task notes are limited to 20,000 characters".into(),
            ));
        }
        if !PRIORITIES.contains(&input.priority) {
            return Err(Error::Invalid("priority must be 0, 1, 5 or 9".into()));
        }
        let email = input.email.as_ref().map(validate_link).transpose()?;
        let id = {
            let mut db = self.db();
            let tx = db.transaction()?;
            if let Some(p) = input.parent_id {
                let pp: Option<Option<i64>> = tx
                    .query_row(
                        "SELECT parent_id FROM tasks WHERE id = ?1 AND deleted = 0",
                        [p],
                        |r| r.get(0),
                    )
                    .optional()?;
                match pp {
                    None => return Err(Error::NotFound("parent task".into())),
                    Some(Some(_)) => {
                        return Err(Error::Invalid("subtasks can't have subtasks".into()))
                    }
                    Some(None) if Some(p) == input.id => {
                        return Err(Error::Invalid("a task can't be its own subtask".into()))
                    }
                    _ => {}
                }
            }
            let t = now();
            let id = match input.id {
                Some(id) => {
                    // A changed reminder fires again.
                    let n = tx.execute(
                        "UPDATE tasks SET list_id = ?2, parent_id = ?3, title = ?4, notes = ?5, due = ?6, due_time = ?7,
                           reminded = CASE WHEN reminder IS ?8 THEN reminded ELSE 0 END, reminder = ?8,
                           priority = ?9, modified = ?10,
                           sync_state = CASE sync_state WHEN 'local' THEN 'local' ELSE 'dirty' END
                         WHERE id = ?1 AND deleted = 0",
                        params![id, input.list_id, input.parent_id, title, input.notes, input.due, input.due_time,
                                input.reminder, input.priority, t],
                    )?;
                    if n == 0 {
                        return Err(Error::NotFound("task".into()));
                    }
                    id
                }
                None => {
                    tx.execute(
                        "INSERT INTO tasks (uid, list_id, parent_id, title, notes, due, due_time, reminder, priority, position, created, modified)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, (SELECT coalesce(max(position), 0) + 1 FROM tasks), ?10, ?10)",
                        params![new_uid(), input.list_id, input.parent_id, title, input.notes, input.due, input.due_time,
                                input.reminder, input.priority, t],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            if let Some(l) = &email {
                tx.execute(
                    "INSERT OR IGNORE INTO email_links (owner, owner_id, account_id, message_id, subject, sender, date)
                     VALUES ('task', ?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, l.account_id, l.message_id, l.subject, l.sender, l.date],
                )?;
            }
            tx.commit()?;
            id
        };
        self.task(id)
    }

    pub fn set_task_completed(&self, id: i64, done: bool) -> Result<Task> {
        {
            let db = self.db();
            let t = now();
            db.execute(
                "UPDATE tasks SET completed = ?2, modified = ?3 WHERE id = ?1 AND deleted = 0",
                params![id, done.then_some(t), t],
            )?;
            // Completing a task completes its subtasks (VTODO semantics).
            if done {
                db.execute(
                    "UPDATE tasks SET completed = ?2, modified = ?2 WHERE parent_id = ?1 AND completed IS NULL",
                    params![id, t],
                )?;
            }
        }
        self.task(id)
    }

    pub fn delete_task(&self, id: i64) -> Result<()> {
        let db = self.db();
        let t = now();
        db.execute(
            "UPDATE tasks SET deleted = 1, modified = ?2 WHERE id = ?1 OR parent_id = ?1",
            params![id, t],
        )?;
        db.execute(
            "DELETE FROM email_links WHERE owner = 'task' AND owner_id = ?1",
            [id],
        )?;
        Ok(())
    }

    /// Open tasks whose reminder is due and hasn't fired; marks them fired.
    pub fn take_due_reminders(&self, at: i64) -> Result<Vec<Task>> {
        let v = {
            let db = self.db();
            Self::task_rows(
                &db,
                "t.completed IS NULL AND t.reminder IS NOT NULL AND t.reminder <= ?1 AND t.reminded = 0 ORDER BY t.reminder LIMIT 20",
                &[&at],
            )?
        };
        let db = self.db();
        for t in &v {
            db.execute("UPDATE tasks SET reminded = 1 WHERE id = ?1", [t.id])?;
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, html: &str) -> NoteInput {
        NoteInput {
            id: None,
            folder_id: None,
            title: title.into(),
            html: html.into(),
            tags: vec![],
            pinned: false,
        }
    }

    #[test]
    fn uid_is_uuid_v4() {
        let u = new_uid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert_ne!(u, new_uid());
    }

    #[test]
    fn note_html_is_sanitized_and_checklists_survive() {
        let s = sanitize_note_html(
            r#"<h2 onclick="x()">Hi</h2><script>alert(1)</script><img src="https://t/x.png">
               <ul data-checklist="evil"><li data-checked="true" style="color:red" class="c">done</li><li data-checked="1">todo</li></ul>
               <a href="javascript:alert(1)">j</a><a href="https://ok.example/">ok</a><a href="/rel">r</a>"#,
        );
        assert!(s.contains("<h2>Hi</h2>"), "{s}");
        assert!(!s.contains("script") && !s.contains("onclick") && !s.contains("<img"));
        assert!(s.contains(r#"<ul data-checklist="">"#), "{s}");
        assert!(s.contains(r#"<li data-checked="true">done</li>"#), "{s}");
        assert!(s.contains(r#"<li data-checked="false">todo</li>"#), "{s}");
        assert!(!s.contains("javascript") && !s.contains("style") && !s.contains("class"));
        assert!(s.contains(r#"href="https://ok.example/""#));
        assert!(!s.contains("/rel\""));
    }

    #[test]
    fn notes_crud_search_tags_links_and_tombstones() {
        let st = Store::open_in_memory().unwrap();
        let folders = st.note_folders().unwrap();
        assert_eq!(
            folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["Personal", "Work", "Ideas"]
        );
        let work = folders[1].id;
        let mut n = note("Neutron roadmap", "<p>Faster <b>search</b> 100%_done</p>");
        n.folder_id = Some(work);
        n.tags = vec!["#Planning".into(), "Projects".into(), "Planning".into()];
        let saved = st.save_note(&n).unwrap();
        assert_eq!(saved.tags, ["Planning", "Projects"]);
        st.save_note(&note("Other", "<p>unrelated</p>")).unwrap();
        let q = |query: &str| {
            st.notes(&NoteQuery {
                query: query.into(),
                ..Default::default()
            })
            .unwrap()
        };
        assert_eq!(q("SEARCH").len(), 1);
        assert_eq!(q("100%_").len(), 1);
        assert_eq!(q("%").len(), 1, "a literal %, not a wildcard");
        assert_eq!(q("1_0").len(), 0, "_ is not a wildcard");
        assert_eq!(q("").len(), 2);
        let by_tag = st
            .notes(&NoteQuery {
                tag: Some("Projects".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_tag.len(), 1);
        assert_eq!(st.note_folders().unwrap()[1].count, 1);
        let link = EmailLink {
            account_id: None,
            message_id: "<q4@example.org>".into(),
            subject: "Re: Q4 plan".into(),
            sender: "Ayşe".into(),
            date: 1,
        };
        st.link_email("note", saved.id, &link).unwrap();
        st.link_email("note", saved.id, &link).unwrap();
        assert_eq!(st.note(saved.id).unwrap().links.len(), 1);
        assert!(q("search")[0].linked);
        // Update keeps the uid; delete leaves a tombstone.
        let mut up = n.clone();
        up.id = Some(saved.id);
        up.title = "Roadmap v2".into();
        let up = st.save_note(&up).unwrap();
        assert_eq!(up.uid, saved.uid);
        st.delete_note(saved.id).unwrap();
        assert!(st.note(saved.id).is_err());
        let (deleted, title): (bool, String) = st
            .db()
            .query_row(
                "SELECT deleted, title FROM notes WHERE id = ?1",
                [saved.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(deleted && title.is_empty(), "tombstone without content");
        assert!(st.save_note(&note(&"x".repeat(301), "")).is_err());
        assert!(st.create_note_folder("bad\nname").is_err());
    }

    #[test]
    fn tasks_views_subtasks_reminders() {
        let st = Store::open_in_memory().unwrap();
        let lists = st.task_lists().unwrap();
        let work = lists[0].id;
        let day_end = 1_000_000;
        let input = |title: &str, due: Option<i64>| TaskInput {
            id: None,
            list_id: Some(work),
            parent_id: None,
            title: title.into(),
            notes: String::new(),
            due,
            due_time: false,
            reminder: None,
            priority: 0,
            email: None,
        };
        let a = st
            .save_task(&TaskInput {
                priority: 1,
                ..input("Overdue high", Some(day_end - 90_000))
            })
            .unwrap();
        st.save_task(&input("Today", Some(day_end - 10))).unwrap();
        st.save_task(&input("Later", Some(day_end + 86_400)))
            .unwrap();
        st.save_task(&input("No date", None)).unwrap();
        let link = EmailLink {
            account_id: Some(1),
            message_id: "<m1@x>".into(),
            subject: "Q2 visual system".into(),
            sender: "Design Team".into(),
            date: 5,
        };
        let e = st
            .save_task(&TaskInput {
                email: Some(link),
                ..input("From mail", Some(day_end + 1))
            })
            .unwrap();
        assert_eq!(e.email.as_ref().unwrap().subject, "Q2 visual system");
        let titles = |v: Vec<Task>| v.into_iter().map(|t| t.title).collect::<Vec<_>>();
        assert_eq!(
            titles(st.tasks(TaskView::Today { day_end }).unwrap()),
            ["Overdue high", "Today"]
        );
        assert_eq!(
            titles(st.tasks(TaskView::Upcoming { day_end }).unwrap()),
            ["From mail", "Later"]
        );
        assert_eq!(
            titles(st.tasks(TaskView::FromEmail).unwrap()),
            ["From mail"]
        );
        let c = st.task_counts(day_end).unwrap();
        assert_eq!(
            (c.today, c.upcoming, c.from_email, c.completed),
            (2, 2, 1, 0)
        );
        // Subtasks: one level only; completing the parent completes them.
        let sub = st
            .save_task(&TaskInput {
                parent_id: Some(a.id),
                ..input("Step 1", None)
            })
            .unwrap();
        assert!(st
            .save_task(&TaskInput {
                parent_id: Some(sub.id),
                ..input("deeper", None)
            })
            .is_err());
        assert_eq!(st.task(a.id).unwrap().subtasks.len(), 1);
        assert_eq!(
            st.task_lists().unwrap()[0].open,
            5,
            "subtasks aren't counted"
        );
        st.set_task_completed(a.id, true).unwrap();
        assert!(st.task(sub.id).unwrap().completed.is_some());
        assert_eq!(
            titles(st.tasks(TaskView::Completed).unwrap()),
            ["Overdue high"]
        );
        assert!(st
            .save_task(&TaskInput {
                priority: 3,
                ..input("bad", None)
            })
            .is_err());
        assert!(st.save_task(&input("   ", None)).is_err());
        // Reminders fire once; editing the time re-arms.
        let r = st
            .save_task(&TaskInput {
                reminder: Some(500),
                ..input("Remind", None)
            })
            .unwrap();
        assert!(st.take_due_reminders(499).unwrap().is_empty());
        assert_eq!(st.take_due_reminders(500).unwrap().len(), 1);
        assert!(st.take_due_reminders(600).unwrap().is_empty());
        st.save_task(&TaskInput {
            id: Some(r.id),
            reminder: Some(700),
            ..input("Remind", None)
        })
        .unwrap();
        assert_eq!(st.take_due_reminders(800).unwrap().len(), 1);
        st.delete_task(a.id).unwrap();
        assert!(st.task(sub.id).is_err(), "subtasks go with the parent");
    }
}
