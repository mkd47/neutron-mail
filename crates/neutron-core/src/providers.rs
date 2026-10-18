//! Built-in provider profiles, email-domain detection, and Mozilla
//! ISPDB / autoconfig lookup for unknown domains.
//!
//! Settings come from each provider's official documentation (links in
//! `source`); see README "Provider profiles".

use serde::Serialize;

use crate::model::{AccountConfig, AuthMethod, Security};
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    /// Mail domains that select this profile automatically.
    pub domains: &'static [&'static str],
    pub imap_host: &'static str,
    pub imap_port: u16,
    pub imap_security: Security,
    pub smtp_host: &'static str,
    pub smtp_port: u16,
    pub smtp_security: Security,
    /// SMTP host for addresses in `domains` when it differs from the
    /// organisational one (Outlook.com consumer accounts).
    pub consumer_smtp_host: Option<&'static str>,
    /// Login methods the provider accepts, preferred first.
    pub auth: &'static [AuthMethod],
    /// Label for the secret field ("App password", "Client authorization code", …).
    pub password_label: &'static str,
    pub hint: &'static str,
    /// Where the user creates an app password / authorization code.
    pub help_url: &'static str,
    /// Official documentation of the server settings.
    pub source: &'static str,
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "263",
        name: "263.net",
        domains: &["263.net", "263.com", "net263.com"],
        imap_host: "imap.263.net",
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.263.net",
        smtp_port: 465,
        smtp_security: Security::Tls,
        consumer_smtp_host: None,
        auth: &[AuthMethod::Password],
        password_label: "Password or client authorization code",
        hint: "If your company enabled client authorization codes (客户端授权码), your web password no longer works in mail apps. Create a code in 263 webmail under Settings → Account security → Client authorization code; it is shown only once. Company domains hosted at 263 work too: pick this tile and enter your full address.",
        help_url: "https://www.263.net/success/mail/pcweb/20231124/1908.html",
        source: "https://www.263.net/success/mail/client/20160603/970.html",
    },
    Provider {
        id: "alimail",
        name: "Alimail (enterprise)",
        domains: &[],
        imap_host: "imap.qiye.aliyun.com",
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.qiye.aliyun.com",
        smtp_port: 465,
        smtp_security: Security::Tls,
        consumer_smtp_host: None,
        auth: &[AuthMethod::Password],
        password_label: "Third-party client password",
        hint: "New Alimail enterprise mailboxes block third-party clients until the administrator allows them. With the third-party client security password (三方客户端安全密码) enabled, generate one in webmail under Settings → Account and security → Account security and use it here instead of your login password.",
        help_url: "https://help.aliyun.com/zh/document_detail/444269.html",
        source: "https://help.aliyun.com/zh/document_detail/36576.html",
    },
    Provider {
        id: "alimail-personal",
        name: "Alimail (personal @aliyun.com)",
        domains: &["aliyun.com"],
        imap_host: "imap.aliyun.com",
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.aliyun.com",
        smtp_port: 465,
        smtp_security: Security::Tls,
        consumer_smtp_host: None,
        auth: &[AuthMethod::Password],
        password_label: "Password",
        hint: "Free personal Alibaba Cloud mailboxes (@aliyun.com) use their own servers and the normal mailbox password.",
        help_url: "https://help.aliyun.com/zh/document_detail/465307.html",
        source: "https://help.aliyun.com/zh/document_detail/465307.html",
    },
    Provider {
        id: "google",
        name: "Google (Gmail)",
        domains: &["gmail.com", "googlemail.com"],
        imap_host: "imap.gmail.com",
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.gmail.com",
        smtp_port: 465,
        smtp_security: Security::Tls,
        consumer_smtp_host: None,
        auth: &[AuthMethod::OauthGoogle, AuthMethod::Password],
        password_label: "App password",
        hint: "Google recommends \"Sign in with Google\" (OAuth). Your normal Google password does not work here; the alternative is a 16-character app password, which needs 2-Step Verification on your Google account. IMAP is always on for personal accounts; Workspace admins can turn it off.",
        help_url: "https://myaccount.google.com/apppasswords",
        source: "https://developers.google.com/workspace/gmail/imap/imap-smtp",
    },
    Provider {
        id: "outlook",
        name: "Outlook (Outlook.com / Microsoft 365)",
        domains: &["outlook.com", "hotmail.com", "live.com", "msn.com", "outlook.de", "hotmail.de"],
        imap_host: "outlook.office365.com",
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.office365.com",
        smtp_port: 587,
        smtp_security: Security::Starttls,
        consumer_smtp_host: Some("smtp-mail.outlook.com"),
        auth: &[AuthMethod::OauthMicrosoft],
        password_label: "",
        hint: "Microsoft requires OAuth2 (\"Sign in with Microsoft\") for IMAP and SMTP; passwords and app passwords are not accepted. Outlook.com: first turn on Settings → Mail → Forwarding and IMAP → \"Let devices and apps use IMAP\". Microsoft 365 work accounts may need the administrator to allow IMAP and SMTP AUTH.",
        help_url: "https://support.microsoft.com/en-us/office/pop-imap-and-smtp-settings-for-outlook-com-d088b986-291d-42b8-9564-9c414e2aa040",
        source: "https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/pop3-and-imap4/pop3-and-imap4",
    },
];

pub fn by_id(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// The lower-cased domain of an address, if it looks valid.
pub fn domain_of(email: &str) -> Option<String> {
    let (local, d) = email.trim().rsplit_once('@')?;
    let d = d.trim_end_matches('.').to_ascii_lowercase();
    (!local.is_empty() && valid_hostname(&d) && d.contains('.')).then_some(d)
}

/// Built-in profile for a known mail domain.
pub fn detect(email: &str) -> Option<&'static Provider> {
    let d = domain_of(email)?;
    PROVIDERS.iter().find(|p| p.domains.contains(&d.as_str()))
}

impl Provider {
    /// SMTP host for this address (consumer domains may differ).
    pub fn smtp_host_for(&self, email: &str) -> &'static str {
        match (self.consumer_smtp_host, domain_of(email)) {
            (Some(h), Some(d)) if self.domains.contains(&d.as_str()) => h,
            _ => self.smtp_host,
        }
    }

    pub fn config(&self, email: &str, display_name: &str, auth: AuthMethod) -> AccountConfig {
        AccountConfig {
            email: email.trim().to_string(),
            display_name: display_name.to_string(),
            username: email.trim().to_string(),
            imap_host: self.imap_host.into(),
            imap_port: self.imap_port,
            imap_security: self.imap_security,
            smtp_host: self.smtp_host_for(email).into(),
            smtp_port: self.smtp_port,
            smtp_security: self.smtp_security,
            extra_ca_pem: None,
            auth,
        }
    }
}

pub fn valid_hostname(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

// ---- Mozilla autoconfig (ISPDB) ----------------------------------------

/// Server settings found by autoconfig.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Discovered {
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_security: Security,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: Security,
    /// The username to use (placeholders resolved).
    pub username: String,
    /// Where the settings came from (URL).
    pub source: String,
    /// The server offers OAuth2 (e.g. a hosted Google/Microsoft domain).
    pub oauth2: bool,
}

pub const MAX_AUTOCONFIG_BYTES: usize = 64 * 1024;

/// Text content of every `<tag ...>…</tag>` directly inside `xml`.
fn elements<'a>(xml: &'a str, tag: &str) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        // Must be the tag itself, not a prefix (e.g. <incomingServerX>).
        if !after.starts_with([' ', '>', '\t', '\n', '\r', '/']) {
            rest = after;
            continue;
        }
        let Some(gt) = after.find('>') else { break };
        let attrs = &after[..gt];
        let body_start = &after[gt + 1..];
        if attrs.ends_with('/') {
            out.push((attrs, ""));
            rest = body_start;
            continue;
        }
        let Some(end) = body_start.find(&close) else {
            break;
        };
        out.push((attrs, &body_start[..end]));
        rest = &body_start[end + close.len()..];
    }
    out
}

fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let i = attrs.find(&format!("{name}="))?;
    let v = &attrs[i + name.len() + 1..];
    let q = v.chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let v = &v[1..];
    Some(&v[..v.find(q)?])
}

fn text<'a>(body: &'a str, tag: &str) -> Option<&'a str> {
    elements(body, tag).first().map(|(_, t)| t.trim())
}

/// Parse a Thunderbird autoconfig `clientConfig` document (v1.1). Only
/// IMAP + SMTP with SSL/TLS or STARTTLS are accepted; plaintext servers are
/// ignored. Placeholders `%EMAILADDRESS%`, `%EMAILLOCALPART%`,
/// `%EMAILDOMAIN%` are resolved for `email`.
pub fn parse_autoconfig(xml: &str, email: &str, source: &str) -> Result<Discovered> {
    if xml.len() > MAX_AUTOCONFIG_BYTES {
        return Err(Error::Invalid("autoconfig document too large".into()));
    }
    if xml.contains("<!DOCTYPE") || xml.contains("<!ENTITY") {
        return Err(Error::Invalid("autoconfig: DTDs are not accepted".into()));
    }
    let (local, domain) = email
        .rsplit_once('@')
        .ok_or_else(|| Error::Invalid("bad email".into()))?;
    let subst = |s: &str| {
        s.replace("%EMAILADDRESS%", email)
            .replace("%EMAILLOCALPART%", local)
            .replace("%EMAILDOMAIN%", domain)
    };
    let pick = |tag: &str, ty: &str| -> Option<(String, u16, Security, String, bool)> {
        elements(xml, tag)
            .into_iter()
            .filter(|(a, _)| attr(a, "type") == Some(ty))
            .filter_map(|(_, body)| {
                let host = subst(text(body, "hostname")?).to_ascii_lowercase();
                let port: u16 = text(body, "port")?.parse().ok()?;
                let sec = match text(body, "socketType")?.to_ascii_uppercase().as_str() {
                    "SSL" | "TLS" => Security::Tls,
                    "STARTTLS" => Security::Starttls,
                    _ => return None, // plain: never
                };
                let user = subst(text(body, "username").unwrap_or("%EMAILADDRESS%"));
                let oauth = elements(body, "authentication")
                    .iter()
                    .any(|(_, t)| t.trim().eq_ignore_ascii_case("OAuth2"));
                (valid_hostname(&host) && port != 0).then_some((host, port, sec, user, oauth))
            })
            // Prefer implicit TLS over STARTTLS.
            .min_by_key(|(_, _, s, _, _)| *s != Security::Tls)
    };
    let (ih, ip, is, user, o1) = pick("incomingServer", "imap")
        .ok_or_else(|| Error::Invalid("autoconfig: no encrypted IMAP server".into()))?;
    let (sh, sp, ss, _, o2) = pick("outgoingServer", "smtp")
        .ok_or_else(|| Error::Invalid("autoconfig: no encrypted SMTP server".into()))?;
    Ok(Discovered {
        imap_host: ih,
        imap_port: ip,
        imap_security: is,
        smtp_host: sh,
        smtp_port: sp,
        smtp_security: ss,
        username: user,
        source: source.to_string(),
        oauth2: o1 || o2,
    })
}

/// The HTTPS URLs tried for a domain, in order: the domain's own
/// autoconfig server, then Mozilla's ISPDB. No plain-HTTP fallback; the
/// email address is not sent (only the domain is part of the URL).
pub fn autoconfig_urls(domain: &str) -> Vec<String> {
    vec![
        format!("https://autoconfig.{domain}/mail/config-v1.1.xml"),
        format!("https://autoconfig.thunderbird.net/v1.1/{domain}"),
    ]
}

/// Look up settings for `email` over HTTPS (rustls + webpki roots,
/// 8 s timeout, redirects only to HTTPS, 64 KB cap).
pub async fn autoconfig(email: &str) -> Result<Discovered> {
    let domain =
        domain_of(email).ok_or_else(|| Error::Invalid("enter a valid email address".into()))?;
    let client = crate::tls::http_builder()
        .map_err(|e| Error::Invalid(format!("autoconfig: {e}")))?
        .https_only(true)
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::limited(3))
        .user_agent("Neutron Mail autoconfig")
        .build()
        .map_err(|e| Error::Invalid(format!("autoconfig: {e}")))?;
    let mut last = String::from("not found");
    for url in autoconfig_urls(&domain) {
        match fetch_capped(&client, &url).await {
            Ok(xml) => match parse_autoconfig(&xml, email.trim(), &url) {
                Ok(d) => return Ok(d),
                Err(e) => last = e.to_string(),
            },
            Err(e) => last = e,
        }
    }
    Err(Error::Invalid(format!(
        "no settings found for {domain} ({last})"
    )))
}

async fn fetch_capped(client: &reqwest::Client, url: &str) -> std::result::Result<String, String> {
    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| e.without_url().to_string())?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_AUTOCONFIG_BYTES {
            return Err("document too large".into());
        }
    }
    String::from_utf8(body).map_err(|_| "not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_consistent() {
        for p in PROVIDERS {
            assert!(
                valid_hostname(p.imap_host) && valid_hostname(p.smtp_host),
                "{}",
                p.id
            );
            assert!(p.source.starts_with("https://") && p.help_url.starts_with("https://"));
            assert!(!p.auth.is_empty());
            // Port and security agree (993/465 implicit TLS, 587 STARTTLS).
            assert_eq!(p.imap_security, Security::Tls);
            assert_eq!(
                p.smtp_port == 587,
                p.smtp_security == Security::Starttls,
                "{}",
                p.id
            );
            assert!(p.config("a@b.test", "", p.auth[0]).validate().is_ok());
        }
        let ids: std::collections::HashSet<_> = PROVIDERS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), PROVIDERS.len());
        // Outlook offers no password path (basic auth is off).
        assert_eq!(
            by_id("outlook").unwrap().auth,
            &[AuthMethod::OauthMicrosoft]
        );
        assert!(by_id("google")
            .unwrap()
            .auth
            .contains(&AuthMethod::Password));
        assert_eq!(by_id("alimail").unwrap().imap_host, "imap.qiye.aliyun.com");
        assert_eq!(
            by_id("alimail-personal").unwrap().smtp_host,
            "smtp.aliyun.com"
        );
    }

    #[test]
    fn domain_detection() {
        assert_eq!(detect("Kaan@GMail.com").unwrap().id, "google");
        assert_eq!(detect("x@googlemail.com").unwrap().id, "google");
        assert_eq!(detect("x@hotmail.com").unwrap().id, "outlook");
        assert_eq!(detect("x@263.net").unwrap().id, "263");
        assert_eq!(detect("x@aliyun.com").unwrap().id, "alimail-personal");
        assert!(detect("x@example.org").is_none());
        assert!(detect("x@gmail.com.evil.test").is_none());
        assert!(detect("not-an-address").is_none());
        let o = by_id("outlook").unwrap();
        assert_eq!(o.smtp_host_for("a@hotmail.com"), "smtp-mail.outlook.com");
        assert_eq!(o.smtp_host_for("a@contoso.com"), "smtp.office365.com");
        assert_eq!(domain_of("a@Example.ORG."), Some("example.org".into()));
        assert_eq!(domain_of("a@exa mple.org"), None);
        assert_eq!(domain_of("a@../etc"), None);
    }

    const ISPDB: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.org">
    <domain>example.org</domain>
    <incomingServer type="pop3"><hostname>pop.example.org</hostname><port>995</port><socketType>SSL</socketType></incomingServer>
    <incomingServer type="imap">
      <hostname>imap-plain.example.org</hostname><port>143</port><socketType>plain</socketType>
      <username>%EMAILADDRESS%</username><authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.example.org</hostname><port>143</port><socketType>STARTTLS</socketType>
      <username>%EMAILLOCALPART%</username>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>IMAP.%EMAILDOMAIN%</hostname><port>993</port><socketType>SSL</socketType>
      <username>%EMAILLOCALPART%</username><authentication>OAuth2</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.org</hostname><port>587</port><socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn ispdb_parsing() {
        let d = parse_autoconfig(ISPDB, "kaan@example.org", "https://x").unwrap();
        assert_eq!(d.imap_host, "imap.example.org");
        assert_eq!((d.imap_port, d.imap_security), (993, Security::Tls));
        assert_eq!(d.username, "kaan");
        assert!(d.oauth2);
        assert_eq!(
            (d.smtp_host.as_str(), d.smtp_port, d.smtp_security),
            ("smtp.example.org", 587, Security::Starttls)
        );
    }

    #[test]
    fn ispdb_rejects_plaintext_bad_hosts_and_dtds() {
        let plain = ISPDB
            .replace(
                "<socketType>STARTTLS</socketType>",
                "<socketType>plain</socketType>",
            )
            .replace(
                "<socketType>SSL</socketType>",
                "<socketType>plain</socketType>",
            );
        assert!(parse_autoconfig(&plain, "a@example.org", "u").is_err());
        let bad = ISPDB
            .replace("IMAP.%EMAILDOMAIN%", "imap.example.org/../x")
            .replace(
                "imap.example.org</hostname><port>143</port><socketType>STARTTLS",
                "evil host</hostname><port>143</port><socketType>STARTTLS",
            );
        assert!(parse_autoconfig(&bad, "a@example.org", "u").is_err());
        let dtd = format!("<!DOCTYPE x [<!ENTITY a \"b\">]>{ISPDB}");
        assert!(parse_autoconfig(&dtd, "a@example.org", "u").is_err());
        assert!(parse_autoconfig(&"x".repeat(MAX_AUTOCONFIG_BYTES + 1), "a@b.c", "u").is_err());
        assert!(autoconfig_urls("example.org")
            .iter()
            .all(|u| u.starts_with("https://")));
    }
}
