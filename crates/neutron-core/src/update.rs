//! "A newer version is available": asks GitHub for the latest release.
//! Only the version number is compared; nothing is downloaded or run.

use std::time::Duration;

use crate::{Error, Result};

pub const RELEASES_API: &str = "https://api.github.com/repos/mkd47/neutron-mail/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Release {
    /// "0.5.0" (without the "v").
    pub version: String,
    /// The release page (notes and downloads).
    pub url: String,
}

/// Numeric parts of "v1.2.3" / "1.2.3-beta" (pre-release suffixes ignored).
fn parts(v: &str) -> Option<Vec<u64>> {
    let core = v.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    core.split('.').map(|p| p.parse().ok()).collect()
}

/// True when `latest` is a higher version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parts(latest), parts(current)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a > b
        }
        _ => false,
    }
}

/// Read the GitHub "latest release" answer.
pub fn parse_latest(json: &str) -> Result<Release> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::Invalid(format!("update check: {e}")))?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| Error::Invalid("update check: no version in the answer".into()))?;
    let url = v["html_url"].as_str().unwrap_or_default();
    if !url.starts_with("https://github.com/") {
        return Err(Error::Invalid(
            "update check: unexpected release link".into(),
        ));
    }
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        url: url.to_string(),
    })
}

/// The newest release, if it is newer than `current`.
pub async fn check(current: &str) -> Result<Option<Release>> {
    let client = crate::tls::http_builder()
        .map_err(|e| Error::Invalid(format!("update check: {e}")))?
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("neutron-mail/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Error::Invalid(format!("update check: {e}")))?;
    let resp = client
        .get(RELEASES_API)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| Error::Invalid(format!("update check: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Invalid(format!(
            "update check: GitHub answered {}",
            resp.status()
        )));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| Error::Invalid(format!("update check: {e}")))?;
    let latest = parse_latest(&body)?;
    Ok(is_newer(&latest.version, current).then_some(latest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("0.5.0", "0.4.0"));
        assert!(is_newer("1.0", "0.99.1"));
        assert!(!is_newer("v0.4.0", "0.4.0"));
        assert!(!is_newer("0.4.0", "0.4.1"));
        assert!(!is_newer("0.5.0-rc1", "0.5.0"));
        assert!(!is_newer("garbage", "0.4.0"));
    }

    #[test]
    fn reads_the_github_answer() {
        let r = parse_latest(
            r#"{"tag_name":"v0.5.0","html_url":"https://github.com/mkd47/neutron-mail/releases/tag/v0.5.0"}"#,
        )
        .unwrap();
        assert_eq!(r.version, "0.5.0");
        assert!(parse_latest(r#"{"tag_name":"v1","html_url":"https://evil.example/"}"#).is_err());
        assert!(parse_latest("{}").is_err());
    }
}
