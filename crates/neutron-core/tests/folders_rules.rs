//! Folder management, move/copy/undo, local rules on new mail and
//! server-side Sieve via ManageSieve, against the local Dovecot
//! (testserver/start.sh with Pigeonhole). Needs NEUTRON_IT=1.

mod common;

use futures::TryStreamExt;
use neutron_core::folders::{self, DeleteMode};
use neutron_core::imap;
use neutron_core::managesieve;
use neutron_core::model::{Folder, Security};
use neutron_core::rules::{Action, Condition, Field, Op, Rule};
use neutron_core::store::{SieveSettings, Store};
use neutron_core::sync;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn msg(subject: &str, mid: &str) -> Vec<u8> {
    format!(
        "From: Elena Marsh <elena@example.org>\r\nTo: kaan@localhost\r\nSubject: {subject}\r\n\
Date: Thu, 15 Oct 2026 10:00:00 +0000\r\nMessage-ID: <{mid}@example.org>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nInvoice body.\r\n"
    )
    .into_bytes()
}

async fn setup() -> (tempfile::TempDir, Store, i64, imap::Session) {
    let cfg = common::config(Security::Tls);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_plaintext(&dir.path().join("c.sqlite3")).unwrap();
    let acc = store.add_account(&cfg).unwrap();
    let s = imap::connect(&cfg, common::PASSWORD).await.unwrap();
    (dir, store, acc, s)
}

async fn resync(store: &Store, acc: i64) -> neutron_core::model::SyncReport {
    let cfg = common::config(Security::Tls);
    sync::sync_account(store, acc, &cfg, common::PASSWORD)
        .await
        .unwrap()
}

fn by_display(store: &Store, acc: i64, d: &str) -> Option<Folder> {
    store
        .folders(acc)
        .unwrap()
        .into_iter()
        .find(|f| f.display == d)
}

fn subjects(store: &Store, f: &Folder) -> Vec<String> {
    store
        .messages(f.id, 0, 500)
        .unwrap()
        .into_iter()
        .map(|m| m.subject)
        .collect()
}

async fn flags_of(s: &mut imap::Session, folder: &str, uid_hint_subject: &str) -> Vec<String> {
    s.select(folder).await.unwrap();
    let uids = s
        .uid_search(format!("SUBJECT \"{uid_hint_subject}\""))
        .await
        .unwrap();
    let u = uids.into_iter().max().expect("message present");
    let f: Vec<_> = s
        .uid_fetch(u.to_string(), "FLAGS")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    f[0].flags().map(|f| format!("{f:?}")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn folder_lifecycle_unicode_nested_protected() {
    if !common::enabled() {
        return;
    }
    let (_d, store, acc, mut s) = setup().await;
    resync(&store, acc).await;
    let top = common::unique("项目");
    let wire = folders::create(&store, &mut s, acc, None, &top)
        .await
        .unwrap();
    assert!(
        wire.is_ascii() && wire.contains('&'),
        "modified UTF-7 on the wire: {wire}"
    );
    resync(&store, acc).await;
    let parent = by_display(&store, acc, &top).expect("decoded name in cache");
    assert_eq!(parent.delimiter.as_deref(), Some("."));
    assert!(parent.subscribed);
    // Nested.
    folders::create(&store, &mut s, acc, Some(parent.id), "子文件夹 Ünïcode")
        .await
        .unwrap();
    // Leaf names can't smuggle the delimiter or wildcards.
    assert!(folders::create(&store, &mut s, acc, Some(parent.id), "a.b")
        .await
        .is_err());
    assert!(folders::create(&store, &mut s, acc, None, "x*")
        .await
        .is_err());
    resync(&store, acc).await;
    let child_name = format!("{top}.子文件夹 Ünïcode");
    let child = by_display(&store, acc, &child_name).expect("child");
    // A message in the child, then rename the parent: the child follows,
    // and its cache survives.
    imap::append(
        &mut s,
        &child.name,
        &msg("in child", &common::unique("c")),
        false,
    )
    .await
    .unwrap();
    resync(&store, acc).await;
    folders::rename(
        &store,
        &mut s,
        parent.id,
        Some(&format!("{top}-renamed")),
        None,
    )
    .await
    .unwrap();
    let moved_child = by_display(&store, acc, &format!("{top}-renamed.子文件夹 Ünïcode"))
        .expect("child renamed in cache");
    let remote = imap::list_folders(&mut s).await.unwrap();
    assert!(
        remote
            .iter()
            .any(|r| r.display == format!("{top}-renamed.子文件夹 Ünïcode") && r.subscribed),
        "the child's subscription follows the rename"
    );
    assert_eq!(moved_child.id, child.id);
    assert_eq!(subjects(&store, &moved_child), vec!["in child"]);
    resync(&store, acc).await;
    assert_eq!(
        subjects(
            &store,
            &by_display(&store, acc, &format!("{top}-renamed.子文件夹 Ünïcode")).unwrap()
        ),
        vec!["in child"]
    );
    // Reparent the child to the top level; a folder can't go into itself.
    let p2 = by_display(&store, acc, &format!("{top}-renamed")).unwrap();
    assert!(
        folders::rename(&store, &mut s, p2.id, None, Some(Some(moved_child.id)))
            .await
            .is_err()
    );
    let leaf = common::unique("toplevel-child");
    folders::rename(&store, &mut s, moved_child.id, Some(&leaf), Some(None))
        .await
        .unwrap();
    resync(&store, acc).await;
    let top_child = by_display(&store, acc, &leaf).expect("reparented");
    // Unsubscribe / subscribe.
    imap::set_subscribed(&mut s, &top_child.name, false)
        .await
        .unwrap();
    resync(&store, acc).await;
    assert!(!by_display(&store, acc, &leaf).unwrap().subscribed);
    imap::set_subscribed(&mut s, &top_child.name, true)
        .await
        .unwrap();
    // Special-use folders are protected.
    for role in ["inbox", "sent", "trash", "drafts", "junk", "archive"] {
        let f = store
            .folder_by_role(acc, role)
            .unwrap()
            .unwrap_or_else(|| panic!("{role}"));
        assert!(
            folders::delete(&store, &mut s, f.id, DeleteMode::DeleteMessages)
                .await
                .is_err(),
            "{role}"
        );
        assert!(
            folders::rename(&store, &mut s, f.id, Some("x"), None)
                .await
                .is_err(),
            "{role}"
        );
    }
    // Delete with "move messages to Trash".
    let trash = store.folder_by_role(acc, "trash").unwrap().unwrap();
    let moved = folders::delete(&store, &mut s, top_child.id, DeleteMode::MoveToTrash)
        .await
        .unwrap();
    assert_eq!(moved, 1);
    assert!(by_display(&store, acc, &leaf).is_none());
    resync(&store, acc).await;
    assert!(subjects(&store, &store.folder(trash.id).unwrap()).contains(&"in child".to_string()));
    let names: Vec<String> = imap::list_folders(&mut s)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.display)
        .collect();
    assert!(!names.contains(&leaf));
    folders::delete(&store, &mut s, p2.id, DeleteMode::DeleteMessages)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn move_copy_undo_and_fallback() {
    if !common::enabled() {
        return;
    }
    let (_d, store, acc, mut s) = setup().await;
    let a = common::unique("MoveA");
    let b = common::unique("MoveB");
    imap::create_folder(&mut s, &a).await.unwrap();
    imap::create_folder(&mut s, &b).await.unwrap();
    for i in 0..3 {
        imap::append(
            &mut s,
            &a,
            &msg(&format!("m{i}"), &common::unique(&format!("mv{i}"))),
            false,
        )
        .await
        .unwrap();
    }
    resync(&store, acc).await;
    let fa = by_display(&store, acc, &a).unwrap();
    let fb = by_display(&store, acc, &b).unwrap();
    let ids: Vec<i64> = store
        .messages(fa.id, 0, 10)
        .unwrap()
        .into_iter()
        .filter(|m| m.subject != "m2")
        .map(|m| m.id)
        .collect();
    let t = folders::transfer(&store, &mut s, &ids, fb.id, false)
        .await
        .unwrap();
    assert_eq!((t.count, t.untracked), (2, 0));
    // Cache updated at once, server agrees after a sync.
    assert_eq!(subjects(&store, &fa), vec!["m2"]);
    resync(&store, acc).await;
    let mut got = subjects(&store, &fb);
    got.sort();
    assert_eq!(got, vec!["m0", "m1"]);
    // Undo.
    assert_eq!(folders::undo(&store, &mut s, &t).await.unwrap(), 2);
    resync(&store, acc).await;
    assert!(subjects(&store, &fb).is_empty());
    assert_eq!(subjects(&store, &fa).len(), 3);
    // Copy keeps the original.
    let one = vec![store.messages(fa.id, 0, 1).unwrap()[0].id];
    let c = folders::transfer(&store, &mut s, &one, fb.id, true)
        .await
        .unwrap();
    resync(&store, acc).await;
    assert_eq!(
        (subjects(&store, &fa).len(), subjects(&store, &fb).len()),
        (3, 1)
    );
    folders::undo(&store, &mut s, &c).await.unwrap();
    // COPY + \Deleted + UID EXPUNGE fallback (servers without MOVE).
    s.select(&a).await.unwrap();
    let uids: Vec<u32> = s.uid_search("ALL").await.unwrap().into_iter().collect();
    imap::move_uids_with(&mut s, &a, &uids, &b, false)
        .await
        .unwrap();
    resync(&store, acc).await;
    assert!(subjects(&store, &fa).is_empty());
    assert_eq!(subjects(&store, &fb).len(), 3);
    for f in [&a, &b] {
        imap::delete_folder(&mut s, f).await.unwrap();
    }
}

fn subject_rule(id_tag: &str, actions: Vec<Action>) -> Rule {
    Rule {
        id: 0,
        name: format!("rule {id_tag}"),
        enabled: true,
        match_all: true,
        conditions: vec![
            Condition {
                field: Field::Subject,
                op: Op::Contains,
                value: id_tag.into(),
                header: String::new(),
            },
            Condition {
                field: Field::From,
                op: Op::Equals,
                value: "elena@example.org".into(),
                header: String::new(),
            },
        ],
        actions,
        server_side: false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_rules_on_new_mail_with_forward_guard() {
    if !common::enabled() {
        return;
    }
    let (_d, store, acc, mut s) = setup().await;
    let tag = common::unique("rulez");
    let dest = common::unique("Invoices");
    imap::create_folder(&mut s, &dest).await.unwrap();
    resync(&store, acc).await; // baseline: rules never run on the first download
    store
        .save_rules(&[
            // Forward anything with the tag (no sender condition, so the
            // forwarded copy that comes back matches too: loop guard).
            Rule {
                conditions: vec![Condition {
                    field: Field::Subject,
                    op: Op::Contains,
                    value: tag.clone(),
                    header: String::new(),
                }],
                ..subject_rule(
                    &tag,
                    vec![Action::Forward {
                        to: "accounting@example.test".into(),
                    }],
                )
            },
            subject_rule(
                &tag,
                vec![
                    Action::Label {
                        keyword: "$billing".into(),
                    },
                    Action::MarkRead,
                    Action::Move {
                        folder: dest.clone(),
                    },
                    Action::Stop,
                ],
            ),
        ])
        .unwrap();
    imap::append(
        &mut s,
        "INBOX",
        &msg(&format!("Invoice {tag}"), &common::unique("r1")),
        false,
    )
    .await
    .unwrap();
    let rep = resync(&store, acc).await;
    assert_eq!(rep.rules_matched, 1, "{rep:?}");
    assert!(rep.rule_errors.is_empty(), "{:?}", rep.rule_errors);
    let inbox = store.folder_by_role(acc, "inbox").unwrap().unwrap();
    assert!(!subjects(&store, &inbox).iter().any(|x| x.contains(&tag)));
    let d = by_display(&store, acc, &dest).unwrap();
    let in_dest = store.messages(d.id, 0, 10).unwrap();
    assert_eq!(in_dest.len(), 1);
    assert!(in_dest[0].seen);
    let fl = flags_of(&mut s, &dest, &tag).await;
    assert!(
        fl.iter().any(|f| f.contains("$billing")) && fl.iter().any(|f| f.contains("Seen")),
        "{fl:?}"
    );
    // The forward arrived (the test SMTP server delivers to this INBOX). It
    // matches the forward rule again but carries Auto-Submitted and comes
    // from our own address, so it is not forwarded again (loop guard).
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let rep2 = resync(&store, acc).await;
    assert!(rep2.rule_errors.is_empty(), "{:?}", rep2.rule_errors);
    assert_eq!(
        rep2.rules_matched, 1,
        "the forward copy matched (and was guarded)"
    );
    let fwd: Vec<_> = store
        .messages(inbox.id, 0, 500)
        .unwrap()
        .into_iter()
        .filter(|m| m.subject == format!("Fwd: Invoice {tag}"))
        .collect();
    assert_eq!(fwd.len(), 1, "exactly one forward");
    let raw = store.raw(fwd[0].id).unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("Auto-Submitted: auto-forwarded"));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let rep3 = resync(&store, acc).await;
    assert_eq!(rep3.new_messages, 0, "no forward storm");

    // "Run now" on a folder: a message that arrived before the rule.
    let run_dest = by_display(&store, acc, &dest).unwrap();
    imap::append(
        &mut s,
        &dest,
        &msg(&format!("old {tag}"), &common::unique("old")),
        false,
    )
    .await
    .unwrap();
    resync(&store, acc).await;
    let cfg = common::config(Security::Tls);
    let unread_rule = vec![Rule {
        id: 9,
        ..subject_rule(&tag, vec![Action::Flag])
    }];
    let ids = store.message_ids(run_dest.id).unwrap();
    let r = neutron_core::rules_exec::run(
        &store,
        &mut s,
        &cfg,
        common::PASSWORD,
        acc,
        run_dest.id,
        &ids,
        &unread_rule,
        false,
    )
    .await
    .unwrap();
    assert_eq!((r.examined, r.matched, r.flagged), (2, 2, 2), "{r:?}");
    assert!(store
        .messages(run_dest.id, 0, 10)
        .unwrap()
        .iter()
        .all(|m| m.flagged));
    imap::delete_folder(&mut s, &dest).await.unwrap();
}

async fn lmtp_deliver(raw: &[u8]) {
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", 10024))
        .await
        .unwrap();
    let (r, mut w) = tcp.into_split();
    let mut r = BufReader::new(r);
    let mut line = String::new();
    async fn expect(
        r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
        line: &mut String,
        code: &str,
    ) {
        loop {
            line.clear();
            r.read_line(line).await.unwrap();
            assert!(line.starts_with(code), "LMTP: {line}");
            if line.as_bytes().get(3) == Some(&b' ') {
                return;
            }
        }
    }
    expect(&mut r, &mut line, "220").await;
    for (cmd, code) in [
        ("LHLO test\r\n", "250"),
        ("MAIL FROM:<elena@example.org>\r\n", "250"),
        ("RCPT TO:<kaan@localhost>\r\n", "250"),
        ("DATA\r\n", "354"),
    ] {
        w.write_all(cmd.as_bytes()).await.unwrap();
        expect(&mut r, &mut line, code).await;
    }
    w.write_all(raw).await.unwrap();
    w.write_all(b"\r\n.\r\n").await.unwrap();
    expect(&mut r, &mut line, "250").await;
    w.write_all(b"QUIT\r\n").await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sieve_upload_and_server_side_filtering() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let (_d, store, acc, mut s) = setup().await;
    let tag = common::unique("sieve");
    let dest = format!("{}-筛选", common::unique("Sieve"));
    let dest_wire = imap::wire_name(&dest, false);
    imap::create_folder(&mut s, &dest_wire).await.unwrap();
    let mut r = subject_rule(
        &tag,
        vec![
            Action::Flag,
            Action::Label {
                keyword: "$sieved".into(),
            },
            Action::Move {
                folder: dest.clone(),
            },
        ],
    );
    r.server_side = true;
    let saved = store.save_rules(&[r]).unwrap();
    let export = neutron_core::rules::to_sieve(&saved, &cfg.email, "Trash");
    assert_eq!(export.exported.len(), 1);

    // Plaintext auth is refused when STARTTLS is missing: we never get that
    // far here because Dovecot offers it; the client checks caps first.
    let mut ms = managesieve::Client::connect(&cfg, 10419, common::PASSWORD)
        .await
        .unwrap();
    assert!(ms.caps.sieve_extensions.iter().any(|e| e == "imap4flags"));
    assert!(ms.check_script("if true { frobnicate; }").await.is_err());
    ms.check_script(&export.script).await.unwrap();
    ms.put_script(managesieve::SCRIPT_NAME, &export.script)
        .await
        .unwrap();
    ms.set_active(managesieve::SCRIPT_NAME).await.unwrap();
    let list = ms.list_scripts().await.unwrap();
    assert!(
        list.contains(&(managesieve::SCRIPT_NAME.to_string(), true)),
        "{list:?}"
    );
    assert_eq!(
        ms.get_script(managesieve::SCRIPT_NAME)
            .await
            .unwrap()
            .trim_end(),
        export.script.trim_end()
    );
    store
        .set_sieve_settings(
            acc,
            SieveSettings {
                port: 10419,
                active: true,
                script_hash: None,
            },
        )
        .unwrap();
    // Server-side rules are not run locally while the script is active.
    assert!(neutron_core::rules_exec::local_rules(&store, acc)
        .unwrap()
        .is_empty());

    lmtp_deliver(&msg(&format!("Server filter {tag}"), &common::unique("sv"))).await;
    lmtp_deliver(&msg("unrelated, stays in INBOX", &common::unique("sv2"))).await;
    // Dovecot filed it on delivery.
    s.select(&dest_wire).await.unwrap();
    let hits = s.uid_search(format!("SUBJECT \"{tag}\"")).await.unwrap();
    assert_eq!(hits.len(), 1, "sieve fileinto");
    let fl = flags_of(&mut s, &dest_wire, &tag).await;
    assert!(
        fl.iter().any(|f| f.contains("Flagged")) && fl.iter().any(|f| f.contains("$sieved")),
        "{fl:?}"
    );
    s.select("INBOX").await.unwrap();
    assert!(s
        .uid_search(format!("SUBJECT \"{tag}\""))
        .await
        .unwrap()
        .is_empty());

    ms.set_active("").await.unwrap();
    ms.delete_script(managesieve::SCRIPT_NAME).await.unwrap();
    ms.logout().await;
    imap::delete_folder(&mut s, &dest_wire).await.unwrap();
}

fn count_subject(store: &Store, folder: &Folder, subject: &str) -> usize {
    store
        .messages(folder.id, 0, 100)
        .unwrap()
        .into_iter()
        .filter(|m| m.subject == subject)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_rule_retries_without_repeating_forward_or_copy() {
    if !common::enabled() {
        return;
    }
    let (_d, store, acc, mut s) = setup().await;
    let tag = common::unique("retry");
    let copies = common::unique("Copies");
    let missing = common::unique("Missing");
    imap::create_folder(&mut s, &copies).await.unwrap();
    resync(&store, acc).await;
    let copy_folder = by_display(&store, acc, &copies).unwrap();
    store
        .save_rules(&[subject_rule(
            &tag,
            vec![
                Action::Copy {
                    folder: copy_folder.display.clone(),
                },
                Action::Forward {
                    to: "accounting@example.test".into(),
                },
                Action::Move {
                    folder: missing.clone(),
                },
            ],
        )])
        .unwrap();
    let subject = format!("Invoice {tag}");
    imap::append(&mut s, "INBOX", &msg(&subject, &common::unique("r")), false)
        .await
        .unwrap();
    let rep = resync(&store, acc).await;
    assert!(
        rep.rule_errors.iter().any(|e| e.contains("retry")),
        "{:?}",
        rep.rule_errors
    );
    let inbox = store.folder_by_role(acc, "inbox").unwrap().unwrap();
    assert_eq!(count_subject(&store, &inbox, &subject), 1);
    assert_eq!(count_subject(&store, &copy_folder, &subject), 1);
    let progress = store.rule_runs(acc).unwrap();
    assert!(
        progress.iter().any(|(_, run)| {
            run.status == "pending"
                && run.done.iter().any(|d| d.starts_with("forward:"))
                && run.done.iter().any(|d| d.starts_with("copy:"))
                && !run.done.iter().any(|d| d.starts_with("move:"))
        }),
        "{progress:?}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let rep2 = resync(&store, acc).await;
    let fwd = format!("Fwd: {subject}");
    assert_eq!(count_subject(&store, &inbox, &fwd), 1, "{rep2:?}");
    assert_eq!(count_subject(&store, &copy_folder, &subject), 1);
    assert_eq!(count_subject(&store, &inbox, &subject), 1);

    store.release_rule_retries(acc).unwrap();
    let rep3 = resync(&store, acc).await;
    assert!(
        rep3.rule_errors.iter().any(|e| e.contains("retry")),
        "{:?}",
        rep3.rule_errors
    );
    assert_eq!(
        count_subject(&store, &inbox, &fwd),
        1,
        "forward was not repeated"
    );
    assert_eq!(
        count_subject(&store, &copy_folder, &subject),
        1,
        "copy was not repeated"
    );
    assert_eq!(count_subject(&store, &inbox, &subject), 1);

    imap::create_folder(&mut s, &missing).await.unwrap();
    store.release_rule_retries(acc).unwrap();
    let rep4 = resync(&store, acc).await;
    assert!(rep4.rule_errors.is_empty(), "{:?}", rep4.rule_errors);
    let dest = by_display(&store, acc, &missing).unwrap();
    assert_eq!(count_subject(&store, &dest, &subject), 1);
    assert_eq!(count_subject(&store, &inbox, &subject), 0);
    assert_eq!(count_subject(&store, &copy_folder, &subject), 1);
    assert_eq!(count_subject(&store, &inbox, &fwd), 1);
    let rep5 = resync(&store, acc).await;
    assert!(rep5.rule_errors.is_empty(), "{:?}", rep5.rule_errors);
    assert_eq!(count_subject(&store, &copy_folder, &subject), 1);
    assert_eq!(count_subject(&store, &inbox, &fwd), 1);
    assert_eq!(count_subject(&store, &dest, &subject), 1);

    imap::delete_folder(&mut s, &copies).await.unwrap();
    imap::delete_folder(&mut s, &missing).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn sieve_local_edit_reuploads_and_legacy_hash_converges() {
    if !common::enabled() {
        return;
    }
    let cfg = common::config(Security::Tls);
    let (_d, store, acc, _s) = setup().await;
    let tag = common::unique("sievever");
    let mut rule = subject_rule(&tag, vec![Action::Flag]);
    rule.server_side = true;
    store.save_rules(&[rule]).unwrap();

    let first = neutron_core::sieve_state::upload(&store, &cfg, common::PASSWORD, acc, 10419)
        .await
        .unwrap();
    let stored = store.sieve_settings(acc).unwrap();
    assert!(stored.active);
    assert_eq!(
        stored.script_hash.as_deref(),
        Some(neutron_core::sieve_state::script_hash(&first.script).as_str())
    );

    let mut ms = managesieve::Client::connect(&cfg, 10419, common::PASSWORD)
        .await
        .unwrap();
    let remote = ms.get_script(managesieve::SCRIPT_NAME).await.unwrap();
    assert_eq!(
        neutron_core::sieve_state::script_hash(&remote),
        stored.script_hash.clone().unwrap()
    );

    let mut rules = store.rules().unwrap();
    rules[0].actions.push(Action::MarkRead);
    store.save_rules(&rules).unwrap();
    let desired = neutron_core::sieve_state::desired_script(&store, acc).unwrap();
    assert!(neutron_core::sieve_state::is_stale(
        &store.sieve_settings(acc).unwrap(),
        &desired.script
    ));
    let still_old = ms.get_script(managesieve::SCRIPT_NAME).await.unwrap();
    assert_eq!(
        neutron_core::sieve_state::script_hash(&still_old),
        neutron_core::sieve_state::script_hash(&first.script)
    );

    assert_eq!(
        neutron_core::sieve_state::reconcile(&store, &cfg, common::PASSWORD, acc)
            .await
            .unwrap(),
        neutron_core::sieve_state::SieveReconcile::Uploaded
    );
    let updated = ms.get_script(managesieve::SCRIPT_NAME).await.unwrap();
    let hash = neutron_core::sieve_state::script_hash(&updated);
    assert_eq!(
        store.sieve_settings(acc).unwrap().script_hash.as_deref(),
        Some(hash.as_str())
    );
    assert_ne!(hash, neutron_core::sieve_state::script_hash(&first.script));
    assert!(updated.contains(r#"\\Seen"#), "{updated}");
    assert_eq!(
        neutron_core::sieve_state::reconcile(&store, &cfg, common::PASSWORD, acc)
            .await
            .unwrap(),
        neutron_core::sieve_state::SieveReconcile::Current
    );

    store
        .set_sieve_settings(
            acc,
            SieveSettings {
                port: 10419,
                active: true,
                script_hash: None,
            },
        )
        .unwrap();
    assert_eq!(
        neutron_core::sieve_state::reconcile(&store, &cfg, common::PASSWORD, acc)
            .await
            .unwrap(),
        neutron_core::sieve_state::SieveReconcile::Current
    );
    assert!(store.sieve_settings(acc).unwrap().script_hash.is_some());

    ms.set_active("").await.unwrap();
    ms.delete_script(managesieve::SCRIPT_NAME).await.unwrap();
    ms.logout().await;
}
