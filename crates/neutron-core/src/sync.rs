//! Account synchronization: folders, new messages, flag changes, expunges.
//!
//! Strategy (prototype): per folder, compare the server's UID set with the
//! cache, fetch new UIDs in full, drop vanished UIDs, refresh flags. This is
//! correct but not bandwidth-optimal; CONDSTORE/QRESYNC are future work.

use std::collections::HashSet;

use crate::imap::{self, Session};

use crate::model::{AccountConfig, SyncReport};
use crate::store::{NewMessage, Store};
use crate::Result;

/// Upper bound of messages downloaded per folder on a single sync, newest
/// first, so a first sync of a huge mailbox stays responsive.
pub const MAX_NEW_PER_FOLDER: usize = 500;

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
    let remote = imap::list_folders(&mut session).await?;
    let names: Vec<String> = remote
        .iter()
        .filter(|f| f.selectable)
        .map(|f| f.name.clone())
        .collect();
    store.retain_folders(account_id, &names)?;
    let mut report = SyncReport::default();
    let days = store.account_settings(account_id)?.sync_days;
    let since = (days > 0).then(|| chrono::Utc::now().timestamp() - i64::from(days) * 86_400);
    for f in remote.iter().filter(|f| f.selectable) {
        let folder_id = store.upsert_folder(account_id, &f.name, f.role)?;
        match sync_folder_window(store, folder_id, &f.name, &mut session, since).await {
            Ok((new, removed)) => {
                report.folders += 1;
                report.new_messages += new;
                report.removed_messages += removed;
            }
            Err(e) => {
                tracing::warn!(folder = %f.name, "folder sync failed: {e}");
                report.failed_folders.push(f.name.clone());
                let _ = session.logout().await;
                session = imap::connect(cfg, password).await?;
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
    sync_folder_window(store, folder_id, name, session, None).await
}

/// Sync one folder; with `since` (unix time) only mail from then on is
/// downloaded and older cached mail is dropped from the cache (the server
/// is not touched).
pub async fn sync_folder_window(
    store: &Store,
    folder_id: i64,
    name: &str,
    session: &mut Session,
    since: Option<i64>,
) -> Result<(usize, usize)> {
    let state = imap::select(session, name).await?;
    store.set_uidvalidity(folder_id, state.uidvalidity)?;
    let local: HashSet<u32> = store.uids(folder_id)?.into_iter().collect();

    let gone: Vec<u32> = local.difference(&state.uids).copied().collect();
    let mut removed = store.remove_uids(folder_id, &gone)?;

    let wanted: HashSet<u32> = match since {
        Some(t) if !state.uids.is_empty() => {
            // A day of slack: SEARCH SINCE uses the server's date (no time zone).
            removed += store.prune_before(folder_id, t - 86_400)?;
            imap::uids_since(session, t).await?
        }
        _ => state.uids.clone(),
    };
    let local: HashSet<u32> = store.uids(folder_id)?.into_iter().collect();
    let mut new: Vec<u32> = wanted.difference(&local).copied().collect();
    new.sort_unstable_by(|a, b| b.cmp(a));
    new.truncate(MAX_NEW_PER_FOLDER);
    let fetched = imap::fetch_full(session, &new).await?;
    for m in &fetched {
        store.insert_message(
            folder_id,
            &NewMessage {
                uid: m.uid,
                seen: m.seen,
                flagged: m.flagged,
                raw: &m.raw,
            },
        )?;
    }

    if !state.uids.is_empty() {
        for (uid, seen, flagged) in imap::fetch_flags(session).await? {
            store.set_flags(folder_id, uid, seen, flagged)?;
        }
    }
    Ok((fetched.len(), removed))
}
