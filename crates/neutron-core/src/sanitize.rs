//! HTML sanitizing for message display (defence layer 1 of 3; the viewer
//! also uses a script-less sandboxed iframe and a restrictive CSP).
//!
//! Policy:
//! * ammonia allow-list: no scripts, styles, forms, iframes, event handlers,
//!   `style` attributes (blocks CSS `url()` loads) or unknown tags.
//! * Links: only http(s) and mailto; `rel="noopener noreferrer nofollow"`.
//! * Images: only `data:image/*` (inline parts are resolved to data URLs
//!   before sanitizing). Every other image source, i.e. remote content and
//!   tracking pixels, is removed and counted.

use std::borrow::Cow;
use std::collections::HashSet;

pub struct Sanitized {
    pub html: String,
    pub remote_images_blocked: usize,
}

pub fn sanitize_html(input: &str) -> Sanitized {
    let schemes: HashSet<&str> = ["http", "https", "mailto", "data"].into_iter().collect();
    let html = ammonia::Builder::default()
        .url_schemes(schemes)
        .link_rel(Some("noopener noreferrer nofollow"))
        .url_relative(ammonia::UrlRelative::Deny)
        .attribute_filter(
            move |element, attribute, value| match (element, attribute) {
                ("img", "src") => is_inline_image(value).then_some(Cow::Borrowed(value)),
                ("img", "srcset") => None,
                (_, "href") if value.trim_start().to_ascii_lowercase().starts_with("data:") => None,
                _ => Some(Cow::Borrowed(value)),
            },
        )
        .clean(input)
        .to_string();

    // ammonia emits normalized markup, so every image whose source was
    // dropped (remote, scheme-relative, srcset-only, SVG) is an `<img` tag
    // without a `src=` attribute.
    let remote_images_blocked = html
        .match_indices("<img")
        .filter(|(i, _)| {
            let tag = &html[*i..html[*i..].find('>').map_or(html.len(), |e| i + e)];
            !tag.contains(" src=")
        })
        .count();
    Sanitized {
        html,
        remote_images_blocked,
    }
}

fn is_inline_image(src: &str) -> bool {
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

/// Content-Security-Policy embedded in every message document. Even if the
/// sanitizer missed something, nothing can execute or fetch from the network.
pub const MESSAGE_CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; font-src 'none'; form-action 'none'; base-uri 'none'";

/// Wrap sanitized HTML into a complete document for the viewer iframe.
pub fn wrap_document(sanitized_body: &str, dark: bool) -> String {
    let (bg, fg, link) = if dark {
        ("#1c1b22", "#e8e6f0", "#a995ff")
    } else {
        ("#ffffff", "#1c1b22", "#6d4aff")
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{MESSAGE_CSP}\">\
<style>body{{font:14px/1.5 system-ui,sans-serif;margin:16px;background:{bg};color:{fg};word-wrap:break-word}}\
a{{color:{link}}}img{{max-width:100%;height:auto}}table{{max-width:100%}}</style>\
</head><body>{sanitized_body}</body></html>"
    )
}
