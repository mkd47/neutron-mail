//! End-to-end feature tests against the local test server (NEUTRON_IT=1):
//! remote images, reply / reply all / forward headers and attachments,
//! Bcc handling, drafts, attachment safety and address history.

mod common;

use mail_parser::MessageParser;
use neutron_core::compose::{self, Mode};
use neutron_core::model::{OutgoingAttachment, OutgoingMessage, Security};
use neutron_core::store::Store;
use neutron_core::{imap, parse, remote, smtp, sync};

const IMG: &str = "http://127.0.0.1:10880";

async fn synced_store() -> (Store, i64) {
    let store = Store::open_in_memory().unwrap();
    let cfg = common::config(Security::Tls);
    let id = store.add_account(&cfg).unwrap();
    sync::sync_account(&store, id, &cfg, common::PASSWORD)
        .await
        .unwrap();
    (store, id)
}

/// Poll the INBOX until a message with this subject arrives.
async fn wait_for(subject: &str) -> (Store, i64, i64) {
    for _ in 0..30 {
        let (store, acc) = synced_store().await;
        let inbox = store.folder_by_role(acc, "inbox").unwrap().unwrap();
        if let Some(m) = store
            .messages(inbox.id, 0, 500)
            .unwrap()
            .into_iter()
            .find(|m| m.subject == subject)
        {
            return (store, acc, m.id);
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    panic!("message {subject:?} never arrived");
}

fn header(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let unfolded = head.replace("\r\n ", " ").replace("\r\n\t", " ");
    unfolded.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
    })
}

async fn seeded(subject_prefix: &str) -> (Store, i64, i64) {
    let (store, acc) = synced_store().await;
    let inbox = store.folder_by_role(acc, "inbox").unwrap().unwrap();
    let m = store
        .messages(inbox.id, 0, 500)
        .unwrap()
        .into_iter()
        .find(|m| m.subject.starts_with(subject_prefix))
        .unwrap_or_else(|| {
            panic!("seed message {subject_prefix:?} missing: run testserver/seed.py")
        });
    (store, acc, m.id)
}

#[tokio::test]
async fn remote_images_fetched_validated_and_ssrf_blocked() {
    if !common::enabled() {
        return;
    }
    let urls: Vec<String> = ["banner.png", "fake.png", "huge.png", "missing.png"]
        .iter()
        .map(|f| format!("{IMG}/{f}"))
        .collect();
    // Default policy: loopback is never contacted.
    let blocked = remote::fetch_images(&urls, false).await.unwrap();
    assert!(blocked.images.is_empty());
    assert_eq!(blocked.failed, 4);
    // Test override: only the real PNG is accepted.
    let r = remote::fetch_images(&urls, true).await.unwrap();
    assert_eq!(r.images.len(), 1, "{:?}", r.images.keys());
    assert!(r.images[&urls[0]].starts_with("data:image/png;base64,iVBORw0KGgo"));
    assert_eq!(r.failed, 3);

    // The newsletter renders inline cid images immediately and the remote
    // banner only after it is fetched; the fake "image" never loads.
    let (store, _acc, id) = seeded("Neutron Weekly").await;
    let (d, remote_urls) = store.render_message(id, &Default::default()).unwrap();
    let html = d.html.unwrap();
    assert_eq!(
        html.matches("src=\"data:image/png;base64,").count(),
        2,
        "{html}"
    );
    assert_eq!(d.remote_images_blocked, 2);
    assert!(
        html.contains(".hero") && html.contains("linear-gradient"),
        "CSS kept"
    );
    let fetched = remote::fetch_images(&remote_urls, true).await.unwrap();
    let (d2, _) = store.render_message(id, &fetched.images).unwrap();
    assert_eq!(d2.remote_images_loaded, 1);
    assert_eq!(d2.remote_images_blocked, 1);
    assert!(!d2.html.unwrap().contains("127.0.0.1"));
}

#[tokio::test]
async fn reply_all_threading_headers_and_bcc_end_to_end() {
    if !common::enabled() {
        return;
    }
    let (store, _acc, parent_id) = seeded("Release planning for v0.2").await;
    let parent_raw = store.raw(parent_id).unwrap();
    let parent_msgid = header(&parent_raw, "Message-ID").unwrap();
    let cfg = common::config(Security::Tls);

    let reply = compose::prepare(&parent_raw, parent_id, Mode::Reply, &cfg.email);
    assert_eq!(reply.to, vec!["Elena Martinez <elena@example.test>"]);
    assert!(reply.cc.is_empty());
    let all = compose::prepare(&parent_raw, parent_id, Mode::ReplyAll, &cfg.email);
    assert_eq!(all.to, vec!["Elena Martinez <elena@example.test>"]);
    assert_eq!(
        all.cc,
        vec![
            "Jasper Collins <jasper@example.test>",
            "Sofia Patel <sofia@example.test>"
        ]
    );
    assert_eq!(all.subject, "Re: Release planning for v0.2");
    assert!(all.body.contains("> Can everyone confirm"));
    assert_eq!(
        all.in_reply_to.as_deref(),
        Some(parent_msgid.trim_matches(['<', '>']))
    );
    assert_eq!(
        all.references,
        vec![
            "kickoff-001@example.test".to_string(),
            parent_msgid.trim_matches(['<', '>']).to_string()
        ]
    );

    let subject = common::unique("Re: Release planning");
    let msg = OutgoingMessage {
        // Send to ourselves so the test server delivers it back.
        to: vec!["Kaan Durmus <kaan@localhost>".into()],
        cc: all.cc.clone(),
        bcc: vec!["hidden@example.test".into()],
        subject: subject.clone(),
        body: all.body.clone(),
        in_reply_to: all.in_reply_to.clone(),
        references: all.references.clone(),
        attachments: vec![],
    };
    smtp::send(&cfg, common::PASSWORD, &msg, &[]).await.unwrap();
    let (store2, _, id) = wait_for(&subject).await;
    let raw = store2.raw(id).unwrap();
    assert_eq!(header(&raw, "In-Reply-To").unwrap(), parent_msgid);
    let refs = header(&raw, "References").unwrap();
    assert!(
        refs.contains("<kickoff-001@example.test>") && refs.contains(&parent_msgid),
        "{refs}"
    );
    assert!(header(&raw, "Bcc").is_none(), "Bcc must not be in headers");
    let env_to = header(&raw, "X-Test-Envelope-To").unwrap();
    for r in [
        "kaan@localhost",
        "jasper@example.test",
        "sofia@example.test",
        "hidden@example.test",
    ] {
        assert!(env_to.contains(r), "{r} missing from envelope {env_to}");
    }
    let headers_only = String::from_utf8_lossy(&raw)
        .split("\r\n\r\n")
        .next()
        .unwrap()
        .replace(&format!("X-Test-Envelope-To: {env_to}"), "");
    assert!(
        !headers_only.contains("hidden@example.test"),
        "Bcc leaked into headers"
    );
}

#[tokio::test]
async fn forward_carries_attachments_byte_for_byte() {
    if !common::enabled() {
        return;
    }
    let (store, acc, parent_id) = seeded("Invoice #4711").await;
    let parent_raw = store.raw(parent_id).unwrap();
    let cfg = common::config(Security::Tls);
    let fwd = compose::prepare(&parent_raw, parent_id, Mode::Forward, &cfg.email);
    assert_eq!(fwd.subject, "Fwd: Invoice #4711 attached");
    assert!(fwd.body.contains("---------- Forwarded message ----------"));
    assert!(fwd.body.contains("From: Billing <billing@example.test>"));
    assert_eq!(fwd.attachments.len(), 2);
    assert!(fwd.in_reply_to.is_none());
    let originals: Vec<OutgoingAttachment> = fwd
        .attachments
        .iter()
        .map(|a| parse::attachment_content(&parent_raw, a.index).unwrap())
        .collect();
    assert!(originals[0].data.starts_with(b"%PDF-1.4"));
    assert!(store.message_account(parent_id).unwrap() == acc);

    // Plus one new file attachment, as from the picker.
    let mut files = originals.clone();
    files.push(OutgoingAttachment {
        name: "agenda.txt".into(),
        mime: "text/plain".into(),
        data: b"1. Budget\n2. Roadmap\n".to_vec(),
    });
    let subject = common::unique("Fwd: Invoice");
    let msg = OutgoingMessage {
        to: vec!["kaan@localhost".into()],
        subject: subject.clone(),
        body: fwd.body.clone(),
        references: fwd.references.clone(),
        ..Default::default()
    };
    smtp::send(&cfg, common::PASSWORD, &msg, &files)
        .await
        .unwrap();
    let (store2, _, id) = wait_for(&subject).await;
    let raw = store2.raw(id).unwrap();
    let d = store2.message_detail(id).unwrap();
    let names: Vec<_> = d.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["invoice-4711.pdf", "notes.txt", "agenda.txt"]);
    for (i, f) in files.iter().enumerate() {
        assert_eq!(
            parse::attachment_content(&raw, i).unwrap().data,
            f.data,
            "attachment {i} differs"
        );
    }
    let parsed = MessageParser::default().parse(&raw).unwrap();
    assert_eq!(parsed.attachment_count(), 3);
}

#[tokio::test]
async fn drafts_saved_resumed_and_replaced() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let subject = common::unique("Draft");
    let msg = OutgoingMessage {
        to: vec!["Ayse Yilmaz <ayse@example.test>".into()],
        bcc: vec!["secret@example.test".into()],
        subject: subject.clone(),
        body: "Unfinished thought".into(),
        ..Default::default()
    };
    let att = OutgoingAttachment {
        name: "plan.txt".into(),
        mime: "text/plain".into(),
        data: b"draft attachment".to_vec(),
    };
    let built = smtp::build(&cfg, &msg, std::slice::from_ref(&att), true).unwrap();
    let mut s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    imap::append_with_flags(&mut s, "Drafts", &built.raw, "(\\Seen \\Draft)")
        .await
        .unwrap();

    let (store, acc) = synced_store().await;
    let drafts = store.folder_by_role(acc, "drafts").unwrap().unwrap();
    let d = store
        .messages(drafts.id, 0, 500)
        .unwrap()
        .into_iter()
        .find(|m| m.subject == subject)
        .expect("draft synced");
    let resumed = compose::prepare(&store.raw(d.id).unwrap(), d.id, Mode::Draft, &cfg.email);
    assert_eq!(resumed.to, vec!["Ayse Yilmaz <ayse@example.test>"]);
    assert_eq!(resumed.bcc, vec!["secret@example.test"], "drafts keep Bcc");
    assert_eq!(resumed.body.trim(), "Unfinished thought");
    assert_eq!(resumed.attachments.len(), 1);

    // Replacing the draft removes the old version from the server.
    imap::delete_uid(&mut s, "Drafts", d.uid).await.unwrap();
    let st = imap::select(&mut s, "Drafts").await.unwrap();
    assert!(!st.uids.contains(&d.uid));
    drop(s.logout().await);

    // A draft may have no recipients; a sendable message may not.
    let empty = OutgoingMessage {
        subject: "x".into(),
        ..Default::default()
    };
    assert!(smtp::build(&cfg, &empty, &[], true).is_ok());
    assert!(smtp::build(&cfg, &empty, &[], false).is_err());
}

#[tokio::test]
async fn received_attachment_names_are_safe_and_risky_types_flagged() {
    if !common::enabled() {
        return;
    }
    let (store, _acc, id) = seeded("Files from IT").await;
    let d = store.message_detail(id).unwrap();
    let got: Vec<(&str, bool)> = d
        .attachments
        .iter()
        .map(|a| (a.name.as_str(), a.risky))
        .collect();
    assert_eq!(
        got,
        vec![
            ("setup.sh", true),
            ("bashrc", false),
            ("reporttxt.exe", true)
        ],
        "{got:?}"
    );
    for a in &d.attachments {
        assert!(!a.name.contains('/') && !a.name.contains("..") && !a.name.starts_with('.'));
    }
}

#[tokio::test]
async fn address_history_from_cache() {
    if !common::enabled() {
        return;
    }
    let (store, acc) = synced_store().await;
    let hits = store.address_history(acc, "ays", 10).unwrap();
    assert!(
        hits.iter()
            .any(|(n, e)| e == "ayse@example.test" && n == "Ayse Yilmaz"),
        "{hits:?}"
    );
    let hits = store.address_history(acc, "jasper@", 10).unwrap();
    assert!(hits.iter().any(|(_, e)| e == "jasper@example.test"));
    // LIKE wildcards in the query are matched literally.
    assert!(store.address_history(acc, "%", 10).unwrap().is_empty());
}
