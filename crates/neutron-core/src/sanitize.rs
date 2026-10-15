//! HTML sanitizing for message display (defence layer 1 of 3; the viewer
//! also uses a script-less sandboxed iframe and a restrictive CSP).
//!
//! Policy:
//! * ammonia allow-list: no scripts, forms, iframes, objects, event
//!   handlers, `<base>`, `<meta>`, `<link>` or unknown tags.
//! * CSS is kept (`style` attributes and `<style>` blocks) so newsletters
//!   render properly, but filtered: `@import`, `expression()`,
//!   `-moz-binding`, `behavior` and `javascript:` are removed, and every
//!   `url(...)` that is not an inline raster `data:` image is neutralized
//!   (or replaced by an already-fetched copy, see below).
//! * Links: only http(s) and mailto; `rel="noopener noreferrer nofollow"`.
//!   Clicks never navigate the viewer: the shell intercepts them.
//! * Images: only `data:image/{png,jpeg,gif,webp}` (inline `cid:` parts are
//!   resolved to data URLs before sanitizing). Remote images are blocked
//!   and counted, unless the caller passes a map of remote URL -> data URL
//!   produced by `remote::fetch_images` (user clicked "Load remote images").

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub struct Sanitized {
    pub html: String,
    /// Remote images (img + CSS) that were blocked.
    pub remote_images_blocked: usize,
    /// Remote images that were replaced by fetched data URLs.
    pub remote_images_loaded: usize,
    /// Every remote (http/https) image URL referenced by the message, so
    /// the caller can offer to fetch them.
    pub remote_urls: Vec<String>,
}

#[derive(Default)]
struct Stats {
    blocked: usize,
    loaded: usize,
    urls: BTreeSet<String>,
}

/// Sanitize with remote images blocked.
pub fn sanitize_html(input: &str) -> Sanitized {
    sanitize_html_with(input, &HashMap::new())
}

/// Sanitize; remote image URLs present in `remote` are replaced by the
/// given (already validated) `data:` URLs.
pub fn sanitize_html_with(input: &str, remote: &HashMap<String, String>) -> Sanitized {
    // ammonia's Builder::default() is 'static, so the filter owns its state.
    let stats = Arc::new(Mutex::new(Stats::default()));
    let remote_map = Arc::new(remote.clone());
    let (fstats, fremote) = (stats.clone(), remote_map.clone());
    let schemes: HashSet<&str> = ["http", "https", "mailto", "data"].into_iter().collect();
    let html = ammonia::Builder::default()
        .url_schemes(schemes)
        .link_rel(Some("noopener noreferrer nofollow"))
        .url_relative(ammonia::UrlRelative::Deny)
        .add_tags(["style", "font", "center"])
        .rm_clean_content_tags(["style"])
        .add_generic_attributes([
            "style",
            "class",
            "align",
            "valign",
            "bgcolor",
            "width",
            "height",
            "border",
            "cellpadding",
            "cellspacing",
            "dir",
            "color",
            "face",
            "size",
            "role",
        ])
        .attribute_filter(move |element, attribute, value| {
            let mut st = fstats.lock().unwrap_or_else(|p| p.into_inner());
            let remote = fremote.as_ref();
            match (element, attribute) {
                ("img", "src") => image_src(value, remote, &mut st),
                ("img", "srcset") => None,
                (_, "href") if value.trim_start().to_ascii_lowercase().starts_with("data:") => None,
                (_, "style") => Some(Cow::Owned(sanitize_css(value, remote, &mut st))),
                _ => Some(Cow::Borrowed(value)),
            }
        })
        .clean(input)
        .to_string();
    let mut st = std::mem::take(&mut *stats.lock().unwrap_or_else(|p| p.into_inner()));

    // `<style>` element contents are not seen by the attribute filter:
    // filter them here. ammonia emits `<style>` without attributes.
    let html = filter_style_blocks(&html, remote, &mut st);

    // Images whose source was dropped entirely (srcset-only, SVG, relative)
    // remain as `<img` without `src=`; count those too.
    let srcless = html
        .match_indices("<img")
        .filter(|(i, _)| {
            let tag = &html[*i..html[*i..].find('>').map_or(html.len(), |e| i + e)];
            !tag.contains(" src=")
        })
        .count();
    Sanitized {
        html,
        remote_images_blocked: srcless + st.blocked,
        remote_images_loaded: st.loaded,
        remote_urls: st.urls.into_iter().collect(),
    }
}

fn remote_url(v: &str) -> Option<String> {
    let t = v.trim();
    let lower = t.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(t.to_string())
    } else if lower.starts_with("//") {
        Some(format!("https:{t}"))
    } else {
        None
    }
}

fn image_src<'u>(
    value: &'u str,
    remote: &HashMap<String, String>,
    st: &mut Stats,
) -> Option<Cow<'u, str>> {
    if is_inline_image(value) {
        return Some(Cow::Borrowed(value));
    }
    if let Some(url) = remote_url(value) {
        st.urls.insert(url.clone());
        if let Some(data) = remote.get(&url) {
            st.loaded += 1;
            return Some(Cow::Owned(data.clone()));
        }
    }
    // Dropped: counted via the src-less <img> pass.
    None
}

pub fn is_inline_image(src: &str) -> bool {
    let s = src.trim_start().to_ascii_lowercase();
    // Raster formats only: SVG may carry script.
    [
        "data:image/png",
        "data:image/jpeg",
        "data:image/jpg",
        "data:image/gif",
        "data:image/webp",
    ]
    .iter()
    .any(|p| s.starts_with(p))
}

fn filter_style_blocks(html: &str, remote: &HashMap<String, String>, st: &mut Stats) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<style>") {
        let body_start = start + "<style>".len();
        let Some(len) = rest[body_start..].find("</style>") else {
            break;
        };
        out.push_str(&rest[..body_start]);
        out.push_str(&sanitize_css(
            &rest[body_start..body_start + len],
            remote,
            st,
        ));
        out.push_str("</style>");
        rest = &rest[body_start + len + "</style>".len()..];
    }
    out.push_str(rest);
    out
}

/// Filter CSS from a mail: removes constructs that can load resources or
/// execute code, and rewrites `url()`. The message CSP (`img-src data:`,
/// `default-src 'none'`) is the backstop for anything this misses, e.g.
/// CSS escape sequences.
fn sanitize_css(css: &str, remote: &HashMap<String, String>, st: &mut Stats) -> String {
    let lower = css.to_ascii_lowercase();
    if [
        "expression(",
        "-moz-binding",
        "behavior:",
        "behaviour:",
        "javascript:",
        "vbscript:",
    ]
    .iter()
    .any(|bad| lower.contains(bad))
    {
        return String::new();
    }
    // Drop @import rules (they fetch remote stylesheets).
    let mut css = css.to_string();
    while let Some(i) = css.to_ascii_lowercase().find("@import") {
        let end = css[i..].find(';').map_or(css.len(), |e| i + e + 1);
        css.replace_range(i..end, "");
    }
    // Rewrite url(...) tokens.
    let mut out = String::with_capacity(css.len());
    let mut rest = css.as_str();
    loop {
        let Some(i) = rest.to_ascii_lowercase().find("url(") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..i]);
        let after = &rest[i + 4..];
        let end = after.find(')').unwrap_or(after.len());
        let inner = after[..end]
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .trim();
        if is_inline_image(inner) {
            out.push_str(&format!("url(\"{}\")", inner.replace('"', "%22")));
        } else if let Some(url) = remote_url(inner) {
            st.urls.insert(url.clone());
            if let Some(data) = remote.get(&url) {
                st.loaded += 1;
                out.push_str(&format!("url(\"{data}\")"));
            } else {
                st.blocked += 1;
                out.push_str("none");
            }
        } else {
            out.push_str("none");
        }
        rest = after.get(end + 1..).unwrap_or("");
    }
    out
}

/// Content-Security-Policy embedded in every message document. Even if the
/// sanitizer missed something, nothing can execute or fetch from the network.
pub const MESSAGE_CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src 'none'; form-action 'none'; base-uri 'none'";

/// Wrap sanitized HTML into a complete document for the viewer iframe.
/// Mail CSS comes after our defaults, so newsletters keep their own look;
/// in dark mode, messages that set their own colors keep them (like most
/// mail clients) and plain ones follow the theme.
pub fn wrap_document(sanitized_body: &str, dark: bool) -> String {
    let (bg, fg, link) = if dark {
        ("#1c1b22", "#e8e6f0", "#a995ff")
    } else {
        ("#ffffff", "#1c1b22", "#6d4aff")
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{MESSAGE_CSP}\">\
<style>html{{background:{bg};color:{fg}}}body{{font:14px/1.5 system-ui,sans-serif;margin:16px;word-wrap:break-word}}\
a{{color:{link};cursor:pointer}}img{{max-width:100%;height:auto}}table{{max-width:100%}}</style>\
</head><body>{sanitized_body}</body></html>"
    )
}
