//! MIME parsing with mail-parser.

use mail_parser::{Encoding, MessageParser, MimeHeaders, PartType};
use std::borrow::Cow;

use crate::attachments::{is_risky, safe_filename};
use crate::model::{AttachmentInfo, MailAddress, OutgoingAttachment};
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
    pub to_list: Vec<MailAddress>,
    pub cc_list: Vec<MailAddress>,
    pub date: i64,
    pub text: String,
    /// Raw (unsanitized) HTML body if the message has one.
    pub html_raw: Option<String>,
    pub preview: String,
    pub attachments: Vec<AttachmentInfo>,
    /// (name, email) of From, To, Cc and Reply-To, for address history.
    pub addresses: Vec<(String, String)>,
    /// Conversation key (see [`thread_key`]), None without any Message-ID.
    pub thread_key: Option<String>,
}

/// Normalized Message-ID for comparisons: no brackets, trimmed, lowercase.
fn norm_id(s: &str) -> Option<String> {
    let s = s
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    (!s.is_empty() && s.len() <= 512).then(|| s.to_ascii_lowercase())
}

fn header_ids(v: &mail_parser::HeaderValue<'_>) -> Vec<String> {
    match v {
        mail_parser::HeaderValue::Text(t) => vec![t.to_string()],
        mail_parser::HeaderValue::TextList(l) => l.iter().map(|t| t.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Conversation key: the thread's root Message-ID, i.e. the first entry of
/// References, else In-Reply-To, else the message's own Message-ID. Every
/// reply in a well-formed thread shares the root's own ID, so they group
/// together whichever arrives first.
pub fn thread_key(msg: &mail_parser::Message<'_>) -> Option<String> {
    header_ids(msg.references())
        .iter()
        .find_map(|r| norm_id(r))
        .or_else(|| {
            header_ids(msg.in_reply_to())
                .iter()
                .find_map(|r| norm_id(r))
        })
        .or_else(|| msg.message_id().and_then(norm_id))
}

/// [`thread_key`] from raw mail, parsing only the headers.
pub fn thread_key_of(raw: &[u8]) -> Option<String> {
    MessageParser::default()
        .parse_headers(raw)
        .and_then(|m| thread_key(&m))
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

fn addr_vec(a: Option<&mail_parser::Address<'_>>) -> Vec<MailAddress> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                let email = x.address()?.trim();
                (!email.is_empty()).then(|| MailAddress {
                    name: x.name().unwrap_or_default().trim().to_string(),
                    email: email.to_string(),
                })
            })
            .collect()
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

    // Resolve cid: references to data: URLs for inline images, remembering
    // which ones the body actually shows (so they aren't listed again as
    // attachments, e.g. logos in a signature).
    let mut shown_cids: Vec<String> = Vec::new();
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
            if replace_cid(html, cid, &url) {
                shown_cids.push(cid.to_string());
            }
        }
    }

    let attachments = listed_attachments(&msg)
        .enumerate()
        .map(|(index, p)| {
            let name = safe_filename(p.attachment_name().unwrap_or("attachment"));
            let mime = mime_of(p);
            let inline = p
                .content_id()
                .map(|c| c.trim_matches(|c| c == '<' || c == '>'))
                .is_some_and(|c| shown_cids.iter().any(|s| s == c));
            AttachmentInfo {
                index,
                inline,
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
        thread_key: thread_key(&msg),
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
        to_list: addr_vec(msg.to()),
        cc_list: addr_vec(msg.cc()),
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

/// Every real attachment (not images already shown in the body), parsed
/// once, for "Save all". Names are already made safe.
pub fn saveable_attachments(raw: &[u8]) -> Vec<OutgoingAttachment> {
    let info = parse(raw).attachments;
    let normalized = legacy_headers(raw);
    let Some(msg) = MessageParser::default().parse(normalized.as_ref()) else {
        return Vec::new();
    };
    listed_attachments(&msg)
        .zip(info.iter())
        .filter(|(_, a)| !a.inline)
        .map(|(p, a)| OutgoingAttachment {
            name: a.name.clone(),
            mime: a.mime.clone(),
            data: p.contents().to_vec(),
        })
        .collect()
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

/// Replace whole `cid:<cid>` references (not `cid:<cid>0`, a different
/// image) with `url`; true if any was found.
fn replace_cid(html: &mut String, cid: &str, url: &str) -> bool {
    let reference = format!("cid:{cid}");
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    let mut found = false;
    while let Some(at) = rest.find(&reference) {
        let end = at + reference.len();
        let whole = rest[end..]
            .chars()
            .next()
            .is_none_or(|c| matches!(c, '"' | '\'' | ')' | '>' | ';' | '\\') || c.is_whitespace());
        out.push_str(&rest[..at]);
        out.push_str(if whole { url } else { &reference });
        found |= whole;
        rest = &rest[end..];
    }
    if found {
        out.push_str(rest);
        *html = out;
    }
    found
}

#[cfg(test)]
mod tests {
    #[test]
    fn cid_references_must_match_exactly() {
        let mut html =
            r#"<img src="cid:img1"><img src="cid:img10"><div style="background:url(cid:img1)">"#
                .to_string();
        assert!(super::replace_cid(&mut html, "img1", "data:x"));
        assert_eq!(
            html,
            r#"<img src="data:x"><img src="cid:img10"><div style="background:url(data:x)">"#
        );
        let mut only10 = r#"<img src="cid:img10">"#.to_string();
        assert!(!super::replace_cid(&mut only10, "img1", "data:x"));
        assert_eq!(only10, r#"<img src="cid:img10">"#);
    }

    #[test]
    fn images_shown_in_the_body_are_marked_inline_not_listed_as_attachments() {
        let png = super::base64(b"\x89PNG\r\n\x1a\n0000");
        let raw = format!(
            "From: a@example.test\r\nSubject: s\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"M\"\r\n\r\n\
--M\r\nContent-Type: multipart/related; boundary=\"R\"\r\n\r\n\
--R\r\nContent-Type: text/html\r\n\r\n<p>Hi</p><img src=\"cid:logo@x\">\r\n\
--R\r\nContent-Type: image/png; name=\"image.png\"\r\nContent-ID: <logo@x>\r\nContent-Disposition: inline; filename=\"image.png\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{png}\r\n\
--R\r\nContent-Type: image/png; name=\"unused.png\"\r\nContent-ID: <other@x>\r\nContent-Disposition: inline; filename=\"unused.png\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{png}\r\n\
--R--\r\n\
--M\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{png}\r\n\
--M--\r\n"
        );
        let p = super::parse(raw.as_bytes());
        let by_name = |n: &str| p.attachments.iter().find(|a| a.name == n).unwrap();
        assert!(by_name("image.png").inline, "logo shown in the body");
        assert!(
            !by_name("unused.png").inline,
            "not referenced: a real attachment"
        );
        assert!(!by_name("report.pdf").inline);
        assert!(p.html_raw.unwrap().contains("data:image/png;base64,"));
        // Indexes stay stable for save/open/forward.
        let idx: Vec<usize> = p.attachments.iter().map(|a| a.index).collect();
        assert_eq!(idx, (0..p.attachments.len()).collect::<Vec<_>>());
    }

    #[test]
    fn recipients_are_listed_one_per_address() {
        let raw = b"From: A <a@example.test>\r\nTo: \"Doe, Jane\" <jane@example.test>, bob@example.test\r\nCc: Team: x@example.test, y@example.test;\r\nSubject: s\r\n\r\nbody";
        let p = super::parse(raw);
        let emails =
            |v: &[crate::model::MailAddress]| v.iter().map(|a| a.email.clone()).collect::<Vec<_>>();
        assert_eq!(
            emails(&p.to_list),
            ["jane@example.test", "bob@example.test"]
        );
        assert_eq!(p.to_list[0].name, "Doe, Jane");
        assert_eq!(p.to_list[1].name, "");
        assert_eq!(emails(&p.cc_list), ["x@example.test", "y@example.test"]);
    }

    #[test]
    fn html_svg_and_xhtml_attachments_are_marked_risky() {
        let png = super::base64(b"%PDF-1.4");
        let html = super::base64(b"<html><script>alert(1)</script></html>");
        let svg = super::base64(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>");
        let raw = format!(
            "From: a@example.test\r\nSubject: s\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"M\"\r\n\r\n\
--M\r\nContent-Type: text/plain\r\n\r\nhi\r\n\
--M\r\nContent-Type: text/html; charset=utf-8; name=\"page.html\"\r\n\
Content-Disposition: attachment; filename=\"page.html\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{html}\r\n\
--M\r\nContent-Type: image/svg+xml; name=\"logo.svg\"\r\n\
Content-Disposition: attachment; filename=\"logo.svg\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{svg}\r\n\
--M\r\nContent-Type: application/xhtml+xml\r\n\
Content-Disposition: attachment; filename=\"note.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{html}\r\n\
--M\r\nContent-Type: application/pdf; name=\"invoice.pdf\"\r\n\
Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n{png}\r\n\
--M--\r\n"
        );
        let p = super::parse(raw.as_bytes());
        let risky = |n: &str| p.attachments.iter().find(|a| a.name == n).unwrap().risky;
        assert!(risky("page.html"));
        assert!(risky("logo.svg"));
        assert!(
            risky("note.bin"),
            "xhtml mime is active even without the extension"
        );
        assert!(!risky("invoice.pdf"));
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
