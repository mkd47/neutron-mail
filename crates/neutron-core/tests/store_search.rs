//! Offline tests (no server needed): cache, FTS search, UIDVALIDITY.
use neutron_core::model::{AccountConfig, Security};
use neutron_core::store::{NewMessage, Store};

fn cfg() -> AccountConfig {
    AccountConfig {
        email: "me@example.test".into(),
        display_name: "Me".into(),
        username: "me".into(),
        imap_host: "imap.example.test".into(),
        imap_port: 993,
        imap_security: Security::Tls,
        smtp_host: "smtp.example.test".into(),
        smtp_port: 587,
        smtp_security: Security::Starttls,
        extra_ca_pem: None,
    }
}

fn raw(subject: &str, from: &str, body: &str) -> Vec<u8> {
    format!("From: {from}\r\nTo: me@example.test\r\nSubject: {subject}\r\nDate: Wed, 14 Oct 2026 10:00:00 +0000\r\n\r\n{body}\r\n").into_bytes()
}

fn setup() -> (Store, i64, i64) {
    let s = Store::open_in_memory().unwrap();
    let a = s.add_account(&cfg()).unwrap();
    let f = s.upsert_folder(a, "INBOX", Some("inbox")).unwrap();
    s.insert_message(
        f,
        &NewMessage {
            uid: 1,
            seen: false,
            flagged: false,
            raw: &raw(
                "Invoice 4711",
                "Billing <billing@example.test>",
                "Please pay the invoice.",
            ),
        },
    )
    .unwrap();
    s.insert_message(
        f,
        &NewMessage {
            uid: 2,
            seen: true,
            flagged: false,
            raw: &raw(
                "Lunch Friday",
                "Ayşe <ayse@example.test>",
                "Ramen at noon? Café nearby.",
            ),
        },
    )
    .unwrap();
    (s, a, f)
}

#[test]
fn search_by_subject_body_sender_prefix_and_diacritics() {
    let (s, a, _) = setup();
    assert_eq!(s.search(a, "invoice", 10).unwrap().len(), 1);
    assert_eq!(s.search(a, "ram", 10).unwrap().len(), 1, "prefix");
    assert_eq!(
        s.search(a, "ayse", 10).unwrap().len(),
        1,
        "sender + diacritics"
    );
    assert_eq!(
        s.search(a, "cafe", 10).unwrap().len(),
        1,
        "diacritics folding"
    );
    assert_eq!(s.search(a, "nothingmatches", 10).unwrap().len(), 0);
}

#[test]
fn search_input_cannot_inject_fts_syntax() {
    let (s, a, _) = setup();
    for q in [
        "\"",
        "invoice OR",
        "NEAR(",
        "subject:x",
        "*",
        "a AND (b",
        "' OR 1=1 --",
        "\u{0}",
    ] {
        assert!(s.search(a, q, 10).is_ok(), "query {q:?} errored");
    }
}

#[test]
fn search_is_scoped_to_account() {
    let (s, _, _) = setup();
    let other = s.add_account(&cfg()).unwrap();
    assert!(s.search(other, "invoice", 10).unwrap().is_empty());
}

#[test]
fn uidvalidity_change_resets_folder() {
    let (s, _, f) = setup();
    assert!(!s.set_uidvalidity(f, 100).unwrap());
    assert_eq!(s.messages(f, 0, 50).unwrap().len(), 2);
    assert!(s.set_uidvalidity(f, 101).unwrap());
    assert!(s.messages(f, 0, 50).unwrap().is_empty());
}

#[test]
fn duplicate_insert_is_idempotent_and_removal_updates_index() {
    let (s, a, f) = setup();
    s.insert_message(
        f,
        &NewMessage {
            uid: 1,
            seen: false,
            flagged: false,
            raw: &raw("Invoice 4711", "x@y.z", "dup"),
        },
    )
    .unwrap();
    assert_eq!(s.messages(f, 0, 50).unwrap().len(), 2);
    assert_eq!(s.remove_uids(f, &[1]).unwrap(), 1);
    assert!(s.search(a, "invoice", 10).unwrap().is_empty());
}

#[test]
fn deleting_account_cascades() {
    let (s, a, f) = setup();
    s.delete_account(a).unwrap();
    assert!(s.messages(f, 0, 50).unwrap().is_empty());
    assert!(s.folders(a).unwrap().is_empty());
}

#[test]
fn account_validation() {
    let s = Store::open_in_memory().unwrap();
    let mut c = cfg();
    c.email = "nope".into();
    assert!(s.add_account(&c).is_err());
}

#[cfg(unix)]
#[test]
fn database_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cache.sqlite3");
    let _s = Store::open(&p).unwrap();
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
