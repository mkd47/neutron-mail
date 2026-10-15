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
        client
            .login(&cfg.username, password)
            .await
            .map_err(|(e, _)| Error::from(e))
    })
    .await
}

#[derive(Debug, Clone)]
pub struct RemoteFolder {
    pub name: String,
    pub role: Option<&'static str>,
    pub selectable: bool,
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

pub async fn list_folders(session: &mut Session) -> Result<Vec<RemoteFolder>> {
    let names: Vec<_> = cmd(async {
        session
            .list(Some(""), Some("*"))
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await?;
    Ok(names
        .iter()
        .map(|n| RemoteFolder {
            name: n.name().to_string(),
            role: role_of(n.name(), n.attributes()),
            selectable: !n
                .attributes()
                .iter()
                .any(|a| matches!(a, NameAttribute::NoSelect)),
        })
        .collect())
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
        let fetches: Vec<_> = cmd(async {
            session
                .uid_fetch(uid_set(chunk), "(UID FLAGS BODY.PEEK[])")
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?;
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

/// Permanently remove one message (used to replace a resumed draft).
/// Uses UID EXPUNGE when the server has UIDPLUS so other messages already
/// flagged \Deleted by another client are not expunged as a side effect.
pub async fn delete_uid(session: &mut Session, folder: &str, uid: u32) -> Result<()> {
    cmd(session.select(folder)).await?;
    cmd(async {
        session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await?;
    let uidplus = cmd(session.capabilities()).await?.has_str("UIDPLUS");
    if uidplus {
        cmd(async {
            session
                .uid_expunge(uid.to_string())
                .await?
                .try_collect::<Vec<_>>()
                .await
        })
        .await?;
    } else {
        cmd(async { session.expunge().await?.try_collect::<Vec<_>>().await }).await?;
    }
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
