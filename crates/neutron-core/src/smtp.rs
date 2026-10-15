//! Message building (mail-builder) and sending (lettre SMTP submission,
//! TLS or mandatory STARTTLS).
//!
//! Bcc recipients only go into the SMTP envelope; they are never written to
//! the headers of the sent message (only to drafts, so a resumed draft
//! keeps them).

use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use lettre::address::Envelope;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use mail_builder::headers::address::Address;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::text::Text;
use mail_builder::mime::MimePart;
use mail_builder::MessageBuilder;

use crate::attachments::{MAX_ATTACHMENTS, MAX_TOTAL_ATTACHMENT_BYTES};
use crate::model::{AccountConfig, OutgoingAttachment, OutgoingMessage, Security};
use crate::{tls, Error, Result};

fn mailbox(s: &str) -> Result<Mailbox> {
    let s = s.trim();
    if s.contains(['\r', '\n']) {
        return Err(Error::Invalid(format!("bad address {s:?}")));
    }
    s.parse()
        .map_err(|e| Error::Invalid(format!("bad address {s:?}: {e}")))
}

fn mailboxes(list: &[String]) -> Result<Vec<Mailbox>> {
    list.iter()
        .filter(|t| !t.trim().is_empty())
        .map(|t| mailbox(t))
        .collect()
}

fn to_addr(m: &Mailbox) -> Address<'static> {
    Address::new_address(
        m.name.clone().filter(|n| !n.is_empty()),
        m.email.to_string(),
    )
}

fn addr_list(list: &[Mailbox]) -> Address<'static> {
    Address::new_list(list.iter().map(to_addr).collect())
}

fn clean_id(id: &str) -> Result<String> {
    let t = id
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    if t.is_empty() || t.contains(['\r', '\n', '<', '>', ' ']) {
        return Err(Error::Invalid(format!("bad message id {id:?}")));
    }
    Ok(t.to_string())
}

fn new_message_id(domain: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(nanos);
    format!("{:x}.{:016x}@{domain}", nanos, h.finish())
}

/// A built message: the bytes to transmit/append, and the SMTP envelope.
pub struct Built {
    pub raw: Vec<u8>,
    pub envelope: Envelope,
    pub message_id: String,
}

/// Build the RFC 5322 message. `for_draft` keeps a Bcc header (drafts only)
/// and allows an empty recipient list.
pub fn build(
    cfg: &AccountConfig,
    m: &OutgoingMessage,
    attachments: &[OutgoingAttachment],
    for_draft: bool,
) -> Result<Built> {
    let to = mailboxes(&m.to)?;
    let cc = mailboxes(&m.cc)?;
    let bcc = mailboxes(&m.bcc)?;
    if !for_draft && to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(Error::Invalid("at least one recipient is required".into()));
    }
    if m.subject.contains(['\r', '\n']) {
        return Err(Error::Invalid("the subject must be a single line".into()));
    }
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(Error::Invalid(format!(
            "at most {MAX_ATTACHMENTS} attachments"
        )));
    }
    let total: u64 = attachments.iter().map(|a| a.data.len() as u64).sum();
    if total > MAX_TOTAL_ATTACHMENT_BYTES {
        return Err(Error::Invalid(format!(
            "attachments are too large ({} MB, max {} MB)",
            total / 1024 / 1024,
            MAX_TOTAL_ATTACHMENT_BYTES / 1024 / 1024
        )));
    }
    let from: Mailbox = Mailbox::new(
        Some(cfg.display_name.clone()).filter(|n| !n.is_empty()),
        cfg.email
            .parse()
            .map_err(|e| Error::Invalid(format!("bad sender address: {e}")))?,
    );
    let domain = from.email.domain().to_string();
    let message_id = new_message_id(&domain);

    let mut b = MessageBuilder::new()
        .from(to_addr(&from))
        .subject(m.subject.clone())
        .message_id(message_id.clone())
        .date(chrono::Utc::now().timestamp())
        .header("User-Agent", Text::new("Neutron Mail"));
    if !to.is_empty() {
        b = b.to(addr_list(&to));
    }
    if !cc.is_empty() {
        b = b.cc(addr_list(&cc));
    }
    if for_draft && !bcc.is_empty() {
        b = b.bcc(addr_list(&bcc));
    }
    if let Some(parent) = m.in_reply_to.as_deref().filter(|s| !s.trim().is_empty()) {
        b = b.in_reply_to(clean_id(parent)?);
    }
    let refs: Vec<String> = m
        .references
        .iter()
        .filter(|r| !r.trim().is_empty())
        .map(|r| clean_id(r))
        .collect::<Result<_>>()?;
    if !refs.is_empty() {
        b = b.references(refs);
    }
    b = b.body(mime_tree(m, attachments, &domain)?);
    let raw = b
        .write_to_vec()
        .map_err(|e| Error::Invalid(format!("cannot build message: {e}")))?;

    let rcpts = to
        .iter()
        .chain(&cc)
        .chain(&bcc)
        .map(|mb| mb.email.clone())
        .collect();
    let envelope = Envelope::new(Some(from.email.clone()), rcpts)
        .map_err(|e| Error::Invalid(format!("envelope: {e}")));
    let envelope = match envelope {
        Ok(e) => e,
        // Drafts may have no recipients yet; give them a dummy envelope
        // (never used for sending).
        Err(_) if for_draft => Envelope::new(Some(from.email.clone()), vec![from.email.clone()])
            .map_err(|e| Error::Invalid(format!("envelope: {e}")))?,
        Err(e) => return Err(e),
    };
    Ok(Built {
        raw,
        envelope,
        message_id,
    })
}

fn transport(cfg: &AccountConfig, password: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let mut params = TlsParameters::builder(cfg.smtp_host.clone());
    if let Some(pem) = cfg.extra_ca_pem.as_deref().filter(|p| !p.trim().is_empty()) {
        let certs = tls::parse_extra_ca(pem)?; // validates with a clear error
        for c in certs {
            params = params.add_root_certificate(Certificate::from_der(c.to_vec())?);
        }
    }
    let params = params.build_rustls()?;
    let tls = match cfg.smtp_security {
        Security::Tls => Tls::Wrapper(params),
        Security::Starttls => Tls::Required(params),
    };
    Ok(
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(cfg.smtp_host.clone())
            .port(cfg.smtp_port)
            .tls(tls)
            .timeout(Some(Duration::from_secs(60)))
            .credentials(Credentials::new(cfg.username.clone(), password.to_owned()))
            .build(),
    )
}

/// Build and send; returns the transmitted bytes (for APPEND to Sent).
pub async fn send(
    cfg: &AccountConfig,
    password: &str,
    m: &OutgoingMessage,
    attachments: &[OutgoingAttachment],
) -> Result<Vec<u8>> {
    let built = build(cfg, m, attachments, false)?;
    transport(cfg, password)?
        .send_raw(&built.envelope, &built.raw)
        .await?;
    Ok(built.raw)
}

/// An iMIP message (RFC 6047): multipart/alternative with a human-readable
/// text part and the iTIP object as `text/calendar; method=…`.
pub fn build_imip(
    cfg: &AccountConfig,
    to: &str,
    subject: &str,
    text: &str,
    ics: &str,
    method: &str,
    in_reply_to: Option<&str>,
) -> Result<Built> {
    let to = mailbox(to)?;
    if subject.contains(['\r', '\n']) || !method.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(Error::Invalid("bad iMIP subject or method".into()));
    }
    let from: Mailbox = Mailbox::new(
        Some(cfg.display_name.clone()).filter(|n| !n.is_empty()),
        cfg.email
            .parse()
            .map_err(|e| Error::Invalid(format!("bad sender address: {e}")))?,
    );
    let domain = from.email.domain().to_string();
    let message_id = new_message_id(&domain);
    let mut b = MessageBuilder::new()
        .from(to_addr(&from))
        .to(to_addr(&to))
        .subject(subject.to_string())
        .message_id(message_id.clone())
        .date(chrono::Utc::now().timestamp())
        .header("User-Agent", Text::new("Neutron Mail"));
    if let Some(p) = in_reply_to.filter(|s| !s.trim().is_empty()) {
        if let Ok(id) = clean_id(p) {
            b = b.in_reply_to(id);
        }
    }
    let cal_ct = ContentType::new("text/calendar")
        .attribute("method", method.to_ascii_uppercase())
        .attribute("charset", "utf-8");
    b = b.body(MimePart::new(
        "multipart/alternative",
        vec![
            MimePart::new(utf8("text/plain"), text.replace("\r\n", "\n")),
            MimePart::new(cal_ct, ics.to_string()),
        ],
    ));
    let raw = b
        .write_to_vec()
        .map_err(|e| Error::Invalid(format!("cannot build message: {e}")))?;
    let envelope = Envelope::new(Some(from.email.clone()), vec![to.email.clone()])
        .map_err(|e| Error::Invalid(format!("envelope: {e}")))?;
    Ok(Built {
        raw,
        envelope,
        message_id,
    })
}

/// Send an already built message.
pub async fn send_built(cfg: &AccountConfig, password: &str, built: &Built) -> Result<()> {
    transport(cfg, password)?
        .send_raw(&built.envelope, &built.raw)
        .await?;
    Ok(())
}

fn utf8(ct: &'static str) -> ContentType<'static> {
    ContentType::new(ct).attribute("charset", "utf-8")
}

/// MIME structure (RFC 2046/2387):
/// mixed [ alternative [ text/plain, related [ text/html, inline images ] ], attachments ]
/// with each level present only when needed.
fn mime_tree(
    m: &OutgoingMessage,
    attachments: &[OutgoingAttachment],
    domain: &str,
) -> Result<MimePart<'static>> {
    let content = match m.html_body.as_deref().filter(|h| !h.trim().is_empty()) {
        None => MimePart::new(utf8("text/plain"), m.body.replace("\r\n", "\n")),
        Some(html) => {
            if html.len() > crate::sanitize::MAX_COMPOSE_HTML_BYTES {
                return Err(Error::Invalid("the message is too large".into()));
            }
            // Authoritative sanitizing: never trust what the editor produced.
            let clean = crate::sanitize::sanitize_compose_html(html);
            // The text alternative is derived from the same HTML, so both
            // parts always say the same thing.
            let text = MimePart::new(utf8("text/plain"), crate::sanitize::html_to_text(&clean));
            let (clean, images) = inline_images_to_cid(&clean, domain);
            let doc = MimePart::new(
                utf8("text/html"),
                format!(
                    "<!DOCTYPE html><html><head><meta charset=\"utf-8\"></head><body>{clean}</body></html>"
                ),
            );
            let html_part = if images.is_empty() {
                doc
            } else {
                let mut parts = vec![doc];
                parts.extend(
                    images
                        .into_iter()
                        .map(|i| MimePart::new(i.mime, i.data).inline().cid(i.cid)),
                );
                MimePart::new("multipart/related", parts)
            };
            MimePart::new("multipart/alternative", vec![text, html_part])
        }
    };
    if attachments.is_empty() {
        return Ok(content);
    }
    let mut parts = vec![content];
    for a in attachments {
        parts.push(
            MimePart::new(a.mime.clone(), a.data.clone())
                .attachment(crate::attachments::safe_filename(&a.name)),
        );
    }
    Ok(MimePart::new("multipart/mixed", parts))
}

struct InlineImage {
    mime: &'static str,
    cid: String,
    data: Vec<u8>,
}

/// Replace `src="data:image/…;base64,…"` images (pasted or quoted) with
/// `cid:` references to inline parts, which mail clients display far more
/// reliably than data URLs. Identical images share one part.
fn inline_images_to_cid(html: &str, domain: &str) -> (String, Vec<InlineImage>) {
    let mut out = String::with_capacity(html.len());
    let mut images: Vec<InlineImage> = Vec::new();
    let mut seen: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let mut rest = html;
    while let Some(i) = rest.find("src=\"data:image/") {
        let start = i + 5; // after src="
        let Some(len) = rest[start..].find('"') else {
            break;
        };
        let url = &rest[start..start + len];
        out.push_str(&rest[..start]);
        let cid = if let Some(c) = seen.get(url) {
            Some(c.clone())
        } else {
            decode_data_image(url).map(|(mime, data)| {
                let cid = format!("img{}.{}@{}", images.len() + 1, rand_hex(), domain);
                images.push(InlineImage {
                    mime,
                    cid: cid.clone(),
                    data,
                });
                seen.insert(url, cid.clone());
                cid
            })
        };
        match cid {
            Some(c) => out.push_str(&format!("cid:{c}")),
            None => out.push_str(url),
        }
        rest = &rest[start + len..];
    }
    out.push_str(rest);
    (out, images)
}

fn decode_data_image(url: &str) -> Option<(&'static str, Vec<u8>)> {
    let (meta, b64) = url.strip_prefix("data:")?.split_once(',')?;
    let meta = meta.to_ascii_lowercase();
    let mime = match meta.strip_suffix(";base64")? {
        "image/png" => "image/png",
        "image/jpeg" | "image/jpg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        _ => return None,
    };
    use base64::Engine as _;
    let b64: String = b64.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    Some((mime, data))
}

fn rand_hex() -> String {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}
