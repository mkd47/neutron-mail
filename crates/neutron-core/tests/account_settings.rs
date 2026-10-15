mod common;

use neutron_core::compose::{self, Mode};
use neutron_core::imap;
use neutron_core::model::{ComposeDraft, Security};
use neutron_core::store::{AccountSettings, Store};
use neutron_core::sync;

fn store_with_account() -> (Store, i64) {
    let store = Store::open_in_memory().unwrap();
    let id = store.add_account(&common::config(Security::Tls)).unwrap();
    (store, id)
}

#[test]
fn settings_default_roundtrip_and_sanitize() {
    let (store, id) = store_with_account();
    assert_eq!(
        store.account_settings(id).unwrap(),
        AccountSettings::default()
    );
    let saved = store
        .set_account_settings(
            id,
            &AccountSettings {
                signature_html: "<p onclick=\"x()\"><b>Kaan</b></p><script>alert(1)</script><img src=\"https://t.example/p.gif\">".into(),
                sync_days: 90,
            },
        )
        .unwrap();
    assert!(saved.signature_html.contains("<b>Kaan</b>"));
    for bad in ["script", "onclick", "https://t.example"] {
        assert!(
            !saved.signature_html.contains(bad),
            "{bad} survived: {}",
            saved.signature_html
        );
    }
    assert_eq!(store.account_settings(id).unwrap(), saved);
}

#[test]
fn settings_rejects_bad_window_unknown_account_and_huge_signature() {
    let (store, id) = store_with_account();
    let s = |d| AccountSettings {
        signature_html: String::new(),
        sync_days: d,
    };
    assert!(store.set_account_settings(id, &s(17)).is_err());
    assert!(store.set_account_settings(id + 99, &s(30)).is_err());
    let huge = AccountSettings {
        signature_html: "a".repeat(300 * 1024),
        sync_days: 0,
    };
    assert!(store.set_account_settings(id, &huge).is_err());
}

#[test]
fn update_account_and_cascade_on_delete() {
    let (store, id) = store_with_account();
    let mut cfg = store.account(id).unwrap().config;
    cfg.display_name = "Kaan D.".into();
    cfg.smtp_port = 2525;
    store.update_account(id, &cfg).unwrap();
    assert_eq!(store.account(id).unwrap().config, cfg);
    assert!(store.update_account(id + 99, &cfg).is_err());
    cfg.imap_host = String::new();
    assert!(store.update_account(id, &cfg).is_err(), "validation runs");

    store.trust_image_sender(id, "b@example.test").unwrap();
    store.trust_image_sender(id, "a@example.test").unwrap();
    assert_eq!(
        store.image_senders(id).unwrap(),
        ["a@example.test", "b@example.test"]
    );
    store.untrust_image_sender(id, "a@example.test").unwrap();
    assert_eq!(store.image_senders(id).unwrap(), ["b@example.test"]);

    store
        .set_account_settings(
            id,
            &AccountSettings {
                signature_html: "x".into(),
                sync_days: 30,
            },
        )
        .unwrap();
    store.delete_account(id).unwrap();
    let id2 = store.add_account(&common::config(Security::Tls)).unwrap();
    assert_eq!(
        store.account_settings(id2).unwrap(),
        AccountSettings::default()
    );
    assert!(store.image_senders(id2).unwrap().is_empty());
}

#[test]
fn signature_goes_between_cursor_and_quote() {
    let sig = "<p><b>Kaan</b> · Neutron</p>";
    let d = compose::with_signature(ComposeDraft::default(), sig);
    let html = d.html.unwrap();
    assert!(html.starts_with("<p><br></p><div>-- </div><div>"), "{html}");
    assert!(html.contains("<b>Kaan</b>"));
    assert!(
        d.body.starts_with("\n\n-- \nKaan · Neutron"),
        "{:?}",
        d.body
    );

    let raw = common::fixture("plain-one", false, false);
    let reply = compose::with_signature(
        compose::prepare(&raw, 1, Mode::Reply, "kaan@localhost"),
        sig,
    );
    let html = reply.html.unwrap();
    let (s, q) = (html.find("-- ").unwrap(), html.find("<blockquote").unwrap());
    assert!(html.starts_with("<p><br></p>") && s < q, "{html}");
    let (s, q) = (
        reply.body.find("\n-- \n").unwrap(),
        reply.body.find("wrote:").unwrap(),
    );
    assert!(s < q, "{:?}", reply.body);

    // The delimiter keeps its trailing space when HTML is flattened for text/plain.
    let text = neutron_core::sanitize::html_to_text(
        &compose::with_signature(ComposeDraft::default(), sig)
            .html
            .unwrap(),
    );
    assert!(
        text.starts_with("-- \nKaan") || text.contains("\n-- \nKaan"),
        "{text:?}"
    );

    let none = compose::with_signature(ComposeDraft::default(), "<script>x</script>");
    assert_eq!(
        none,
        ComposeDraft::default(),
        "empty after sanitizing = no signature"
    );
}

#[tokio::test]
async fn sync_window_skips_and_prunes_old_mail() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let folder = common::unique("WIN");
    let mut s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    s.create(&folder).await.unwrap();
    imap::append(
        &mut s,
        &folder,
        &common::fixture("recent-one", false, false),
        false,
    )
    .await
    .unwrap();
    let old = String::from_utf8(common::fixture("old-one", false, false))
        .unwrap()
        .replace(
            "Wed, 14 Oct 2026 10:00:00 +0000",
            "Wed, 01 Jan 2020 10:00:00 +0000",
        );
    s.append(
        &folder,
        None,
        Some("\"01-Jan-2020 10:00:00 +0000\""),
        old.as_bytes(),
    )
    .await
    .unwrap();

    let store = Store::open_in_memory().unwrap();
    let acc = store.add_account(&cfg).unwrap();
    let fid = store.upsert_folder(acc, &folder, None).unwrap();

    // Fresh cache with a 30-day window: the 2020 message is never downloaded.
    let r = sync::sync_folder_window(&store, fid, &folder, &mut s, 30)
        .await
        .unwrap();
    assert_eq!(r, (1, 0));
    // Window off: everything; window back on: the old one is pruned locally only.
    assert_eq!(
        sync::sync_folder(&store, fid, &folder, &mut s)
            .await
            .unwrap(),
        (1, 0)
    );
    assert_eq!(store.messages(fid, 0, 50).unwrap().len(), 2);
    let r = sync::sync_folder_window(&store, fid, &folder, &mut s, 30)
        .await
        .unwrap();
    assert_eq!(r, (0, 1));
    let left = store.messages(fid, 0, 50).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].subject, "recent-one");
    assert_eq!(
        imap::select_state(&mut s, &folder, false)
            .await
            .unwrap()
            .exists,
        2,
        "server untouched"
    );
    s.delete(&folder).await.ok();
    s.logout().await.ok();
}
