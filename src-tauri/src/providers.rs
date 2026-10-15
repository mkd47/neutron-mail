//! Provider tiles, autoconfig and OAuth sign-in commands, plus the
//! credential resolver every IMAP/SMTP call goes through.

use std::time::Duration;

use neutron_core::{
    imap,
    model::{Account, AccountConfig, AuthMethod},
    oauth::{self, OAuthApp, Tokens},
    providers::{self, Discovered, Provider},
    Error,
};
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::{commands::Res, watcher, AppState};

/// One refresh at a time, so concurrent syncs don't race on the refresh
/// token (providers may rotate it).
static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The secret to present for `account_id`: the password, or for OAuth
/// accounts a fresh access token (refreshed and re-stored when stale).
pub async fn credential(state: &AppState, account_id: i64) -> Res<String> {
    let account = state.store.account(account_id)?;
    let secret = state.secrets.get(account_id)?;
    if !account.config.auth.is_oauth() {
        return Ok(secret);
    }
    let tokens = Tokens::from_secret(&secret)?;
    if !tokens.stale_now() {
        return Ok(tokens.access_token);
    }
    let _g = REFRESH.lock().await;
    // Another task may have refreshed while we waited.
    let tokens = Tokens::from_secret(&state.secrets.get(account_id)?)?;
    if !tokens.stale_now() {
        return Ok(tokens.access_token);
    }
    let app = OAuthApp::for_method(account.config.auth)?;
    let fresh = oauth::refresh(&app, &tokens).await?;
    state.secrets.set(account_id, &fresh.to_secret())?;
    tracing::info!(account = account_id, "OAuth access token refreshed");
    Ok(fresh.access_token)
}

#[derive(Serialize)]
pub struct ProviderInfo {
    #[serde(flatten)]
    provider: Provider,
    /// Which OAuth methods have a client ID in this build.
    oauth_ready: bool,
}

#[tauri::command]
pub fn list_providers() -> Vec<ProviderInfo> {
    providers::PROVIDERS
        .iter()
        .map(|p| ProviderInfo {
            provider: p.clone(),
            oauth_ready: p
                .auth
                .iter()
                .any(|m| m.is_oauth() && OAuthApp::configured(*m)),
        })
        .collect()
}

#[tauri::command]
pub fn detect_provider(email: String) -> Option<&'static str> {
    providers::detect(&email).map(|p| p.id)
}

#[tauri::command]
pub async fn autoconfig_lookup(email: String) -> Res<Discovered> {
    providers::autoconfig(&email).await
}

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// Sign in with Google/Microsoft in the system browser (PKCE + loopback),
/// verify IMAP with XOAUTH2, then store the account and the tokens.
/// With `account_id`, re-authorizes an existing account instead.
#[tauri::command]
pub async fn oauth_sign_in(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AccountConfig,
    account_id: Option<i64>,
) -> Res<Account> {
    config.validate()?;
    if !config.auth.is_oauth() {
        return Err(Error::Invalid("this account does not use OAuth".into()));
    }
    let client = OAuthApp::for_method(config.auth)?;
    let pkce = oauth::Pkce::new()?;
    let lb = oauth::Loopback::bind().await?;
    let url = client.authorize_url(&pkce, &lb.redirect_uri, &config.email);
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| Error::Invalid(format!("could not open the browser: {e}")))?;
    let redirect = lb.redirect_uri.clone();
    let code = lb.wait_for_code(&pkce.state, SIGN_IN_TIMEOUT).await?;
    let tokens = oauth::exchange_code(&client, &code, &pkce.verifier, &redirect).await?;
    let mut session = imap::connect(&config, &tokens.access_token).await?;
    drop(session.logout().await);
    let id = match account_id {
        Some(id) => {
            state.store.account(id)?;
            watcher::stop(&state, id);
            state.store.update_account(id, &config)?;
            state.secrets.set(id, &tokens.to_secret())?;
            id
        }
        None => {
            let id = state.store.add_account(&config)?;
            if let Err(e) = state.secrets.set(id, &tokens.to_secret()) {
                let _ = state.store.delete_account(id);
                return Err(e);
            }
            id
        }
    };
    tracing::info!(
        account = id,
        method = config.auth.as_str(),
        "OAuth account signed in"
    );
    watcher::start(&app, id);
    state.store.account(id)
}

/// Kept for symmetry with the UI: whether the method is usable.
#[tauri::command]
pub fn oauth_configured(method: AuthMethod) -> bool {
    OAuthApp::configured(method)
}
