//! Automatic replies ("out of office"): settings, and running them on the
//! server through the Sieve script when the server supports it. Replying
//! from this computer happens during sync (`neutron_core::autoreply`).

use std::time::Duration;

use neutron_core::{autoreply::AutoReply, managesieve, Error};
use serde::Serialize;
use tauri::State;

use crate::commands::Res;
use crate::organize::{remove_script, upload_script};
use crate::AppState;

#[derive(Serialize)]
pub struct ServerSupport {
    /// The server can send the reply itself (ManageSieve + vacation).
    available: bool,
    /// Start/end dates also work on the server (date + relational).
    dates: bool,
    /// Why not, in plain words (empty when available).
    detail: String,
    /// The provider's own auto-reply setting, when known (e.g. 263 webmail).
    provider_hint: Option<String>,
}

fn provider_hint(imap_host: &str) -> Option<String> {
    let h = imap_host.to_ascii_lowercase();
    if h.ends_with("263.net") || h.ends_with("263.com") {
        Some("263 has its own automatic reply in webmail (Settings → 自动回复 / Automatic reply). Turn it on there too if replies should also go out while this computer is off; mail apps can't switch it on for you.".into())
    } else if h.ends_with("gmail.com") || h.ends_with("googlemail.com") {
        Some("Gmail's own vacation responder is in Gmail on the web: Settings → General → Vacation responder.".into())
    } else if h.contains("outlook") || h.contains("office365") {
        Some("Outlook's own automatic replies are in Outlook on the web: Settings → Mail → Automatic replies.".into())
    } else if h.contains("aliyun") {
        Some("Alimail's own automatic reply is in webmail settings (自动回复).".into())
    } else {
        None
    }
}

#[tauri::command]
pub fn get_auto_reply(state: State<'_, AppState>, account_id: i64) -> Res<AutoReply> {
    state.store.auto_reply(account_id)
}

/// Can this account's server send the automatic reply itself?
#[tauri::command]
pub async fn auto_reply_server_support(
    state: State<'_, AppState>,
    account_id: i64,
) -> Res<ServerSupport> {
    let account = state.store.account(account_id)?;
    let hint = provider_hint(&account.config.imap_host);
    let unavailable = |detail: &str| ServerSupport {
        available: false,
        dates: false,
        detail: detail.to_string(),
        provider_hint: hint.clone(),
    };
    if account.config.auth.is_oauth() {
        return Ok(unavailable(
            "Server rules aren't available for accounts that sign in with Google or Microsoft.",
        ));
    }
    let port = state.store.sieve_settings(account_id)?.port;
    let password = crate::providers::credential(&state, account_id).await?;
    let connected = tokio::time::timeout(
        Duration::from_secs(10),
        managesieve::Client::connect(&account.config, port, &password),
    )
    .await;
    let ms = match connected {
        Ok(Ok(ms)) => ms,
        _ => {
            return Ok(unavailable(
                "This server doesn't offer server-side rules (ManageSieve) to mail apps.",
            ))
        }
    };
    let ext = ms.caps.sieve_extensions.clone();
    ms.logout().await;
    let has = |e: &str| ext.iter().any(|x| x.eq_ignore_ascii_case(e));
    if !has("vacation") {
        return Ok(unavailable(
            "This server's rules can't send automatic replies (no Sieve vacation).",
        ));
    }
    Ok(ServerSupport {
        available: true,
        dates: has("date") && has("relational"),
        detail: String::new(),
        provider_hint: hint,
    })
}

/// Save the settings. On the server, the Sieve script is updated to match
/// (uploaded with the reply, or without it / removed when it's turned off);
/// if that fails, the previous settings are kept.
#[tauri::command]
pub async fn set_auto_reply(
    state: State<'_, AppState>,
    account_id: i64,
    settings: AutoReply,
) -> Res<AutoReply> {
    let before = state.store.auto_reply(account_id)?;
    let saved = state.store.set_auto_reply(account_id, &settings)?;
    let sieve = state.store.sieve_settings(account_id)?;
    let was_on_server = before.enabled && before.on_server && sieve.active;
    let result = if saved.enabled && saved.on_server {
        upload_script(&state, account_id, sieve.port)
            .await
            .map(|_| ())
    } else if was_on_server {
        // Take the reply out of the server script; keep any server rules.
        let rules_left = !crate::organize::export(&state, account_id)?
            .exported
            .is_empty();
        if rules_left {
            upload_script(&state, account_id, sieve.port)
                .await
                .map(|_| ())
        } else {
            remove_script(&state, account_id).await
        }
    } else {
        Ok(())
    };
    if let Err(e) = result {
        let restored = state.store.restore_auto_reply(account_id, &before);
        return Err(Error::Invalid(match restored {
            Ok(()) => format!("couldn't update the automatic reply on the server: {e}"),
            Err(r) => format!(
                "couldn't update the automatic reply on the server ({e}), and the previous settings couldn't be restored: {r}"
            ),
        }));
    }
    tracing::info!(
        account = account_id,
        enabled = saved.enabled,
        on_server = saved.on_server,
        "automatic reply updated"
    );
    Ok(saved)
}
