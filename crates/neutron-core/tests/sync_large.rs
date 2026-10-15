//! Incremental sync against the local Dovecot (CONDSTORE) with several
//! thousand messages, plus the IMAP command timeout (audit O13).
//! Needs NEUTRON_IT=1 and testserver/start.sh. NM_LARGE_N sets the size.

mod common;

use std::time::Instant;

use futures::TryStreamExt;
use neutron_core::imap;
use neutron_core::model::Security;
use neutron_core::store::Store;
use neutron_core::sync::{self, SyncMode};

fn msg(i: usize) -> Vec<u8> {
    format!(
        "From: Bulk Sender <bulk{}@example.test>\r\nTo: kaan@localhost\r\nSubject: Bulk message {i}\r\n\
Date: Wed, 14 Oct 2026 10:00:00 +0000\r\nMessage-ID: <bulk-{i}@example.test>\r\n\
Content-Type: text/plain; charset=utf-8\r\n\r\nBody of bulk message number {i}, about gliders.\r\n",
        i % 37
    )
    .into_bytes()
}

fn all_summaries(store: &Store, fid: i64) -> Vec<neutron_core::model::MessageSummary> {
    let mut out = Vec::new();
    loop {
        let page = store.messages(fid, out.len() as i64, 500).unwrap();
        if page.is_empty() {
            return out;
        }
        out.extend(page);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn large_incremental_sync() {
    if !common::enabled() {
        return;
    }
    let n: usize = std::env::var("NM_LARGE_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    let cfg = common::config(Security::Tls);
    let folder = common::unique("Large");
    let mut s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    s.create(&folder).await.unwrap();
    let t = Instant::now();
    for i in 0..n {
        imap::append(&mut s, &folder, &msg(i), i % 2 == 0)
            .await
            .unwrap();
    }
    eprintln!("TIMING seed {n} messages via APPEND: {:?}", t.elapsed());

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_plaintext(&dir.path().join("c.sqlite3")).unwrap();
    let acc = store.add_account(&cfg).unwrap();
    let fid = store.upsert_folder(acc, &folder, None).unwrap();

    // 1. First sync: full, no cap (the old code stopped at 500).
    let t = Instant::now();
    let st = sync::sync_folder_detailed(&store, fid, &folder, &mut s, 0, true)
        .await
        .unwrap();
    eprintln!("TIMING first full sync of {n}: {:?} ({st:?})", t.elapsed());
    assert_eq!(st.mode, SyncMode::Full);
    assert!(st.condstore, "Dovecot advertises CONDSTORE");
    assert_eq!(st.new, n);
    assert_eq!(store.uids(fid).unwrap().len(), n);

    // 2. Nothing changed: incremental, no UID SEARCH ALL, no flag fetch.
    let t = Instant::now();
    let st = sync::sync_folder_detailed(&store, fid, &folder, &mut s, 0, true)
        .await
        .unwrap();
    let noop = t.elapsed();
    eprintln!("TIMING no-change incremental sync: {noop:?} ({st:?})");
    assert_eq!(st.mode, SyncMode::Incremental);
    assert_eq!((st.new, st.removed, st.flag_updates), (0, 0, 0));
    assert!(!st.full_uid_search);

    // 3. Another client: 10 new, 3 flag changes, no expunge.
    let mut other = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    for i in n..n + 10 {
        imap::append(&mut other, &folder, &msg(i), false)
            .await
            .unwrap();
    }
    other.select(&folder).await.unwrap();
    let _: Vec<_> = other
        .uid_store("2,4,6", "+FLAGS.SILENT (\\Flagged)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let t = Instant::now();
    let st = sync::sync_folder_detailed(&store, fid, &folder, &mut s, 0, true)
        .await
        .unwrap();
    eprintln!(
        "TIMING incremental (10 new, 3 flags): {:?} ({st:?})",
        t.elapsed()
    );
    assert_eq!(st.new, 10);
    // CHANGEDSINCE returns only the 3 changed + the 10 new messages, not all n.
    assert_eq!(st.flag_updates, 13);
    assert!(!st.full_uid_search);
    let flagged = all_summaries(&store, fid)
        .iter()
        .filter(|m| m.flagged)
        .count();
    assert_eq!(flagged, 3);

    // 4. Expunge 5 messages elsewhere.
    let _: Vec<_> = other
        .uid_store("100:104", "+FLAGS.SILENT (\\Deleted)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let _: Vec<_> = other.expunge().await.unwrap().try_collect().await.unwrap();
    let t = Instant::now();
    let st = sync::sync_folder_detailed(&store, fid, &folder, &mut s, 0, true)
        .await
        .unwrap();
    eprintln!(
        "TIMING incremental with 5 expunges: {:?} ({st:?})",
        t.elapsed()
    );
    assert_eq!(st.removed, 5);
    assert!(st.full_uid_search);
    let uids = store.uids(fid).unwrap();
    assert_eq!(uids.len(), n + 10 - 5);
    assert!(!uids.contains(&102));

    // 5. Fallback without CONDSTORE (fresh cache, then incremental).
    let store2 = Store::open_in_memory().unwrap();
    let acc2 = store2.add_account(&cfg).unwrap();
    let fid2 = store2.upsert_folder(acc2, &folder, None).unwrap();
    let t = Instant::now();
    let st = sync::sync_folder_detailed(&store2, fid2, &folder, &mut s, 0, false)
        .await
        .unwrap();
    eprintln!("TIMING full sync without CONDSTORE: {:?}", t.elapsed());
    assert!(!st.condstore);
    assert_eq!(st.new, n + 5);
    let _: Vec<_> = other
        .uid_store("1", "+FLAGS.SILENT (\\Flagged)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let t = Instant::now();
    let st = sync::sync_folder_detailed(&store2, fid2, &folder, &mut s, 0, false)
        .await
        .unwrap();
    eprintln!(
        "TIMING no-CONDSTORE incremental (full flag refresh): {:?}",
        t.elapsed()
    );
    assert_eq!(st.mode, SyncMode::Incremental);
    assert_eq!(st.flag_updates, n + 5, "fallback refetches every flag");
    assert!(all_summaries(&store2, fid2)
        .iter()
        .any(|m| m.uid == 1 && m.flagged));

    // 6. A changed sync window forces one full pass.
    let st = sync::sync_folder_detailed(&store, fid, &folder, &mut s, 30, true)
        .await
        .unwrap();
    assert_eq!(st.mode, SyncMode::Full);

    other.logout().await.ok();
    s.delete(&folder).await.ok();
    s.logout().await.ok();
}
