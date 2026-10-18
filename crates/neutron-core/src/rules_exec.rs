//! Running rules against cached messages over IMAP (and SMTP for
//! forwards), keeping the cache consistent.
//!
//! Automatic runs record each action as it succeeds. The sync checkpoint
//! has already moved past the new mail, so a failed move, copy or forward
//! is retried from `rule_runs` on a later sync. Actions that already
//! succeeded are not repeated.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::imap::{self, Session};
use crate::model::{AccountConfig, Folder};
use crate::rules::{self, MessageFacts, Plan, Rule};
use crate::store::{self, RuleAdmission, Store};
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

/// Stable ids for the effects of a plan. A retry skips ids already stored.
pub fn action_ids(plan: &Plan) -> Vec<String> {
    let mut keys = Vec::new();
    if plan.mark_read {
        keys.push("seen".into());
    }
    if plan.flag {
        keys.push("flagged".into());
    }
    for keyword in &plan.keywords {
        keys.push(format!("keyword:{keyword}"));
    }
    for folder in &plan.copies {
        keys.push(format!("copy:{folder}"));
    }
    for to in &plan.forwards {
        keys.push(format!("forward:{to}"));
    }
    if let Some(folder) = &plan.move_to {
        keys.push(format!("move:{folder}"));
    } else if plan.delete {
        keys.push("delete".into());
    }
    keys
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

/// Rules that run locally for this account. Only rules expressible in
/// Sieve are left to the server when our script is active there; rules
/// rejected by the exporter must retain their local fallback.
pub fn local_rules(store: &Store, account_id: i64) -> Result<Vec<Rule>> {
    let sieve_active = store.sieve_settings(account_id)?.active;
    let rules = store.rules()?;
    let exported: BTreeSet<i64> = if sieve_active {
        let account = store.account(account_id)?;
        let trash = store
            .folder_by_role(account_id, "trash")?
            .map(|f| f.display)
            .unwrap_or_else(|| "Trash".into());
        rules::to_sieve(&rules, &account.config.email, &trash)
            .exported
            .into_iter()
            .collect()
    } else {
        BTreeSet::new()
    };
    Ok(rules
        .into_iter()
        .filter(|r| r.enabled && !exported.contains(&r.id))
        .collect())
}

/// Apply `rules` to cached messages `ids` of folder `folder_id`.
/// `automatic` runs (new mail, and due retries) skip messages whose actions
/// already finished, and record partial progress when an action fails.
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
    if automatic {
        settle_missing(store, account_id, folder_id)?;
    }
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
    let now = chrono::Utc::now().timestamp();
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
        let done = if automatic {
            match store.admit_rule_run(account_id, &key, folder_id, uid, now)? {
                RuleAdmission::Finished => {
                    rep.skipped
                        .push(format!("UID {uid}: rules already ran on this message"));
                    continue;
                }
                RuleAdmission::Waiting { .. } => {
                    rep.skipped
                        .push(format!("UID {uid}: rule actions are waiting to retry"));
                    continue;
                }
                RuleAdmission::Proceed { done } => done,
            }
        } else {
            Vec::new()
        };
        let Some(id) = store.ids_for_uids(fid, &[uid])?.first().copied() else {
            continue;
        };
        let raw = store.raw(id)?;
        let facts = MessageFacts::from_raw(&raw, &cfg.email);
        let plan = match rules::plan(rules, &facts, &source.display) {
            Ok(p) => p,
            Err(e) => {
                if automatic {
                    rep.errors
                        .push(note_failure(store, account_id, &key, uid, &e.to_string())?);
                } else {
                    rep.errors.push(e.to_string());
                }
                continue;
            }
        };
        if plan.is_empty() {
            if automatic {
                store.complete_rule_run(account_id, &key)?;
            }
            continue;
        }
        rep.matched += 1;
        rep.skipped.extend(plan.skipped.iter().cloned());
        let done: BTreeSet<String> = done.into_iter().collect();
        let track = automatic.then_some(key.as_str());
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
            &done,
            track,
            &mut rep,
            &mut touched,
        )
        .await
        {
            tracing::warn!(uid, "rule action failed: {e}");
            if automatic {
                rep.errors
                    .push(note_failure(store, account_id, &key, uid, &e.to_string())?);
            } else {
                rep.errors.push(format!("UID {uid}: {e}"));
            }
        } else if automatic {
            store.complete_rule_run(account_id, &key)?;
        }
    }
    rep.touched = touched.into_iter().collect();
    Ok(rep)
}

/// Due retries whose message is gone cannot be continued. Mark them failed
/// so they are reported once instead of being selected forever.
fn settle_missing(store: &Store, account_id: i64, folder_id: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let cached: BTreeSet<u32> = store.uids(folder_id)?.into_iter().collect();
    for uid in store.due_rule_uids(account_id, folder_id, now)? {
        if cached.contains(&uid) {
            continue;
        }
        store.fail_pending_uid(
            account_id,
            folder_id,
            uid,
            &format!(
                "UID {uid}: the message is no longer in this folder, so unfinished rule actions were left incomplete"
            ),
        )?;
    }
    Ok(())
}

/// Record one failure and return the sentence shown in the sync report.
/// The same sentence is what a later sync reports if this process stops
/// before the caller can show it.
fn note_failure(
    store: &Store,
    account_id: i64,
    message_key: &str,
    uid: u32,
    error: &str,
) -> Result<String> {
    let now = chrono::Utc::now().timestamp();
    let prev = store
        .rule_run(account_id, message_key)?
        .map(|r| r.attempts)
        .unwrap_or(0);
    let attempts = prev + 1;
    let message = if attempts >= store::MAX_RULE_ATTEMPTS {
        format!("UID {uid}: {error} (giving up after {attempts} tries)")
    } else {
        let mins = (store::rule_backoff_secs(attempts) + 59) / 60;
        format!(
            "UID {uid}: {error} (retry {attempts}/{} in {mins} min)",
            store::MAX_RULE_ATTEMPTS
        )
    };
    store.fail_rule_run(account_id, message_key, &message, now)?;
    Ok(message)
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
    done: &BTreeSet<String>,
    track: Option<&str>,
    rep: &mut RunReport,
    touched: &mut BTreeSet<String>,
) -> Result<()> {
    let mut problems = Vec::new();
    let remember = |key: &str| -> Result<()> {
        if let Some(message_key) = track {
            store.record_rule_action(account_id, message_key, key)?;
        }
        Ok(())
    };

    let mut flags = Vec::new();
    let mut flag_keys: Vec<String> = Vec::new();
    if plan.mark_read && !done.contains("seen") {
        flags.push("\\Seen".to_string());
        flag_keys.push("seen".into());
    }
    if plan.flag && !done.contains("flagged") {
        flags.push("\\Flagged".to_string());
        flag_keys.push("flagged".into());
    }
    for keyword in plan.keywords.iter().filter(|k| imap::valid_keyword(k)) {
        let key = format!("keyword:{keyword}");
        if !done.contains(&key) {
            flags.push(keyword.clone());
            flag_keys.push(key);
        }
    }
    if !flags.is_empty() {
        session_select(session, &source.name).await?;
        imap::store_uids(
            session,
            &[uid],
            &format!("+FLAGS.SILENT ({})", flags.join(" ")),
        )
        .await?;
        if flag_keys.iter().any(|k| k == "seen") {
            store.set_seen_uid_local(source.id, uid)?;
        }
        if flag_keys.iter().any(|k| k == "flagged") {
            store.set_flagged_local(source.id, uid)?;
        }
        for key in &flag_keys {
            remember(key)?;
        }
        rep.flagged += 1;
    }

    for folder_name in &plan.copies {
        let key = format!("copy:{folder_name}");
        if done.contains(&key) {
            continue;
        }
        let Some(target) = resolve_folder(folders, folder_name) else {
            problems.push(format!("copy: no folder \"{folder_name}\""));
            continue;
        };
        if let Err(e) = imap::copy_uids(session, &source.name, &[uid], &target.name).await {
            problems.push(format!("copy to {folder_name}: {e}"));
            continue;
        }
        remember(&key)?;
        touched.insert(target.name.clone());
        rep.copied += 1;
    }
    for to in &plan.forwards {
        let key = format!("forward:{to}");
        if done.contains(&key) {
            continue;
        }
        if !store.take_forward_budget(account_id, rules::MAX_FORWARDS_PER_HOUR)? {
            // The storm guard is a decision, not a failed send. Remember it
            // so the next retry does not send the forward anyway.
            remember(&key)?;
            rep.skipped.push(format!(
                "forward to {to}: more than {} forwards in the last hour (storm guard)",
                rules::MAX_FORWARDS_PER_HOUR
            ));
            continue;
        }
        let built = crate::smtp::build_auto_forward(cfg, to, &facts.subject, raw)?;
        if let Err(e) = crate::smtp::send_built(cfg, password, &built).await {
            problems.push(format!("forward to {to}: {e}"));
            continue;
        }
        remember(&key)?;
        tracing::info!("rule forwarded a message");
        rep.forwarded += 1;
    }

    // Filing waits until every copy and forward has succeeded. Moving first
    // would drop the message from the inbox and make the failed action
    // impossible to retry.
    if problems.is_empty() {
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
            let key = match &plan.move_to {
                Some(folder) => format!("move:{folder}"),
                None => "delete".to_string(),
            };
            if !done.contains(&key) {
                if let Err(e) = imap::move_uids(session, &source.name, &[uid], &d.name).await {
                    problems.push(format!("move to {}: {e}", d.display));
                } else {
                    store.remove_uids(source.id, &[uid])?;
                    remember(&key)?;
                    touched.insert(d.name);
                    rep.moved += 1;
                }
            }
        }
    }
    if !problems.is_empty() {
        return Err(Error::Invalid(problems.join("; ")));
    }
    Ok(())
}

async fn session_select(session: &mut Session, name: &str) -> Result<()> {
    tokio::time::timeout(imap::COMMAND_TIMEOUT, session.select(name))
        .await
        .map_err(|_| Error::Timeout)??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Action, Condition, Field, Op, Rule};

    fn plan_for(actions: Vec<Action>) -> Plan {
        let rule = Rule {
            id: 1,
            name: "t".into(),
            enabled: true,
            match_all: true,
            conditions: vec![Condition {
                field: Field::Subject,
                op: Op::Contains,
                value: "invoice".into(),
                header: String::new(),
            }],
            actions,
            server_side: false,
        };
        let facts = MessageFacts::from_raw(
            b"From: Elena <elena@example.org>\r\nTo: me@example.test\r\nSubject: invoice\r\n\r\nbody\r\n",
            "me@example.test",
        );
        rules::plan([&rule], &facts, "Inbox").unwrap()
    }

    #[test]
    fn action_ids_keep_forward_and_copy_distinct() {
        let plan = plan_for(vec![
            Action::MarkRead,
            Action::Copy {
                folder: "Archive".into(),
            },
            Action::Forward {
                to: "accounting@example.test".into(),
            },
            Action::Move {
                folder: "Invoices".into(),
            },
        ]);
        assert_eq!(
            action_ids(&plan),
            vec![
                "seen",
                "copy:Archive",
                "forward:accounting@example.test",
                "move:Invoices",
            ]
        );
    }
}
