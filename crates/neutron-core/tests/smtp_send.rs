mod common;

use neutron_core::model::{OutgoingMessage, Security};
use neutron_core::store::Store;
use neutron_core::{imap, smtp, sync};

#[tokio::test]
async fn send_then_receive_and_search() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let subject = common::unique("send-test");
    let raw = smtp::send(
        &cfg,
        common::PASSWORD,
        &OutgoingMessage {
            to: vec!["Kaan <kaan@localhost>".into()],
            cc: vec![],
            subject: subject.clone(),
            body: "Hello from the integration test about quokkas.".into(),
        },
    )
    .await
    .expect("send");
    assert!(String::from_utf8_lossy(&raw).contains(&subject));

    // Copy to Sent like the app does.
    let mut s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    imap::append(&mut s, "Sent", &raw, true).await.unwrap();

    // The test SMTP server delivers into INBOX; sync and search for it.
    let store = Store::open_in_memory().unwrap();
    let acc = store.add_account(&cfg).unwrap();
    let inbox = store.upsert_folder(acc, "INBOX", Some("inbox")).unwrap();
    sync::sync_folder(&store, inbox, "INBOX", &mut s)
        .await
        .unwrap();
    let hits = store.search(acc, &subject, 10).unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    let d = store.message_detail(hits[0].summary.id).unwrap();
    assert!(d.text.contains("quokkas"));
    assert_eq!(d.summary.from_addr, "kaan@localhost");
    s.logout().await.ok();
}

#[tokio::test]
async fn smtp_rejects_bad_credentials() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let r = smtp::send(
        &cfg,
        "wrong",
        &OutgoingMessage {
            to: vec!["kaan@localhost".into()],
            cc: vec![],
            subject: "x".into(),
            body: "x".into(),
        },
    )
    .await;
    assert!(r.is_err());
}

#[tokio::test]
async fn smtp_rejects_untrusted_certificate() {
    if !common::enabled() {
        return;
    }
    let mut cfg = common::config(Security::Tls);
    cfg.extra_ca_pem = None;
    let r = smtp::send(
        &cfg,
        common::PASSWORD,
        &OutgoingMessage {
            to: vec!["kaan@localhost".into()],
            cc: vec![],
            subject: "x".into(),
            body: "x".into(),
        },
    )
    .await;
    assert!(r.is_err());
}

#[test]
fn build_rejects_header_injection_and_empty_recipients() {
    let cfg = common::config(Security::Tls);
    let empty = smtp::build(
        &cfg,
        &OutgoingMessage {
            to: vec![],
            cc: vec![],
            subject: "s".into(),
            body: "b".into(),
        },
    );
    assert!(empty.is_err());
    let inj = smtp::build(
        &cfg,
        &OutgoingMessage {
            to: vec!["a@b.c\r\nBcc: victim@evil.test".into()],
            cc: vec![],
            subject: "s".into(),
            body: "b".into(),
        },
    );
    assert!(inj.is_err());
    let m = smtp::build(
        &cfg,
        &OutgoingMessage {
            to: vec!["a@b.c".into()],
            cc: vec![],
            subject: "hi\r\nBcc: victim@evil.test".into(),
            body: "b".into(),
        },
    )
    .unwrap();
    let raw = String::from_utf8(m.formatted()).unwrap();
    assert!(!raw.contains("\r\nBcc: victim"), "{raw}");
}
