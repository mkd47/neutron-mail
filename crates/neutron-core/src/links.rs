//! Link clicks from the message viewer. The shell intercepts every
//! navigation out of the sandboxed iframe and calls [`analyze`]; the UI
//! then shows the real destination and asks before opening it.
//!
//! * Only `http`, `https` and `mailto` are accepted.
//! * Known tracking redirects are unwrapped: LinkedIn `/comm/` email links
//!   (tracking params removed), LinkedIn `/redir/redirect?url=`, and
//!   generic `?url=`/`?u=`/`?redirect=`/`?target=` style redirectors.
//! * `utm_*` and other common tracking parameters are stripped from the
//!   "clean" variant; the original link stays available.

use serde::Serialize;
use url::Url;

use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LinkInfo {
    /// The link exactly as clicked.
    pub original: String,
    /// Destination without tracking (== original if nothing to clean).
    pub clean: String,
    /// Host of `clean` in ASCII/punycode form, so homograph tricks are visible.
    pub host: String,
    /// "http", "https" or "mailto".
    pub scheme: String,
    /// Human explanation of what was unwrapped, if anything.
    pub note: Option<String>,
    pub mailto: Option<Mailto>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Mailto {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
}

const TRACKING_PARAMS: &[&str] = &[
    // LinkedIn email tracking
    "trk",
    "trkemail",
    "lipi",
    "midtoken",
    "midsig",
    "eid",
    "otptoken",
    "loid",
    "refid",
    "trackingid",
    "ssid",
    "fid",
    "lici",
    "li_fat_id",
    // generic
    "fbclid",
    "gclid",
    "dclid",
    "msclkid",
    "mc_cid",
    "mc_eid",
    "_hsenc",
    "_hsmi",
    "mkt_tok",
    "oly_anon_id",
    "oly_enc_id",
    "vero_id",
    "yclid",
    "igshid",
];
const REDIRECT_PARAMS: &[&str] = &[
    "url",
    "u",
    "redirect",
    "redirect_url",
    "redirect_uri",
    "target",
    "dest",
    "destination",
    "to",
    "link",
    "goto",
];

fn is_tracking(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_") || TRACKING_PARAMS.contains(&k.as_str())
}

fn strip_tracking(mut u: Url) -> Url {
    let kept: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !is_tracking(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(kept);
    }
    u
}

fn is_linkedin(host: &str) -> bool {
    host == "linkedin.com" || host.ends_with(".linkedin.com")
}

fn embedded_redirect(u: &Url) -> Option<Url> {
    u.query_pairs()
        .filter(|(k, _)| REDIRECT_PARAMS.contains(&k.to_ascii_lowercase().as_str()))
        .filter_map(|(_, v)| Url::parse(&v).ok())
        .find(|t| matches!(t.scheme(), "http" | "https"))
}

pub fn analyze(raw: &str) -> Result<LinkInfo> {
    let original = raw.trim().to_string();
    let u = Url::parse(&original).map_err(|_| Error::Invalid("not a valid link".into()))?;
    match u.scheme() {
        "mailto" => {
            let m = parse_mailto(&u);
            Ok(LinkInfo {
                clean: original.clone(),
                original,
                host: String::new(),
                scheme: "mailto".into(),
                note: None,
                mailto: Some(m),
            })
        }
        "http" | "https" => {
            let host = u.host_str().unwrap_or("").to_ascii_lowercase();
            let (clean, note) = if is_linkedin(&host) && u.path().starts_with("/redir/redirect") {
                match embedded_redirect(&u) {
                    Some(t) => (
                        strip_tracking(t),
                        Some("LinkedIn redirect unwrapped".to_string()),
                    ),
                    None => (strip_tracking(u.clone()), None),
                }
            } else if is_linkedin(&host) && u.path().starts_with("/comm/") {
                let mut t = u.clone();
                let path = u.path().trim_start_matches("/comm").to_string();
                t.set_path(&path);
                (
                    strip_tracking(t),
                    Some("LinkedIn email tracking removed".to_string()),
                )
            } else if host == "lnkd.in" {
                (u.clone(), Some("Shortened LinkedIn link: the final destination is only known after opening it".to_string()))
            } else if let Some(t) = embedded_redirect(&u) {
                let note = format!("Redirect via {host} unwrapped");
                (strip_tracking(t), Some(note))
            } else {
                let t = strip_tracking(u.clone());
                let note = (t != u).then(|| "Tracking parameters removed".to_string());
                (t, note)
            };
            Ok(LinkInfo {
                host: clean.host_str().unwrap_or("").to_string(),
                scheme: clean.scheme().to_string(),
                clean: clean.to_string(),
                original,
                note,
                mailto: None,
            })
        }
        other => Err(Error::Invalid(format!(
            "links of type \"{other}:\" are not opened"
        ))),
    }
}

fn split_addrs(s: &str) -> Vec<String> {
    s.split(',')
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect()
}

fn parse_mailto(u: &Url) -> Mailto {
    let path = percent_decode(u.path());
    let mut m = Mailto {
        to: split_addrs(&path),
        ..Default::default()
    };
    for (k, v) in u.query_pairs() {
        match k.to_ascii_lowercase().as_str() {
            "to" => m.to.extend(split_addrs(&v)),
            "cc" => m.cc.extend(split_addrs(&v)),
            "bcc" => m.bcc.extend(split_addrs(&v)),
            // Single line: never let a link inject header lines.
            "subject" => m.subject = v.replace(['\r', '\n'], " "),
            "body" => m.body = v.into_owned(),
            _ => {}
        }
    }
    m
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// Whether the app webview may navigate to `url` itself. Everything else
/// (links clicked in a message) is intercepted. `app_origins` are the
/// app's own origins (`tauri://localhost`, `http://tauri.localhost`, the dev
/// server in debug builds).
pub fn is_internal_navigation(url: &str, app_origins: &[&str]) -> bool {
    let lower = url.to_ascii_lowercase();
    lower == "about:blank"
        || lower == "about:srcdoc"
        || app_origins.iter().any(|o| {
            lower == *o
                || lower.starts_with(&format!("{o}/"))
                || lower.starts_with(&format!("{o}?"))
                || lower.starts_with(&format!("{o}#"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linkedin_comm_link() {
        let i = analyze("https://www.linkedin.com/comm/jobs/view/4012345678/?trackingId=abc%3D%3D&refId=xyz&lipi=urn%3Ali%3Apage&midToken=AQH&midSig=0x&trk=eml-email_job_alert&trkEmail=eml-x&eid=1&otpToken=MTA&currentJobId=4012345678").unwrap();
        assert_eq!(
            i.clean,
            "https://www.linkedin.com/jobs/view/4012345678/?currentJobId=4012345678"
        );
        assert_eq!(i.host, "www.linkedin.com");
        assert!(i.note.is_some());
    }

    #[test]
    fn linkedin_redirect() {
        let i = analyze("https://www.linkedin.com/redir/redirect?url=https%3A%2F%2Fexample%2Eorg%2Fpost%3Futm_source%3Dli%26id%3D7&urlhash=Ab1&trk=x").unwrap();
        assert_eq!(i.clean, "https://example.org/post?id=7");
        assert_eq!(i.host, "example.org");
    }

    #[test]
    fn generic_redirect_and_utm() {
        let i =
            analyze("https://click.example.net/r?u=https%3A%2F%2Fshop.example%2Fa&x=1").unwrap();
        assert_eq!(i.clean, "https://shop.example/a");
        let j = analyze("https://example.org/a?utm_source=n&utm_medium=e&id=3").unwrap();
        assert_eq!(j.clean, "https://example.org/a?id=3");
        let k = analyze("https://example.org/plain").unwrap();
        assert_eq!(k.clean, k.original);
        assert!(k.note.is_none());
    }

    #[test]
    fn idn_host_shown_as_punycode() {
        let i = analyze("https://раураl.com/login").unwrap();
        assert!(i.host.starts_with("xn--"), "{}", i.host);
    }

    #[test]
    fn rejects_other_schemes() {
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "ftp://x/y",
            "smb://h/s",
        ] {
            assert!(analyze(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mailto() {
        let i = analyze("mailto:a@example.org,b@example.org?cc=c@example.org&subject=Hi%0D%0ABcc:%20x@evil&body=Line%201%0ALine%202").unwrap();
        let m = i.mailto.unwrap();
        assert_eq!(m.to, vec!["a@example.org", "b@example.org"]);
        assert_eq!(m.cc, vec!["c@example.org"]);
        assert!(!m.subject.contains('\n') && !m.subject.contains('\r'));
        assert_eq!(m.body, "Line 1\nLine 2");
    }

    #[test]
    fn internal_navigation() {
        let o = ["tauri://localhost", "http://tauri.localhost"];
        assert!(is_internal_navigation("tauri://localhost/index.html", &o));
        assert!(is_internal_navigation("about:srcdoc", &o));
        assert!(!is_internal_navigation(
            "https://www.linkedin.com/comm/x",
            &o
        ));
        assert!(!is_internal_navigation(
            "http://tauri.localhost.evil.test/",
            &o
        ));
    }
}
