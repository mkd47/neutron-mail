//! Account synchronization: folders, new messages, flag changes, expunges.
//!
//! Incremental strategy per folder (RFC 3501 + RFC 7162 CONDSTORE):
//! - UIDVALIDITY changed: the cached folder is dropped and resynced.
//! - New mail: only `UID SEARCH UID <last UIDNEXT>:*` is asked for, not the
//!   whole UID list. Unchanged UIDNEXT and EXISTS skip the search entirely.
//! - Expunges: if EXISTS equals the last EXISTS plus the new UIDs, nothing
//!   was expunged; otherwise one `UID SEARCH ALL` finds the vanished UIDs.
//! - Flags: with CONDSTORE, `FETCH (FLAGS) (CHANGEDSINCE <modseq>)` and
//!   nothing at all when HIGHESTMODSEQ is unchanged; without CONDSTORE all
//!   flags are refetched (the old behaviour).
//! - First sync, a changed sync window or a cache from before v0.4: one
//!   full pass (UID SEARCH ALL/SINCE, diff with the cache).
//!
//! New messages are downloaded newest first in pages, each page inserted
//! in one transaction, with no upper limit.

use std::collections::HashSet;

use crate::imap::{self, Session};

use crate::model::{AccountConfig, SyncReport};
use crate::store::{FolderSyncState, NewMessage, Store};
use crate::Result;

/// Messages downloaded and committed per page.
pub const PAGE_SIZE: usize = 500;

/// How a folder was synced (for tests and logging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// No usable previous state: full UID diff.
    Full,
    /// Incremental from the stored UIDNEXT/EXISTS.
    Incremental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderSyncStats {
    pub mode: SyncMode,
    pub new: usize,
    pub removed: usize,
    /// Number of flag updates the server sent.
    pub flag_updates: usize,
    /// Whether a full `UID SEARCH ALL` was needed (first sync or expunge).
    pub full_uid_search: bool,
    pub condstore: bool,
}

/// Sync every selectable folder. A failure in one folder (e.g. it was
/// deleted by another client mid-sync, which makes some servers drop the
/// connection) is recorded and the sync continues on a fresh connection.
pub async fn sync_account(
    store: &Store,
    account_id: i64,
    cfg: &AccountConfig,
    password: &str,
) -> Result<SyncReport> {
    let mut session = imap::connect(cfg, password).await?;
    let mut condstore = imap::has_condstore(&mut session).await?;
    let remote = imap::list_folders(&mut session).await?;
    let names: Vec<String> = remote
        .iter()
        .filter(|f| f.selectable)
        .map(|f| f.name.clone())
        .collect();
    store.retain_folders(account_id, &names)?;
    let mut report = SyncReport::default();
    let days = store.account_settings(account_id)?.sync_days;

    for f in remote.iter().filter(|f| f.selectable) {
        let folder_id = store.upsert_folder(account_id, &f.name, f.role)?;
        match sync_folder_detailed(store, folder_id, &f.name, &mut session, days, condstore).await {
            Ok(st) => {
                report.folders += 1;
                report.new_messages += st.new;
                report.removed_messages += st.removed;
            }
            Err(e) => {
                tracing::warn!(folder = %f.name, "folder sync failed: {e}");
                report.failed_folders.push(f.name.clone());
                let _ = session.logout().await;
                session = imap::connect(cfg, password).await?;
                condstore = imap::has_condstore(&mut session).await?;
            }
        }
    }
    let _ = session.logout().await;
    Ok(report)
}

pub async fn sync_folder(
    store: &Store,
    folder_id: i64,
    name: &str,
    session: &mut Session,
) -> Result<(usize, usize)> {
    let condstore = imap::has_condstore(session).await?;
    let st = sync_folder_detailed(store, folder_id, name, session, 0, condstore).await?;
    Ok((st.new, st.removed))
}

/// [`sync_folder_detailed`] with CONDSTORE autodetected, as (new, removed).
pub async fn sync_folder_window(
    store: &Store,
    folder_id: i64,
    name: &str,
    session: &mut Session,
    days: u32,
) -> Result<(usize, usize)> {
    let condstore = imap::has_condstore(session).await?;
    let st = sync_folder_detailed(store, folder_id, name, session, days, condstore).await?;
    Ok((st.new, st.removed))
}

/// Sync one folder; with `days > 0` only mail from the last `days` days is
/// downloaded and older cached mail is dropped from the cache (the server
/// is not touched).
pub async fn sync_folder_detailed(
    store: &Store,
    folder_id: i64,
    name: &str,
    session: &mut Session,
    days: u32,
    condstore: bool,
) -> Result<FolderSyncStats> {
    let since = (days > 0).then(|| chrono::Utc::now().timestamp() - i64::from(days) * 86_400);
    let state = imap::select_state(session, name, condstore).await?;
    let reset = store.set_uidvalidity(folder_id, state.uidvalidity)?;
    let prev = if reset {
        None
    } else {
        store
            .folder_sync_state(folder_id)?
            .filter(|p| p.window_days == days && state.uidnext.is_some())
    };
    let mut stats = FolderSyncStats {
        mode: if prev.is_some() {
            SyncMode::Incremental
        } else {
            SyncMode::Full
        },
        new: 0,
        removed: 0,
        flag_updates: 0,
        full_uid_search: false,
        condstore: state.highestmodseq.is_some(),
    };
    if let Some(t) = since {
        // A day of slack: SEARCH SINCE uses the server's date (no time zone).
        stats.removed += store.prune_before(folder_id, t - 86_400)?;
    }
    let local: HashSet<u32> = store.uids(folder_id)?.into_iter().collect();

    let (gone, to_fetch, flags_since, max_uid): (Vec<u32>, Vec<u32>, Option<Option<u64>>, u32) =
        match prev {
            Some(p) => {
                let unchanged = state.uidnext == Some(p.uidnext) && state.exists == p.exists;
                let new_all = if unchanged || state.exists == 0 {
                    HashSet::new()
                } else {
                    imap::uids_from(session, p.uidnext, None).await?
                };
                let gone: Vec<u32> = if state.exists as usize == p.exists as usize + new_all.len() {
                    Vec::new()
                } else {
                    stats.full_uid_search = true;
                    let all = imap::all_uids(session, state.exists).await?;
                    local.difference(&all).copied().collect()
                };
                let wanted = match since {
                    Some(t) if !new_all.is_empty() => {
                        imap::uids_from(session, p.uidnext, Some(t)).await?
                    }
                    _ => new_all,
                };
                let to_fetch = wanted.difference(&local).copied().collect();
                // Some(None) = full flag refresh, Some(Some(m)) = CHANGEDSINCE m, None = skip.
                let flags = match (p.highestmodseq, state.highestmodseq) {
                    (Some(old), Some(now)) if old == now => None,
                    (Some(old), Some(_)) => Some(Some(old)),
                    _ => Some(None),
                };
                (gone, to_fetch, flags, 0)
            }
            None => {
                stats.full_uid_search = true;
                let all = imap::all_uids(session, state.exists).await?;
                let gone = local.difference(&all).copied().collect();
                let wanted = match since {
                    Some(t) if !all.is_empty() => imap::uids_since(session, t).await?,
                    _ => all.clone(),
                };
                let to_fetch = wanted.difference(&local).copied().collect();
                (
                    gone,
                    to_fetch,
                    Some(None),
                    all.iter().copied().max().unwrap_or(0),
                )
            }
        };

    stats.removed += store.remove_uids(folder_id, &gone)?;

    if let Some(changed_since) = flags_since {
        if state.exists > 0 && !local.is_empty() {
            let flags = imap::fetch_flags(session, changed_since).await?;
            stats.flag_updates = flags.len();
            store.set_flags_bulk(folder_id, &flags)?;
        }
    }

    let mut to_fetch = to_fetch;
    to_fetch.sort_unstable_by(|a, b| b.cmp(a));
    for page in to_fetch.chunks(PAGE_SIZE) {
        let fetched = imap::fetch_full(session, page).await?;
        let batch: Vec<NewMessage<'_>> = fetched
            .iter()
            .map(|m| NewMessage {
                uid: m.uid,
                seen: m.seen,
                flagged: m.flagged,
                raw: &m.raw,
            })
            .collect();
        store.insert_messages(folder_id, &batch)?;
        stats.new += batch.len();
    }

    // Only record state after everything above succeeded, so an interrupted
    // sync is simply repeated from the old state next time.
    let uidnext = state.uidnext.unwrap_or(max_uid.saturating_add(1));
    store.set_folder_sync_state(
        folder_id,
        state.uidnext.map(|_| FolderSyncState {
            uidnext,
            highestmodseq: state.highestmodseq,
            exists: state.exists,
            window_days: days,
        }),
    )?;
    Ok(stats)
}
