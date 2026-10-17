//! Loopback-only protocol regressions: targeted actions must never emit
//! an unscoped EXPUNGE, which could delete other clients' messages.
//! Window synchronization must follow the server's INTERNALDATE search.
//! No live mailbox, credentials, or NEUTRON_IT opt-in is required.

use std::sync::Arc;
use std::time::Duration;

use neutron_core::imap;
use neutron_core::model::{AccountConfig, Security};
use neutron_core::store::{FolderSyncState, NewMessage, Store};
use neutron_core::sync::{self, SyncMode};
use rustls_pki_types::pem::PemObject;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn mock_server(
    capabilities: &'static str,
) -> (imap::Session, tokio::task::JoinHandle<Vec<String>>) {
    mock_server_with(capabilities, |command| {
        if command.starts_with("SELECT ") {
            "* FLAGS (\\Seen \\Deleted)\r\n* 2 EXISTS\r\n* OK [UIDVALIDITY 123] valid\r\n* OK [UIDNEXT 10] next\r\n".into()
        } else {
            String::new()
        }
    })
    .await
}

async fn mock_server_with(
    capabilities: &'static str,
    response: impl FnMut(&str) -> String + Send + 'static,
) -> (imap::Session, tokio::task::JoinHandle<Vec<String>>) {
    let (config, server) = mock_account(capabilities, response).await;
    let session = imap::connect(&config, "test-only-password").await.unwrap();
    (session, server)
}

async fn mock_account(
    capabilities: &'static str,
    mut response: impl FnMut(&str) -> String + Send + 'static,
) -> (AccountConfig, tokio::task::JoinHandle<Vec<String>>) {
    let certs = rustls_pki_types::CertificateDer::pem_file_iter(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/imap/server.pem"
    ))
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap();
    let key = rustls_pki_types::PrivateKeyDer::from_pem_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/imap/server-key.pem"
    ))
    .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(tcp).await.unwrap();
        let (read, mut write) = tokio::io::split(tls);
        let mut read = BufReader::new(read);
        write
            .write_all(b"* OK loopback test server\r\n")
            .await
            .unwrap();
        let mut commands = Vec::new();
        loop {
            let mut line = String::new();
            if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                break;
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            commands.push(command.to_string());
            if command == "CAPABILITY" {
                let caps = if capabilities.is_empty() {
                    "IMAP4rev1".to_string()
                } else {
                    format!("IMAP4rev1 {capabilities}")
                };
                write
                    .write_all(format!("* CAPABILITY {caps}\r\n").as_bytes())
                    .await
                    .unwrap();
            } else {
                let reply = response(command);
                if let Some((prefix, reason)) = reply.split_once("TAGGED-NO ") {
                    write.write_all(prefix.as_bytes()).await.unwrap();
                    write
                        .write_all(format!("{tag} NO {reason}\r\n").as_bytes())
                        .await
                        .unwrap();
                    continue;
                }
                if let Some(prefix) = reply.strip_suffix("DISCONNECT") {
                    write.write_all(prefix.as_bytes()).await.unwrap();
                    break;
                }
                write.write_all(reply.as_bytes()).await.unwrap();
            }
            write
                .write_all(format!("{tag} OK done\r\n").as_bytes())
                .await
                .unwrap();
        }
        commands
    });
    (test_config(port), server)
}

fn test_config(port: u16) -> AccountConfig {
    AccountConfig {
        email: "test@example.test".into(),
        display_name: "Test".into(),
        username: "test".into(),
        imap_host: "127.0.0.1".into(),
        imap_port: port,
        imap_security: Security::Tls,
        smtp_host: "127.0.0.1".into(),
        smtp_port: 465,
        smtp_security: Security::Tls,
        extra_ca_pem: Some(include_str!("fixtures/imap/ca.pem").into()),
        auth: Default::default(),
    }
}

async fn commands_after_drop(
    session: imap::Session,
    server: tokio::task::JoinHandle<Vec<String>>,
) -> Vec<String> {
    drop(session);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("mock server stopped")
        .unwrap()
}

#[tokio::test]
async fn unsupported_move_is_rejected_before_any_mailbox_mutation() {
    let (mut session, server) = mock_server("").await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        imap::move_uids(&mut session, "INBOX", &[7], "Archive"),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(neutron_core::Error::Invalid(_))));
    let commands = commands_after_drop(session, server).await;
    assert!(commands
        .iter()
        .all(|c| c.starts_with("LOGIN ") || c == "CAPABILITY"));
}

#[tokio::test]
async fn unsupported_delete_is_rejected_before_marking_the_message_deleted() {
    let (mut session, server) = mock_server("").await;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        imap::delete_uid(&mut session, "INBOX", 7),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(neutron_core::Error::Invalid(_))));
    let commands = commands_after_drop(session, server).await;
    assert!(commands
        .iter()
        .all(|c| c.starts_with("LOGIN ") || c == "CAPABILITY"));
}

#[tokio::test]
async fn uidplus_move_and_delete_expunge_only_the_requested_uids() {
    let (mut session, server) = mock_server("UIDPLUS").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        imap::move_uids(&mut session, "INBOX", &[7, 8], "Archive")
            .await
            .unwrap();
        imap::delete_uid(&mut session, "INBOX", 9).await.unwrap();
    })
    .await
    .unwrap();
    let commands = commands_after_drop(session, server).await;
    assert!(commands.iter().any(|c| c.starts_with("UID COPY 7,8 ")));
    assert!(commands.iter().any(|c| c == "UID EXPUNGE 7,8"));
    assert!(commands.iter().any(|c| c == "UID EXPUNGE 9"));
    assert!(!commands.iter().any(|c| c == "EXPUNGE"));
}

#[tokio::test]
async fn native_move_does_not_require_uidplus() {
    let (mut session, server) = mock_server("MOVE").await;
    tokio::time::timeout(
        Duration::from_secs(5),
        imap::move_uids(&mut session, "INBOX", &[7], "Archive"),
    )
    .await
    .unwrap()
    .unwrap();
    let commands = commands_after_drop(session, server).await;
    assert!(commands.iter().any(|c| c.starts_with("UID MOVE 7 ")));
    assert!(!commands
        .iter()
        .any(|c| c.contains("EXPUNGE") || c.contains("STORE")));
}

fn window_message(uid: u32) -> String {
    // UIDs 1 and 2 arrived recently despite an old/missing Date header.
    // UID 3 has an old INTERNALDATE despite a future Date header.
    let date = match uid {
        1 => "Date: Wed, 01 Jan 2020 10:00:00 +0000\r\n",
        3 => "Date: Thu, 01 Jan 2099 10:00:00 +0000\r\n",
        _ => "",
    };
    format!("From: sender@example.test\r\nTo: test@example.test\r\nSubject: Message {uid}\r\n{date}\r\nBody {uid}\r\n")
}

fn window_response(command: &str) -> String {
    if command.starts_with("SELECT ") {
        return "* FLAGS (\\Seen)\r\n* 4 EXISTS\r\n* OK [UIDVALIDITY 123] valid\r\n* OK [UIDNEXT 5] next\r\n* OK [HIGHESTMODSEQ 10] modseq\r\n".into();
    }
    if command.starts_with("UID SEARCH SINCE ") {
        return "* SEARCH 1 2 4\r\n".into();
    }
    if command == "UID SEARCH ALL" {
        return "* SEARCH 1 2 3 4\r\n".into();
    }
    if let Some(fetch) = command.strip_prefix("UID FETCH ") {
        let (set, query) = fetch.split_once(' ').unwrap();
        let uids: Vec<u32> = if set == "1:*" {
            vec![1, 2, 3, 4]
        } else {
            set.split(',').map(|s| s.parse().unwrap()).collect()
        };
        return uids
            .into_iter()
            .map(|uid| {
                if query.contains("BODY.PEEK[]") {
                    let raw = window_message(uid);
                    format!(
                        "* {uid} FETCH (UID {uid} FLAGS () BODY[] {{{}}}\r\n{raw})\r\n",
                        raw.len()
                    )
                } else {
                    format!("* {uid} FETCH (UID {uid} FLAGS ())\r\n")
                }
            })
            .collect();
    }
    String::new()
}

fn window_store(cutoff: Option<i64>, cached: &[u32]) -> (Store, i64) {
    let store = Store::open_in_memory().unwrap();
    let account = store.add_account(&test_config(993)).unwrap();
    let folder = store
        .upsert_folder(account, "INBOX", Some("inbox"))
        .unwrap();
    store.set_uidvalidity(folder, 123).unwrap();
    for &uid in cached {
        store
            .insert_message(
                folder,
                &NewMessage {
                    uid,
                    seen: false,
                    flagged: false,
                    raw: window_message(uid).as_bytes(),
                },
            )
            .unwrap();
    }
    store
        .set_folder_sync_state(
            folder,
            Some(FolderSyncState {
                uidnext: 4,
                highestmodseq: Some(10),
                exists: 3,
                window_days: 30,
                window_cutoff: cutoff,
            }),
        )
        .unwrap();
    (store, folder)
}

#[tokio::test]
async fn daily_window_reconciliation_uses_internal_dates_and_keeps_new_mail_rules() {
    let cutoff = (chrono::Utc::now().timestamp().div_euclid(86_400) - 31) * 86_400;
    let (store, folder) = window_store(Some(cutoff), &[1, 2, 3]);
    let (mut session, server) = mock_server_with("CONDSTORE", window_response).await;
    let first = tokio::time::timeout(
        Duration::from_secs(5),
        sync::sync_folder_detailed(&store, folder, "INBOX", &mut session, 30, true),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first.mode, SyncMode::Incremental);
    assert!(first.full_uid_search);
    assert_eq!(first.removed, 1);
    assert_eq!(first.new_uids, vec![4]);
    assert_eq!(store.uids(folder).unwrap(), vec![1, 2, 4]);
    let current = store.folder_sync_state(folder).unwrap().unwrap();
    let since = current.window_cutoff.unwrap();
    assert_eq!(since % 86_400, 0);

    let second = tokio::time::timeout(
        Duration::from_secs(5),
        sync::sync_folder_detailed(&store, folder, "INBOX", &mut session, 30, true),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(second.mode, SyncMode::Incremental);
    assert!(!second.full_uid_search);
    assert_eq!((second.new, second.removed), (0, 0));
    assert_eq!(store.uids(folder).unwrap(), vec![1, 2, 4]);
    let commands = commands_after_drop(session, server).await;
    let searches: Vec<_> = commands
        .iter()
        .filter(|c| c.starts_with("UID SEARCH "))
        .collect();
    assert_eq!(searches.len(), 1, "same-day sync does not repeat SEARCH");
    assert_eq!(
        *searches[0],
        format!(
            "UID SEARCH SINCE {}",
            chrono::DateTime::from_timestamp(since, 0)
                .unwrap()
                .format("%d-%b-%Y")
        )
    );
}

#[tokio::test]
async fn legacy_window_checkpoint_recovers_mail_without_reapplying_old_rules() {
    let (store, folder) = window_store(None, &[2, 3]);
    let (mut session, server) = mock_server_with("CONDSTORE", window_response).await;
    let stats = tokio::time::timeout(
        Duration::from_secs(5),
        sync::sync_folder_detailed(&store, folder, "INBOX", &mut session, 30, true),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(stats.full_uid_search);
    assert_eq!(stats.mode, SyncMode::Incremental);
    assert_eq!((stats.new, stats.removed), (2, 1));
    assert_eq!(store.uids(folder).unwrap(), vec![1, 2, 4]);
    // UID 1 was incorrectly pruned by the old Date-header implementation;
    // restore it, but only UID 4 is a new arrival for automatic rules.
    assert_eq!(stats.new_uids, vec![4]);
    assert!(store
        .folder_sync_state(folder)
        .unwrap()
        .unwrap()
        .window_cutoff
        .is_some());
    commands_after_drop(session, server).await;
}

#[tokio::test]
async fn folder_inventory_is_available_before_first_download() {
    let (cfg, server) = mock_account("", |command| {
        if command.starts_with("LIST ") {
            "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren) \"/\" \"Sales\"\r\n* LIST (\\HasNoChildren) \"/\" {9}\r\n已发送\r\n".into()
        } else if command.starts_with("LSUB ") {
            "* LSUB () \"/\" \"INBOX\"\r\n".into()
        } else if command.starts_with("SELECT ") {
            "* 0 EXISTS\r\n* OK [UIDVALIDITY 123] valid\r\n* OK [UIDNEXT 1] next\r\n".into()
        } else { String::new() }
    }).await;
    let store = Store::open_in_memory().unwrap();
    let account = store.add_account(&cfg).unwrap();
    let listed = std::sync::atomic::AtomicBool::new(false);
    let report =
        sync::sync_account_with_progress(&store, account, &cfg, "test-only-password", |p| {
            if p.stage == "folders" || p.stage == "folder" {
                let fs = store.folders(account).unwrap();
                assert_eq!(
                    fs.len(),
                    3,
                    "all folders must exist before any message download"
                );
                assert!(fs.iter().any(|f| f.display == "已发送"));
                listed.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        })
        .await
        .unwrap();
    assert!(listed.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(report.folders, 3);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn rejected_subscription_listing_does_not_discard_list_results() {
    let (mut s, server) = mock_server_with("", |command| {
        if command.starts_with("LIST ") {
            "* LIST (\\HasChildren \\NoSelect) \".\" \"Root\"\r\n* LIST (\\HasNoChildren) \".\" \"Root.&XfJT0ZAB-\"\r\n* LIST () NIL \"INBOX\"\r\n".into()
        } else if command.starts_with("LSUB ") {
            "TAGGED-NO subscription list unavailable".into()
        } else { String::new() }
    }).await;
    let folders = imap::list_folders(&mut s).await.unwrap();
    assert_eq!(folders.len(), 3);
    assert!(!folders[0].selectable);
    assert_eq!(folders[1].display, "Root.已发送");
    assert_eq!(folders[1].delimiter.as_deref(), Some("."));
    assert!(folders.iter().all(|f| f.subscribed));
    commands_after_drop(s, server).await;
}

#[tokio::test]
async fn incomplete_or_rejected_list_never_prunes_cached_folders() {
    for ending in ["TAGGED-NO listing unavailable", "DISCONNECT"] {
        let (cfg, server) = mock_account("", move |command| {
            if command.starts_with("LIST ") {
                format!("* LIST () \"/\" \"INBOX\"\r\n{ending}")
            } else {
                String::new()
            }
        })
        .await;
        let store = Store::open_in_memory().unwrap();
        let account = store.add_account(&cfg).unwrap();
        let cached = store.upsert_folder(account, "Saved", None).unwrap();
        let result = sync::sync_account(&store, account, &cfg, "test-only-password").await;
        assert!(result.is_err(), "{ending} must fail discovery");
        assert_eq!(store.folders(account).unwrap()[0].id, cached);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn rejected_message_fetch_does_not_advance_sync_checkpoint() {
    let (mut s, server) = mock_server_with("", |command| {
        if command.starts_with("SELECT ") {
            "* 2 EXISTS\r\n* OK [UIDVALIDITY 123] valid\r\n* OK [UIDNEXT 10] next\r\n".into()
        } else if command.starts_with("UID SEARCH ") {
            "* SEARCH 1 2\r\n".into()
        } else if command.starts_with("UID FETCH ") {
            "TAGGED-NO temporarily throttled".into()
        } else {
            String::new()
        }
    })
    .await;
    let store = Store::open_in_memory().unwrap();
    let account = store.add_account(&test_config(993)).unwrap();
    let fid = store
        .upsert_folder(account, "INBOX", Some("inbox"))
        .unwrap();
    assert!(
        sync::sync_folder_detailed(&store, fid, "INBOX", &mut s, 0, false)
            .await
            .is_err()
    );
    assert!(store.folder_sync_state(fid).unwrap().is_none());
    assert!(store.uids(fid).unwrap().is_empty());
    commands_after_drop(s, server).await;
}

fn store_with_inbox(uids: &[u32]) -> (Store, i64, Vec<i64>) {
    let store = Store::open_in_memory().unwrap();
    let account = store.add_account(&test_config(1)).unwrap();
    let fid = store
        .upsert_folder(account, "INBOX", Some("inbox"))
        .unwrap();
    let ids = uids
        .iter()
        .map(|&uid| {
            let raw = format!("Message-ID: <m{uid}@example.test>\r\nSubject: m{uid}\r\n\r\nbody");
            store
                .insert_message(
                    fid,
                    &NewMessage {
                        uid,
                        seen: true,
                        flagged: false,
                        raw: raw.as_bytes(),
                    },
                )
                .unwrap()
        })
        .collect();
    (store, fid, ids)
}

#[tokio::test]
async fn unsupported_permanent_delete_is_rejected_before_marking_anything() {
    let (mut session, server) = mock_server("").await;
    let (store, fid, ids) = store_with_inbox(&[7]);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        neutron_core::folders::delete_permanently(&store, &mut session, &ids),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(neutron_core::Error::Invalid(_))));
    assert_eq!(store.uids(fid).unwrap(), vec![7]);
    let commands = commands_after_drop(session, server).await;
    assert!(commands
        .iter()
        .all(|c| c.starts_with("LOGIN ") || c == "CAPABILITY"));
}

#[tokio::test]
async fn flags_and_permanent_delete_touch_only_the_selected_messages() {
    let (mut session, server) = mock_server("UIDPLUS").await;
    let (store, fid, ids) = store_with_inbox(&[7, 8, 9]);
    let picked = &ids[..2];
    tokio::time::timeout(Duration::from_secs(5), async {
        use neutron_core::folders::{self, MessageFlag};
        folders::set_flag(&store, &mut session, picked, MessageFlag::Flagged, true)
            .await
            .unwrap();
        folders::set_flag(&store, &mut session, picked, MessageFlag::Seen, false)
            .await
            .unwrap();
        let summaries = store.messages(fid, 0, 10).unwrap();
        for m in &summaries {
            let picked = m.uid != 9;
            assert_eq!(m.flagged, picked, "uid {}", m.uid);
            assert_eq!(m.seen, !picked, "uid {}", m.uid);
        }
        let (_, n) = folders::delete_permanently(&store, &mut session, picked)
            .await
            .unwrap();
        assert_eq!(n, 2);
    })
    .await
    .unwrap();
    assert_eq!(store.uids(fid).unwrap(), vec![9]);
    let commands = commands_after_drop(session, server).await;
    assert!(commands
        .iter()
        .any(|c| c == "UID STORE 7,8 +FLAGS.SILENT (\\Flagged)"));
    assert!(commands
        .iter()
        .any(|c| c == "UID STORE 7,8 -FLAGS.SILENT (\\Seen)"));
    assert!(commands
        .iter()
        .any(|c| c == "UID STORE 7,8 +FLAGS.SILENT (\\Deleted)"));
    assert!(commands.iter().any(|c| c == "UID EXPUNGE 7,8"));
    assert!(!commands.iter().any(|c| c == "EXPUNGE"));
}

async fn listed_names(capabilities: &'static str, wire: &'static str) -> Vec<(String, String)> {
    let (mut session, server) = mock_server_with(capabilities, move |command| {
        if command.starts_with("LIST ") || command.starts_with("LSUB ") {
            format!("* LIST () \"/\" INBOX\r\n* LIST () \"/\" \"{wire}\"\r\n")
        } else {
            String::new()
        }
    })
    .await;
    let folders = tokio::time::timeout(Duration::from_secs(5), imap::list_folders(&mut session))
        .await
        .unwrap()
        .unwrap();
    drop(commands_after_drop(session, server).await);
    folders.into_iter().map(|f| (f.name, f.display)).collect()
}

#[tokio::test]
async fn folder_names_with_ampersands_display_correctly_in_both_encodings() {
    // UTF8=ACCEPT: names are plain UTF-8, "&" is literal.
    let utf8 = listed_names("UTF8=ACCEPT", "R&D Team").await;
    assert!(
        utf8.contains(&("R&D Team".into(), "R&D Team".into())),
        "{utf8:?}"
    );
    // Modified UTF-7: "&-" is an escaped "&".
    let mutf7 = listed_names("", "R&-D Team").await;
    assert!(
        mutf7.contains(&("R&-D Team".into(), "R&D Team".into())),
        "{mutf7:?}"
    );
}
