//! Running rules against cached messages over IMAP (and SMTP for
//! forwards), keeping the cache consistent.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::imap::{self, Session};
use crate::model::{AccountConfig, Folder};
use crate::rules::{self, MessageFacts, Rule};
use crate::store::Store;
use crate::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RunReport {
    pub examined: usize,
    pub matched: usize,
    pub moved: usize,
    pub copied: usize,
    pub flagged: usize,
    pub forwarded: usize,
    /// Wire names of folders that received mail (resync them).
    pub touched: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

/// Map a folder named in a rule (display name, or wire name) to the
/// account's wire name.
pub fn resolve_folder<'a>(folders: &'a [Folder], name: &str) -> Option<&'a Folder> {
    folders
        .iter()
        .find(|f| f.display == name)
        .or_else(|| folders.iter().find(|f| f.name == name))
        .or_else(|| {
            folders
                .iter()
                .find(|f| f.display.eq_ignore_ascii_case(name))
        })
}

/// Rules that run locally for this account (server-side ones are left to
/// the server when our Sieve script is active there).
pub fn local_rules(store: &Store, account_id: i64) -> Result<Vec<Rule>> {
    let sieve_active = store.sieve_settings(account_id)?.active;
    Ok(store
        .rules()?
        .into_iter()
        .filter(|r| r.enabled && !(sieve_active && r.server_side))
        .collect())
}

/// Apply `rules` to cached messages `ids` of folder `folder_id`.
/// `automatic` runs (new mail) skip messages rules have already seen.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    store: &Store,
    session: &mut Session,
    cfg: &AccountConfig,
    password: &str,
    account_id: i64,
    folder_id: i64,
    ids: &[i64],
    rules: &[Rule],
    automatic: bool,
) -> Result<RunReport> {
    let mut rep = RunReport::default();
    if rules.is_empty() || ids.is_empty() {
        return Ok(rep);
    }
    let folders = store.folders(account_id)?;
    let source = folders
        .iter()
        .find(|f| f.id == folder_id)
        .cloned()
        .ok_or_else(|| Error::NotFound(format!("folder {folder_id}")))?;
    let trash = folders
        .iter()
        .find(|f| f.role.as_deref() == Some("trash"))
        .cloned();
    let refs = store.message_refs(ids)?;
    let mut touched = BTreeSet::new();
    for (acc, fid, uid, mid) in refs {
        if acc != account_id || fid != folder_id {
            continue;
        }
        rep.examined += 1;
        let key = if mid.is_empty() {
            format!(
                "uid:{fid}:{}:{uid}",
                store.folder_uidvalidity(fid)?.unwrap_or(0)
            )
        } else {
            format!("mid:{mid}")
        };
        if automatic && !store.claim_rule_run(account_id, &key)? {
            rep.skipped
                .push(format!("UID {uid}: rules already ran on this message"));
            continue;
        }
        let Some(id) = store.ids_for_uids(fid, &[uid])?.first().copied() else {
            continue;
        };
        let raw = store.raw(id)?;
        let facts = MessageFacts::from_raw(&raw, &cfg.email);
        let plan = match rules::plan(rules, &facts, &source.display) {
            Ok(p) => p,
            Err(e) => {
                rep.errors.push(e.to_string());
                continue;
            }
        };
        if plan.is_empty() {
            continue;
        }
        rep.matched += 1;
        rep.skipped.extend(plan.skipped.iter().cloned());
        if let Err(e) = apply(
            store,
            session,
            cfg,
            password,
            account_id,
            &source,
            trash.as_ref(),
            &folders,
            uid,
            &raw,
            &facts,
            &plan,
            &mut rep,
            &mut touched,
        )
        .await
        {
            tracing::warn!(uid, "rule action failed: {e}");
            rep.errors.push(format!("UID {uid}: {e}"));
        }
    }
    rep.touched = touched.into_iter().collect();
    Ok(rep)
}

#[allow(clippy::too_many_arguments)]
async fn apply(
    store: &Store,
    session: &mut Session,
    cfg: &AccountConfig,
    password: &str,
    account_id: i64,
    source: &Folder,
    trash: Option<&Folder>,
    folders: &[Folder],
    uid: u32,
    raw: &[u8],
    facts: &MessageFacts,
    plan: &rules::Plan,
    rep: &mut RunReport,
    touched: &mut BTreeSet<String>,
) -> Result<()> {
    let mut flags = Vec::new();
    if plan.mark_read {
        flags.push("\\Seen".to_string());
    }
    if plan.flag {
        flags.push("\\Flagged".to_string());
    }
    flags.extend(
        plan.keywords
            .iter()
            .filter(|k| imap::valid_keyword(k))
            .cloned(),
    );
    if !flags.is_empty() {
        session_select(session, &source.name).await?;
        imap::store_uids(
            session,
            &[uid],
            &format!("+FLAGS.SILENT ({})", flags.join(" ")),
        )
        .await?;
        if plan.mark_read {
            store.set_seen_uid_local(source.id, uid)?;
        }
        if plan.flag {
            store.set_flagged_local(source.id, uid)?;
        }
        rep.flagged += 1;
    }
    for c in &plan.copies {
        let Some(t) = resolve_folder(folders, c) else {
            rep.errors.push(format!("copy: no folder \"{c}\""));
            continue;
        };
        imap::copy_uids(session, &source.name, &[uid], &t.name).await?;
        touched.insert(t.name.clone());
        rep.copied += 1;
    }
    for to in &plan.forwards {
        if !store.take_forward_budget(account_id, rules::MAX_FORWARDS_PER_HOUR)? {
            rep.skipped.push(format!(
                "forward to {to}: more than {} forwards in the last hour (storm guard)",
                rules::MAX_FORWARDS_PER_HOUR
            ));
            continue;
        }
        let built = crate::smtp::build_auto_forward(cfg, to, &facts.subject, raw)?;
        crate::smtp::send_built(cfg, password, &built).await?;
        tracing::info!("rule forwarded a message");
        rep.forwarded += 1;
    }
    let dest = match (&plan.move_to, plan.delete) {
        (Some(m), _) => Some(
            resolve_folder(folders, m)
                .ok_or_else(|| Error::Invalid(format!("move: no folder \"{m}\"")))?
                .clone(),
        ),
        (None, true) => trash.cloned(),
        _ => None,
    };
    if let Some(d) = dest.filter(|d| d.id != source.id) {
        imap::move_uids(session, &source.name, &[uid], &d.name).await?;
        store.remove_uids(source.id, &[uid])?;
        touched.insert(d.name.clone());
        rep.moved += 1;
    }
    Ok(())
}

async fn session_select(session: &mut Session, name: &str) -> Result<()> {
    tokio::time::timeout(imap::COMMAND_TIMEOUT, session.select(name))
        .await
        .map_err(|_| Error::Timeout)??;
    Ok(())
}
