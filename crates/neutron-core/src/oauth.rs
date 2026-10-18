//! OAuth2 for Google and Microsoft: authorization-code flow with PKCE (S256)
//! and a loopback redirect (RFC 8252), plus XOAUTH2 SASL strings.
//!
//! Client IDs are never hardcoded. They are read at runtime from
//! `NEUTRON_GOOGLE_CLIENT_ID` / `NEUTRON_MICROSOFT_CLIENT_ID`, falling back
//! to the same variables at build time (`option_env!`). Google desktop
//! clients also issue a (non-confidential) client secret, read from
//! `NEUTRON_GOOGLE_CLIENT_SECRET` the same way. Tokens are stored as JSON in
//! the Secret Service entry that would otherwise hold the password.

use std::time::Duration;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::model::AuthMethod;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthApp {
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    /// Extra authorization-request parameters.
    pub extra: Vec<(String, String)>,
}

fn setting(name: &str, built: Option<&'static str>) -> Option<String> {
    std::env::var(name)
        .ok()
        .or_else(|| built.map(str::to_owned))
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

impl OAuthApp {
    /// The configured app for `method`, or an error explaining which
    /// variable to set.
    pub fn for_method(method: AuthMethod) -> Result<Self> {
        match method {
            AuthMethod::OauthGoogle => Ok(Self {
                auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
                token_url: "https://oauth2.googleapis.com/token".into(),
                scopes: vec!["https://mail.google.com/".into()],
                client_id: setting(
                    "NEUTRON_GOOGLE_CLIENT_ID",
                    option_env!("NEUTRON_GOOGLE_CLIENT_ID"),
                )
                .ok_or_else(|| not_configured("Google", "NEUTRON_GOOGLE_CLIENT_ID"))?,
                client_secret: setting(
                    "NEUTRON_GOOGLE_CLIENT_SECRET",
                    option_env!("NEUTRON_GOOGLE_CLIENT_SECRET"),
                ),
                extra: vec![
                    ("access_type".into(), "offline".into()),
                    ("prompt".into(), "consent".into()),
                ],
            }),
            AuthMethod::OauthMicrosoft => {
                let tenant = setting(
                    "NEUTRON_MICROSOFT_TENANT",
                    option_env!("NEUTRON_MICROSOFT_TENANT"),
                )
                .unwrap_or_else(|| "common".into());
                if !tenant
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
                {
                    return Err(Error::Invalid(
                        "NEUTRON_MICROSOFT_TENANT is not a valid tenant".into(),
                    ));
                }
                Ok(Self {
                    auth_url: format!(
                        "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"
                    ),
                    token_url: format!(
                        "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"
                    ),
                    scopes: vec![
                        "https://outlook.office.com/IMAP.AccessAsUser.All".into(),
                        "https://outlook.office.com/SMTP.Send".into(),
                        "offline_access".into(),
                    ],
                    client_id: setting(
                        "NEUTRON_MICROSOFT_CLIENT_ID",
                        option_env!("NEUTRON_MICROSOFT_CLIENT_ID"),
                    )
                    .ok_or_else(|| not_configured("Microsoft", "NEUTRON_MICROSOFT_CLIENT_ID"))?,
                    client_secret: None,
                    extra: vec![],
                })
            }
            AuthMethod::Password => Err(Error::Invalid("not an OAuth account".into())),
        }
    }

    /// Whether a client ID is configured (for the UI).
    pub fn configured(method: AuthMethod) -> bool {
        Self::for_method(method).is_ok()
    }

    pub fn authorize_url(&self, pkce: &Pkce, redirect_uri: &str, login_hint: &str) -> String {
        let mut u = url::Url::parse(&self.auth_url).expect("static URL");
        {
            let mut q = u.query_pairs_mut();
            q.append_pair("response_type", "code")
                .append_pair("client_id", &self.client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("scope", &self.scopes.join(" "))
                .append_pair("state", &pkce.state)
                .append_pair("code_challenge", &pkce.challenge())
                .append_pair("code_challenge_method", "S256");
            if !login_hint.is_empty() {
                q.append_pair("login_hint", login_hint);
            }
            for (k, v) in &self.extra {
                q.append_pair(k, v);
            }
        }
        u.into()
    }
}

fn not_configured(who: &str, var: &str) -> Error {
    Error::Invalid(format!(
        "Sign in with {who} is not configured in this build: set {var} (see README, \"OAuth setup\")"
    ))
}

/// PKCE verifier (RFC 7636) and CSRF `state`, both 256-bit random.
pub struct Pkce {
    pub verifier: zeroize::Zeroizing<String>,
    pub state: String,
}

fn random_b64(n: usize) -> Result<String> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| Error::Secret(format!("random: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(b))
}

impl Pkce {
    pub fn new() -> Result<Self> {
        Ok(Self {
            verifier: zeroize::Zeroizing::new(random_b64(32)?),
            state: random_b64(32)?,
        })
    }
    pub fn challenge(&self) -> String {
        challenge_for(&self.verifier)
    }
}

pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// SASL XOAUTH2 initial response (before base64; async-imap and lettre
/// encode it themselves).
pub fn xoauth2_string(user: &str, access_token: &str) -> String {
    format!("user={user}\x01auth=Bearer {access_token}\x01\x01")
}

pub fn xoauth2_base64(user: &str, access_token: &str) -> String {
    STANDARD.encode(xoauth2_string(user, access_token))
}

/// Stored token set (JSON in the Secret Service).
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub expires_at: i64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tokens {{ expires_at: {} .. }}", self.expires_at)
    }
}

impl Tokens {
    pub fn to_secret(&self) -> String {
        serde_json::to_string(self).expect("serializable")
    }
    pub fn from_secret(s: &str) -> Result<Self> {
        serde_json::from_str(s)
            .map_err(|_| Error::Secret("stored OAuth token is unreadable; sign in again".into()))
    }
    /// True if the access token expires within 2 minutes.
    pub fn stale(&self, now: i64) -> bool {
        self.expires_at - 120 <= now
    }
    pub fn stale_now(&self) -> bool {
        self.stale(now())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Only HTTPS endpoints, except loopback (used by the tests' mock server).
fn check_endpoint(u: &str) -> Result<()> {
    let p = url::Url::parse(u).map_err(|_| Error::Invalid("bad token URL".into()))?;
    let loopback = matches!(
        p.host_str(),
        Some("127.0.0.1") | Some("[::1]") | Some("localhost")
    );
    if p.scheme() == "https" || (p.scheme() == "http" && loopback) {
        Ok(())
    } else {
        Err(Error::Invalid("token endpoint must use HTTPS".into()))
    }
}

async fn token_request(
    app: &OAuthApp,
    form: &[(&str, &str)],
    old_refresh: Option<&str>,
) -> Result<Tokens> {
    check_endpoint(&app.token_url)?;
    let body = {
        let mut b = url::form_urlencoded::Serializer::new(String::new());
        b.append_pair("client_id", &app.client_id);
        if let Some(s) = &app.client_secret {
            b.append_pair("client_secret", s);
        }
        for (k, v) in form {
            b.append_pair(k, v);
        }
        b.finish()
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Error::Invalid(format!("OAuth: {e}")))?;
    let resp = client
        .post(&app.token_url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| Error::Invalid(format!("OAuth token request failed: {}", e.without_url())))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Invalid(format!("OAuth: {}", e.without_url())))?;
    let r: TokenResponse = serde_json::from_slice(&bytes).map_err(|_| {
        Error::Invalid(format!(
            "OAuth: unexpected token response (HTTP {})",
            status.as_u16()
        ))
    })?;
    match (r.access_token, r.error) {
        (Some(at), None) if status.is_success() => Ok(Tokens {
            access_token: at,
            // Providers may omit the refresh token on refresh: keep the old one.
            refresh_token: r.refresh_token.or(old_refresh.map(str::to_owned)),
            expires_at: now() + r.expires_in.unwrap_or(3600).clamp(60, 86_400),
        }),
        (_, err) => Err(Error::Invalid(format!(
            "OAuth sign-in failed: {}{}",
            err.unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
            r.error_description
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        ))),
    }
}

pub async fn exchange_code(
    app: &OAuthApp,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<Tokens> {
    token_request(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ],
        None,
    )
    .await
}

pub async fn refresh(app: &OAuthApp, tokens: &Tokens) -> Result<Tokens> {
    let rt = tokens
        .refresh_token
        .as_deref()
        .ok_or_else(|| Error::Secret("no refresh token stored; sign in again".into()))?;
    let mut form = vec![("grant_type", "refresh_token"), ("refresh_token", rt)];
    let scope = app.scopes.join(" ");
    if app.client_secret.is_none() {
        form.push(("scope", &scope)); // Microsoft wants the scopes again
    }
    token_request(app, &form, Some(rt)).await
}

/// A loopback listener on 127.0.0.1 with an ephemeral port.
pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
}

impl Loopback {
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/"),
        })
    }

    /// Wait for the browser redirect; returns the authorization code once a
    /// request with the matching `state` arrives. Requests with a wrong
    /// state are answered with an error and ignored. An idle connection
    /// (the per-read timeout) or a reset connection is dropped and the wait
    /// continues until `timeout`. One probe from the browser must not abort
    /// sign-in.
    pub async fn wait_for_code(self, state: &str, timeout: Duration) -> Result<String> {
        let result = tokio::time::timeout(timeout, async {
            loop {
                let (mut sock, _) = match self.listener.accept().await {
                    Ok(conn) => conn,
                    // A peer can reset before accept() returns the socket.
                    // Keep waiting; a dead listener is still fatal.
                    Err(e) if abandoned_peer(&e) => {
                        tracing::debug!(error = %e, "ignoring abandoned OAuth callback accept");
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Err(e) => return Err(Error::from(e)),
                };
                let Some(req) = read_callback_headers(&mut sock).await else {
                    tracing::debug!("ignoring idle or reset OAuth callback connection");
                    continue;
                };
                let target = req
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("GET "))
                    .and_then(|l| l.split(' ').next())
                    .unwrap_or("");
                let parsed = url::Url::parse(&format!("http://127.0.0.1{target}")).ok();
                let get = |k: &str| {
                    parsed
                        .as_ref()
                        .and_then(|u| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()))
                };
                let result = if get("state").as_deref() != Some(state) {
                    None
                } else if let Some(code) = get("code") {
                    Some(Ok(code))
                } else {
                    Some(Err(Error::Invalid(format!(
                        "sign-in was cancelled or denied: {}",
                        get("error").unwrap_or_else(|| "no code".into())
                    ))))
                };
                let msg = match &result {
                    Some(Ok(_)) => "Signed in. You can close this tab and return to Neutron Mail.",
                    Some(Err(_)) => "Sign-in did not complete. Return to Neutron Mail.",
                    None => "Unexpected request.",
                };
                let html = format!(
                    "<!doctype html><meta charset=utf-8><title>Neutron Mail</title><p style=\"font-family:sans-serif\">{msg}</p>"
                );
                let _ = sock
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{html}",
                            html.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                let _ = sock.shutdown().await;
                if let Some(r) = result {
                    return r;
                }
            }
        })
        .await;
        match result {
            Ok(code) => code,
            Err(_) => Err(Error::Timeout),
        }
    }
}

/// How long to wait for bytes on one callback connection before abandoning
/// it and accepting another. This is not the sign-in deadline.
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(5);

fn abandoned_peer(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

/// `None` when this socket did not deliver a complete request head: it went
/// idle, reset, or closed. The caller keeps waiting for a real callback.
async fn read_callback_headers(sock: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 8192];
    let mut n = 0;
    while n < buf.len() {
        let chunk =
            match tokio::time::timeout(CALLBACK_READ_TIMEOUT, sock.read(&mut buf[n..])).await {
                Ok(Ok(n)) if n > 0 => n,
                _ => return None,
            };
        n += chunk;
        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&buf[..n]).into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(token_url: String) -> OAuthApp {
        OAuthApp {
            auth_url: "https://auth.example/authorize".into(),
            token_url,
            scopes: vec!["a".into(), "offline_access".into()],
            client_id: "cid".into(),
            client_secret: None,
            extra: vec![],
        }
    }

    #[test]
    fn pkce_s256_rfc7636_vector() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let p = Pkce::new().unwrap();
        assert_eq!(p.verifier.len(), 43);
        assert_ne!(p.state, Pkce::new().unwrap().state);
    }

    #[test]
    fn xoauth2_format() {
        // Example from Google's XOAUTH2 documentation.
        assert_eq!(
            xoauth2_base64("someuser@example.com", "ya29.vF9dft4qmTc2Nvb3RlckBhdHRhdmlzdGEuY29tCg"),
            "dXNlcj1zb21ldXNlckBleGFtcGxlLmNvbQFhdXRoPUJlYXJlciB5YTI5LnZGOWRmdDRxbVRjMk52YjNSbGNrQmhkSFJoZG1semRHRXVZMjl0Q2cBAQ=="
        );
    }

    #[test]
    fn authorize_url_has_pkce_and_state() {
        let a = app("https://t.example/token".into());
        let p = Pkce::new().unwrap();
        let u =
            url::Url::parse(&a.authorize_url(&p, "http://127.0.0.1:5555/", "k@x.test")).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["code_challenge"], p.challenge());
        assert_eq!(q["state"], p.state);
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5555/");
        assert_eq!(q["scope"], "a offline_access");
        assert!(!u.as_str().contains(p.verifier.as_str()));
    }

    #[test]
    fn missing_client_id_is_explained() {
        // SAFETY: tests in this module don't read these vars concurrently.
        if option_env!("NEUTRON_MICROSOFT_CLIENT_ID").is_none()
            && std::env::var("NEUTRON_MICROSOFT_CLIENT_ID").is_err()
        {
            let e = OAuthApp::for_method(AuthMethod::OauthMicrosoft)
                .unwrap_err()
                .to_string();
            assert!(e.contains("NEUTRON_MICROSOFT_CLIENT_ID"));
        }
        assert!(OAuthApp::for_method(AuthMethod::Password).is_err());
        assert!(check_endpoint("http://evil.example/token").is_err());
        assert!(check_endpoint("https://oauth2.googleapis.com/token").is_ok());
    }

    /// Minimal mock token endpoint: answers each POST with the next canned
    /// JSON body and records the request bodies.
    async fn mock_token_server(
        replies: Vec<(u16, &'static str)>,
    ) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://127.0.0.1:{}/token", l.local_addr().unwrap().port());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut s, _) = l.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = s.read(&mut tmp).await.unwrap();
                    buf.extend_from_slice(&tmp[..n]);
                    let txt = String::from_utf8_lossy(&buf).to_string();
                    if let Some(i) = txt.find("\r\n\r\n") {
                        let len: usize = txt
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= i + 4 + len {
                            tx.send(txt[i + 4..].to_string()).unwrap();
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (url, rx)
    }

    #[tokio::test]
    async fn full_flow_against_mock_endpoint() {
        let (url, mut bodies) = mock_token_server(vec![
            (200, r#"{"access_token":"AT1","refresh_token":"RT1","expires_in":3599,"token_type":"Bearer"}"#),
            (200, r#"{"access_token":"AT2","expires_in":3599}"#),
            (400, r#"{"error":"invalid_grant","error_description":"Token has been revoked"}"#),
        ])
        .await;
        let a = app(url);
        let pkce = Pkce::new().unwrap();

        // Browser side: the provider redirects to the loopback URI.
        let lb = Loopback::bind().await.unwrap();
        let redirect = lb.redirect_uri.clone();
        let state = pkce.state.clone();
        let browser = tokio::spawn({
            let redirect = redirect.clone();
            async move {
                let c = reqwest::Client::new();
                // A forged request with the wrong state is ignored.
                let r = c
                    .get(format!("{redirect}?code=EVIL&state=wrong"))
                    .send()
                    .await
                    .unwrap();
                assert!(r.text().await.unwrap().contains("Unexpected"));
                let r = c
                    .get(format!("{redirect}?code=C0DE&state={state}"))
                    .send()
                    .await
                    .unwrap();
                assert!(r.text().await.unwrap().contains("Signed in"));
            }
        });
        let code = lb
            .wait_for_code(&pkce.state, Duration::from_secs(10))
            .await
            .unwrap();
        browser.await.unwrap();
        assert_eq!(code, "C0DE");

        let t = exchange_code(&a, &code, &pkce.verifier, &redirect)
            .await
            .unwrap();
        assert_eq!(t.access_token, "AT1");
        assert_eq!(t.refresh_token.as_deref(), Some("RT1"));
        assert!(!t.stale(now()));
        let sent: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(bodies.recv().await.unwrap().as_bytes())
                .into_owned()
                .collect();
        assert_eq!(sent["grant_type"], "authorization_code");
        assert_eq!(sent["code_verifier"], pkce.verifier.as_str());
        assert_eq!(sent["redirect_uri"], redirect);
        assert_eq!(sent["client_id"], "cid");

        // Round-trip through the keyring representation.
        let t = Tokens::from_secret(&t.to_secret()).unwrap();
        assert!(!format!("{t:?}").contains("AT1"));

        // Refresh keeps the old refresh token when none is returned.
        let t2 = refresh(&a, &t).await.unwrap();
        assert_eq!(
            (t2.access_token.as_str(), t2.refresh_token.as_deref()),
            ("AT2", Some("RT1"))
        );
        let sent: std::collections::HashMap<String, String> =
            url::form_urlencoded::parse(bodies.recv().await.unwrap().as_bytes())
                .into_owned()
                .collect();
        assert_eq!(sent["grant_type"], "refresh_token");
        assert_eq!(sent["scope"], "a offline_access");

        let e = refresh(&a, &t2).await.unwrap_err().to_string();
        assert!(e.contains("invalid_grant") && e.contains("revoked"), "{e}");
    }

    #[tokio::test]
    async fn denied_consent_and_timeout() {
        let p = Pkce::new().unwrap();
        let lb = Loopback::bind().await.unwrap();
        let u = format!("{}?error=access_denied&state={}", lb.redirect_uri, p.state);
        tokio::spawn(async move {
            let _ = reqwest::get(u).await;
        });
        let e = lb
            .wait_for_code(&p.state, Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("access_denied"));
        let lb = Loopback::bind().await.unwrap();
        assert!(matches!(
            lb.wait_for_code("s", Duration::from_millis(100)).await,
            Err(Error::Timeout)
        ));
    }

    fn loopback_addr(redirect: &str) -> String {
        redirect
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string()
    }

    async fn raw_get(addr: &str, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;
        let mut sock = TcpStream::connect(addr).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        sock.write_all(req.as_bytes()).await.unwrap();
        let mut body = Vec::new();
        sock.read_to_end(&mut body).await.unwrap();
        String::from_utf8_lossy(&body).into_owned()
    }

    /// A reset or idle connection used to fail the whole sign-in (the 5s
    /// read timeout and connection-reset were returned with `?`). Time is
    /// paused so the idle timeout does not wait on the wall clock.
    #[tokio::test(start_paused = true)]
    async fn abandoned_callback_does_not_abort_sign_in() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpStream;

        let lb = Loopback::bind().await.unwrap();
        let addr = loopback_addr(&lb.redirect_uri);
        let wait =
            tokio::spawn(async move { lb.wait_for_code("good", Duration::from_secs(60)).await });

        // Reach accept() before connecting.
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(1)).await;

        // RST before a request is written. SO_LINGER 0 aborts the socket;
        // tokio deprecates set_linger because a normal drop can block, which
        // is acceptable in this test.
        let reset = TcpStream::connect(&addr).await.unwrap();
        #[allow(deprecated)]
        reset.set_linger(Some(Duration::ZERO)).unwrap();
        drop(reset);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        // Idle peer: the 5s header timeout must abandon this socket only.
        let mut idle = TcpStream::connect(&addr).await.unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(6)).await;
        // The abandoned socket may already be closed. Don't fail the test on that.
        let _ = idle.shutdown().await;
        drop(idle);

        let body = raw_get(&addr, "/?code=C0DE&state=good").await;
        assert!(body.contains("Signed in"), "{body}");
        assert_eq!(wait.await.unwrap().unwrap(), "C0DE");
    }
}
