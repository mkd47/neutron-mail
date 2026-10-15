#![allow(dead_code)]
//! Shared helpers for integration tests that need the local test mail
//! server (testserver/start.sh). They are skipped unless NEUTRON_IT=1.

use neutron_core::model::{AccountConfig, Security};

pub const PASSWORD: &str = "testpass";

pub fn enabled() -> bool {
    if std::env::var("NEUTRON_IT").as_deref() == Ok("1") {
        true
    } else {
        eprintln!("skipping: set NEUTRON_IT=1 and run testserver/start.sh to enable");
        false
    }
}

pub fn enabled_quiet() -> bool {
    std::env::var("NEUTRON_IT").as_deref() == Ok("1")
}

pub fn ca_pem() -> String {
    let base = std::env::var("NM_TEST_BASE").unwrap_or_else(|_| "/workspace/testmail".into());
    std::fs::read_to_string(format!("{base}/certs/ca.pem"))
        .expect("test CA cert (run testserver/start.sh)")
}

pub fn config(imap_security: Security) -> AccountConfig {
    AccountConfig {
        email: "kaan@localhost".into(),
        display_name: "Kaan Durmus".into(),
        username: "kaan@localhost".into(),
        imap_host: "localhost".into(),
        imap_port: if imap_security == Security::Tls {
            10993
        } else {
            10143
        },
        imap_security,
        smtp_host: "localhost".into(),
        smtp_port: 10587,
        smtp_security: Security::Starttls,
        auth: Default::default(),
        // Offline tests (no server, e.g. CI) only build configs; they never connect.
        extra_ca_pem: if enabled_quiet() {
            Some(ca_pem())
        } else {
            None
        },
    }
}

pub fn unique(tag: &str) -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{tag}-{n}")
}

pub fn fixture(subject: &str, html: bool, attachment: bool) -> Vec<u8> {
    let boundary = "nm-boundary-1";
    let mut s = format!(
        "From: Fixture Sender <fixture@example.test>\r\nTo: kaan@localhost\r\nSubject: {subject}\r\n\
Date: Wed, 14 Oct 2026 10:00:00 +0000\r\nMessage-ID: <{subject}@example.test>\r\nMIME-Version: 1.0\r\n"
    );
    if !html && !attachment {
        s.push_str(
            "Content-Type: text/plain; charset=utf-8\r\n\r\nPlain body about zeppelins.\r\n",
        );
        return s.into_bytes();
    }
    s.push_str(&format!(
        "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"
    ));
    s.push_str(&format!("--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nText part about zeppelins.\r\n"));
    if html {
        s.push_str(&format!("--{boundary}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
<p onclick=\"x()\">HTML <b>zeppelins</b></p><img src=\"https://tracker.example.test/p.gif\"><script>alert(1)</script>\r\n"));
    }
    if attachment {
        s.push_str(&format!("--{boundary}\r\nContent-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQKJSBmYWtlCg==\r\n"));
    }
    s.push_str(&format!("--{boundary}--\r\n"));
    s.into_bytes()
}
