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
use mail_builder::headers::text::Text;
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
        .header("User-Agent", Text::new("Neutron Mail"))
        .text_body(m.body.replace("\r\n", "\n"));
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
    for a in attachments {
        b = b.attachment(
            a.mime.clone(),
            crate::attachments::safe_filename(&a.name),
            a.data.clone(),
        );
    }
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
