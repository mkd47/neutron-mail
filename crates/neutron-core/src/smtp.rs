//! Sending via lettre (SMTP submission, TLS or mandatory STARTTLS).

use std::time::Duration;

use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::model::{AccountConfig, OutgoingMessage, Security};
use crate::{tls, Error, Result};

fn mailbox(s: &str) -> Result<Mailbox> {
    s.trim()
        .parse()
        .map_err(|e| Error::Invalid(format!("bad address {s:?}: {e}")))
}

/// Build the RFC 5322 message (plain text body).
pub fn build(cfg: &AccountConfig, m: &OutgoingMessage) -> Result<Message> {
    if m.to.iter().all(|t| t.trim().is_empty()) {
        return Err(Error::Invalid("at least one recipient is required".into()));
    }
    let from = Mailbox::new(
        Some(cfg.display_name.clone()).filter(|n| !n.is_empty()),
        cfg.email
            .parse()
            .map_err(|e| Error::Invalid(format!("bad sender address: {e}")))?,
    );
    let mut b = Message::builder()
        .from(from)
        .subject(m.subject.clone())
        .user_agent("Neutron Mail".into());
    for t in m.to.iter().filter(|t| !t.trim().is_empty()) {
        b = b.to(mailbox(t)?);
    }
    for c in m.cc.iter().filter(|t| !t.trim().is_empty()) {
        b = b.cc(mailbox(c)?);
    }
    b.header(ContentType::TEXT_PLAIN)
        .body(m.body.clone())
        .map_err(|e| Error::Invalid(e.to_string()))
}

fn transport(cfg: &AccountConfig, password: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let mut params = TlsParameters::builder(cfg.smtp_host.clone());
    if let Some(pem) = cfg.extra_ca_pem.as_deref().filter(|p| !p.trim().is_empty()) {
        tls::parse_extra_ca(pem)?; // validate early with a clear error
        params = params.add_root_certificate(Certificate::from_pem(pem.as_bytes())?);
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
            .timeout(Some(Duration::from_secs(30)))
            .credentials(Credentials::new(cfg.username.clone(), password.to_owned()))
            .build(),
    )
}

/// Send and return the formatted message bytes (for APPEND to Sent).
pub async fn send(cfg: &AccountConfig, password: &str, m: &OutgoingMessage) -> Result<Vec<u8>> {
    let msg = build(cfg, m)?;
    let raw = msg.formatted();
    transport(cfg, password)?.send(msg).await?;
    Ok(raw)
}
