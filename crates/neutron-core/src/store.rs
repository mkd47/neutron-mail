//! Local cache in SQLite with an FTS5 full-text index.
//!
//! Raw RFC 5322 messages are stored so the viewer can re-parse and
//! re-sanitize with the current policy. The cache is plaintext at rest
//! (see docs/AUDIT.md); rely on disk encryption or add SQLCipher later.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{
    Account, AccountConfig, AuthMethod, Folder, MessageDetail, MessageSummary, SearchHit, Security,
};
use crate::parse::{self, ParsedMessage};
use crate::secrets::CacheKey;
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
CREATE TABLE IF NOT EXISTS invite_responses (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  partstat TEXT NOT NULL,
  responded_at INTEGER NOT NULL,
  in_calendar INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS image_senders (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  email TEXT NOT NULL COLLATE NOCASE,
  PRIMARY KEY(account_id, email)
);
CREATE TABLE IF NOT EXISTS account_settings (
  account_id INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  signature_html TEXT NOT NULL DEFAULT '',
  sync_days INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS rules (
  id INTEGER PRIMARY KEY,
  position INTEGER NOT NULL,
  def TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS rule_runs (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  message_key TEXT NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY(account_id, message_key)
);
CREATE TABLE IF NOT EXISTS rule_forwards (
  account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_folder_date ON messages(folder_id, date DESC);
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  subject, sender, body, tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
  DELETE FROM messages_fts WHERE rowid = old.id;
END;
"#;

/// Incremental-sync bookkeeping per folder (added in v0.4; older caches
/// get the columns as NULL, which forces one full resync).
fn migrate_folder_sync_state(conn: &Connection) -> Result<()> {
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(folders)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    for (c, ty) in [
        ("uidnext", "INTEGER"),
        ("highestmodseq", "INTEGER"),
        ("exists_count", "INTEGER"),
        ("window_days", "INTEGER"),
        ("window_cutoff", "INTEGER"),
        ("display", "TEXT"),
        ("delimiter", "TEXT"),
        ("subscribed", "INTEGER NOT NULL DEFAULT 1"),
    ] {
        if !cols.iter().any(|x| x == c) {
            conn.execute_batch(&format!("ALTER TABLE folders ADD COLUMN {c} {ty}"))?;
        }
    }
    Ok(())
}

/// What the last completed sync of a folder saw on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderSyncState {
    pub uidnext: u32,
    pub highestmodseq: Option<u64>,
    pub exists: u32,
    pub window_days: u32,
    /// UTC midnight used for the last full SEARCH SINCE reconciliation.
    /// None for an unlimited window or a checkpoint from an older cache.
    pub window_cutoff: Option<i64>,
}

pub struct Store {
    conn: Mutex<Connection>,
}

/// Sync windows offered in settings (days; 0 = all mail).
pub const SYNC_WINDOWS: [u32; 5] = [0, 30, 90, 365, 730];
const MAX_SIGNATURE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AccountSettings {
    /// Signature as compose-sanitized HTML ("" = none).
    pub signature_html: String,
    /// Only keep/download mail from the last N days (0 = all).
    pub sync_days: u32,
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
    /// Open (or create) the encrypted cache at `path` (SQLCipher, AES-256).
    /// A plaintext cache from an older version is encrypted in place; a cache
    /// that this key cannot decrypt (key lost from the keyring) is moved aside
    /// and a fresh one is created, since everything in it can be re-synced
    /// except the account list.
    pub fn open_encrypted(path: &Path, key: &CacheKey) -> Result<(Self, OpenReport)> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut report = OpenReport::default();
        if is_plaintext_sqlite(path) {
            encrypt_plaintext_db(path, key)?;
            report.migrated = true;
        }
        let conn = match open_keyed(path, key) {
            Ok(c) => c,
            Err(Error::Db(e)) if is_not_a_db(&e) => {
                let aside = path.with_extension(format!(
                    "sqlite3.unreadable-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                ));
                tracing::warn!("cache cannot be decrypted with the stored key; moving it aside");
                std::fs::rename(path, &aside)?;
                for ext in ["sqlite3-wal", "sqlite3-shm"] {
                    let _ = std::fs::remove_file(path.with_extension(ext));
                }
                report.unreadable_moved_to = Some(aside);
                open_keyed(path, key)?
            }
            Err(e) => return Err(e),
        };
        restrict_permissions(path);
        Ok((Self::init(conn)?, report))
    }

    /// Unencrypted on-disk cache. Only used by tests; the app always uses
    /// [`Store::open_encrypted`] or, without a keyring, [`Store::open_in_memory`].
    pub fn open_plaintext(path: &Path) -> Result<Self> {
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
        migrate_folder_sync_state(&conn)?;
        crate::pim::migrate(&conn)?;
        let has_auth = conn
            .prepare("SELECT 1 FROM pragma_table_info('accounts') WHERE name = 'auth'")?
            .exists([])?;
        if !has_auth {
            conn.execute_batch(
                "ALTER TABLE accounts ADD COLUMN auth TEXT NOT NULL DEFAULT 'password'",
            )?;
        }
        let has_sieve = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('account_settings') WHERE name = 'sieve_port'",
            )?
            .exists([])?;
        if !has_sieve {
            conn.execute_batch(
                "ALTER TABLE account_settings ADD COLUMN sieve_port INTEGER NOT NULL DEFAULT 4190;
                 ALTER TABLE account_settings ADD COLUMN sieve_active INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
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

    pub(crate) fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- accounts -------------------------------------------------------

    pub fn add_account(&self, c: &AccountConfig) -> Result<i64> {
        c.validate()?;
        let db = self.db();
        db.execute(
            "INSERT INTO accounts (email, display_name, username, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, extra_ca_pem, auth)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![c.email, c.display_name, c.username, c.imap_host, c.imap_port, c.imap_security.as_str(),
                    c.smtp_host, c.smtp_port, c.smtp_security.as_str(), c.extra_ca_pem, c.auth.as_str()],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT id, email, display_name, username, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, extra_ca_pem, auth FROM accounts ORDER BY id",
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
                    auth: AuthMethod::parse(&r.get::<_, String>(11)?),
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

    /// Replace an account's server settings (the caller verifies the login first).
    pub fn update_account(&self, id: i64, c: &AccountConfig) -> Result<()> {
        c.validate()?;
        let n = self.db().execute(
            "UPDATE accounts SET email=?2, display_name=?3, username=?4, imap_host=?5, imap_port=?6, imap_security=?7,
               smtp_host=?8, smtp_port=?9, smtp_security=?10, extra_ca_pem=?11, auth=?12 WHERE id=?1",
            params![id, c.email, c.display_name, c.username, c.imap_host, c.imap_port, c.imap_security.as_str(),
                    c.smtp_host, c.smtp_port, c.smtp_security.as_str(), c.extra_ca_pem, c.auth.as_str()],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("account {id}")));
        }
        Ok(())
    }

    pub fn account_settings(&self, id: i64) -> Result<AccountSettings> {
        Ok(self
            .db()
            .query_row(
                "SELECT signature_html, sync_days FROM account_settings WHERE account_id = ?1",
                [id],
                |r| {
                    Ok(AccountSettings {
                        signature_html: r.get(0)?,
                        sync_days: r.get(1)?,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    /// Store settings; the signature is sanitized with the compose policy.
    pub fn set_account_settings(&self, id: i64, s: &AccountSettings) -> Result<AccountSettings> {
        if s.signature_html.len() > MAX_SIGNATURE_BYTES {
            return Err(Error::Invalid(
                "the signature is too large (max 256 KB)".into(),
            ));
        }
        if !SYNC_WINDOWS.contains(&s.sync_days) {
            return Err(Error::Invalid("unsupported sync window".into()));
        }
        let clean = AccountSettings {
            signature_html: crate::sanitize::sanitize_compose_html(&s.signature_html)
                .trim()
                .to_string(),
            sync_days: s.sync_days,
        };
        self.account(id)?;
        self.db().execute(
            "INSERT INTO account_settings (account_id, signature_html, sync_days) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id) DO UPDATE SET signature_html = excluded.signature_html, sync_days = excluded.sync_days",
            params![id, clean.signature_html, clean.sync_days],
        )?;
        Ok(clean)
    }

    pub fn image_senders(&self, account_id: i64) -> Result<Vec<String>> {
        let db = self.db();
        let mut st =
            db.prepare("SELECT email FROM image_senders WHERE account_id = ?1 ORDER BY email")?;
        let rows = st.query_map([account_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Drop cached messages older than `cutoff` (unix time) from a folder.
    /// Only the local cache is affected; the server keeps everything.
    pub fn prune_before(&self, folder_id: i64, cutoff: i64) -> Result<usize> {
        Ok(self.db().execute(
            "DELETE FROM messages WHERE folder_id = ?1 AND date < ?2",
            params![folder_id, cutoff],
        )?)
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
                    (SELECT COUNT(*) FROM messages m WHERE m.folder_id = f.id),
                    COALESCE(f.display, f.name), f.delimiter, f.subscribed
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
                display: r.get(6)?,
                delimiter: r.get(7)?,
                subscribed: r.get(8)?,
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
            db.execute(
                "UPDATE folders SET uidnext = NULL, highestmodseq = NULL, exists_count = NULL, window_cutoff = NULL WHERE id = ?1",
                [folder_id],
            )?;
        }
        db.execute(
            "UPDATE folders SET uidvalidity = ?2 WHERE id = ?1",
            params![folder_id, v],
        )?;
        Ok(reset)
    }

    pub fn folder_sync_state(&self, folder_id: i64) -> Result<Option<FolderSyncState>> {
        Ok(self
            .db()
            .query_row(
                "SELECT uidnext, highestmodseq, exists_count, window_days, window_cutoff FROM folders WHERE id = ?1",
                [folder_id],
                |r| {
                    let un: Option<u32> = r.get(0)?;
                    let ex: Option<u32> = r.get(2)?;
                    let hm: Option<i64> = r.get(1)?;
                    let wd: Option<u32> = r.get(3)?;
                    Ok(match (un, ex) {
                        (Some(uidnext), Some(exists)) => Some(FolderSyncState {
                            uidnext,
                            exists,
                            highestmodseq: hm.map(|v| v as u64),
                            window_days: wd.unwrap_or(0),
                            window_cutoff: r.get(4)?,
                        }),
                        _ => None,
                    })
                },
            )
            .optional()?
            .flatten())
    }

    /// `None` clears the state (next sync is a full one).
    pub fn set_folder_sync_state(&self, folder_id: i64, st: Option<FolderSyncState>) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET uidnext = ?2, highestmodseq = ?3, exists_count = ?4, window_days = ?5, window_cutoff = ?6 WHERE id = ?1",
            params![
                folder_id,
                st.map(|s| s.uidnext),
                st.and_then(|s| s.highestmodseq).map(|v| v as i64),
                st.map(|s| s.exists),
                st.map(|s| s.window_days),
                st.and_then(|s| s.window_cutoff)
            ],
        )?;
        Ok(())
    }

    /// Insert many messages in one transaction (fast bulk sync).
    pub fn insert_messages(&self, folder_id: i64, ms: &[NewMessage<'_>]) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        for m in ms {
            insert_in_tx(&tx, folder_id, m)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Apply many flag updates in one transaction.
    pub fn set_flags_bulk(&self, folder_id: i64, flags: &[(u32, bool, bool)]) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "UPDATE messages SET seen = ?3, flagged = ?4 WHERE folder_id = ?1 AND uid = ?2 AND (seen <> ?3 OR flagged <> ?4)",
            )?;
            for (uid, seen, flagged) in flags {
                st.execute(params![folder_id, uid, seen, flagged])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The user's recorded answer to an invitation: (PARTSTAT, added to calendar).
    pub fn invite_response(&self, message_id: i64) -> Result<Option<(String, bool)>> {
        Ok(self
            .db()
            .query_row(
                "SELECT partstat, in_calendar FROM invite_responses WHERE message_id = ?1",
                [message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn set_invite_response(
        &self,
        message_id: i64,
        partstat: &str,
        in_calendar: bool,
    ) -> Result<()> {
        self.db().execute(
            "INSERT INTO invite_responses (message_id, partstat, responded_at, in_calendar) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(message_id) DO UPDATE SET partstat = excluded.partstat,
               responded_at = excluded.responded_at, in_calendar = excluded.in_calendar",
            params![message_id, partstat, chrono::Utc::now().timestamp(), in_calendar],
        )?;
        Ok(())
    }

    /// Highest cached UID in a folder (None if empty).
    pub fn max_uid(&self, folder_id: i64) -> Result<Option<u32>> {
        Ok(self.db().query_row(
            "SELECT max(uid) FROM messages WHERE folder_id = ?1",
            [folder_id],
            |r| r.get(0),
        )?)
    }

    /// Unread messages with a UID above `uid`, newest first.
    pub fn unseen_after(
        &self,
        folder_id: i64,
        uid: u32,
        limit: i64,
    ) -> Result<Vec<MessageSummary>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {SUMMARY_COLS} FROM messages m WHERE m.folder_id = ?1 AND m.uid > ?2 AND m.seen = 0 ORDER BY m.uid DESC LIMIT ?3"
        ))?;
        let rows = st.query_map(params![folder_id, uid, limit], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ---- messages -------------------------------------------------------

    pub fn uids(&self, folder_id: i64) -> Result<Vec<u32>> {
        let db = self.db();
        let mut st = db.prepare("SELECT uid FROM messages WHERE folder_id = ?1")?;
        let rows = st.query_map([folder_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn insert_message(&self, folder_id: i64, m: &NewMessage<'_>) -> Result<i64> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let id = insert_in_tx(&tx, folder_id, m)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn remove_uids(&self, folder_id: i64, uids: &[u32]) -> Result<usize> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let mut n = 0;
        for uid in uids {
            n += tx.execute(
                "DELETE FROM messages WHERE folder_id = ?1 AND uid = ?2",
                params![folder_id, uid],
            )?;
        }
        tx.commit()?;
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

    /// Starred messages in all accounts (To-do "Flagged emails"), not in Trash/Junk.
    pub fn flagged_messages(&self, limit: i64) -> Result<Vec<MessageSummary>> {
        let db = self.db();
        let mut st = db.prepare(&format!(
            "SELECT {SUMMARY_COLS} FROM messages m JOIN folders f ON f.id = m.folder_id
             WHERE m.flagged = 1 AND coalesce(f.role, '') NOT IN ('trash', 'junk')
             ORDER BY m.date DESC LIMIT ?1"
        ))?;
        let rows = st.query_map([limit.clamp(1, 500)], summary_from_row)?;
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
        let (summary, p) = self.parsed_message(id)?;
        let s = parse::sanitized_html_with(&p, remote);
        let urls = s
            .as_ref()
            .map(|s| s.remote_urls.clone())
            .unwrap_or_default();
        Ok((detail_from_parsed(summary, p, s), urls))
    }

    pub fn message_detail(&self, id: i64) -> Result<MessageDetail> {
        let (summary, p) = self.parsed_message(id)?;
        let sanitized = parse::sanitized_html(&p);
        Ok(detail_from_parsed(summary, p, sanitized))
    }

    fn parsed_message(&self, id: i64) -> Result<(MessageSummary, ParsedMessage)> {
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
        Ok((summary, parse::parse(&raw)))
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

fn detail_from_parsed(
    summary: MessageSummary,
    p: ParsedMessage,
    sanitized: Option<crate::sanitize::Sanitized>,
) -> MessageDetail {
    MessageDetail {
        summary,
        to: p.to,
        cc: p.cc,
        text: p.text,
        remote_images_blocked: sanitized.as_ref().map_or(0, |s| s.remote_images_blocked),
        remote_images_loaded: sanitized.as_ref().map_or(0, |s| s.remote_images_loaded),
        html: sanitized.map(|s| s.html),
        attachments: p.attachments,
    }
}

#[derive(Debug, Default)]
pub struct OpenReport {
    /// A plaintext cache was found and encrypted.
    pub migrated: bool,
    /// The cache could not be decrypted and was moved to this path.
    pub unreadable_moved_to: Option<std::path::PathBuf>,
}

fn insert_in_tx(tx: &rusqlite::Transaction<'_>, folder_id: i64, m: &NewMessage<'_>) -> Result<i64> {
    // A repeated sync must leave the raw message, FTS index and address
    // history consistent. Check before MIME parsing so retries are cheap.
    if let Some(id) = tx
        .prepare_cached("SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2")?
        .query_row(params![folder_id, m.uid], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    let p: ParsedMessage = parse::parse(m.raw);
    let body_index = if p.text.trim().is_empty() {
        p.html_raw
            .as_deref()
            .map(parse::strip_tags)
            .unwrap_or_default()
    } else {
        p.text.clone()
    };
    tx.execute(
            "INSERT INTO messages (folder_id, uid, message_id, subject, from_name, from_addr, to_addrs, cc_addrs, date, preview, seen, flagged, has_attachments, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![folder_id, m.uid, p.message_id, p.subject, p.from_name, p.from_addr, p.to, p.cc, p.date, p.preview,
                    m.seen, m.flagged, !p.attachments.is_empty(), m.raw],
        )?;
    let id = tx.last_insert_rowid();
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
    Ok(id)
}

fn is_not_a_db(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::NotADatabase)
    )
}

/// True if the file exists and starts with the plain SQLite header.
/// (An SQLCipher database starts with random salt instead.)
pub fn is_plaintext_sqlite(path: &Path) -> bool {
    use std::io::Read;
    let mut h = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut h))
        .map(|()| &h == b"SQLite format 3\0")
        .unwrap_or(false)
}

fn open_keyed(path: &Path, key: &CacheKey) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_key(&conn, "main", key)?;
    // Fails with NotADatabase if the key is wrong.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })?;
    Ok(conn)
}

fn apply_key(conn: &Connection, schema: &str, key: &CacheKey) -> Result<()> {
    // PRAGMA arguments can't be bound as parameters; the value is our own
    // hex string, never user input.
    let v = key.pragma_value();
    conn.execute_batch(&zeroize::Zeroizing::new(format!(
        "PRAGMA {schema}.key = {};",
        v.as_str()
    )))?;
    Ok(())
}

/// Encrypt an existing plaintext cache in place:
/// checkpoint the WAL, export into a new encrypted file, verify it, swap it
/// in atomically, then overwrite and delete the plaintext copy.
fn encrypt_plaintext_db(path: &Path, key: &CacheKey) -> Result<()> {
    let tmp = path.with_extension("sqlite3.encrypting");
    let _ = std::fs::remove_file(&tmp);
    {
        let plain = Connection::open(path)?;
        plain.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        let version: i64 = plain.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let tmp_s = tmp
            .to_str()
            .ok_or_else(|| Error::Invalid("non-UTF-8 cache path".into()))?;
        // SQLCipher treats a key string of the form x'<hex>' as a raw key.
        let raw = zeroize::Zeroizing::new(format!("x'{}'", key.hex().as_str()));
        plain.execute("ATTACH DATABASE ?1 AS enc KEY ?2", [tmp_s, raw.as_str()])?;
        plain.query_row("SELECT sqlcipher_export('enc')", [], |_| Ok(()))?;
        plain.execute_batch(&format!("PRAGMA enc.user_version = {version};"))?;
        plain.execute_batch("DETACH DATABASE enc;")?;
    }
    restrict_permissions(&tmp);
    // Verify before replacing anything.
    let check = open_keyed(&tmp, key)?;
    let ok: String = check.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    drop(check);
    if ok != "ok" {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Invalid(format!(
            "encrypted copy failed integrity check: {ok}"
        )));
    }
    // Keep the plaintext bytes to scrub them after the swap.
    let old = path.with_extension("sqlite3.plaintext-old");
    std::fs::rename(path, &old)?;
    std::fs::rename(&tmp, path)?;
    for ext in ["sqlite3-wal", "sqlite3-shm"] {
        let p = path.with_extension(ext);
        let _ = scrub_and_remove(&p);
    }
    scrub_and_remove(&old)?;
    if let Some(dir) = path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    tracing::info!("plaintext cache encrypted");
    Ok(())
}

/// Best effort: overwrite with zeros, fsync, delete. (On SSDs and
/// copy-on-write filesystems old blocks may survive; see AUDIT.md.)
fn scrub_and_remove(p: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let len = match std::fs::metadata(p) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(p)?;
        let zeros = vec![0u8; 64 * 1024];
        let mut left = len;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            f.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        f.sync_all()?;
    }
    std::fs::remove_file(p)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

// ---- folder management, rules ----------------------------------------------

/// Sieve settings of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SieveSettings {
    pub port: u16,
    /// Our script is uploaded and active on the server.
    pub active: bool,
}

impl Store {
    /// Insert/update a folder from the server's LIST.
    pub fn upsert_remote_folder(
        &self,
        account_id: i64,
        f: &crate::imap::RemoteFolder,
    ) -> Result<i64> {
        let db = self.db();
        db.execute(
            "INSERT INTO folders (account_id, name, role, display, delimiter, subscribed) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account_id, name) DO UPDATE SET role = excluded.role, display = excluded.display,
               delimiter = excluded.delimiter, subscribed = excluded.subscribed",
            params![account_id, f.name, f.role, f.display, f.delimiter, f.subscribed],
        )?;
        Ok(db.query_row(
            "SELECT id FROM folders WHERE account_id = ?1 AND name = ?2",
            params![account_id, f.name],
            |r| r.get(0),
        )?)
    }

    pub fn folder(&self, folder_id: i64) -> Result<Folder> {
        let account_id: i64 = self
            .db()
            .query_row(
                "SELECT account_id FROM folders WHERE id = ?1",
                [folder_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("folder {folder_id}")))?;
        self.folders(account_id)?
            .into_iter()
            .find(|f| f.id == folder_id)
            .ok_or_else(|| Error::NotFound(format!("folder {folder_id}")))
    }

    /// Rename a folder and its subfolders in the cache (keeps cached mail;
    /// the next sync checks UIDVALIDITY as usual).
    pub fn rename_folder_local(
        &self,
        account_id: i64,
        old_wire: &str,
        new_wire: &str,
        delimiter: Option<&str>,
    ) -> Result<()> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let rows: Vec<(i64, String)> = {
            let mut st = tx.prepare("SELECT id, name FROM folders WHERE account_id = ?1")?;
            let v = st
                .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            v
        };
        for (id, name) in rows {
            let renamed = if name == old_wire {
                Some(new_wire.to_string())
            } else {
                delimiter.filter(|d| !d.is_empty()).and_then(|d| {
                    name.strip_prefix(&format!("{old_wire}{d}"))
                        .map(|rest| format!("{new_wire}{d}{rest}"))
                })
            };
            if let Some(n) = renamed {
                tx.execute(
                    "UPDATE folders SET name = ?2, display = ?3 WHERE id = ?1",
                    params![id, n, crate::mutf7::decode(&n)],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_folder_local(&self, folder_id: i64) -> Result<()> {
        self.db()
            .execute("DELETE FROM folders WHERE id = ?1", [folder_id])?;
        Ok(())
    }

    pub fn set_folder_subscribed(&self, folder_id: i64, on: bool) -> Result<()> {
        self.db().execute(
            "UPDATE folders SET subscribed = ?2 WHERE id = ?1",
            params![folder_id, on],
        )?;
        Ok(())
    }

    /// (folder id, uid, Message-ID) of cached messages, all in one account.
    pub fn message_refs(&self, ids: &[i64]) -> Result<Vec<(i64, i64, u32, String)>> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT f.account_id, m.folder_id, m.uid, m.message_id FROM messages m JOIN folders f ON f.id = m.folder_id WHERE m.id = ?1",
        )?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(r) = st
                .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .optional()?
            {
                out.push(r);
            }
        }
        Ok(out)
    }

    /// Ids of all cached messages in a folder (for "Run rules now").
    pub fn message_ids(&self, folder_id: i64) -> Result<Vec<i64>> {
        let db = self.db();
        let mut st = db.prepare("SELECT id FROM messages WHERE folder_id = ?1 ORDER BY uid")?;
        let rows = st.query_map([folder_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Cached message ids for UIDs of a folder.
    pub fn ids_for_uids(&self, folder_id: i64, uids: &[u32]) -> Result<Vec<i64>> {
        let db = self.db();
        let mut st = db.prepare("SELECT id FROM messages WHERE folder_id = ?1 AND uid = ?2")?;
        let mut out = Vec::new();
        for u in uids {
            if let Some(id) = st
                .query_row(params![folder_id, u], |r| r.get(0))
                .optional()?
            {
                out.push(id);
            }
        }
        Ok(out)
    }

    pub fn set_flagged_local(&self, folder_id: i64, uid: u32) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET flagged = 1 WHERE folder_id = ?1 AND uid = ?2",
            params![folder_id, uid],
        )?;
        Ok(())
    }

    pub fn set_seen_uid_local(&self, folder_id: i64, uid: u32) -> Result<()> {
        self.db().execute(
            "UPDATE messages SET seen = 1 WHERE folder_id = ?1 AND uid = ?2",
            params![folder_id, uid],
        )?;
        Ok(())
    }

    // ---- rules ----

    pub fn rules(&self) -> Result<Vec<crate::rules::Rule>> {
        let db = self.db();
        let mut st = db.prepare("SELECT id, def FROM rules ORDER BY position, id")?;
        let rows: Vec<(i64, String)> = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        rows.into_iter()
            .map(|(id, def)| {
                let mut r: crate::rules::Rule = serde_json::from_str(&def)
                    .map_err(|e| Error::Invalid(format!("stored rule {id} is unreadable: {e}")))?;
                r.id = id;
                Ok(r)
            })
            .collect()
    }

    /// Replace all rules (ordered). Rules keep their id when it is given
    /// and exists; new rules get one. Returns the stored rules.
    pub fn save_rules(&self, rules: &[crate::rules::Rule]) -> Result<Vec<crate::rules::Rule>> {
        crate::rules::validate_all(rules)?;
        {
            let mut db = self.db();
            let tx = db.transaction()?;
            let keep: Vec<i64> = rules.iter().map(|r| r.id).filter(|id| *id > 0).collect();
            let existing: Vec<i64> = {
                let mut st = tx.prepare("SELECT id FROM rules")?;
                let v = st
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                v
            };
            for id in existing.iter().filter(|id| !keep.contains(id)) {
                tx.execute("DELETE FROM rules WHERE id = ?1", [id])?;
            }
            for (pos, r) in rules.iter().enumerate() {
                let mut stored = r.clone();
                stored.id = 0;
                let def =
                    serde_json::to_string(&stored).map_err(|e| Error::Invalid(e.to_string()))?;
                if r.id > 0 && existing.contains(&r.id) {
                    tx.execute(
                        "UPDATE rules SET position = ?2, def = ?3 WHERE id = ?1",
                        params![r.id, pos as i64, def],
                    )?;
                } else {
                    tx.execute(
                        "INSERT INTO rules (position, def) VALUES (?1, ?2)",
                        params![pos as i64, def],
                    )?;
                }
            }
            tx.commit()?;
        }
        self.rules()
    }

    /// Record that rules ran on a message; false if they already had
    /// (loop guard against re-processing, e.g. after a UIDVALIDITY reset or
    /// a message filed back into the INBOX).
    pub fn claim_rule_run(&self, account_id: i64, message_key: &str) -> Result<bool> {
        if message_key.is_empty() {
            return Ok(true);
        }
        let n = self.db().execute(
            "INSERT OR IGNORE INTO rule_runs (account_id, message_key, at) VALUES (?1, ?2, ?3)",
            params![account_id, message_key, chrono::Utc::now().timestamp()],
        )?;
        Ok(n == 1)
    }

    /// Count a forward against the hourly budget; false = over the limit.
    pub fn take_forward_budget(&self, account_id: i64, limit: i64) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let db = self.db();
        db.execute("DELETE FROM rule_forwards WHERE at < ?1", [now - 3600])?;
        let used: i64 = db.query_row(
            "SELECT COUNT(*) FROM rule_forwards WHERE account_id = ?1",
            [account_id],
            |r| r.get(0),
        )?;
        if used >= limit {
            return Ok(false);
        }
        db.execute(
            "INSERT INTO rule_forwards (account_id, at) VALUES (?1, ?2)",
            params![account_id, now],
        )?;
        Ok(true)
    }

    pub fn sieve_settings(&self, account_id: i64) -> Result<SieveSettings> {
        Ok(self
            .db()
            .query_row(
                "SELECT sieve_port, sieve_active FROM account_settings WHERE account_id = ?1",
                [account_id],
                |r| {
                    Ok(SieveSettings {
                        port: r.get(0)?,
                        active: r.get(1)?,
                    })
                },
            )
            .optional()?
            .unwrap_or(SieveSettings {
                port: crate::managesieve::DEFAULT_PORT,
                active: false,
            }))
    }

    pub fn set_sieve_settings(&self, account_id: i64, s: SieveSettings) -> Result<()> {
        if s.port == 0 {
            return Err(Error::Invalid("bad ManageSieve port".into()));
        }
        self.account(account_id)?;
        self.db().execute(
            "INSERT INTO account_settings (account_id, sieve_port, sieve_active) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id) DO UPDATE SET sieve_port = excluded.sieve_port, sieve_active = excluded.sieve_active",
            params![account_id, s.port, s.active],
        )?;
        Ok(())
    }
}
