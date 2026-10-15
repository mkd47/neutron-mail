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
    mut response: impl FnMut(&str) -> String + Send + 'static,
) -> (imap::Session, tokio::task::JoinHandle<Vec<String>>) {
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
                write.write_all(response(command).as_bytes()).await.unwrap();
            }
            write
                .write_all(format!("{tag} OK done\r\n").as_bytes())
                .await
                .unwrap();
        }
        commands
    });
    let session = imap::connect(&test_config(port), "test-only-password")
        .await
        .unwrap();
    (session, server)
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
