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
//!   full pass (UID SEARCH ALL/SINCE, diff with the cache). Windowed folders
//!   also reconcile once per day using the server's INTERNALDATE, never the
//!   sender's Date header.
//!
//! New messages are downloaded newest first in pages, each page inserted
//! in one transaction, with no upper limit.

use std::collections::HashSet;

use crate::imap::{self, Session};

use crate::model::{AccountConfig, SyncReport};
use crate::store::{FolderSyncState, NewMessage, Store};
use crate::Result;

/// Messages downloaded and committed per page.
pub const PAGE_SIZE: usize = 50;

/// UI progress contains folder names/counts, never message contents.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncProgress {
    pub stage: &'static str,
    pub folder: Option<String>,
    pub completed_folders: usize,
    pub total_folders: usize,
    pub downloaded: usize,
    pub total_messages: usize,
}

/// How a folder was synced (for tests and logging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// No usable previous baseline: full UID diff.
    Full,
    /// Existing baseline; a daily window reconciliation may still be needed.
    Incremental,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSyncStats {
    /// New-arrival UIDs downloaded in this pass (for rules on new mail).
    pub new_uids: Vec<u32>,
    pub mode: SyncMode,
    pub new: usize,
    pub removed: usize,
    /// Number of flag updates the server sent.
    pub flag_updates: usize,
    /// Whether a full mailbox/window UID search was needed.
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
    sync_account_with_progress(store, account_id, cfg, password, |_| {}).await
}

pub async fn sync_account_with_progress(
    store: &Store,
    account_id: i64,
    cfg: &AccountConfig,
    password: &str,
    progress: impl Fn(SyncProgress),
) -> Result<SyncReport> {
    progress(SyncProgress {
        stage: "connecting",
        folder: None,
        completed_folders: 0,
        total_folders: 0,
        downloaded: 0,
        total_messages: 0,
    });
    let mut session = imap::connect(cfg, password).await?;
    let mut condstore = imap::has_condstore(&mut session).await?;
    progress(SyncProgress {
        stage: "listing",
        folder: None,
        completed_folders: 0,
        total_folders: 0,
        downloaded: 0,
        total_messages: 0,
    });
    let remote = imap::list_folders(&mut session).await?;
    let names: Vec<String> = remote
        .iter()
        .filter(|f| f.selectable)
        .map(|f| f.name.clone())
        .collect();
    // Publish the complete inventory before any folder's potentially large
    // download. Previously later folders were invisible until earlier ones finished.
    let mut folders = Vec::new();
    for f in remote.iter().filter(|f| f.selectable) {
        folders.push((f, store.upsert_remote_folder(account_id, f)?));
    }
    store.retain_folders(account_id, &names)?;
    let total_folders = folders.len();
    progress(SyncProgress {
        stage: "folders",
        folder: None,
        completed_folders: 0,
        total_folders,
        downloaded: 0,
        total_messages: 0,
    });
    let mut report = SyncReport::default();
    let days = store.account_settings(account_id)?.sync_days;

    let mut inbox_new: Option<(i64, Vec<u32>)> = None;
    for (completed_folders, (f, folder_id)) in folders.into_iter().enumerate() {
        let on_page = |downloaded, total_messages| {
            progress(SyncProgress {
                stage: "messages",
                folder: Some(f.display.clone()),
                completed_folders,
                total_folders,
                downloaded,
                total_messages,
            })
        };
        progress(SyncProgress {
            stage: "folder",
            folder: Some(f.display.clone()),
            completed_folders,
            total_folders,
            downloaded: 0,
            total_messages: 0,
        });
        // A broken/very large folder must not hold all the other folders
        // indefinitely. Committed pages survive; no checkpoint is advanced on timeout.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5 * 60),
            sync_folder_with_progress(
                store,
                folder_id,
                &f.name,
                &mut session,
                days,
                condstore,
                on_page,
            ),
        )
        .await
        .unwrap_or(Err(crate::Error::Timeout));
        match result {
            Ok(st) => {
                report.folders += 1;
                report.new_messages += st.new;
                report.removed_messages += st.removed;
                // Rules run on new INBOX mail, not on the first download
                // of an existing mailbox.
                if f.role == Some("inbox")
                    && st.mode == SyncMode::Incremental
                    && !st.new_uids.is_empty()
                {
                    inbox_new = Some((folder_id, st.new_uids.clone()));
                }
            }
            Err(e) => {
                tracing::warn!(folder = %f.name, "folder sync failed: {e}");
                report.failed_folders.push(f.name.clone());
                // A timed-out response leaves protocol framing uncertain. Drop
                // the socket rather than attempting LOGOUT on that session.
                drop(session);
                session = imap::connect(cfg, password).await?;
                condstore = imap::has_condstore(&mut session).await?;
            }
        }
    }
    // Automatic replies first, so rules that move mail can't hide it.
    if let Some((fid, uids)) = &inbox_new {
        if let Err(e) =
            crate::autoreply::process(store, cfg, password, account_id, *fid, uids).await
        {
            tracing::warn!(account = account_id, "automatic reply failed: {e}");
            report.rule_errors.push(format!("Automatic reply: {e}"));
        }
    }
    if let Some((fid, uids)) = inbox_new {
        match run_rules_on_new(store, &mut session, cfg, password, account_id, fid, &uids).await {
            Ok(rep) => {
                report.rules_matched = rep.matched;
                for name in &rep.touched {
                    if let Some(f) = store
                        .folders(account_id)?
                        .into_iter()
                        .find(|f| &f.name == name)
                    {
                        let _ =
                            sync_folder_detailed(store, f.id, name, &mut session, days, condstore)
                                .await;
                    }
                }
                report.rule_errors = rep.errors;
            }
            Err(e) => report.rule_errors.push(e.to_string()),
        }
    }
    let _ = imap::logout(&mut session).await;
    Ok(report)
}

async fn run_rules_on_new(
    store: &Store,
    session: &mut Session,
    cfg: &AccountConfig,
    password: &str,
    account_id: i64,
    folder_id: i64,
    uids: &[u32],
) -> Result<crate::rules_exec::RunReport> {
    let rules = crate::rules_exec::local_rules(store, account_id)?;
    if rules.is_empty() {
        return Ok(Default::default());
    }
    let ids = store.ids_for_uids(folder_id, uids)?;
    crate::rules_exec::run(
        store, session, cfg, password, account_id, folder_id, &ids, &rules, true,
    )
    .await
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
    sync_folder_with_progress(store, folder_id, name, session, days, condstore, |_, _| {}).await
}

#[allow(clippy::too_many_arguments)]
async fn sync_folder_with_progress(
    store: &Store,
    folder_id: i64,
    name: &str,
    session: &mut Session,
    days: u32,
    condstore: bool,
    progress: impl Fn(usize, usize),
) -> Result<FolderSyncStats> {
    // SEARCH SINCE uses calendar dates. Persist the same UTC day that
    // imap::uids_since formats, so the boundary advances only once per day.
    let since = (days > 0).then(|| {
        let cutoff = chrono::Utc::now().timestamp() - i64::from(days) * 86_400;
        cutoff.div_euclid(86_400) * 86_400
    });
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
        new_uids: Vec::new(),
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
    let local: HashSet<u32> = store.uids(folder_id)?.into_iter().collect();

    let (gone, to_fetch, flags_since, max_uid): (Vec<u32>, Vec<u32>, Option<Option<u64>>, u32) =
        match prev {
            Some(p) if p.window_cutoff == since => {
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
            _ => {
                stats.full_uid_search = true;
                let wanted = match since {
                    Some(t) if state.exists > 0 => imap::uids_since(session, t).await?,
                    _ => imap::all_uids(session, state.exists).await?,
                };
                // The server's INTERNALDATE decides which messages belong
                // in the cache. Date headers may be old, absent or forged.
                let gone = local.difference(&wanted).copied().collect();
                let to_fetch = wanted.difference(&local).copied().collect();
                (
                    gone,
                    to_fetch,
                    Some(None),
                    wanted.iter().copied().max().unwrap_or(0),
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
    progress(0, to_fetch.len());
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
        progress(stats.new, to_fetch.len());
        // A daily/legacy window reconciliation can recover older cached
        // mail. Only genuinely new UIDs should trigger automatic rules.
        stats.new_uids.extend(
            batch
                .iter()
                .map(|m| m.uid)
                .filter(|uid| prev.is_none_or(|p| *uid >= p.uidnext)),
        );
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
            window_cutoff: since,
        }),
    )?;
    Ok(stats)
}
