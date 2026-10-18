//! The ManageSieve script Neutron uploaded for an account, identified by a
//! hash of its bytes. A local edit (rules or the server-side automatic
//! reply) changes the hash, and the next sync uploads the new script.
//! Caches from before this column existed have no hash; that is drift too.

use sha2::{Digest, Sha256};

use crate::managesieve::{self, Client};
use crate::model::AccountConfig;
use crate::rules::SieveExport;
use crate::store::{SieveSettings, Store};
use crate::Result;

/// What [`reconcile`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SieveReconcile {
    /// Server filtering is off.
    Inactive,
    /// The active server script already matches the local rules.
    Current,
    /// The local script was uploaded and activated.
    Uploaded,
}

/// SHA-256 hex of the script, ignoring a trailing newline the server may add.
pub fn script_hash(script: &str) -> String {
    let digest = Sha256::digest(script.trim_end().as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
        out.push(char::from(b"0123456789abcdef"[(byte & 0xf) as usize]));
    }
    out
}

/// The script this account should have on the server right now.
pub fn desired_script(store: &Store, account_id: i64) -> Result<SieveExport> {
    let account = store.account(account_id)?;
    let trash = store
        .folder_by_role(account_id, "trash")?
        .map(|f| f.display)
        .unwrap_or_else(|| "Trash".into());
    let reply = store.auto_reply(account_id)?;
    let vacation = (reply.enabled && reply.on_server).then(|| {
        let own: Vec<String> = std::iter::once(account.config.email.clone())
            .chain(
                store
                    .account_settings(account_id)
                    .map(|s| s.identities)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|i| i.email),
            )
            .collect();
        crate::autoreply::sieve_block(&reply, &own)
    });
    Ok(crate::rules::to_sieve_with(
        &store.rules()?,
        &account.config.email,
        &trash,
        vacation,
    ))
}

/// `active` with a missing or different hash means the server may still be
/// running an older script.
pub fn is_stale(settings: &SieveSettings, script: &str) -> bool {
    settings.active && settings.script_hash.as_deref() != Some(script_hash(script).as_str())
}

/// Upload `desired_script` and record its hash. Always writes, including
/// when the server already has the same bytes (the explicit Upload button).
pub async fn upload(
    store: &Store,
    cfg: &AccountConfig,
    password: &str,
    account_id: i64,
    port: u16,
) -> Result<SieveExport> {
    let export = desired_script(store, account_id)?;
    let hash = script_hash(&export.script);
    let mut ms = Client::connect(cfg, port, password).await?;
    let outcome = async {
        ms.check_script(&export.script).await?;
        ms.put_script(managesieve::SCRIPT_NAME, &export.script)
            .await?;
        ms.set_active(managesieve::SCRIPT_NAME).await?;
        store.set_sieve_settings(
            account_id,
            SieveSettings {
                port,
                active: true,
                script_hash: Some(hash),
            },
        )?;
        Ok(export)
    }
    .await;
    ms.logout().await;
    outcome
}

/// Deactivate and delete our script, and forget the uploaded hash.
pub async fn remove(
    store: &Store,
    cfg: &AccountConfig,
    password: &str,
    account_id: i64,
) -> Result<()> {
    let settings = store.sieve_settings(account_id)?;
    let mut ms = Client::connect(cfg, settings.port, password).await?;
    let outcome = async {
        let ours = ms
            .list_scripts()
            .await?
            .into_iter()
            .find(|(name, _)| name == managesieve::SCRIPT_NAME);
        if let Some((_, active)) = ours {
            if active {
                ms.set_active("").await?;
            }
            ms.delete_script(managesieve::SCRIPT_NAME).await?;
        }
        store.set_sieve_settings(
            account_id,
            SieveSettings {
                port: settings.port,
                active: false,
                script_hash: None,
            },
        )?;
        Ok(())
    }
    .await;
    ms.logout().await;
    outcome
}

/// Upload the local script when the stored hash or the server copy disagrees.
/// A matching local hash with the server unreachable is left as-is so a
/// network blip does not mark a current script stale.
pub async fn reconcile(
    store: &Store,
    cfg: &AccountConfig,
    password: &str,
    account_id: i64,
) -> Result<SieveReconcile> {
    let settings = store.sieve_settings(account_id)?;
    if !settings.active {
        return Ok(SieveReconcile::Inactive);
    }
    let export = desired_script(store, account_id)?;
    let hash = script_hash(&export.script);
    let mut ms = match Client::connect(cfg, settings.port, password).await {
        Ok(ms) => ms,
        Err(e) if settings.script_hash.as_deref() == Some(hash.as_str()) => {
            tracing::warn!(account = account_id, "sieve verify skipped: {e}");
            return Ok(SieveReconcile::Current);
        }
        Err(e) => return Err(e),
    };
    let outcome = async {
        let remote = remote_script_hash(&mut ms).await?;
        if remote.as_deref() == Some(hash.as_str()) {
            if settings.script_hash.as_deref() != Some(hash.as_str()) {
                store.set_sieve_settings(
                    account_id,
                    SieveSettings {
                        port: settings.port,
                        active: true,
                        script_hash: Some(hash),
                    },
                )?;
            }
            return Ok(SieveReconcile::Current);
        }
        ms.check_script(&export.script).await?;
        ms.put_script(managesieve::SCRIPT_NAME, &export.script)
            .await?;
        ms.set_active(managesieve::SCRIPT_NAME).await?;
        store.set_sieve_settings(
            account_id,
            SieveSettings {
                port: settings.port,
                active: true,
                script_hash: Some(hash),
            },
        )?;
        tracing::info!(account = account_id, "sieve script re-uploaded");
        Ok(SieveReconcile::Uploaded)
    }
    .await;
    ms.logout().await;
    outcome
}

async fn remote_script_hash(ms: &mut Client) -> Result<Option<String>> {
    let list = ms.list_scripts().await?;
    let active = list
        .iter()
        .any(|(name, on)| name == managesieve::SCRIPT_NAME && *on);
    if !active {
        return Ok(None);
    }
    let body = ms.get_script(managesieve::SCRIPT_NAME).await?;
    Ok(Some(script_hash(&body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_ignores_a_trailing_newline_and_changes_with_content() {
        assert_eq!(script_hash("keep;\n"), script_hash("keep;"));
        assert_ne!(script_hash("keep;"), script_hash("keep;\nfileinto \"A\";"));
        assert_eq!(script_hash("keep;").len(), 64);
    }

    #[test]
    fn missing_hash_on_an_active_script_is_stale() {
        assert!(!is_stale(
            &SieveSettings {
                port: 4190,
                active: false,
                script_hash: None,
            },
            "keep;"
        ));
        assert!(is_stale(
            &SieveSettings {
                port: 4190,
                active: true,
                script_hash: None,
            },
            "keep;"
        ));
        assert!(!is_stale(
            &SieveSettings {
                port: 4190,
                active: true,
                script_hash: Some(script_hash("keep;")),
            },
            "keep;"
        ));
        assert!(is_stale(
            &SieveSettings {
                port: 4190,
                active: true,
                script_hash: Some("outdated".into()),
            },
            "keep;"
        ));
    }
}
