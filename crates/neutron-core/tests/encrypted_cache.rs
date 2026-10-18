//! SQLCipher cache: encrypted at rest, plaintext migration, wrong/lost key.
use neutron_core::model::{AccountConfig, Security};
use neutron_core::secrets::CacheKey;
use neutron_core::store::{is_plaintext_sqlite, NewMessage, Store};
use std::path::Path;

const SECRET: &str = "Quarterly numbers: 4711 bananas";

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

fn fill(s: &Store) {
    let a = s.add_account(&cfg()).unwrap();
    let f = s.upsert_folder(a, "INBOX", Some("inbox")).unwrap();
    // Enough rows to spill into several pages and the WAL.
    for uid in 1..=200u32 {
        let raw = format!(
            "From: Billing <billing@example.test>\r\nTo: me@example.test\r\nSubject: Report {uid}\r\nDate: Wed, 14 Oct 2026 10:00:00 +0000\r\n\r\n{SECRET} #{uid}\r\n"
        );
        s.insert_message(
            f,
            &NewMessage {
                uid,
                seen: false,
                flagged: false,
                raw: raw.as_bytes(),
            },
        )
        .unwrap();
    }
}

/// No file next to the cache may contain the plaintext marker.
fn assert_no_plaintext(dir: &Path) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let bytes = std::fs::read(&p).unwrap();
        assert!(
            !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
            "plaintext found in {}",
            p.display()
        );
        assert!(
            !bytes.windows(15).any(|w| w == b"SQLite format 3"),
            "plain SQLite header in {}",
            p.display()
        );
    }
}

fn hits(s: &Store) -> usize {
    let a = s.accounts().unwrap()[0].id;
    s.search(a, "bananas", None, 500).unwrap().len()
}

#[test]
fn new_cache_is_encrypted_and_reopens_with_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cache.sqlite3");
    let key = CacheKey::random().unwrap();
    {
        let (s, r) = Store::open_encrypted(&p, &key).unwrap();
        assert!(!r.migrated && r.unreadable_moved_to.is_none());
        fill(&s);
    }
    assert!(!is_plaintext_sqlite(&p));
    assert_no_plaintext(dir.path());
    let same = CacheKey::from_hex(key.hex()).unwrap();
    let (s, _) = Store::open_encrypted(&p, &same).unwrap();
    assert_eq!(hits(&s), 200, "FTS works on the encrypted cache");
}

#[test]
fn plaintext_cache_is_migrated_and_scrubbed() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cache.sqlite3");
    {
        let s = Store::open_plaintext(&p).unwrap();
        fill(&s);
    } // leaves a WAL with data in it
    assert!(is_plaintext_sqlite(&p));
    let key = CacheKey::random().unwrap();
    let (s, r) = Store::open_encrypted(&p, &key).unwrap();
    assert!(r.migrated);
    assert_eq!(hits(&s), 200, "all data survives the migration");
    assert_eq!(s.accounts().unwrap().len(), 1);
    drop(s);
    assert_no_plaintext(dir.path());
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n.contains("plaintext-old") || n.contains("encrypting")),
        "temporary files removed: {names:?}"
    );
    // Opening again does not migrate twice.
    let (_s, r) = Store::open_encrypted(&p, &key).unwrap();
    assert!(!r.migrated);
}

/// Database written by rusqlite 0.37's bundled SQLCipher 4.6.1 (page size
/// 4096, HMAC-SHA512, raw key `x'<64 hex>'`). 0.40 bundles SQLCipher 4.14;
/// the 4.x on-disk format is unchanged, so the same key must open the file
/// instead of moving it aside and starting empty.
#[test]
fn sqlcipher_4_6_1_database_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cache.sqlite3");
    let src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sqlcipher/cache-4.6.1.sqlite3");
    std::fs::copy(&src, &p).unwrap();
    let salt = {
        let bytes = std::fs::read(&p).unwrap();
        bytes[..16].to_vec()
    };
    let key =
        CacheKey::from_hex("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff")
            .unwrap();
    assert_eq!(
        fixture_note(&p),
        "neutron-mail-v0.5.1-sqlcipher",
        "SQLCipher 4.14 must decrypt a 4.6.1 file with the same raw key"
    );
    let (s, r) = Store::open_encrypted(&p, &key).unwrap();
    assert!(!r.migrated);
    assert!(r.unreadable_moved_to.is_none());
    assert!(s.accounts().unwrap().is_empty());
    drop(s);
    let opened = std::fs::read(&p).unwrap();
    assert_eq!(
        &opened[..16],
        salt.as_slice(),
        "a failed key would replace the file with a new salt"
    );
    assert_eq!(fixture_note(&p), "neutron-mail-v0.5.1-sqlcipher");
    let (s, r) = Store::open_encrypted(&p, &key).unwrap();
    assert!(!r.migrated && r.unreadable_moved_to.is_none());
    assert!(s.accounts().unwrap().is_empty());
    assert_eq!(fixture_note(&p), "neutron-mail-v0.5.1-sqlcipher");
}

fn fixture_note(path: &Path) -> String {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(
        "PRAGMA key = \"x'00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff'\";",
    )
    .unwrap();
    conn.query_row("SELECT note FROM fixture", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn wrong_key_moves_cache_aside_instead_of_failing() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cache.sqlite3");
    {
        let (s, _) = Store::open_encrypted(&p, &CacheKey::random().unwrap()).unwrap();
        fill(&s);
    }
    let (s, r) = Store::open_encrypted(&p, &CacheKey::random().unwrap()).unwrap();
    let aside = r.unreadable_moved_to.expect("moved aside");
    assert!(aside.exists(), "old file kept for the user, not deleted");
    assert!(s.accounts().unwrap().is_empty(), "fresh cache");
}

#[test]
fn key_hex_is_validated() {
    assert!(CacheKey::from_hex("abc").is_none());
    assert!(CacheKey::from_hex(&"g".repeat(64)).is_none());
    assert!(CacheKey::from_hex(&"A".repeat(64)).is_some());
    let k = CacheKey::random().unwrap();
    assert_eq!(k.hex().len(), 64);
    assert_ne!(k.hex().as_str(), CacheKey::random().unwrap().hex().as_str());
    assert_eq!(
        format!("{k:?}"),
        "CacheKey(..)",
        "Debug never prints the key"
    );
}
