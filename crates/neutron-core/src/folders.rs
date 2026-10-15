//! Folder management and moving/copying mail, keeping the cache in step
//! with the server. Special-use folders (RFC 6154) and INBOX are protected.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::imap::{self, Session};
use crate::model::Folder;
use crate::store::Store;
use crate::{Error, Result};

pub fn is_protected(f: &Folder) -> bool {
    f.role.is_some() || f.name.eq_ignore_ascii_case("INBOX")
}

/// The delimiter for an account: the folder's own, else the most common.
pub fn delimiter_for(folders: &[Folder], f: Option<&Folder>) -> Option<String> {
    f.and_then(|f| f.delimiter.clone())
        .or_else(|| folders.iter().find_map(|f| f.delimiter.clone()))
}

fn leaf_check(leaf: &str, delim: Option<&str>) -> Result<String> {
    let leaf = leaf.trim();
    if leaf.is_empty() || delim.is_some_and(|d| !d.is_empty() && leaf.contains(d)) {
        return Err(Error::Invalid(match delim {
            Some(d) => format!("folder names can't be empty or contain \"{d}\""),
            None => "folder names can't be empty".into(),
        }));
    }
    imap::validate_folder_name(leaf, delim)?;
    Ok(leaf.to_string())
}

async fn utf8(session: &mut Session) -> Result<bool> {
    imap::has_capability(session, "UTF8=ACCEPT").await
}

/// Wire name for `leaf` under `parent` (None = top level).
pub async fn child_wire(
    session: &mut Session,
    folders: &[Folder],
    parent: Option<&Folder>,
    leaf: &str,
) -> Result<String> {
    let delim = delimiter_for(folders, parent);
    let leaf = leaf_check(leaf, delim.as_deref())?;
    let enc = imap::wire_name(&leaf, utf8(session).await?);
    Ok(match (parent, delim) {
        (Some(p), Some(d)) => format!("{}{d}{enc}", p.name),
        (Some(_), None) => return Err(Error::Invalid("this server has no subfolders".into())),
        (None, _) => enc,
    })
}

fn descendants<'a>(folders: &'a [Folder], f: &Folder) -> Vec<&'a Folder> {
    let Some(d) = f.delimiter.as_deref().filter(|d| !d.is_empty()) else {
        return Vec::new();
    };
    let prefix = format!("{}{d}", f.name);
    folders
        .iter()
        .filter(|x| x.name.starts_with(&prefix))
        .collect()
}

pub async fn create(
    store: &Store,
    session: &mut Session,
    account_id: i64,
    parent_id: Option<i64>,
    leaf: &str,
) -> Result<String> {
    let folders = store.folders(account_id)?;
    let parent = match parent_id {
        Some(id) => Some(
            folders
                .iter()
                .find(|f| f.id == id)
                .ok_or_else(|| Error::NotFound("parent folder".into()))?,
        ),
        None => None,
    };
    let wire = child_wire(session, &folders, parent, leaf).await?;
    if folders.iter().any(|f| f.name == wire) {
        return Err(Error::Invalid(
            "a folder with that name already exists".into(),
        ));
    }
    imap::create_folder(session, &wire).await?;
    // Cache it now (with the server's attributes) so the UI shows it at once.
    for rf in imap::list_folders(session).await? {
        if rf.name == wire {
            store.upsert_remote_folder(account_id, &rf)?;
        }
    }
    Ok(wire)
}

/// Rename (`new_leaf`) and/or move under `new_parent` (Some(None) = top level).
pub async fn rename(
    store: &Store,
    session: &mut Session,
    folder_id: i64,
    new_leaf: Option<&str>,
    new_parent: Option<Option<i64>>,
) -> Result<String> {
    let f = store.folder(folder_id)?;
    if is_protected(&f) {
        return Err(Error::Invalid(format!(
            "\"{}\" is a special folder and can't be renamed or moved",
            f.display
        )));
    }
    let folders = store.folders(f.account_id)?;
    let delim = delimiter_for(&folders, Some(&f));
    let cur_leaf = match delim.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => f.display.rsplit(d).next().unwrap_or(&f.display).to_string(),
        None => f.display.clone(),
    };
    let cur_parent_wire = delim
        .as_deref()
        .filter(|d| !d.is_empty())
        .and_then(|d| f.name.rsplit_once(d).map(|(p, _)| p.to_string()));
    let parent: Option<&Folder> = match new_parent {
        Some(Some(pid)) => {
            let p = folders
                .iter()
                .find(|x| x.id == pid)
                .ok_or_else(|| Error::NotFound("parent folder".into()))?;
            if p.id == f.id || descendants(&folders, &f).iter().any(|d| d.id == p.id) {
                return Err(Error::Invalid("a folder can't be moved into itself".into()));
            }
            Some(p)
        }
        Some(None) => None,
        None => cur_parent_wire
            .as_ref()
            .and_then(|w| folders.iter().find(|x| &x.name == w)),
    };
    let leaf = new_leaf.unwrap_or(&cur_leaf);
    let wire = match (parent, &new_parent, &cur_parent_wire) {
        // Parent unchanged but not in the cache (e.g. a \\Noselect parent).
        (None, None, Some(pw)) => {
            let d = delim.clone().unwrap_or_default();
            format!(
                "{pw}{d}{}",
                imap::wire_name(&leaf_check(leaf, delim.as_deref())?, utf8(session).await?)
            )
        }
        _ => child_wire(session, &folders, parent, leaf).await?,
    };
    if wire == f.name {
        return Ok(wire);
    }
    if folders.iter().any(|x| x.name == wire) {
        return Err(Error::Invalid(
            "a folder with that name already exists".into(),
        ));
    }
    imap::rename_folder(session, &f.name, &wire, delim.as_deref()).await?;
    store.rename_folder_local(f.account_id, &f.name, &wire, delim.as_deref())?;
    Ok(wire)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    /// Move the messages (of the folder and its subfolders) to Trash first.
    MoveToTrash,
    /// Delete the folders with their messages.
    DeleteMessages,
}

/// Delete a folder and its subfolders (deepest first).
pub async fn delete(
    store: &Store,
    session: &mut Session,
    folder_id: i64,
    mode: DeleteMode,
) -> Result<usize> {
    let f = store.folder(folder_id)?;
    if is_protected(&f) {
        return Err(Error::Invalid(format!(
            "\"{}\" is a special folder and can't be deleted",
            f.display
        )));
    }
    let folders = store.folders(f.account_id)?;
    let mut doomed: Vec<Folder> = descendants(&folders, &f).into_iter().cloned().collect();
    if doomed.iter().any(is_protected) {
        return Err(Error::Invalid(
            "a special folder is inside this folder".into(),
        ));
    }
    doomed.push(f.clone());
    doomed.sort_by_key(|x| std::cmp::Reverse(x.name.len()));
    let trash = folders
        .iter()
        .find(|x| x.role.as_deref() == Some("trash"))
        .cloned();
    let mut moved = 0;
    for d in &doomed {
        if mode == DeleteMode::MoveToTrash {
            let t = trash
                .as_ref()
                .ok_or_else(|| Error::Invalid("this account has no Trash folder".into()))?;
            let st = imap::select_state(session, &d.name, false).await?;
            let uids: Vec<u32> = imap::all_uids(session, st.exists)
                .await?
                .into_iter()
                .collect();
            imap::move_uids(session, &d.name, &uids, &t.name).await?;
            moved += uids.len();
        }
        // Deselect so the server lets us delete it.
        let _ = imap::select_state(session, "INBOX", false).await;
        imap::delete_folder(session, &d.name).await?;
        store.delete_folder_local(d.id)?;
    }
    Ok(moved)
}

/// What a move did, enough to undo it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub account_id: i64,
    pub target_folder_id: i64,
    pub copy: bool,
    /// (source folder id, Message-ID) per moved message.
    pub items: Vec<(i64, String)>,
    /// Messages without a Message-ID (can't be found again for undo).
    pub untracked: usize,
    pub count: usize,
}

/// Move or copy cached messages (from any folders of one account) to a folder.
pub async fn transfer(
    store: &Store,
    session: &mut Session,
    ids: &[i64],
    target_folder_id: i64,
    copy: bool,
) -> Result<Transfer> {
    let target = store.folder(target_folder_id)?;
    let refs = store.message_refs(ids)?;
    if refs.is_empty() {
        return Err(Error::NotFound("messages".into()));
    }
    if refs.iter().any(|r| r.0 != target.account_id) {
        return Err(Error::Invalid(
            "messages can only be moved within one account".into(),
        ));
    }
    let mut by_src: BTreeMap<i64, Vec<(u32, String)>> = BTreeMap::new();
    for (_, fid, uid, mid) in refs {
        if fid != target.id {
            by_src.entry(fid).or_default().push((uid, mid));
        }
    }
    let mut t = Transfer {
        account_id: target.account_id,
        target_folder_id: target.id,
        copy,
        items: Vec::new(),
        untracked: 0,
        count: 0,
    };
    for (fid, msgs) in by_src {
        let src = store.folder(fid)?;
        let uids: Vec<u32> = msgs.iter().map(|m| m.0).collect();
        if copy {
            imap::copy_uids(session, &src.name, &uids, &target.name).await?;
        } else {
            imap::move_uids(session, &src.name, &uids, &target.name).await?;
            store.remove_uids(src.id, &uids)?;
        }
        t.count += uids.len();
        for (_, mid) in msgs {
            if mid.is_empty() {
                t.untracked += 1;
            } else {
                t.items.push((fid, mid));
            }
        }
    }
    Ok(t)
}

/// Undo a move: find the messages in the target by Message-ID and move
/// them back (a copy is undone by deleting the copies). Returns how many.
pub async fn undo(store: &Store, session: &mut Session, t: &Transfer) -> Result<usize> {
    let target = store.folder(t.target_folder_id)?;
    let mut by_src: BTreeMap<i64, Vec<&str>> = BTreeMap::new();
    for (fid, mid) in &t.items {
        by_src.entry(*fid).or_default().push(mid);
    }
    let mut n = 0;
    for (fid, mids) in by_src {
        let src = store.folder(fid)?;
        let mut uids = Vec::new();
        for m in mids {
            // The newest copy with that Message-ID is the one we moved.
            if let Some(u) = imap::uids_by_message_id(session, &target.name, m)
                .await?
                .last()
            {
                uids.push(*u);
            }
        }
        if t.copy {
            for u in &uids {
                imap::delete_uid(session, &target.name, *u).await?;
            }
        } else {
            imap::move_uids(session, &target.name, &uids, &src.name).await?;
        }
        store.remove_uids(target.id, &uids)?;
        n += uids.len();
    }
    Ok(n)
}
