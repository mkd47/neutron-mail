//! Audit O13: IMAP commands time out against a stalled server.
//! Separate test binary because it lowers the global command timeout.

mod common;

use std::time::{Duration, Instant};

use neutron_core::imap;
use neutron_core::model::Security;

/// A server that accepts LOGIN and then never answers (O13): the next
/// command must fail with a timeout instead of hanging forever.
#[tokio::test]
async fn stalled_server_times_out() {
    if !common::enabled() {
        return;
    }
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let base = std::env::var("NM_TEST_BASE").unwrap_or_else(|_| "/workspace/testmail".into());
    let certs: Vec<_> =
        rustls_pki_types::CertificateDer::pem_file_iter(format!("{base}/certs/cert.pem"))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
    use rustls_pki_types::pem::PemObject;
    let key =
        rustls_pki_types::PrivateKeyDer::from_pem_file(format!("{base}/certs/key.pem")).unwrap();
    let sc = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(sc));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(tcp).await.unwrap();
        let (r, mut w) = tokio::io::split(tls);
        let mut r = BufReader::new(r);
        w.write_all(b"* OK [CAPABILITY IMAP4rev1] stall\r\n")
            .await
            .unwrap();
        let mut line = String::new();
        r.read_line(&mut line).await.unwrap();
        let tag = line.split(' ').next().unwrap().to_string();
        w.write_all(format!("{tag} OK logged in\r\n").as_bytes())
            .await
            .unwrap();
        // connect() asks for capabilities (UTF8=ACCEPT probe); answer that.
        line.clear();
        r.read_line(&mut line).await.unwrap();
        let tag = line.split(' ').next().unwrap().to_string();
        w.write_all(format!("* CAPABILITY IMAP4rev1\r\n{tag} OK done\r\n").as_bytes())
            .await
            .unwrap();
        // Read and ignore everything else, never respond.
        loop {
            line.clear();
            if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
        }
    });
    let mut cfg = common::config(Security::Tls);
    cfg.imap_port = port;
    imap::set_command_timeout(Duration::from_secs(2));
    let mut s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    let t = Instant::now();
    let r = imap::select_state(&mut s, "INBOX", false).await;
    imap::set_command_timeout(imap::COMMAND_TIMEOUT);
    assert!(
        matches!(r, Err(neutron_core::Error::Timeout)),
        "{:?}",
        r.err()
    );
    assert!(t.elapsed() < Duration::from_secs(10));
}
