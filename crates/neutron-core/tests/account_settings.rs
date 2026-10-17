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
                identities: vec![],
                identity_signatures: Default::default(),
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
        identities: vec![],
        identity_signatures: Default::default(),
        signature_html: String::new(),
        sync_days: d,
    };
    assert!(store.set_account_settings(id, &s(17)).is_err());
    assert!(store.set_account_settings(id + 99, &s(30)).is_err());
    let huge = AccountSettings {
        identities: vec![],
        identity_signatures: Default::default(),
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
                identities: vec![],
                identity_signatures: Default::default(),
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

fn alias(name: &str, email: &str) -> neutron_core::model::MailAddress {
    neutron_core::model::MailAddress {
        name: name.into(),
        email: email.into(),
    }
}

#[test]
fn sender_aliases_are_validated_deduplicated_and_stored() {
    let (store, id) = store_with_account();
    let primary = store.account(id).unwrap().config.email;
    let settings = |identities| AccountSettings {
        identities,
        ..Default::default()
    };
    let saved = store
        .set_account_settings(
            id,
            &settings(vec![
                alias(" Sales ", " sales@example.test "),
                alias("dup", "SALES@example.test"),
                alias("me", &primary),
                alias("", ""),
                alias("", "info@example.test"),
            ]),
        )
        .unwrap();
    assert_eq!(
        saved.identities,
        vec![
            alias("Sales", "sales@example.test"),
            alias("", "info@example.test")
        ]
    );
    assert_eq!(store.account_settings(id).unwrap(), saved);
    for bad in [
        alias("x", "not an address"),
        alias("Evil\r\nBcc: a@b.test", "ok@example.test"),
    ] {
        assert!(store
            .set_account_settings(id, &settings(vec![bad]))
            .is_err());
    }
    let many: Vec<_> = (0..21)
        .map(|i| alias("", &format!("a{i}@example.test")))
        .collect();
    assert!(store.set_account_settings(id, &settings(many)).is_err());
}

#[test]
fn only_configured_aliases_can_be_used_as_sender() {
    let cfg = common::config(Security::Tls);
    let aliases = [alias("Sales Team", "sales@example.test")];
    let same = neutron_core::smtp::with_sender(&cfg, &aliases, None).unwrap();
    assert_eq!(same.email, cfg.email);
    let sales =
        neutron_core::smtp::with_sender(&cfg, &aliases, Some("SALES@example.test")).unwrap();
    assert_eq!(
        (sales.email.as_str(), sales.display_name.as_str()),
        ("sales@example.test", "Sales Team")
    );
    assert_eq!(sales.username, cfg.username);
    assert_eq!(sales.smtp_host, cfg.smtp_host);
    assert!(neutron_core::smtp::with_sender(&cfg, &aliases, Some("ceo@example.test")).is_err());
}

#[test]
fn replies_use_the_alias_the_mail_was_sent_to_and_never_address_it() {
    let raw = b"From: Bob <bob@example.test>\r\nTo: sales@example.test, carol@example.test\r\nCc: me@example.test\r\nMessage-ID: <x@example.test>\r\nSubject: Quote\r\n\r\nHi";
    let own = ["me@example.test", "sales@example.test"];
    let reply = compose::prepare_as(raw, 1, Mode::ReplyAll, &own);
    assert_eq!(reply.from.as_deref(), Some("sales@example.test"));
    let all: Vec<&String> = reply.to.iter().chain(reply.cc.iter()).collect();
    assert!(all.iter().any(|a| a.contains("bob@example.test")));
    assert!(all.iter().any(|a| a.contains("carol@example.test")));
    assert!(
        !all.iter()
            .any(|a| a.contains("sales@") || a.contains("me@")),
        "{all:?}"
    );
    let fwd = compose::prepare_as(raw, 1, Mode::Forward, &own);
    assert_eq!(fwd.from.as_deref(), Some("sales@example.test"));
    let plain = compose::prepare_as(raw, 1, Mode::Reply, &["me@example.test"]);
    assert_eq!(plain.from, None);
}

#[test]
fn automatic_replies_store_settings_and_answer_each_sender_once_per_period() {
    use neutron_core::autoreply::AutoReply;
    let (store, id) = store_with_account();
    assert_eq!(store.auto_reply(id).unwrap(), AutoReply::default());
    assert!(
        store
            .set_auto_reply(
                id,
                &AutoReply {
                    enabled: true,
                    ..Default::default()
                }
            )
            .is_err(),
        "needs a message"
    );
    let on = store
        .set_auto_reply(
            id,
            &AutoReply {
                enabled: true,
                subject: "  Away  ".into(),
                body: "Back Monday".into(),
                days: 2,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(on.enabled_at > 0, "switching on records when");
    assert_eq!(on.subject, "Away");
    assert_eq!(store.auto_reply(id).unwrap(), on);
    // Once per sender every `days` days, case-insensitively.
    let t = 1_000_000;
    assert!(store.auto_reply_due(id, "bob@example.test", t, 2).unwrap());
    store.record_auto_reply(id, "Bob@Example.test", t).unwrap();
    assert!(!store
        .auto_reply_due(id, "bob@example.test", t + 86_400, 2)
        .unwrap());
    assert!(store
        .auto_reply_due(id, "bob@example.test", t + 2 * 86_400, 2)
        .unwrap());
    // Editing while on keeps the start time and the log; a new period resets both.
    let edited = store
        .set_auto_reply(
            id,
            &AutoReply {
                body: "Back Tuesday".into(),
                ..on.clone()
            },
        )
        .unwrap();
    assert_eq!(edited.enabled_at, on.enabled_at);
    assert!(!store
        .auto_reply_due(id, "bob@example.test", t + 1, 2)
        .unwrap());
    store
        .set_auto_reply(
            id,
            &AutoReply {
                enabled: false,
                ..edited.clone()
            },
        )
        .unwrap();
    store
        .set_auto_reply(
            id,
            &AutoReply {
                enabled: true,
                ..edited
            },
        )
        .unwrap();
    assert!(store
        .auto_reply_due(id, "bob@example.test", t + 1, 2)
        .unwrap());
}

#[test]
fn aliases_can_have_their_own_signature() {
    let (store, id) = store_with_account();
    let mut sigs = std::collections::BTreeMap::new();
    sigs.insert(
        "Sales@Example.test".to_string(),
        "<b>Sales team</b><script>x()</script>".to_string(),
    );
    sigs.insert("gone@example.test".to_string(), "<b>Old</b>".to_string());
    sigs.insert("info@example.test".to_string(), "  ".to_string());
    let saved = store
        .set_account_settings(
            id,
            &AccountSettings {
                signature_html: "<b>Kaan</b>".into(),
                identities: vec![
                    alias("Sales", "sales@example.test"),
                    alias("", "info@example.test"),
                ],
                identity_signatures: sigs,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        saved.identity_signatures.keys().collect::<Vec<_>>(),
        vec!["sales@example.test"],
        "unknown aliases and empty signatures are dropped"
    );
    let stored = store.account_settings(id).unwrap();
    assert_eq!(stored, saved);
    assert_eq!(
        stored.signature_for(Some("SALES@example.test")),
        "<b>Sales team</b>"
    );
    assert_eq!(
        stored.signature_for(Some("info@example.test")),
        "<b>Kaan</b>"
    );
    assert_eq!(stored.signature_for(None), "<b>Kaan</b>");
}

#[test]
fn templates_are_sanitized_named_and_editable() {
    let (store, _) = store_with_account();
    let t = store
        .save_template(
            None,
            " Thanks ",
            "<p onclick=\"x()\">Thanks, <b>got it</b></p><script>y()</script>",
        )
        .unwrap();
    assert_eq!(t.name, "Thanks");
    assert!(!t.html.contains("onclick") && !t.html.contains("script"));
    assert!(store.save_template(None, "", "<p>x</p>").is_err());
    assert!(store.save_template(None, "Empty", "<p> </p>").is_err());
    let a = store
        .save_template(None, "away", "<p>I'm away</p>")
        .unwrap();
    let names: Vec<_> = store
        .templates()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(
        names,
        vec!["away", "Thanks"],
        "sorted by name, case-insensitive"
    );
    store
        .save_template(Some(a.id), "Away", "<p>Back Monday</p>")
        .unwrap();
    assert!(store.templates().unwrap()[0].html.contains("Back Monday"));
    store.delete_template(t.id).unwrap();
    assert_eq!(store.templates().unwrap().len(), 1);
    assert!(store.save_template(Some(999), "x", "<p>x</p>").is_err());
}

#[test]
fn scheduled_messages_are_claimed_once_retried_and_released() {
    use neutron_core::store::NewScheduled;
    let (store, id) = store_with_account();
    let to = vec!["b@example.test".to_string()];
    let add = |at| {
        store
            .add_scheduled(&NewScheduled {
                account_id: id,
                send_at: at,
                subject: "Hi",
                recipients: "Bob",
                env_from: "me@example.test",
                env_to: &to,
                raw: b"Subject: Hi\r\n\r\nx",
                replace_draft: None,
                replace_message_id: Some("<d1@x>"),
            })
            .unwrap()
    };
    let soon = add(1_000);
    let later = add(5_000);
    assert_eq!(store.claim_due_scheduled(2_000).unwrap(), vec![soon]);
    assert!(
        store.claim_due_scheduled(2_000).unwrap().is_empty(),
        "claimed once"
    );
    assert!(!store.claim_scheduled(soon).unwrap(), "already sending");
    store
        .scheduled_failed(soon, "421 try later", Some(3_000))
        .unwrap();
    let s = &store.scheduled().unwrap()[0];
    assert_eq!((s.state.as_str(), s.attempts), ("waiting", 1));
    assert_eq!(store.claim_due_scheduled(2_500).unwrap(), Vec::<i64>::new());
    assert_eq!(store.claim_due_scheduled(3_000).unwrap(), vec![soon]);
    store.scheduled_failed(soon, "gave up", None).unwrap();
    assert_eq!(store.scheduled().unwrap()[0].state, "failed");
    assert!(
        store.claim_scheduled(soon).unwrap(),
        "a failed one can be sent now"
    );
    store.release_scheduled(soon, Some(9_000)).unwrap();
    let p = store.scheduled_payload(soon).unwrap();
    assert_eq!(
        (p.env_to, p.replace_message_id.as_deref()),
        (to.clone(), Some("<d1@x>"))
    );
    assert!(store.claim_scheduled(later).unwrap());
    assert_eq!(store.reset_sending_scheduled().unwrap(), 1);
    store.delete_scheduled(later).unwrap();
    let left = store.scheduled().unwrap();
    assert_eq!(
        (left.len(), left[0].send_at, left[0].attempts),
        (1, 9_000, 0)
    );
}
