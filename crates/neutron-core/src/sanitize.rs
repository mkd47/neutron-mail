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

/// Max size of composed HTML (including inline images as data URLs).
pub const MAX_COMPOSE_HTML_BYTES: usize = 20 * 1024 * 1024;

/// CSS properties kept in composed mail. The composer is part of the app
/// page (not a sandboxed iframe), so layout properties that could draw over
/// the app UI (`position`, `z-index`, `transform`, …) are not allowed, and
/// neither are background images.
const COMPOSE_CSS_PROPS: &[&str] = &[
    "color",
    "background-color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "font-variant",
    "text-align",
    "text-decoration",
    "text-indent",
    "text-transform",
    "letter-spacing",
    "line-height",
    "white-space",
    "vertical-align",
    "direction",
    "margin",
    "margin-top",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "padding",
    "padding-top",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "border",
    "border-left",
    "border-right",
    "border-top",
    "border-bottom",
    "border-color",
    "border-style",
    "border-width",
    "border-collapse",
    "border-spacing",
    "border-radius",
    "list-style-type",
    "width",
    "max-width",
    "height",
];

fn compose_css(css: &str) -> String {
    let mut st = Stats::default();
    let base = sanitize_css(css, &HashMap::new(), &mut st);
    base.split(';')
        .filter_map(|decl| {
            let (prop, value) = decl.split_once(':')?;
            let prop = prop.trim().to_ascii_lowercase();
            let value = value.trim();
            let ok = COMPOSE_CSS_PROPS.contains(&prop.as_str())
                && !value.to_ascii_lowercase().contains("url(")
                && !value.is_empty();
            ok.then(|| format!("{prop}: {value}"))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Sanitize HTML written in (or pasted into, or quoted by) the composer.
/// Stricter than the viewer policy because the composer is not sandboxed:
/// no `<style>` blocks, no classes or ids, a CSS property allow-list, no
/// remote images (inline raster `data:` images only), links http(s)/mailto.
/// Used on paste, for quoting, and again (authoritatively) before sending.
pub fn sanitize_compose_html(input: &str) -> String {
    let schemes: HashSet<&str> = ["http", "https", "mailto", "data"].into_iter().collect();
    let mut tag_attrs = HashMap::new();
    tag_attrs.insert("blockquote", ["type"].into_iter().collect::<HashSet<_>>());
    tag_attrs.insert("a", ["href", "title"].into_iter().collect());
    tag_attrs.insert(
        "img",
        ["src", "alt", "width", "height"].into_iter().collect(),
    );
    tag_attrs.insert(
        "td",
        ["colspan", "rowspan", "align", "valign"]
            .into_iter()
            .collect(),
    );
    tag_attrs.insert(
        "th",
        ["colspan", "rowspan", "align", "valign"]
            .into_iter()
            .collect(),
    );
    tag_attrs.insert("ol", ["start"].into_iter().collect());
    let out = ammonia::Builder::default()
        .url_schemes(schemes)
        .link_rel(Some("noopener noreferrer nofollow"))
        .url_relative(ammonia::UrlRelative::Deny)
        .add_tags(["font"])
        .tag_attributes(tag_attrs)
        .generic_attributes(["style", "dir", "align"].into_iter().collect())
        .attribute_filter(|element, attribute, value| match (element, attribute) {
            ("img", "src") => is_inline_image(value).then_some(Cow::Borrowed(value)),
            (_, "href") if value.trim_start().to_ascii_lowercase().starts_with("data:") => None,
            (_, "style") => {
                let c = compose_css(value);
                (!c.is_empty()).then_some(Cow::Owned(c))
            }
            _ => Some(Cow::Borrowed(value)),
        })
        .clean(input)
        .to_string();
    // Images whose source was dropped (remote, SVG…) would be empty boxes.
    remove_srcless_imgs(&out)
}

fn remove_srcless_imgs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<img") {
        out.push_str(&rest[..i]);
        let end = rest[i..].find('>').map_or(rest.len(), |e| i + e + 1);
        let tag = &rest[i..end];
        if tag.contains(" src=") {
            out.push_str(tag);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Escape text for inclusion in HTML.
pub fn escape_html(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// Plain text as HTML paragraphs (line breaks kept).
pub fn text_to_html(s: &str) -> String {
    escape_html(s.trim_end())
        .replace("\r\n", "\n")
        .replace('\n', "<br>")
}

/// Plain-text rendering of compose HTML for the text/plain alternative:
/// block elements become line breaks, `<blockquote>` lines get `> `
/// prefixes, links keep their URL, list items get bullets.
/// Expects sanitizer output (well-formed, lower-case tags, escaped text).
pub fn html_to_text(html: &str) -> String {
    let mut lines: Vec<(usize, String)> = vec![(0, String::new())];
    let mut depth = 0usize;
    let mut href: Vec<Option<String>> = Vec::new();
    let mut ol: Vec<Option<u32>> = Vec::new();
    let mut rest = html;
    let newline = |lines: &mut Vec<(usize, String)>, depth: usize| {
        if lines.last().is_some_and(|l| !l.1.is_empty()) {
            lines.push((depth, String::new()));
        } else if let Some(l) = lines.last_mut() {
            l.0 = depth;
        }
    };
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('<') {
            let end = stripped.find('>').map_or(rest.len(), |e| e + 2);
            let tag = &rest[1..end.saturating_sub(1).max(1)];
            rest = &rest[end.min(rest.len())..];
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            match (name.as_str(), closing) {
                ("br", _) => lines.push((depth, String::new())),
                ("blockquote", false) => {
                    depth += 1;
                    newline(&mut lines, depth);
                }
                ("blockquote", true) => {
                    depth = depth.saturating_sub(1);
                    lines.push((depth, String::new()));
                }
                ("ol", false) => ol.push(Some(1)),
                ("ul", false) => ol.push(None),
                ("ol" | "ul", true) => {
                    ol.pop();
                    newline(&mut lines, depth);
                }
                ("li", false) => {
                    newline(&mut lines, depth);
                    let indent = "  ".repeat(ol.len().saturating_sub(1));
                    let bullet = match ol.last_mut() {
                        Some(Some(n)) => {
                            let b = format!("{n}. ");
                            *n += 1;
                            b
                        }
                        _ => "• ".to_string(),
                    };
                    if let Some(l) = lines.last_mut() {
                        l.1.push_str(&indent);
                        l.1.push_str(&bullet);
                    }
                }
                ("a", false) => {
                    let h = tag
                        .split("href=\"")
                        .nth(1)
                        .and_then(|v| v.split('"').next())
                        .map(unescape);
                    href.push(h);
                }
                ("a", true) => {
                    if let Some(Some(h)) = href.pop() {
                        let h = h.strip_prefix("mailto:").unwrap_or(&h).to_string();
                        if let Some(l) = lines.last_mut() {
                            if !l.1.trim_end().ends_with(&h) {
                                l.1.push_str(&format!(" <{h}>"));
                            }
                        }
                    }
                }
                (
                    "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr" | "pre" | "table"
                    | "hr",
                    _,
                ) => newline(&mut lines, depth),
                ("td" | "th", true) => {
                    if let Some(l) = lines.last_mut() {
                        l.1.push('\t');
                    }
                }
                _ => {}
            }
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let text = unescape(&rest[..end]).replace(['\n', '\r'], " ");
            if let Some(l) = lines.last_mut() {
                if l.1.is_empty() {
                    l.0 = depth;
                    l.1.push_str(text.trim_start());
                } else {
                    l.1.push_str(&text);
                }
            }
            rest = &rest[end..];
        }
    }
    // Collapse runs of blank lines, keep quote prefixes.
    let mut out = String::new();
    let mut blank = 0;
    for (d, l) in &lines {
        let l = l.trim_end();
        if l.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        let prefix = ">".repeat(*d);
        if *d > 0 {
            out.push_str(&prefix);
            if !l.is_empty() {
                out.push(' ');
            }
        }
        out.push_str(l);
        out.push('\n');
    }
    out.trim().to_string() + "\n"
}

fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        o.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';').filter(|&n| n <= 10) else {
            o.push('&');
            rest = &tail[1..];
            continue;
        };
        let ent = &tail[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                o.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                o.push('&');
                rest = &tail[1..];
            }
        }
    }
    o.push_str(rest);
    o
}
