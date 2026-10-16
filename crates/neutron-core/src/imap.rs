//! IMAP access via async-imap on tokio. Only encrypted connections are
//! possible: implicit TLS or STARTTLS (mandatory upgrade; a server that
//! refuses STARTTLS is an error, never a plaintext fallback).

use std::collections::HashSet;
use std::time::Duration;

use async_imap::types::Flag;
use async_imap::types::NameAttribute;
use futures::TryStreamExt;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::model::{AccountConfig, Security};
use crate::{tls, Error, Result};

pub type Session = async_imap::Session<TlsStream<TcpStream>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Upper bound for any single IMAP command (including streaming its
/// response). A server that never completes a command (e.g. answers with
/// an unparsable tagged BAD, audit O13) makes the call fail instead of
/// hanging the sync forever.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

static COMMAND_TIMEOUT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(COMMAND_TIMEOUT.as_millis() as u64);

/// Override the per-command timeout (tests use a short one).
pub fn set_command_timeout(d: Duration) {
    COMMAND_TIMEOUT_MS.store(d.as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
}

fn command_timeout() -> Duration {
    Duration::from_millis(COMMAND_TIMEOUT_MS.load(std::sync::atomic::Ordering::Relaxed))
}

async fn timed<T>(fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(CONNECT_TIMEOUT, fut)
        .await
        .map_err(|_| Error::Timeout)?
}

/// Run one IMAP command with [`COMMAND_TIMEOUT`].
async fn cmd<T, E: Into<Error>>(
    fut: impl std::future::Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    match tokio::time::timeout(command_timeout(), fut).await {
        Ok(r) => r.map_err(Into::into),
        Err(_) => Err(Error::Timeout),
    }
}

struct XOAuth2 {
    response: String,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;
    fn process(&mut self, _challenge: &[u8]) -> String {
        // First call sends the token; a server error challenge (JSON) is
        // answered with an empty line so the server sends the tagged NO.
        std::mem::take(&mut self.response)
    }
}

/// Open an authenticated session.
pub async fn connect(cfg: &AccountConfig, password: &str) -> Result<Session> {
    timed(async {
        let tcp = TcpStream::connect((cfg.imap_host.as_str(), cfg.imap_port)).await?;
        let extra = cfg.extra_ca_pem.as_deref();
        let tls_stream = match cfg.imap_security {
            Security::Tls => tls::wrap(tcp, &cfg.imap_host, extra).await?,
            Security::Starttls => {
                let mut plain = async_imap::Client::new(tcp);
                let _greeting = plain.read_response().await;
                plain.run_command_and_check_ok("STARTTLS", None).await?;
                tls::wrap(plain.into_inner(), &cfg.imap_host, extra).await?
            }
        };
        let mut client = async_imap::Client::new(tls_stream);
        if cfg.imap_security == Security::Tls {
            let _greeting = client.read_response().await;
        }
        let mut session = if cfg.auth.is_oauth() {
            // `password` is the OAuth2 access token (RFC 7628 / XOAUTH2).
            let auth = XOAuth2 {
                response: crate::oauth::xoauth2_string(&cfg.username, password),
            };
            client
                .authenticate("XOAUTH2", auth)
                .await
                .map_err(|(e, _)| Error::from(e))?
        } else {
            client
                .login(&cfg.username, password)
                .await
                .map_err(|(e, _)| Error::from(e))?
        };
        // Every session uses the same name encoding (UTF-8 when the server
        // accepts it, modified UTF-7 otherwise), so stored wire names work
        // on any connection.
        enable_utf8(&mut session).await?;
        Ok(session)
    })
    .await
}

#[derive(Debug, Clone)]
pub struct RemoteFolder {
    /// Name as on the wire (modified UTF-7 unless UTF8=ACCEPT); used for
    /// every command.
    pub name: String,
    /// Decoded Unicode name for display.
    pub display: String,
    pub role: Option<&'static str>,
    pub selectable: bool,
    /// Hierarchy delimiter (e.g. "." or "/"), None for flat namespaces.
    pub delimiter: Option<String>,
    pub subscribed: bool,
}

fn role_of(name: &str, attrs: &[NameAttribute<'_>]) -> Option<&'static str> {
    for a in attrs {
        let r = match a {
            NameAttribute::Sent => Some("sent"),
            NameAttribute::Drafts => Some("drafts"),
            NameAttribute::Trash => Some("trash"),
            NameAttribute::Junk => Some("junk"),
            NameAttribute::Archive => Some("archive"),
            NameAttribute::Flagged => Some("flagged"),
            NameAttribute::All => Some("all"),
            _ => None,
        };
        if r.is_some() {
            return r;
        }
    }
    if name.eq_ignore_ascii_case("INBOX") {
        return Some("inbox");
    }
    None
}

// async-imap 0.12's name stream discards the tagged completion status.
// Read parsed responses ourselves so NO/BAD/EOF can never masquerade as
// a complete inventory and cause retain_folders to remove cached folders.
async fn checked_names(
    session: &mut Session,
    command: &str,
) -> std::result::Result<Vec<RemoteFolder>, async_imap::error::Error> {
    use async_imap::error::Error as ImapError;
    use async_imap::imap_proto::{MailboxDatum, Response, Status};
    let tag = session.run_command(command).await?;
    let mut folders = Vec::new();
    loop {
        let response = session
            .read_response()
            .await?
            .ok_or(ImapError::ConnectionLost)?;
        match response.parsed() {
            Response::Done {
                tag: got, status, ..
            } if *got == tag => {
                return match status {
                    Status::Ok => Ok(folders),
                    Status::No => Err(ImapError::No("folder listing rejected".into())),
                    _ => Err(ImapError::Bad("folder listing rejected".into())),
                };
            }
            Response::Data {
                status: Status::Bye,
                ..
            } => return Err(ImapError::ConnectionLost),
            Response::MailboxData(MailboxDatum::List(data)) => {
                folders.push(RemoteFolder {
                    name: data.name.to_string(),
                    display: crate::mutf7::decode(&data.name),
                    role: role_of(&data.name, &data.name_attributes),
                    selectable: !data
                        .name_attributes
                        .iter()
                        .any(|a| matches!(a, NameAttribute::NoSelect)),
                    delimiter: data.delimiter.as_deref().map(str::to_string),
                    subscribed: false,
                });
            }
            _ => {}
        }
    }
}

pub async fn list_folders(session: &mut Session) -> Result<Vec<RemoteFolder>> {
    let mut folders = cmd(checked_names(session, "LIST \"\" \"*\"")).await?;
    let subscribed: Option<HashSet<String>> = cmd(async {
        match checked_names(session, "LSUB \"\" \"*\"").await {
            Ok(names) => Ok(Some(names.into_iter().map(|n| n.name).collect())),
            Err(async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_)) => {
                tracing::warn!("LSUB rejected; displaying all discovered folders");
                Ok(None)
            }
            Err(e) => Err(e),
        }
    })
    .await?;
    tracing::debug!(folders = folders.len(), "IMAP LIST completed");
    for f in &mut folders {
        f.subscribed = subscribed.as_ref().is_none_or(|s| s.contains(&f.name))
            || f.name.eq_ignore_ascii_case("INBOX");
        tracing::debug!(mailbox = %f.name, selectable = f.selectable, subscribed = f.subscribed, delimiter = ?f.delimiter, "discovered IMAP folder");
    }
    Ok(folders)
}

/// LOGOUT has the same deadline as every other command.
pub async fn logout(session: &mut Session) -> Result<()> {
    cmd(session.logout()).await
}

/// Enable RFC 6855 UTF8=ACCEPT when offered; afterwards mailbox names are
/// UTF-8 on the wire. Returns whether it is in effect.
pub async fn enable_utf8(session: &mut Session) -> Result<bool> {
    if !cmd(session.capabilities()).await?.has_str("UTF8=ACCEPT") {
        return Ok(false);
    }
    cmd(session.run_command_and_check_ok("ENABLE UTF8=ACCEPT")).await?;
    Ok(true)
}

/// Wire form of a Unicode mailbox name.
pub fn wire_name(name: &str, utf8: bool) -> String {
    if utf8 {
        name.to_string()
    } else {
        crate::mutf7::encode(name)
    }
}

/// Reject names the server would misparse or that hide as other folders.
pub fn validate_folder_name(name: &str, delimiter: Option<&str>) -> Result<()> {
    let bad = name.trim().is_empty()
        || name.len() > 512
        || name.chars().any(|c| c.is_control() || c == '*' || c == '%')
        || name.eq_ignore_ascii_case("INBOX");
    let empty_part = delimiter
        .filter(|d| !d.is_empty())
        .is_some_and(|d| name.split(d).any(|p| p.trim().is_empty()));
    if bad || empty_part {
        return Err(Error::Invalid(format!(
            "\"{name}\" is not a valid folder name"
        )));
    }
    Ok(())
}

pub async fn create_folder(session: &mut Session, wire: &str) -> Result<()> {
    cmd(session.create(wire)).await?;
    cmd(session.subscribe(wire)).await
}

/// Rename a folder (with its subfolders) and move the subscriptions of the
/// folder and its subfolders along (RENAME doesn't on every server).
pub async fn rename_folder(
    session: &mut Session,
    from: &str,
    to: &str,
    delim: Option<&str>,
) -> Result<()> {
    let mut subs: Vec<String> = Vec::new();
    if let Some(d) = delim.filter(|d| !d.is_empty()) {
        let prefix = format!("{from}{d}");
        subs = cmd(async {
            session
                .lsub(Some(""), Some(&format!("{prefix}*")))
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?
        .iter()
        .map(|n| n.name().to_string())
        .filter(|n| n.starts_with(&prefix))
        .collect();
    }
    cmd(session.rename(from, to)).await?;
    let _ = cmd(session.unsubscribe(from)).await;
    cmd(session.subscribe(to)).await?;
    for old in subs {
        let new = format!("{to}{}", &old[from.len()..]);
        let _ = cmd(session.unsubscribe(&old)).await;
        let _ = cmd(session.subscribe(&new)).await;
    }
    Ok(())
}

pub async fn delete_folder(session: &mut Session, wire: &str) -> Result<()> {
    // Deleting the selected mailbox makes some servers (Dovecot) drop the
    // connection: select another one read-only first.
    cmd(session.examine("INBOX")).await?;
    let _ = cmd(session.unsubscribe(wire)).await;
    cmd(session.delete(wire)).await
}

pub async fn set_subscribed(session: &mut Session, wire: &str, on: bool) -> Result<()> {
    if on {
        cmd(session.subscribe(wire)).await
    } else {
        cmd(session.unsubscribe(wire)).await
    }
}

pub async fn has_capability(session: &mut Session, cap: &str) -> Result<bool> {
    Ok(cmd(session.capabilities()).await?.has_str(cap))
}

/// Move UIDs between folders: `UID MOVE` (RFC 6851) when available,
/// otherwise `UID COPY` + `\Deleted` + `UID EXPUNGE` (UIDPLUS).
/// Servers with neither extension are rejected before changing any mail:
/// plain EXPUNGE would also delete unrelated messages marked by other clients.
pub async fn move_uids(session: &mut Session, from: &str, uids: &[u32], to: &str) -> Result<()> {
    let have_move = has_capability(session, "MOVE").await?;
    move_uids_with(session, from, uids, to, have_move).await
}

/// [`move_uids`] with the MOVE extension forced on/off (tests use the fallback).
pub async fn move_uids_with(
    session: &mut Session,
    from: &str,
    uids: &[u32],
    to: &str,
    use_move: bool,
) -> Result<()> {
    if uids.is_empty() {
        return Ok(());
    }
    if !use_move && !has_capability(session, "UIDPLUS").await? {
        return Err(Error::Invalid(
            "this server needs MOVE or UIDPLUS to move messages safely".into(),
        ));
    }
    cmd(session.select(from)).await?;
    for chunk in uids.chunks(200) {
        let set = uid_set(chunk);
        if use_move {
            cmd(session.uid_mv(&set, to)).await?;
        } else {
            cmd(session.uid_copy(&set, to)).await?;
            store_uids(session, chunk, "+FLAGS.SILENT (\\Deleted)").await?;
            cmd(async {
                session
                    .uid_expunge(&set)
                    .await?
                    .try_collect::<Vec<_>>()
                    .await
            })
            .await?;
        }
    }
    Ok(())
}

pub async fn copy_uids(session: &mut Session, from: &str, uids: &[u32], to: &str) -> Result<()> {
    if uids.is_empty() {
        return Ok(());
    }
    cmd(session.select(from)).await?;
    for chunk in uids.chunks(200) {
        cmd(session.uid_copy(uid_set(chunk), to)).await?;
    }
    Ok(())
}

/// `UID STORE` on the selected folder, e.g. `+FLAGS.SILENT (\Seen)`.
pub async fn store_uids(session: &mut Session, uids: &[u32], what: &str) -> Result<()> {
    for chunk in uids.chunks(200) {
        cmd(async {
            session
                .uid_store(uid_set(chunk), what)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?;
    }
    Ok(())
}

/// UIDs in `folder` whose Message-ID header equals `message_id`.
pub async fn uids_by_message_id(
    session: &mut Session,
    folder: &str,
    message_id: &str,
) -> Result<Vec<u32>> {
    let mid: String = message_id
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    if mid.is_empty() {
        return Ok(Vec::new());
    }
    cmd(session.select(folder)).await?;
    let mut v: Vec<u32> = cmd(session.uid_search(format!("HEADER Message-ID \"{mid}\"")))
        .await?
        .into_iter()
        .collect();
    v.sort_unstable();
    Ok(v)
}

/// IMAP keyword (flag atom) check: RFC 3501 atom chars, no backslash.
pub fn valid_keyword(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.bytes().all(|b| {
            b.is_ascii_graphic()
                && !matches!(b, b'(' | b')' | b'{' | b'%' | b'*' | b'"' | b'\\' | b']')
        })
}

pub struct FolderState {
    pub uidvalidity: u32,
    pub uidnext: Option<u32>,
    pub exists: u32,
    /// HIGHESTMODSEQ when the server supports CONDSTORE (RFC 7162).
    pub highestmodseq: Option<u64>,
}

pub async fn has_condstore(session: &mut Session) -> Result<bool> {
    let caps = cmd(session.capabilities()).await?;
    Ok(caps.has_str("CONDSTORE") || caps.has_str("QRESYNC"))
}

/// SELECT the folder, enabling CONDSTORE when `condstore` is set.
pub async fn select_state(
    session: &mut Session,
    folder: &str,
    condstore: bool,
) -> Result<FolderState> {
    let mb = if condstore {
        cmd(session.select_condstore(folder)).await?
    } else {
        cmd(session.select(folder)).await?
    };
    Ok(FolderState {
        uidvalidity: mb.uid_validity.unwrap_or(0),
        uidnext: mb.uid_next,
        exists: mb.exists,
        highestmodseq: if condstore { mb.highest_modseq } else { None },
    })
}

/// Every UID in the selected folder.
pub async fn all_uids(session: &mut Session, exists: u32) -> Result<HashSet<u32>> {
    if exists == 0 {
        return Ok(HashSet::new());
    }
    cmd(session.uid_search("ALL")).await
}

/// UIDs >= `from` (optionally also SINCE `since`). `UID SEARCH UID n:*`
/// always matches the highest UID even if it is below n, so filter.
pub async fn uids_from(
    session: &mut Session,
    from: u32,
    since: Option<i64>,
) -> Result<HashSet<u32>> {
    let mut q = format!("UID {}:*", from.max(1));
    if let Some(t) = since {
        q.push_str(&format!(" SINCE {}", imap_day(t)));
    }
    let r = cmd(session.uid_search(q)).await?;
    Ok(r.into_iter().filter(|u| *u >= from).collect())
}

fn imap_day(since: i64) -> String {
    chrono::DateTime::from_timestamp(since, 0)
        .unwrap_or_default()
        .format("%d-%b-%Y")
        .to_string()
}

/// UIDs of messages with an internal date on or after `since` (unix time).
/// IMAP SEARCH SINCE has day granularity (RFC 3501 §6.4.4).
pub async fn uids_since(session: &mut Session, since: i64) -> Result<HashSet<u32>> {
    cmd(session.uid_search(format!("SINCE {}", imap_day(since)))).await
}

pub struct FetchedMessage {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
    pub raw: Vec<u8>,
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Fetch full messages (BODY.PEEK so \Seen is not set) for the given UIDs.
pub async fn fetch_full(session: &mut Session, uids: &[u32]) -> Result<Vec<FetchedMessage>> {
    let mut out = Vec::with_capacity(uids.len());
    for chunk in uids.chunks(50) {
        // The deadline applies to each message, not the whole page: 50
        // large messages on a slow link can legitimately take longer than
        // one command timeout, and a page that always times out would stop
        // the folder from ever syncing.
        let mut fetches = Vec::with_capacity(chunk.len());
        {
            let stream = cmd(session.uid_fetch(uid_set(chunk), "(UID FLAGS BODY.PEEK[])")).await?;
            futures::pin_mut!(stream);
            while let Some(f) = cmd(stream.try_next()).await? {
                fetches.push(f);
            }
        }
        // The upstream fetch stream also drops tagged NO/BAD. A missing
        // UID/body must not advance the sync checkpoint as if it was saved.
        let returned: HashSet<u32> = fetches
            .iter()
            .filter(|f| f.body().is_some())
            .filter_map(|f| f.uid)
            .collect();
        if chunk.iter().any(|uid| !returned.contains(uid)) {
            return Err(Error::Imap(
                "server did not return all requested messages; sync will retry".into(),
            ));
        }
        for f in fetches {
            let (Some(uid), Some(body)) = (f.uid, f.body()) else {
                continue;
            };
            let flags: Vec<Flag<'_>> = f.flags().collect();
            out.push(FetchedMessage {
                uid,
                seen: flags.contains(&Flag::Seen),
                flagged: flags.contains(&Flag::Flagged),
                raw: body.to_vec(),
            });
        }
    }
    Ok(out)
}

/// (uid, seen, flagged) for every message in the selected folder, or with
/// `changed_since` (CONDSTORE) only for messages whose MODSEQ is higher.
pub async fn fetch_flags(
    session: &mut Session,
    changed_since: Option<u64>,
) -> Result<Vec<(u32, bool, bool)>> {
    let query = match changed_since {
        Some(m) => format!("(UID FLAGS) (CHANGEDSINCE {m})"),
        None => "(UID FLAGS)".to_string(),
    };
    let fetches: Vec<_> = cmd(async {
        session
            .uid_fetch("1:*", query)
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await?;
    Ok(fetches
        .iter()
        .filter_map(|f| {
            let flags: Vec<Flag<'_>> = f.flags().collect();
            f.uid.map(|u| {
                (
                    u,
                    flags.contains(&Flag::Seen),
                    flags.contains(&Flag::Flagged),
                )
            })
        })
        .collect())
}

pub async fn mark_seen(session: &mut Session, folder: &str, uid: u32) -> Result<()> {
    cmd(session.select(folder)).await?;
    cmd(async {
        session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Seen)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await?;
    Ok(())
}

pub async fn append(session: &mut Session, folder: &str, raw: &[u8], seen: bool) -> Result<()> {
    cmd(session.append(folder, seen.then_some("(\\Seen)"), None, raw)).await
}

/// APPEND with an explicit flag list, e.g. `(\\Seen \\Draft)` for drafts.
pub async fn append_with_flags(
    session: &mut Session,
    folder: &str,
    raw: &[u8],
    flags: &str,
) -> Result<()> {
    cmd(session.append(folder, Some(flags), None, raw)).await
}

/// SELECT a folder (read-write).
pub async fn select(session: &mut Session, folder: &str) -> Result<()> {
    cmd(session.select(folder)).await?;
    Ok(())
}

/// Permanently remove the given UIDs from a folder. Requires UIDPLUS so
/// messages already flagged \Deleted by another client cannot be expunged
/// as a side effect; unsupported servers are rejected before any change.
pub async fn expunge_uids(session: &mut Session, folder: &str, uids: &[u32]) -> Result<()> {
    if uids.is_empty() {
        return Ok(());
    }
    if !has_capability(session, "UIDPLUS").await? {
        return Err(Error::Invalid(
            "this server needs UIDPLUS to delete messages permanently".into(),
        ));
    }
    cmd(session.select(folder)).await?;
    for chunk in uids.chunks(200) {
        store_uids(session, chunk, "+FLAGS.SILENT (\\Deleted)").await?;
        let set = uid_set(chunk);
        cmd(async {
            session
                .uid_expunge(&set)
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?;
    }
    Ok(())
}

/// Permanently remove one message (used to replace a resumed draft).
/// Requires UIDPLUS so messages already flagged \Deleted by another client
/// cannot be expunged as a side effect. Unsupported servers are rejected
/// before marking anything deleted.
pub async fn delete_uid(session: &mut Session, folder: &str, uid: u32) -> Result<()> {
    if !has_capability(session, "UIDPLUS").await? {
        return Err(Error::Invalid(
            "this server needs UIDPLUS to delete a single message safely".into(),
        ));
    }
    cmd(session.select(folder)).await?;
    cmd(async {
        session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await?;
    cmd(async {
        session
            .uid_expunge(uid.to_string())
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await?;
    Ok(())
}

pub async fn supports_idle(session: &mut Session) -> Result<bool> {
    Ok(cmd(session.capabilities()).await?.has_str("IDLE"))
}

/// Wait on INBOX with IDLE until the server reports a change or `max_wait`
/// elapses. Returns the session for reuse and whether a change was seen.
pub async fn idle_inbox(mut session: Session, max_wait: Duration) -> Result<(Session, bool)> {
    cmd(session.select("INBOX")).await?;
    let mut handle = session.idle();
    cmd(handle.init()).await?;
    let (wait, _stop) = handle.wait_with_timeout(max_wait);
    let changed = matches!(
        wait.await?,
        async_imap::extensions::idle::IdleResponse::NewData(_)
    );
    let session = cmd(handle.done()).await?;
    Ok((session, changed))
}
