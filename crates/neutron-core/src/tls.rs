//! TLS configuration. Certificates are always validated against the Mozilla
//! root store (webpki-roots) plus an optional, explicitly configured extra CA.
//! There is deliberately no "accept invalid certificates" switch.

use std::sync::Arc;

use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::{pem::PemObject, CertificateDer, ServerName};
use tokio::net::TcpStream;
use tokio_rustls::{client::TlsStream, TlsConnector};

use crate::{Error, Result};

pub fn parse_extra_ca(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    // Pasted PEM often loses its final newline (or has CRLF), which the
    // parser rejects as a missing end marker. Normalize first. The error
    // deliberately omits the parser's detail, which echoes raw bytes.
    let mut normalized = pem.replace("\r\n", "\n").trim().to_owned();
    normalized.push('\n');
    let certs: Vec<_> = CertificateDer::pem_slice_iter(normalized.as_bytes())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| {
            Error::Tls("invalid CA certificate: expected a PEM \"BEGIN CERTIFICATE\" block".into())
        })?;
    if certs.is_empty() {
        return Err(Error::Tls("no certificate found in CA PEM".into()));
    }
    Ok(certs)
}

pub fn client_config(extra_ca_pem: Option<&str>) -> Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = extra_ca_pem.filter(|p| !p.trim().is_empty()) {
        for cert in parse_extra_ca(pem)? {
            roots
                .add(cert)
                .map_err(|e| Error::Tls(format!("cannot add CA certificate: {e}")))?;
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Tls(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

pub async fn wrap(
    stream: TcpStream,
    host: &str,
    extra_ca_pem: Option<&str>,
) -> Result<TlsStream<TcpStream>> {
    let connector = TlsConnector::from(client_config(extra_ca_pem)?);
    let name = ServerName::try_from(host.to_string())
        .map_err(|_| Error::Tls(format!("invalid server name {host}")))?;
    connector
        .connect(name, stream)
        .await
        .map_err(|e| Error::Tls(e.to_string()))
}
