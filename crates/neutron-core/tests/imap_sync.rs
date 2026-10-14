mod common;

use neutron_core::imap;
use neutron_core::model::Security;
use neutron_core::store::Store;
use neutron_core::sync;

async fn seeded_folder(sec: Security) -> (Store, i64, i64, String) {
    let cfg = common::config(sec);
    let folder = common::unique("IT");
    let mut s = imap::connect(&cfg, common::PASSWORD)
        .await
        .expect("connect");
    s.create(&folder).await.expect("create folder");
    imap::append(
        &mut s,
        &folder,
        &common::fixture("plain-one", false, false),
        false,
    )
    .await
    .unwrap();
    imap::append(
        &mut s,
        &folder,
        &common::fixture("html-two", true, false),
        false,
    )
    .await
    .unwrap();
    imap::append(
        &mut s,
        &folder,
        &common::fixture("attach-three", false, true),
        false,
    )
    .await
    .unwrap();
    let store = Store::open_in_memory().unwrap();
    let acc = store.add_account(&cfg).unwrap();
    let fid = store.upsert_folder(acc, &folder, None).unwrap();
    let (new, removed) = sync::sync_folder(&store, fid, &folder, &mut s)
        .await
        .unwrap();
    assert_eq!((new, removed), (3, 0));
    s.logout().await.ok();
    (store, acc, fid, folder)
}

#[tokio::test]
async fn sync_over_implicit_tls() {
    if !common::enabled() {
        return;
    }
    let (store, acc, fid, folder) = seeded_folder(Security::Tls).await;
    let list = store.messages(fid, 0, 50).unwrap();
    assert_eq!(list.len(), 3);
    let attach = list.iter().find(|m| m.subject == "attach-three").unwrap();
    assert!(attach.has_attachments);
    assert!(!attach.seen, "BODY.PEEK must not set \\Seen");

    let html = list.iter().find(|m| m.subject == "html-two").unwrap();
    let d = store.message_detail(html.id).unwrap();
    let h = d.html.expect("html part");
    assert!(h.contains("<b>zeppelins</b>"));
    assert!(!h.contains("script") && !h.contains("onclick") && !h.contains("tracker"));
    assert_eq!(d.remote_images_blocked, 1);

    let d = store.message_detail(attach.id).unwrap();
    assert_eq!(d.attachments.len(), 1);
    assert_eq!(d.attachments[0].name, "report.pdf");

    // Search over synced content.
    let hits = store.search(acc, "zeppel", 20).unwrap();
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert!(hits.iter().any(|h| h.snippet.contains('\u{2}')));

    // Expunge on server is reflected; re-sync is idempotent.
    let cfg = common::config(Security::Tls);
    let mut s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    s.select(&folder).await.unwrap();
    use futures::TryStreamExt;
    let _: Vec<_> = s
        .uid_store(attach.uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let _: Vec<_> = s.expunge().await.unwrap().try_collect().await.unwrap();
    let (new, removed) = sync::sync_folder(&store, fid, &folder, &mut s)
        .await
        .unwrap();
    assert_eq!((new, removed), (0, 1));
    assert_eq!(store.messages(fid, 0, 50).unwrap().len(), 2);

    // Mark seen on server propagates on next sync.
    let plain = store
        .messages(fid, 0, 50)
        .unwrap()
        .into_iter()
        .find(|m| m.subject == "plain-one")
        .unwrap();
    imap::mark_seen(&mut s, &folder, plain.uid).await.unwrap();
    sync::sync_folder(&store, fid, &folder, &mut s)
        .await
        .unwrap();
    assert!(
        store
            .messages(fid, 0, 50)
            .unwrap()
            .iter()
            .find(|m| m.uid == plain.uid)
            .unwrap()
            .seen
    );
    s.delete(&folder).await.ok();
    s.logout().await.ok();
}

#[tokio::test]
async fn sync_over_starttls() {
    if !common::enabled() {
        return;
    }
    let (store, _acc, fid, folder) = seeded_folder(Security::Starttls).await;
    assert_eq!(store.messages(fid, 0, 50).unwrap().len(), 3);
    let mut s = imap::connect(&common::config(Security::Starttls), common::PASSWORD)
        .await
        .unwrap();
    assert!(imap::supports_idle(&mut s).await.unwrap());
    s.delete(&folder).await.ok();
    s.logout().await.ok();
}

#[tokio::test]
async fn full_account_sync_lists_special_folders() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let store = Store::open_in_memory().unwrap();
    let acc = store.add_account(&cfg).unwrap();
    let r = sync::sync_account(&store, acc, &cfg, common::PASSWORD)
        .await
        .unwrap();
    assert!(r.folders >= 4);
    let folders = store.folders(acc).unwrap();
    assert_eq!(folders[0].role.as_deref(), Some("inbox"));
    assert!(folders.iter().any(|f| f.role.as_deref() == Some("sent")));
}

#[tokio::test]
async fn rejects_untrusted_certificate() {
    if !common::enabled() {
        return;
    }
    let mut cfg = common::config(Security::Tls);
    cfg.extra_ca_pem = None; // self-signed test cert is not in the Mozilla roots
    let err = imap::connect(&cfg, common::PASSWORD)
        .await
        .err()
        .expect("must fail");
    assert!(err.to_string().to_lowercase().contains("tls"), "{err}");
}

#[tokio::test]
async fn rejects_wrong_password() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    assert!(imap::connect(&cfg, "wrong").await.is_err());
}

#[tokio::test]
async fn idle_wakes_on_new_mail() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    let waiter = tokio::spawn(imap::idle_inbox(s, std::time::Duration::from_secs(20)));
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let mut s2 = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    imap::append(
        &mut s2,
        "INBOX",
        &common::fixture(&common::unique("idle"), false, false),
        false,
    )
    .await
    .unwrap();
    let (mut s, changed) = waiter.await.unwrap().unwrap();
    assert!(changed, "IDLE should report new data");
    s.logout().await.ok();
    s2.logout().await.ok();
}
