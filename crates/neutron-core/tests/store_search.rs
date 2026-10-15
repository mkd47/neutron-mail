//! Offline tests (no server needed): cache, FTS search, UIDVALIDITY.
use neutron_core::model::{AccountConfig, Security};
use neutron_core::store::{FolderSyncState, NewMessage, Store};

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
        auth: Default::default(),
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
fn legacy_sync_checkpoint_migrates_and_round_trips_window_cutoff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let s = Store::open_plaintext(&path).unwrap();
    let a = s.add_account(&cfg()).unwrap();
    let f = s.upsert_folder(a, "INBOX", Some("inbox")).unwrap();
    let legacy = FolderSyncState {
        uidnext: 8,
        highestmodseq: Some(20),
        exists: 7,
        window_days: 30,
        window_cutoff: None,
    };
    s.set_folder_sync_state(f, Some(legacy)).unwrap();
    drop(s);
    // Reproduce the actual pre-migration schema, not just a NULL field.
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("ALTER TABLE folders DROP COLUMN window_cutoff")
        .unwrap();
    drop(db);
    let s = Store::open_plaintext(&path).unwrap();
    assert_eq!(s.folder_sync_state(f).unwrap(), Some(legacy));
    let current = FolderSyncState {
        window_cutoff: Some(1_789_430_400),
        ..legacy
    };
    s.set_folder_sync_state(f, Some(current)).unwrap();
    assert_eq!(s.folder_sync_state(f).unwrap(), Some(current));
    s.set_folder_sync_state(f, None).unwrap();
    assert_eq!(s.folder_sync_state(f).unwrap(), None);
}

#[test]
fn duplicate_insert_is_idempotent_and_removal_updates_index() {
    let (s, a, f) = setup();
    let original = s.ids_for_uids(f, &[1]).unwrap()[0];
    let original_raw = s.raw(original).unwrap();
    let inserted = s
        .insert_message(
            f,
            &NewMessage {
                uid: 1,
                seen: false,
                flagged: false,
                raw: &raw("Invoice 4711", "x@y.z", "dup"),
            },
        )
        .unwrap();
    assert_eq!(inserted, original);
    assert_eq!(s.raw(original).unwrap(), original_raw);
    assert_eq!(s.messages(f, 0, 50).unwrap().len(), 2);
    assert_eq!(s.search(a, "billing", 10).unwrap().len(), 1);
    assert_eq!(s.search(a, "pay", 10).unwrap().len(), 1);
    assert!(s.search(a, "dup", 10).unwrap().is_empty());
    assert!(s.address_history(a, "x@y.z", 10).unwrap().is_empty());
    assert_eq!(s.remove_uids(f, &[1]).unwrap(), 1);
    assert!(s.search(a, "invoice", 10).unwrap().is_empty());
}

#[test]
fn duplicate_batch_does_not_inflate_address_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let s = Store::open_plaintext(&path).unwrap();
    let a = s.add_account(&cfg()).unwrap();
    let f = s.upsert_folder(a, "INBOX", Some("inbox")).unwrap();
    let raw = raw("Hello", "sender@example.test", "Original body");
    let message = NewMessage {
        uid: 1,
        seen: false,
        flagged: false,
        raw: &raw,
    };
    s.insert_message(f, &message).unwrap();
    s.insert_messages(f, &[message]).unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let uses: i64 = db
        .query_row(
            "SELECT uses FROM addresses WHERE account_id = ?1 AND email = ?2",
            rusqlite::params![a, "sender@example.test"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(uses, 1);
}

#[test]
fn rendering_preserves_details_and_substitutes_only_supplied_images() {
    let (s, _, f) = setup();
    let raw = b"From: sender@example.test\r\nTo: me@example.test\r\nSubject: HTML mail\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>Hello</p><img src=\"https://example.test/image.png\"><script>alert(1)</script>";
    let id = s
        .insert_message(
            f,
            &NewMessage {
                uid: 3,
                seen: false,
                flagged: true,
                raw,
            },
        )
        .unwrap();
    let plain = s.message_detail(id).unwrap();
    let (blocked, urls) = s.render_message(id, &Default::default()).unwrap();
    assert_eq!(plain, blocked);
    assert_eq!(urls, vec!["https://example.test/image.png"]);
    assert_eq!(blocked.remote_images_blocked, 1);
    let remote = std::collections::HashMap::from([(
        urls[0].clone(),
        "data:image/png;base64,iVBORw0KGgo=".to_string(),
    )]);
    let (loaded, _) = s.render_message(id, &remote).unwrap();
    assert_eq!(loaded.summary.id, id);
    assert_eq!(loaded.to, plain.to);
    assert_eq!(loaded.text, plain.text);
    assert_eq!(loaded.remote_images_blocked, 0);
    assert_eq!(loaded.remote_images_loaded, 1);
    let html = loaded.html.unwrap();
    assert!(html.contains("data:image/png;base64,iVBORw0KGgo="));
    assert!(!html.contains("<script"));
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
    let key = neutron_core::secrets::CacheKey::random().unwrap();
    let _s = Store::open_encrypted(&p, &key).unwrap();
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn extra_ca_accepts_trimmed_and_crlf_pem_and_errors_concisely() {
    // A real (throwaway) self-signed certificate is not needed: use the test
    // CA if the test server has been started, otherwise skip quietly.
    let base = std::env::var("NM_TEST_BASE").unwrap_or_else(|_| "/workspace/testmail".into());
    if let Ok(pem) = std::fs::read_to_string(format!("{base}/certs/ca.pem")) {
        assert_eq!(
            neutron_core::tls::parse_extra_ca(pem.trim()).unwrap().len(),
            1
        );
        let crlf = pem.replace('\n', "\r\n");
        assert_eq!(neutron_core::tls::parse_extra_ca(&crlf).unwrap().len(), 1);
    }
    let broken = "-----BEGIN CERTIFICATE-----\nAAAA\n";
    let err = neutron_core::tls::parse_extra_ca(broken)
        .unwrap_err()
        .to_string();
    assert!(err.len() < 120, "error should be concise: {err}");
}

#[test]
fn auth_method_round_trips() {
    let s = Store::open_in_memory().unwrap();
    let mut c = cfg();
    let pw = s.add_account(&c).unwrap();
    c.auth = neutron_core::model::AuthMethod::OauthMicrosoft;
    let oa = s.add_account(&c).unwrap();
    assert_eq!(s.account(pw).unwrap().config.auth, Default::default());
    assert_eq!(
        s.account(oa).unwrap().config.auth,
        neutron_core::model::AuthMethod::OauthMicrosoft
    );
    c.auth = neutron_core::model::AuthMethod::OauthGoogle;
    s.update_account(oa, &c).unwrap();
    assert!(s.account(oa).unwrap().config.auth.is_oauth());
    // Older configs without the field still deserialize (password).
    let json = serde_json::to_value(cfg()).unwrap();
    let mut obj = json.as_object().unwrap().clone();
    obj.remove("auth");
    let back: AccountConfig = serde_json::from_value(obj.into()).unwrap();
    assert_eq!(back.auth, Default::default());
}

#[test]
fn decoding_upgrade_repairs_cached_text_and_search_without_redownloading() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old-cache.sqlite3");
    let store = Store::open_plaintext(&path).unwrap();
    let account = store.add_account(&cfg()).unwrap();
    let fid = store
        .upsert_folder(account, "INBOX", Some("inbox"))
        .unwrap();
    let text = "春秋航空采购通知";
    let (body, _, _) = encoding_rs::GBK.encode(text);
    let raw = format!("From: =?GBK?B?{}?= <sender@example.test>\r\nSubject: =?GBK?B?{}?=\r\nContent-Type: text/plain; charset=GBK\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",neutron_core::parse::base64(&body),neutron_core::parse::base64(&body),neutron_core::parse::base64(&body)).into_bytes();
    let id = store
        .insert_message(
            fid,
            &NewMessage {
                uid: 42,
                seen: true,
                flagged: true,
                raw: &raw,
            },
        )
        .unwrap();
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("DELETE FROM cache_metadata WHERE key='text-decoder'", [])
        .unwrap();
    db.execute(
        "UPDATE messages SET subject='���', preview='���', from_name='���' WHERE id=?1",
        [id],
    )
    .unwrap();
    db.execute("DELETE FROM messages_fts WHERE rowid=?1", [id])
        .unwrap();
    let uses: i64 = db
        .query_row(
            "SELECT uses FROM addresses WHERE account_id=?1 AND email='sender@example.test'",
            [account],
            |r| r.get(0),
        )
        .unwrap();
    drop(db);
    let store = Store::open_plaintext(&path).unwrap();
    let row = store.messages(fid, 0, 10).unwrap().remove(0);
    assert_eq!(row.id, id);
    assert_eq!(row.subject, text);
    assert_eq!(row.preview, text);
    assert!(row.seen && row.flagged);
    assert_eq!(store.raw(id).unwrap(), raw);
    assert_eq!(store.search(account, text, 10).unwrap().len(), 1);
    drop(store);
    let db = rusqlite::Connection::open(&path).unwrap();
    let after: i64 = db
        .query_row(
            "SELECT uses FROM addresses WHERE account_id=?1 AND email='sender@example.test'",
            [account],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(uses, after);
    drop(db);
    // A second open does not repeat/duplicate the migration.
    let store = Store::open_plaintext(&path).unwrap();
    assert_eq!(store.messages(fid, 0, 10).unwrap().len(), 1);
}
