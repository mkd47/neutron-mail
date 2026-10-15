//! Live tests against a real evolution-data-server on the session bus.
//! Enabled with NEUTRON_EDS=1 (run inside `dbus-run-session`, see
//! testserver/README or docs/AUDIT.md). Skipped otherwise.

use neutron_core::gnome::{parse_vcard, Eds};

fn enabled() -> bool {
    let on = std::env::var("NEUTRON_EDS").as_deref() == Ok("1");
    if !on {
        eprintln!("skipping: set NEUTRON_EDS=1 inside a D-Bus session with EDS");
    }
    on
}

#[tokio::test]
async fn eds_add_and_search_any_field() {
    if !enabled() {
        return;
    }
    let eds = Eds::connect().await.expect("connect to EDS");
    assert!(eds.book_count() >= 1);
    let tag = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let email = format!("zeynep.{tag}@example.test");
    let vcard = format!("BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Zeynep Testkişi{tag}\r\nN:Testkişi{tag};Zeynep;;;\r\nEMAIL;TYPE=WORK:{email}\r\nEND:VCARD\r\n");
    let uid = eds.add_contact_vcard(&vcard).await.expect("create contact");
    assert!(!uid.is_empty());

    // Partial name (case-insensitive) and partial email both match.
    let by_name = eds.search(&format!("testkişi{tag}"), 10).await.unwrap();
    assert!(by_name.iter().any(|h| h.email == email), "{by_name:?}");
    let by_mail = eds
        .search(&format!("zeynep.{}", &tag[..6]), 10)
        .await
        .unwrap();
    assert!(by_mail.iter().any(|h| h.email == email), "{by_mail:?}");
    // Quotes in the query must not break the S-expression.
    assert!(eds.search("\") (or", 5).await.is_ok());
    assert!(eds
        .search("no-such-contact-xyz-123", 5)
        .await
        .unwrap()
        .is_empty());
    let c = parse_vcard(&vcard);
    assert_eq!(c.emails, vec![email]);
}

#[tokio::test]
async fn eds_missing_degrades_to_error_not_hang() {
    if std::env::var("NEUTRON_EDS_ABSENT").as_deref() != Ok("1") {
        return;
    }
    let t = std::time::Instant::now();
    assert!(Eds::connect().await.is_err());
    assert!(t.elapsed().as_secs() < 15);
}
