//! Local cache in SQLite with an FTS5 full-text index.
//!
//! Raw RFC 5322 messages are stored so the viewer can re-parse and
//! re-sanitize with the current policy. The cache is plaintext at rest
//! (see docs/AUDIT.md); rely on disk encryption or add SQLCipher later.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{
    Account, AccountConfig, Folder, MessageDetail, MessageSummary, SearchHit, Security,
};
use crate::parse::{self, ParsedMessage};
use crate::{Error, Result};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY,
  email TEXT NOT NULL,
  display_name TEXT NOT NULL,
  username TEXT NOT NULL,
  imap_host TEXT NOT NULL, imap_port INTEGER NOT NULL, imap_security TEXT NOT NULL,
  smtp_host TEXT NOT NULL, smtp_port INTEGER NOT NULL, smtp_security TEXT NOT NULL,
  extra_ca_pem TEXT
);
CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  role TEXT,
  uidvalidity INTEGER,
  UNIQUE(account_id, name)
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  uid INTEGER NOT NULL,
  message_id TEXT NOT NULL DEFAULT '',
  subject TEXT NOT NULL,
  from_name TEXT NOT NULL,
  from_addr TEXT NOT NULL,
  to_addrs TEXT NOT NULL,
  cc_addrs TEXT NOT NULL,
  date INTEGER NOT NULL,
  preview TEXT NOT NULL,
  seen INTEGER NOT NULL DEFAULT 0,
  flagged INTEGER NOT NULL DEFAULT 0,
  has_attachments INTEGER NOT NULL DEFAULT 0,
  raw BLOB NOT NULL,
  UNIQUE(folder_id, uid)
);
CREATE TABLE IF NOT EXISTS addresses (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  email TEXT NOT NULL COLLATE NOCASE,
  name TEXT NOT NULL DEFAULT '',
  uses INTEGER NOT NULL DEFAULT 0,
  last_seen INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(account_id, email)
);
CREATE TABLE IF NOT EXISTS image_senders (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  email TEXT NOT NULL COLLATE NOCASE,
  PRIMARY KEY(account_id, email)
);
CREATE INDEX IF NOT EXISTS messages_folder_date ON messages(folder_id, date DESC);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  subject, sender, body, tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
  DELETE FROM messages_fts WHERE rowid = old.id;
END;
"#;

pub struct Store {
    conn: Mutex<Connection>,
}

const SUMMARY_COLS: &str =
    "m.id, m.folder_id, m.uid, m.subject, m.from_name, m.from_addr, m.date, m.preview, m.seen, m.flagged, m.has_attachments";

fn summary_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageSummary> {
    Ok(MessageSummary {
        id: r.get(0)?,
        folder_id: r.get(1)?,
        uid: r.get(2)?,
        subject: r.get(3)?,
        from_name: r.get(4)?,
        from_addr: r.get(5)?,
        date: r.get(6)?,
        preview: r.get(7)?,
        seen: r.get(8)?,
        flagged: r.get(9)?,
        has_attachments: r.get(10)?,
    })
}

pub struct NewMessage<'a> {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub raw: &'a [u8],
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        restrict_permissions(path);
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)?;
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.backfill_addresses()?;
        Ok(store)
    }

    /// Caches created before the address table existed: index once.
    fn backfill_addresses(&self) -> Result<()> {
        let mut db = self.db();
        let empty: bool = db.query_row("SELECT NOT EXISTS (SELECT 1 FROM addresses)", [], |r| {
            r.get(0)
        })?;
        if !empty {
            return Ok(());
        }
        let tx = db.transaction()?;
        {
            let mut st = tx.prepare(
                "SELECT f.account_id, m.raw FROM messages m JOIN folders f ON f.id = m.folder_id",
            )?;
            let rows: Vec<(i64, Vec<u8>)> = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (account_id, raw) in rows {
                let p = parse::parse(&raw);
                for (name, email) in &p.addresses {
                    tx.execute(
                        "INSERT INTO addresses (account_id, email, name, uses, last_seen) VALUES (?1, ?2, ?3, 1, ?4)
                         ON CONFLICT(account_id, email) DO UPDATE SET uses = uses + 1, last_seen = max(last_seen, excluded.last_seen)",
                        params![account_id, email, name, p.date],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- accounts -------------------------------------------------------

    pub fn add_account(&self, c: &AccountConfig) -> Result<i64> {
        c.validate()?;
        let db = self.db();
        db.execute(
            "INSERT INTO accounts (email, display_name, username, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, extra_ca_pem)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![c.email, c.display_name, c.username, c.imap_host, c.imap_port, c.imap_security.as_str(),
                    c.smtp_host, c.smtp_port, c.smtp_security.as_str(), c.extra_ca_pem],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, email, display_name, username, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, extra_ca_pem FROM accounts ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                AccountConfig {
                    email: r.get(1)?,
                    display_name: r.get(2)?,
                    username: r.get(3)?,
                    imap_host: r.get(4)?,
                    imap_port: r.get(5)?,
                    imap_security: Security::Tls,
                    smtp_host: r.get(7)?,
                    smtp_port: r.get(8)?,
                    smtp_security: Security::Tls,
                    extra_ca_pem: r.get(10)?,
                },
                r.get::<_, String>(6)?,
                r.get::<_, String>(9)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, mut config, imap_sec, smtp_sec) = row?;
            config.imap_security = Security::parse(&imap_sec)?;
            config.smtp_security = Security::parse(&smtp_sec)?;
            out.push(Account { id, config });
        }
        Ok(out)
    }

    pub fn account(&self, id: i64) -> Result<Account> {
        self.accounts()?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| Error::NotFound(format!("account {id}")))
    }

    pub fn delete_account(&self, id: i64) -> Result<()> {
        self.db()
            .execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        Ok(())
    }

    // ---- folders --------------------------------------------------------

    pub fn upsert_folder(&self, account_id: i64, name: &str, role: Option<&str>) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO folders (account_id, name, role) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, name) DO UPDATE SET role = excluded.role",
            params![account_id, name, role],
        )?;
        Ok(db.query_row(
            "SELECT id FROM folders WHERE account_id = ?1 AND name = ?2",
            params![account_id, name],
            |r| r.get(0),
        )?)
    }

    /// Remove cached folders that no longer exist on the server.
    pub fn retain_folders(&self, account_id: i64, keep: &[String]) -> Result<()> {
        let db = self.db();
        let mut st = db.prepare("SELECT id, name FROM folders WHERE account_id = ?1")?;
        let existing: Vec<(i64, String)> = st
            .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, name) in existing {
            if !keep.contains(&name) {
                db.execute("DELETE FROM folders WHERE id = ?1", [id])?;
            }
        }
        Ok(())
    }

    pub fn folders(&self, account_id: i64) -> Result<Vec<Folder>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT f.id, f.account_id, f.name, f.role,
                    (SELECT COUNT(*) FROM messages m WHERE m.folder_id = f.id AND m.seen = 0),
                    (SELECT COUNT(*) FROM messages m WHERE m.folder_id = f.id)
             FROM folders f WHERE f.account_id = ?1
             ORDER BY CASE f.role WHEN 'inbox' THEN 0 WHEN 'drafts' THEN 1 WHEN 'sent' THEN 2
                                  WHEN 'archive' THEN 3 WHEN 'junk' THEN 4 WHEN 'trash' THEN 5 ELSE 6 END, f.name",
        )?;
        let rows = st.query_map([account_id], |r| {
            Ok(Folder {
                id: r.get(0)?,
                account_id: r.get(1)?,
                name: r.get(2)?,
                role: r.get(3)?,
                unread: r.get(4)?,
                total: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn folder_uidvalidity(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self
            .db()
            .query_row(
                "SELECT uidvalidity FROM folders WHERE id = ?1",
                [folder_id],
                |r| r.get::<_, Option<u32>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Record UIDVALIDITY; if it changed, drop the folder's cached messages.
    pub fn set_uidvalidity(&self, folder_id: i64, v: u32) -> Result<bool> {
        let old = self.folder_uidvalidity(folder_id)?;
        let reset = old.is_some_and(|o| o != v);
        let db = self.db();
        if reset {
            db.execute("DELETE FROM messages WHERE folder_id = ?1", [folder_id])?;
        }
        db.execute(
            "UPDATE folders SET uidvalidity = ?2 WHERE id = ?1",
            params![folder_id, v],
        )?;
        Ok(reset)
    }

    // ---- messages -------------------------------------------------------

    pub fn uids(&self, folder_id: i64) -> Result<Vec<u32>> {
        let db = self.db();
        let mut st = db.prepare("SELECT uid FROM messages WHERE folder_id = ?1")?;
        let rows = st.query_map([folder_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn insert_message(&self, folder_id: i64, m: &NewMessage<'_>) -> Result<i64> {
        let p: ParsedMessage = parse::parse(m.raw);
        let body_index = if p.text.trim().is_empty() {
            p.html_raw
                .as_deref()
                .map(parse::strip_tags)
                .unwrap_or_default()
        } else {
            p.text.clone()
        };
        let mut db = self.db();
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO messages (folder_id, uid, message_id, subject, from_name, from_addr, to_addrs, cc_addrs, date, preview, seen, flagged, has_attachments, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(folder_id, uid) DO NOTHING",
            params![folder_id, m.uid, p.message_id, p.subject, p.from_name, p.from_addr, p.to, p.cc, p.date, p.preview,
                    m.seen, m.flagged, !p.attachments.is_empty(), m.raw],
        )?;
        let id: i64 = tx.query_row(
            "SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2",
            params![folder_id, m.uid],
            |r| r.get(0),
        )?;
        for (name, email) in &p.addresses {
            tx.execute(
                "INSERT INTO addresses (account_id, email, name, uses, last_seen)
                 SELECT f.account_id, ?2, ?3, 1, ?4 FROM folders f WHERE f.id = ?1
                 ON CONFLICT(account_id, email) DO UPDATE SET uses = uses + 1,
                   last_seen = max(last_seen, excluded.last_seen),
                   name = CASE WHEN excluded.name <> '' THEN excluded.name ELSE name END",
                params![folder_id, email, name, p.date],
            )?;
        }
        tx.execute("DELETE FROM messages_fts WHERE rowid = ?1", [id])?;
        tx.execute(
            "INSERT INTO messages_fts (rowid, subject, sender, body) VALUES (?1, ?2, ?3, ?4)",
            params![
                id,
                p.subject,
                format!("{} {}", p.from_name, p.from_addr),
                body_index
            ],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn remove_uids(&self, folder_id: i64, uids: &[u32]) -> Result<usize> {
        let db = self.db();
        let mut n = 0;
        for uid in uids {
            n += db.execute(
                "DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2",
                params![folder_id, uid],
            )?;
        }
        Ok(n)
    }

    pub fn set_flags(&self, folder_id: i64, uid: u32, seen: bool, flagged: bool) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET seen = ?3, flagged = ?4 WHERE folder_id = ?1 AND uid = ?2",
            params![folder_id, uid, seen, flagged],
        )?;
        Ok(())
    }

    pub fn messages(&self, folder_id: i64, offset: i64, limit: i64) -> Result<Vec<MessageSummary>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {SUMMARY_COLS} FROM messages m WHERE m.folder_id = ?1 ORDER BY m.date DESC, m.uid DESC LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = st.query_map(
            params![folder_id, limit.clamp(1, 500), offset.max(0)],
            summary_from_row,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Returns (folder name, account id, uid) for IMAP flag updates.
    pub fn message_location(&self, id: i64) -> Result<(i64, String, u32)> {
        self.db()
            .query_row(
                "SELECT f.account_id, f.name, m.uid FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("message {id}")))
    }

    pub fn mark_seen_local(&self, id: i64) -> Result<()> {
        self.db()
            .execute("UPDATE messages SET seen = 1 WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Raw RFC 5322 bytes of a cached message.
    pub fn raw(&self, id: i64) -> Result<Vec<u8>> {
        self.db()
            .query_row("SELECT raw FROM messages WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("message {id}")))
    }

    /// Account owning a cached message.
    pub fn message_account(&self, id: i64) -> Result<i64> {
        Ok(self.message_location(id)?.0)
    }

    pub fn folder_by_role(&self, account_id: i64, role: &str) -> Result<Option<Folder>> {
        Ok(self
            .folders(account_id)?
            .into_iter()
            .find(|f| f.role.as_deref() == Some(role)))
    }

    pub fn trust_image_sender(&self, account_id: i64, email: &str) -> Result<()> {
        self.db().execute(
            "INSERT OR IGNORE INTO image_senders (account_id, email) VALUES (?1, ?2)",
            params![account_id, email.trim()],
        )?;
        Ok(())
    }

    pub fn untrust_image_sender(&self, account_id: i64, email: &str) -> Result<()> {
        self.db().execute(
            "DELETE FROM image_senders WHERE account_id = ?1 AND email = ?2",
            params![account_id, email.trim()],
        )?;
        Ok(())
    }

    pub fn is_image_sender_trusted(&self, account_id: i64, email: &str) -> Result<bool> {
        Ok(self
            .db()
            .query_row(
                "SELECT 1 FROM image_senders WHERE account_id = ?1 AND email = ?2",
                params![account_id, email.trim()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Addresses seen in cached mail whose name or email contains `q`,
    /// most used first.
    pub fn address_history(
        &self,
        account_id: i64,
        q: &str,
        limit: i64,
    ) -> Result<Vec<(String, String)>> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let db = self.db();
        let mut st = db.prepare(
            "SELECT name, email FROM addresses
             WHERE account_id = ?1 AND (email LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')
             ORDER BY uses DESC, last_seen DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![account_id, pattern, limit.clamp(1, 50)], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Render a message, embedding the given fetched remote images.
    /// Also returns the remote image URLs the message references.
    pub fn render_message(
        &self,
        id: i64,
        remote: &std::collections::HashMap<String, String>,
    ) -> Result<(MessageDetail, Vec<String>)> {
        let mut d = self.message_detail(id)?;
        let p = parse::parse(&self.raw(id)?);
        let s = parse::sanitized_html_with(&p, remote);
        let urls = s
            .as_ref()
            .map(|s| s.remote_urls.clone())
            .unwrap_or_default();
        if let Some(s) = s {
            d.html = Some(s.html);
            d.remote_images_blocked = s.remote_images_blocked;
            d.remote_images_loaded = s.remote_images_loaded;
        }
        Ok((d, urls))
    }

    pub fn message_detail(&self, id: i64) -> Result<MessageDetail> {
        let (summary, raw) = {
            let db = self.db();
            db.query_row(
                &format!("SELECT {SUMMARY_COLS}, m.raw FROM messages m WHERE m.id = ?1"),
                [id],
                |r| Ok((summary_from_row(r)?, r.get::<_, Vec<u8>>(11)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("message {id}")))?
        };
        let p = parse::parse(&raw);
        let sanitized = parse::sanitized_html(&p);
        Ok(MessageDetail {
            summary,
            to: p.to,
            cc: p.cc,
            text: p.text,
            remote_images_blocked: sanitized.as_ref().map_or(0, |s| s.remote_images_blocked),
            remote_images_loaded: sanitized.as_ref().map_or(0, |s| s.remote_images_loaded),
            html: sanitized.map(|s| s.html),
            attachments: p.attachments,
        })
    }

    pub fn search(&self, account_id: i64, input: &str, limit: i64) -> Result<Vec<SearchHit>> {
        let Some(q) = crate::search::fts_query(input) else {
            return Ok(Vec::new());
        };
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {SUMMARY_COLS}, snippet(messages_fts, -1, char(2), char(3), '…', 12)
             FROM messages_fts JOIN messages m ON m.id = messages_fts.rowid
             JOIN folders f ON f.id = m.folder_id
             WHERE messages_fts MATCH ?1 AND f.account_id = ?2
             ORDER BY bm25(messages_fts), m.date DESC LIMIT ?3"
        ))?;
        let rows = st.query_map(params![q, account_id, limit.clamp(1, 200)], |r| {
            Ok(SearchHit {
                summary: summary_from_row(r)?,
                snippet: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}
