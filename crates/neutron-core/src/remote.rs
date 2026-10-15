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
//!   max 5 MB per image, 25 MB admitted response bodies and 40 distinct
//!   images per message, including bodies discarded after validation fails.
//! * Environment HTTP(S)/ALL proxies fail closed for images: a proxy can
//!   resolve target names itself, bypassing the public-only DNS resolver.
//!   We never silently route around a configured privacy proxy. VPN tunnel
//!   routing is unaffected. NO_PROXY exclusions are intentionally not used.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
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
        // The caller refuses configured environment proxies first. Disable
        // automatic proxy discovery as well so all target DNS goes through
        // our resolver, including redirects.
        .no_proxy()
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

/// Shared across every in-flight response. Failed/invalid images consume
/// the budget too, so retrying another URL cannot repeatedly reuse it.
struct DownloadBudget {
    remaining: AtomicUsize,
}

impl DownloadBudget {
    fn new(bytes: usize) -> Self {
        Self {
            remaining: AtomicUsize::new(bytes),
        }
    }

    fn exhausted(&self) -> bool {
        self.remaining.load(Ordering::Relaxed) == 0
    }

    fn charge(&self, bytes: usize) -> bool {
        let mut previous = self.remaining.load(Ordering::Relaxed);
        loop {
            match self.remaining.compare_exchange_weak(
                previous,
                previous.saturating_sub(bytes),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return bytes <= previous,
                Err(current) => previous = current,
            }
        }
    }
}

const PROXY_ENV: &[&str] = &[
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
];

fn proxy_configured<'a>(settings: impl IntoIterator<Item = (&'a str, &'a str)>) -> bool {
    settings
        .into_iter()
        .any(|(name, value)| PROXY_ENV.contains(&name) && !value.trim().is_empty())
}

async fn fetch_one(
    c: &reqwest::Client,
    url: &str,
    allow_private: bool,
    budget: &DownloadBudget,
) -> Result<String> {
    if budget.exhausted() {
        return Err(Error::Invalid("message image budget exhausted".into()));
    }
    let u = Url::parse(url).map_err(|_| Error::Invalid("bad image URL".into()))?;
    if !url_allowed(&u, allow_private) {
        return Err(Error::Invalid("image host not allowed".into()));
    }
    let mut resp = c
        .get(u)
        .send()
        .await
        .map_err(|e| Error::Invalid(format!("image fetch failed: {}", e.without_url())))?;
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
        .map_err(|e| Error::Invalid(format!("image read failed: {}", e.without_url())))?
    {
        // Check before retaining bytes, not after collecting all images.
        // Transport buffers and one final in-flight chunk can still contain
        // extra bytes, but they are never appended or base64 encoded.
        if !budget.charge(chunk.len()) {
            return Err(Error::Invalid("message image budget exhausted".into()));
        }
        if chunk.len() > MAX_IMAGE_BYTES.saturating_sub(data.len()) {
            return Err(Error::Invalid("image too large".into()));
        }
        data.extend_from_slice(&chunk);
    }
    let mime = sniff_image(&data).ok_or_else(|| Error::Invalid("not a raster image".into()))?;
    // Encode directly into the final string instead of retaining a second
    // full base64 copy while format! builds the data URL.
    let mut encoded = format!("data:{mime};base64,");
    base64::engine::general_purpose::STANDARD.encode_string(&data, &mut encoded);
    Ok(encoded)
}

/// Fetch distinct remote image URLs, at most [`MAX_IMAGES`], with up to
/// 6 in parallel and one shared response-body budget. HTTP proxy settings
/// cause all images to remain blocked; NO_PROXY does not override this.
pub async fn fetch_images(urls: &[String], allow_private: bool) -> Result<FetchReport> {
    let settings: Vec<_> = PROXY_ENV
        .iter()
        .filter_map(|&name| std::env::var(name).ok().map(|value| (name, value)))
        .collect();
    if proxy_configured(settings.iter().map(|(name, value)| (*name, value.as_str()))) {
        tracing::debug!("remote images blocked: configured HTTP proxy cannot enforce public-only destination DNS");
        return Ok(FetchReport {
            images: HashMap::new(),
            failed: urls.iter().collect::<HashSet<_>>().len(),
        });
    }
    let c = client(allow_private)?;
    Ok(fetch_images_with_client(
        c,
        urls.to_vec(),
        allow_private,
        Arc::new(DownloadBudget::new(MAX_TOTAL_BYTES)),
    )
    .await)
}

async fn fetch_images_with_client(
    c: reqwest::Client,
    urls: Vec<String>,
    allow_private: bool,
    budget: Arc<DownloadBudget>,
) -> FetchReport {
    let urls: Vec<_> = {
        let mut seen = HashSet::new();
        urls.into_iter()
            .filter(|u| seen.insert(u.clone()))
            .collect()
    };
    let mut report = FetchReport {
        failed: urls.len().saturating_sub(MAX_IMAGES),
        ..FetchReport::default()
    };
    // Each future owns its URL/client and shares only an Arc budget.
    // Borrowed iterator items can make the enclosing future fail Send's
    // higher-ranked lifetime requirements in Tauri async commands.
    let results = stream::iter(urls.into_iter().take(MAX_IMAGES))
        .map(move |url| {
            let c = c.clone();
            let budget = Arc::clone(&budget);
            async move {
                let result = fetch_one(&c, &url, allow_private, &budget).await;
                (url, result)
            }
        })
        .buffer_unordered(6);
    futures::pin_mut!(results);
    while let Some((url, result)) = results.next().await {
        match result {
            Ok(data) => {
                report.images.insert(url, data);
            }
            Err(e) => {
                tracing::debug!("remote image skipped: {e}");
                report.failed += 1;
            }
        }
    }
    report
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

    #[test]
    fn image_fetch_future_is_send() {
        // Tauri runs get_message as a Send async command. Keep this bound
        // checked by the core tests even when the desktop crate is absent.
        fn assert_send<T: Send>(_: T) {}
        let urls = vec!["https://example.org/image.png".to_string()];
        assert_send(fetch_images(&urls, false));
    }

    #[test]
    fn environment_proxies_fail_closed_even_with_no_proxy() {
        for name in PROXY_ENV {
            assert!(proxy_configured([(*name, "http://proxy.example:8080")]));
            assert!(!proxy_configured([(*name, "  ")]));
        }
        assert!(proxy_configured([
            ("HTTPS_PROXY", "http://proxy.example:8080"),
            ("NO_PROXY", "*"),
        ]));
        assert!(!proxy_configured([("NO_PROXY", "*"), ("OTHER", "proxy")]));
    }

    /// Local-only HTTP server with one transfer-encoding chunk per body.
    /// No external services or process environment mutation are needed.
    async fn image_server(
        body: Vec<u8>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 2048];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                count.fetch_add(1, Ordering::Relaxed);
                let mut response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
                    body.len()
                )
                .into_bytes();
                response.extend_from_slice(&body);
                response.extend_from_slice(b"\r\n0\r\n\r\n");
                let _ = socket.write_all(&response).await;
            }
        });
        (base, requests, server)
    }

    fn png_body(bytes: usize) -> Vec<u8> {
        let mut body = b"\x89PNG\r\n\x1a\n".to_vec();
        body.resize(bytes, 0);
        body
    }

    fn decoded_bytes(report: &FetchReport) -> usize {
        report
            .images
            .values()
            .map(|data| {
                base64::engine::general_purpose::STANDARD
                    .decode(data.split_once(',').unwrap().1)
                    .unwrap()
                    .len()
            })
            .sum()
    }

    #[tokio::test]
    async fn duplicate_urls_do_not_repeat_requests_or_use_up_the_image_limit() {
        let (base, requests, server) = image_server(png_body(64)).await;
        let mut urls = vec![format!("{base}/first.png"); MAX_IMAGES];
        urls.push(format!("{base}/second.png"));
        let c = reqwest::Client::builder().no_proxy().build().unwrap();
        let report =
            fetch_images_with_client(c, urls, true, Arc::new(DownloadBudget::new(1024))).await;
        server.abort();
        assert_eq!(report.images.len(), 2);
        assert_eq!(report.failed, 0);
        assert_eq!(requests.load(Ordering::Relaxed), 2);
        assert_eq!(decoded_bytes(&report), 128);
    }

    #[tokio::test]
    async fn concurrent_chunked_images_share_one_download_budget() {
        let (base, requests, server) = image_server(png_body(64)).await;
        let urls: Vec<_> = (0..20).map(|i| format!("{base}/{i}.png")).collect();
        let c = reqwest::Client::builder().no_proxy().build().unwrap();
        let budget = Arc::new(DownloadBudget::new(128));
        let report = fetch_images_with_client(c, urls.clone(), true, Arc::clone(&budget)).await;
        server.abort();
        assert!(decoded_bytes(&report) <= 128);
        assert!(report.images.len() <= 2);
        assert_eq!(report.images.len() + report.failed, urls.len());
        assert!(budget.exhausted());
        assert!(requests.load(Ordering::Relaxed) < urls.len());
    }

    #[tokio::test]
    async fn invalid_images_consume_the_budget_and_stop_later_requests() {
        let (base, requests, server) = image_server(vec![b'x'; 64]).await;
        let c = reqwest::Client::builder().no_proxy().build().unwrap();
        let budget = DownloadBudget::new(64);
        let invalid = fetch_one(&c, &format!("{base}/invalid.png"), true, &budget).await;
        assert!(invalid
            .unwrap_err()
            .to_string()
            .contains("not a raster image"));
        let skipped = fetch_one(&c, &format!("{base}/next.png"), true, &budget).await;
        server.abort();
        assert!(skipped
            .unwrap_err()
            .to_string()
            .contains("budget exhausted"));
        assert_eq!(requests.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn over_budget_chunk_is_rejected_without_returning_an_image() {
        let (base, _, server) = image_server(png_body(64)).await;
        let c = reqwest::Client::builder().no_proxy().build().unwrap();
        let budget = DownloadBudget::new(63);
        let result = fetch_one(&c, &format!("{base}/large.png"), true, &budget).await;
        server.abort();
        assert!(result.unwrap_err().to_string().contains("budget exhausted"));
        assert!(budget.exhausted());
    }
}
