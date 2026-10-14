//! MIME parsing with mail-parser.

use mail_parser::{MessageParser, MimeHeaders, PartType};

use crate::model::AttachmentInfo;
use crate::sanitize::{sanitize_html, Sanitized};

/// Max size of an inline (cid:) image converted to a data URL.
const MAX_INLINE_IMAGE: usize = 2 * 1024 * 1024;
const PREVIEW_CHARS: usize = 160;

#[derive(Debug, Clone, Default)]
pub struct ParsedMessage {
    pub message_id: String,
    pub subject: String,
    pub from_name: String,
    pub from_addr: String,
    pub to: String,
    pub cc: String,
    pub date: i64,
    pub text: String,
    /// Raw (unsanitized) HTML body if the message has one.
    pub html_raw: Option<String>,
    pub preview: String,
    pub attachments: Vec<AttachmentInfo>,
}

fn addr_list(a: Option<&mail_parser::Address<'_>>) -> String {
    a.map(|a| {
        a.iter()
            .map(|x| match (x.name(), x.address()) {
                (Some(n), Some(e)) => format!("{n} <{e}>"),
                (None, Some(e)) => e.to_string(),
                (Some(n), None) => n.to_string(),
                (None, None) => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_default()
}

pub fn parse(raw: &[u8]) -> ParsedMessage {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return ParsedMessage {
            subject: "(unparseable message)".into(),
            ..Default::default()
        };
    };

    let first_from = msg.from().and_then(|a| a.first());
    // Use real text/html parts only (mail-parser can synthesize HTML from
    // text parts; we render text-only messages as text instead).
    let html_parts: Vec<String> = msg
        .html_body
        .iter()
        .filter_map(|&i| match msg.parts.get(i as usize).map(|p| &p.body) {
            Some(PartType::Html(h)) => Some(h.to_string()),
            _ => None,
        })
        .collect();
    let text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    let mut html_raw = if html_parts.is_empty() {
        None
    } else {
        Some(html_parts.join("<hr>"))
    };

    // Resolve cid: references to data: URLs for inline images.
    if let Some(html) = html_raw.as_mut() {
        for part in msg.parts.iter() {
            let (Some(cid), Some(ct)) = (part.content_id(), part.content_type()) else {
                continue;
            };
            if !ct.ctype().eq_ignore_ascii_case("image") {
                continue;
            }
            let data = part.contents();
            if data.len() > MAX_INLINE_IMAGE {
                continue;
            }
            let sub = ct.subtype().unwrap_or("png").to_ascii_lowercase();
            let url = format!("data:image/{sub};base64,{}", base64(data));
            let cid = cid.trim_matches(|c| c == '<' || c == '>');
            *html = html.replace(&format!("cid:{cid}"), &url);
        }
    }

    let attachments = msg
        .attachments()
        .filter(|p| p.content_id().is_none() || p.attachment_name().is_some())
        .map(|p| AttachmentInfo {
            name: p.attachment_name().unwrap_or("attachment").to_string(),
            mime: p
                .content_type()
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: p.contents().len(),
        })
        .collect();

    let preview_src = if text.trim().is_empty() {
        html_raw.as_deref().map(strip_tags).unwrap_or_default()
    } else {
        text.clone()
    };
    let preview: String = preview_src
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(PREVIEW_CHARS)
        .collect();

    ParsedMessage {
        message_id: msg.message_id().unwrap_or_default().to_string(),
        subject: msg.subject().unwrap_or("(no subject)").to_string(),
        from_name: first_from
            .and_then(|a| a.name())
            .unwrap_or_default()
            .to_string(),
        from_addr: first_from
            .and_then(|a| a.address())
            .unwrap_or_default()
            .to_string(),
        to: addr_list(msg.to()),
        cc: addr_list(msg.cc()),
        date: msg.date().map(|d| d.to_timestamp()).unwrap_or(0),
        text,
        html_raw,
        preview,
        attachments,
    }
}

/// Sanitize the HTML part (if any) of a parsed message.
pub fn sanitized_html(p: &ParsedMessage) -> Option<Sanitized> {
    p.html_raw.as_deref().map(sanitize_html)
}

/// Very small tag stripper for previews / search indexing only (never used
/// for rendering).
pub fn strip_tags(html: &str) -> String {
    let cleaned = ammonia::Builder::empty().clean(html).to_string();
    cleaned
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_reference() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
