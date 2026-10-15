//! Fetching remote images when the user explicitly asks ("Load remote
//! images" for one message, or "always for this sender").
//!
//! Images are downloaded by the Rust core, validated and embedded as
//! `data:` URLs, so the viewer's CSP (`img-src data:`) never has to allow
//! the network. Safeguards:
//! * http/https only, max 3 redirects, 10 s timeout per image, no cookies,
//!   no Referer, generic User-Agent.
//! * No requests to loopback, private, link-local, CGNAT, multicast or
//!   otherwise non-public addresses (custom DNS resolver + checks on IP
//!   literals and redirect targets), so a mail cannot make the client probe
//!   the local network (SSRF). Tests can lift this with
//!   `NEUTRON_ALLOW_PRIVATE_IMAGE_HOSTS=1` (debug builds only).
//! * Content must be PNG, JPEG, GIF or WebP by magic bytes (never SVG/HTML),
//!   max 5 MB per image, 25 MB and 40 images per message.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures::{stream, StreamExt};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

use crate::{Error, Result};

pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_IMAGES: usize = 40;

#[derive(Debug, Default)]
pub struct FetchReport {
    /// remote URL -> data URL
    pub images: HashMap<String, String>,
    pub failed: usize,
}

/// Whether private-network hosts are allowed (integration tests only).
pub fn allow_private_from_env() -> bool {
    cfg!(debug_assertions)
        && std::env::var("NEUTRON_ALLOW_PRIVATE_IMAGE_HOSTS").as_deref() == Ok("1")
}

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
                || (o[0] == 198 && (18..20).contains(&o[1])) // benchmarking
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] == 0x2001 && s[1] == 0x0db8)) // documentation
        }
    }
}

struct PublicOnlyResolver {
    allow_private: bool,
}

impl Resolve for PublicOnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow_private = self.allow_private;
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| allow_private || is_public_ip(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} does not resolve to a public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

fn url_allowed(u: &Url, allow_private: bool) -> bool {
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) => allow_private || is_public_ip(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => allow_private || is_public_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(d)) => {
            allow_private || !(d == "localhost" || d.ends_with(".localhost"))
        }
        None => false,
    }
}

fn client(allow_private: bool) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(PublicOnlyResolver { allow_private }))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 3 {
                attempt.error("too many redirects")
            } else if url_allowed(attempt.url(), allow_private) {
                attempt.follow()
            } else {
                attempt.error("redirect to a non-public address")
            }
        }))
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("Mozilla/5.0")
        .referer(false)
        .build()
        .map_err(|e| Error::Invalid(format!("http client: {e}")))
}

/// Detect a raster image type from its first bytes.
pub fn sniff_image(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

async fn fetch_one(c: &reqwest::Client, url: &str, allow_private: bool) -> Result<String> {
    let u = Url::parse(url).map_err(|_| Error::Invalid("bad image URL".into()))?;
    if !url_allowed(&u, allow_private) {
        return Err(Error::Invalid("image host not allowed".into()));
    }
    let mut resp = c
        .get(u)
        .send()
        .await
        .map_err(|e| Error::Invalid(format!("image fetch failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Invalid(format!(
            "image fetch: HTTP {}",
            resp.status()
        )));
    }
    if resp
        .content_length()
        .is_some_and(|l| l > MAX_IMAGE_BYTES as u64)
    {
        return Err(Error::Invalid("image too large".into()));
    }
    let mut data = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| Error::Invalid(format!("image read failed: {e}")))?
    {
        data.extend_from_slice(&chunk);
        if data.len() > MAX_IMAGE_BYTES {
            return Err(Error::Invalid("image too large".into()));
        }
    }
    let mime = sniff_image(&data).ok_or_else(|| Error::Invalid("not a raster image".into()))?;
    Ok(format!(
        "data:{mime};base64,{}",
        crate::parse::base64(&data)
    ))
}

/// Fetch the given remote image URLs (deduplicated, at most
/// [`MAX_IMAGES`]) with up to 6 in parallel.
pub async fn fetch_images(urls: &[String], allow_private: bool) -> Result<FetchReport> {
    let c = client(allow_private)?;
    let results: Vec<(String, Result<String>)> =
        stream::iter(urls.iter().take(MAX_IMAGES).cloned())
            .map(|u| {
                let c = c.clone();
                async move {
                    let r = fetch_one(&c, &u, allow_private).await;
                    (u, r)
                }
            })
            .buffer_unordered(6)
            .collect()
            .await;
    let mut report = FetchReport::default();
    let mut total = 0usize;
    for (u, r) in results {
        match r {
            Ok(data) if total + data.len() <= MAX_TOTAL_BYTES * 4 / 3 + 1024 => {
                total += data.len();
                report.images.insert(u, data);
            }
            Ok(_) => report.failed += 1,
            Err(e) => {
                tracing::debug!("remote image skipped: {e}");
                report.failed += 1;
            }
        }
    }
    report.failed += urls.len().saturating_sub(MAX_IMAGES);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ip_classification() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(private.parse().unwrap()), "{private}");
        }
        for public in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public_ip(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn url_rules() {
        let ok = |s: &str| url_allowed(&Url::parse(s).unwrap(), false);
        assert!(ok("https://cdn.example.org/a.png"));
        assert!(!ok("http://127.0.0.1:8080/a.png"));
        assert!(!ok("http://localhost/a.png"));
        assert!(!ok("http://[::1]/a.png"));
        assert!(!ok("http://169.254.169.254/latest/meta-data"));
        assert!(!ok("file:///etc/passwd"));
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff_image(b"<svg xmlns="), None);
        assert_eq!(sniff_image(b"<html>"), None);
    }
}
