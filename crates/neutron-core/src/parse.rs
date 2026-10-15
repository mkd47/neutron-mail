//! MIME parsing with mail-parser.

use mail_parser::{Encoding, MessageParser, MimeHeaders, PartType};
use std::borrow::Cow;

use crate::attachments::{is_risky, safe_filename};
use crate::model::{AttachmentInfo, OutgoingAttachment};
use crate::sanitize::{sanitize_html, sanitize_html_with, Sanitized};

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
    /// (name, email) of From, To, Cc and Reply-To, for address history.
    pub addresses: Vec<(String, String)>,
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
    let normalized = legacy_headers(raw);
    let Some(mut msg) = MessageParser::default().parse(normalized.as_ref()) else {
        return ParsedMessage {
            subject: "(unparseable message)".into(),
            ..Default::default()
        };
    };

    // Missing/UTF-8 labels on legacy Chinese mail: recover only when UTF-8
    // is actually invalid, strict GB18030 succeeds and produces CJK text.
    // Other explicit charsets remain authoritative; never touch binary parts.
    for part in &mut msg.parts {
        if !matches!(part.body, PartType::Text(_) | PartType::Html(_)) {
            continue;
        }
        let charset = part.content_type().and_then(|ct| ct.attribute("charset"));
        if charset.is_some_and(|c| {
            !matches!(
                c.to_ascii_lowercase().as_str(),
                "utf-8" | "utf8" | "us-ascii" | "ascii"
            )
        }) {
            continue;
        }
        let Some(bytes) = normalized.get(part.offset_body as usize..part.offset_end as usize)
        else {
            continue;
        };
        let decoded: Cow<'_, [u8]> = match part.encoding {
            Encoding::None => bytes.into(),
            Encoding::Base64 => match mail_parser::decoders::base64::base64_decode(bytes) {
                Some(b) => b.into(),
                None => continue,
            },
            Encoding::QuotedPrintable => {
                match mail_parser::decoders::quoted_printable::quoted_printable_decode(bytes) {
                    Some(b) => b.into(),
                    None => continue,
                }
            }
        };
        if let Some(text) = legacy_chinese(&decoded) {
            part.body = if matches!(part.body, PartType::Html(_)) {
                PartType::Html(text.into())
            } else {
                PartType::Text(text.into())
            };
        }
    }

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
            // Raster formats only (SVG can carry script).
            if !matches!(sub.as_str(), "png" | "jpeg" | "jpg" | "gif" | "webp") {
                continue;
            }
            let url = format!("data:image/{sub};base64,{}", base64(data));
            let cid = cid.trim_matches(|c| c == '<' || c == '>');
            *html = html.replace(&format!("cid:{cid}"), &url);
        }
    }

    let attachments = listed_attachments(&msg)
        .enumerate()
        .map(|(index, p)| {
            let name = safe_filename(p.attachment_name().unwrap_or("attachment"));
            let mime = mime_of(p);
            AttachmentInfo {
                index,
                risky: is_risky(&name, &mime, p.contents()),
                name,
                mime,
                size: p.contents().len(),
            }
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
        addresses: [msg.from(), msg.to(), msg.cc(), msg.reply_to()]
            .into_iter()
            .flatten()
            .flat_map(|a| a.iter())
            .filter_map(|a| {
                let e = a.address()?.trim();
                (e.contains('@') && e.len() <= 254)
                    .then(|| (a.name().unwrap_or("").trim().to_string(), e.to_string()))
            })
            .collect(),
    }
}

fn legacy_chinese(bytes: &[u8]) -> Option<String> {
    if std::str::from_utf8(bytes).is_ok() {
        return None;
    }
    let text = encoding_rs::GB18030.decode_without_bom_handling_and_without_replacement(bytes)?;
    text.chars()
        .any(|c| matches!(c, '\u{3400}'..='\u{9fff}' | '\u{20000}'..='\u{2ffff}'))
        .then(|| text.into_owned())
}

/// Normalize only malformed raw 8-bit *display* headers. The stored raw mail
/// and MIME body/attachment bytes remain unchanged, including nested messages.
fn legacy_headers(raw: &[u8]) -> Cow<'_, [u8]> {
    let mut result = Vec::new();
    let mut offset = 0;
    let mut display_header = false;
    let mut changed = false;
    for line in raw.split_inclusive(|b| *b == b'\n') {
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        if !matches!(line.first(), Some(b' ' | b'\t')) {
            display_header = line.iter().position(|b| *b == b':').is_some_and(|n| {
                [b"subject".as_slice(), b"from", b"to", b"cc", b"reply-to"]
                    .iter()
                    .any(|h| line[..n].eq_ignore_ascii_case(h))
            });
        }
        if let Some(text) = display_header.then(|| legacy_chinese(line)).flatten() {
            result.extend_from_slice(text.as_bytes());
            changed = true;
        } else {
            result.extend_from_slice(line);
        }
        offset += line.len();
    }
    if changed {
        result.extend_from_slice(&raw[offset..]);
        Cow::Owned(result)
    } else {
        Cow::Borrowed(raw)
    }
}

/// Attachments shown to the user: real attachments plus named parts.
/// Unnamed inline (cid) images are rendered in the body instead.
fn listed_attachments<'a>(
    msg: &'a mail_parser::Message<'a>,
) -> impl Iterator<Item = &'a mail_parser::MessagePart<'a>> {
    msg.attachments()
        .filter(|p| p.content_id().is_none() || p.attachment_name().is_some())
}

fn mime_of(p: &mail_parser::MessagePart<'_>) -> String {
    p.content_type()
        .map(|c| {
            format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")).to_ascii_lowercase()
        })
        .unwrap_or_else(|| "application/octet-stream".into())
}

/// The first `text/calendar` (or `application/ics`) part of a message, as
/// text, if it is within the size limit.
pub fn calendar_part(raw: &[u8]) -> Option<String> {
    let msg = MessageParser::default().parse(raw)?;
    msg.parts.iter().find_map(|p| {
        let ct = p.content_type()?;
        let sub = ct.subtype().unwrap_or("").to_ascii_lowercase();
        let is_cal = (ct.ctype().eq_ignore_ascii_case("text") && sub == "calendar")
            || (ct.ctype().eq_ignore_ascii_case("application") && sub == "ics");
        if !is_cal || p.contents().len() > crate::ical::MAX_ICS_BYTES {
            return None;
        }
        Some(String::from_utf8_lossy(p.contents()).into_owned())
    })
}

/// Content of attachment `index` (same numbering as `AttachmentInfo`).
pub fn attachment_content(raw: &[u8], index: usize) -> Option<OutgoingAttachment> {
    let msg = MessageParser::default().parse(raw)?;
    let p = listed_attachments(&msg).nth(index)?;
    Some(OutgoingAttachment {
        name: safe_filename(p.attachment_name().unwrap_or("attachment")),
        mime: mime_of(p),
        data: p.contents().to_vec(),
    })
}

/// Sanitize the HTML part (if any) of a parsed message.
pub fn sanitized_html(p: &ParsedMessage) -> Option<Sanitized> {
    p.html_raw.as_deref().map(sanitize_html)
}

/// Sanitize with fetched remote images substituted.
pub fn sanitized_html_with(
    p: &ParsedMessage,
    remote: &std::collections::HashMap<String, String>,
) -> Option<Sanitized> {
    p.html_raw.as_deref().map(|h| sanitize_html_with(h, remote))
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

pub fn base64(data: &[u8]) -> String {
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
