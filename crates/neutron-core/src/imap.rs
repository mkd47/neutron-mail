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

async fn timed<T>(fut: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(CONNECT_TIMEOUT, fut)
        .await
        .map_err(|_| Error::Timeout)?
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
    let names: Vec<_> = session
        .list(Some(""), Some("*"))
        .await?
        .try_collect()
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
    pub uids: HashSet<u32>,
}

pub async fn select(session: &mut Session, folder: &str) -> Result<FolderState> {
    let mb = session.select(folder).await?;
    let uidvalidity = mb.uid_validity.unwrap_or(0);
    let uids: HashSet<u32> = if mb.exists == 0 {
        HashSet::new()
    } else {
        session.uid_search("ALL").await?
    };
    Ok(FolderState { uidvalidity, uids })
}

/// UIDs of messages with an internal date on or after `since` (unix time).
/// IMAP SEARCH SINCE has day granularity (RFC 3501 §6.4.4).
pub async fn uids_since(session: &mut Session, since: i64) -> Result<HashSet<u32>> {
    let day = chrono::DateTime::from_timestamp(since, 0)
        .unwrap_or_default()
        .format("%d-%b-%Y");
    Ok(session.uid_search(format!("SINCE {day}")).await?)
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
        let fetches: Vec<_> = session
            .uid_fetch(uid_set(chunk), "(UID FLAGS BODY.PEEK[])")
            .await?
            .try_collect()
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

/// (uid, seen, flagged) for every message in the selected folder.
pub async fn fetch_flags(session: &mut Session) -> Result<Vec<(u32, bool, bool)>> {
    let fetches: Vec<_> = session
        .uid_fetch("1:*", "(UID FLAGS)")
        .await?
        .try_collect()
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
    session.select(folder).await?;
    let _: Vec<_> = session
        .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Seen)")
        .await?
        .try_collect()
        .await?;
    Ok(())
}

pub async fn append(session: &mut Session, folder: &str, raw: &[u8], seen: bool) -> Result<()> {
    session
        .append(folder, seen.then_some("(\\Seen)"), None, raw)
        .await?;
    Ok(())
}

/// APPEND with an explicit flag list, e.g. `(\\Seen \\Draft)` for drafts.
pub async fn append_with_flags(
    session: &mut Session,
    folder: &str,
    raw: &[u8],
    flags: &str,
) -> Result<()> {
    session.append(folder, Some(flags), None, raw).await?;
    Ok(())
}

/// Permanently remove one message (used to replace a resumed draft).
/// Uses UID EXPUNGE when the server has UIDPLUS so other messages already
/// flagged \Deleted by another client are not expunged as a side effect.
pub async fn delete_uid(session: &mut Session, folder: &str, uid: u32) -> Result<()> {
    session.select(folder).await?;
    let _: Vec<_> = session
        .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
        .await?
        .try_collect()
        .await?;
    let uidplus = session.capabilities().await?.has_str("UIDPLUS");
    if uidplus {
        let _: Vec<_> = session
            .uid_expunge(uid.to_string())
            .await?
            .try_collect()
            .await?;
    } else {
        let _: Vec<_> = session.expunge().await?.try_collect().await?;
    }
    Ok(())
}

pub async fn supports_idle(session: &mut Session) -> Result<bool> {
    Ok(session.capabilities().await?.has_str("IDLE"))
}

/// Wait on INBOX with IDLE until the server reports a change or `max_wait`
/// elapses. Returns the session for reuse and whether a change was seen.
pub async fn idle_inbox(mut session: Session, max_wait: Duration) -> Result<(Session, bool)> {
    session.select("INBOX").await?;
    let mut handle = session.idle();
    handle.init().await?;
    let (wait, _stop) = handle.wait_with_timeout(max_wait);
    let changed = matches!(
        wait.await?,
        async_imap::extensions::idle::IdleResponse::NewData(_)
    );
    let session = handle.done().await?;
    Ok((session, changed))
}
