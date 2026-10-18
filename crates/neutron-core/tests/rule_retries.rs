//! Automatic rule runs keep a per-action ledger. A failed attempt retries
//! after a bounded backoff and does not repeat actions that already succeeded.
use neutron_core::model::{AccountConfig, Security};
use neutron_core::rules::{Action, Condition, Field, Op, Rule};
use neutron_core::rules_exec::action_ids;
use neutron_core::store::{
    rule_backoff_secs, AccountSettings, RuleAdmission, SieveSettings, Store, MAX_RULE_ATTEMPTS,
};

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

fn rule(actions: Vec<Action>) -> Rule {
    Rule {
        id: 1,
        name: "file invoices".into(),
        enabled: true,
        match_all: true,
        conditions: vec![Condition {
            field: Field::Subject,
            op: Op::Contains,
            value: "invoice".into(),
            header: String::new(),
        }],
        actions,
        server_side: false,
    }
}

#[test]
fn a_forward_that_was_not_sent_returns_its_hourly_slot() {
    let store = Store::open_in_memory().unwrap();
    let account = store.add_account(&cfg()).unwrap();
    let limit = neutron_core::rules::MAX_FORWARDS_PER_HOUR;
    // Nothing reserved is not an error and does not open a slot.
    store.refund_forward_budget(account).unwrap();
    for _ in 0..limit {
        assert!(store.take_forward_budget(account, limit).unwrap());
    }
    assert!(!store.take_forward_budget(account, limit).unwrap());
    store.refund_forward_budget(account).unwrap();
    assert!(store.take_forward_budget(account, limit).unwrap());
    assert!(!store.take_forward_budget(account, limit).unwrap());
}

#[test]
fn backoff_grows_and_stays_bounded() {
    assert_eq!(rule_backoff_secs(1), 60);
    assert_eq!(rule_backoff_secs(2), 120);
    assert_eq!(rule_backoff_secs(3), 240);
    assert_eq!(rule_backoff_secs(4), 480);
    assert_eq!(rule_backoff_secs(5), 960);
    assert_eq!(rule_backoff_secs(9), 960);
}

#[test]
fn retry_skips_actions_that_already_succeeded_then_gives_up() {
    let store = Store::open_in_memory().unwrap();
    let account = store.add_account(&cfg()).unwrap();
    let now = 1_700_000_000;
    let key = "mid:<m1@example.test>";

    match store.admit_rule_run(account, key, 4, 9, now).unwrap() {
        RuleAdmission::Proceed { done } => assert!(done.is_empty()),
        other => panic!("first run should proceed, got {other:?}"),
    }
    store
        .record_rule_action(account, key, "forward:accounting@example.test")
        .unwrap();
    store
        .record_rule_action(account, key, "copy:Invoices")
        .unwrap();

    let failure = store
        .fail_rule_run(account, key, "move: no folder \"Archive\"", now)
        .unwrap();
    assert_eq!((failure.attempts, failure.gave_up), (1, false));
    assert_eq!(failure.next_try, Some(now + rule_backoff_secs(1)));
    assert!(matches!(
        store.admit_rule_run(account, key, 4, 9, now).unwrap(),
        RuleAdmission::Waiting { .. }
    ));
    assert_eq!(
        store.due_rule_uids(account, 4, now).unwrap(),
        Vec::<u32>::new()
    );
    store.release_rule_retries(account).unwrap();
    match store.admit_rule_run(account, key, 4, 9, now).unwrap() {
        RuleAdmission::Proceed { done } => {
            assert_eq!(
                done,
                vec![
                    "forward:accounting@example.test".to_string(),
                    "copy:Invoices".to_string(),
                ]
            );
        }
        other => panic!("due retry should proceed with progress kept, got {other:?}"),
    }
    assert_eq!(store.due_rule_uids(account, 4, now).unwrap(), vec![9]);

    let mut last = failure;
    for step in 2..=MAX_RULE_ATTEMPTS {
        last = store
            .fail_rule_run(
                account,
                key,
                "move: no folder \"Archive\"",
                now + step * 100,
            )
            .unwrap();
    }
    assert!(last.gave_up);
    assert_eq!(last.attempts, MAX_RULE_ATTEMPTS);
    assert!(matches!(
        store
            .admit_rule_run(account, key, 4, 9, now + 1_000_000)
            .unwrap(),
        RuleAdmission::Finished
    ));
    let state = store.rule_run(account, key).unwrap().unwrap();
    assert_eq!(state.status, "failed");
    assert!(state.done.iter().any(|d| d.starts_with("forward:")));
    assert!(state.done.iter().any(|d| d.starts_with("copy:")));
    assert_eq!(
        store.unreported_rule_failures(account).unwrap(),
        vec!["move: no folder \"Archive\"".to_string()]
    );
    store.mark_rule_failures_reported(account).unwrap();
    assert!(store.unreported_rule_failures(account).unwrap().is_empty());

    // A later success is claimed and is not selected again.
    let done_key = "mid:<m2@example.test>";
    store.admit_rule_run(account, done_key, 4, 10, now).unwrap();
    store.complete_rule_run(account, done_key).unwrap();
    assert!(matches!(
        store.admit_rule_run(account, done_key, 4, 10, now).unwrap(),
        RuleAdmission::Finished
    ));
}

#[test]
fn editing_a_server_rule_makes_the_stored_script_stale() {
    let store = Store::open_in_memory().unwrap();
    let account = store.add_account(&cfg()).unwrap();
    let mut filed = rule(vec![Action::Flag]);
    filed.server_side = true;
    store.save_rules(&[filed]).unwrap();
    let first = neutron_core::sieve_state::desired_script(&store, account).unwrap();
    let hash = neutron_core::sieve_state::script_hash(&first.script);
    store
        .set_sieve_settings(
            account,
            SieveSettings {
                port: 4190,
                active: true,
                script_hash: Some(hash),
            },
        )
        .unwrap();
    assert!(!neutron_core::sieve_state::is_stale(
        &store.sieve_settings(account).unwrap(),
        &first.script
    ));
    let mut rules = store.rules().unwrap();
    rules[0].actions.push(Action::MarkRead);
    store.save_rules(&rules).unwrap();
    let next = neutron_core::sieve_state::desired_script(&store, account).unwrap();
    assert_ne!(first.script, next.script);
    assert!(neutron_core::sieve_state::is_stale(
        &store.sieve_settings(account).unwrap(),
        &next.script
    ));
}

#[test]
fn action_ids_match_the_ledger_keys() {
    let facts_rule = rule(vec![
        Action::MarkRead,
        Action::Flag,
        Action::Copy {
            folder: "Invoices".into(),
        },
        Action::Forward {
            to: "accounting@example.test".into(),
        },
        Action::Delete,
    ]);
    let plan = neutron_core::rules::plan(
        [&facts_rule],
        &neutron_core::rules::MessageFacts::from_raw(
            b"From: Elena <elena@example.org>\r\nTo: me@example.test\r\nSubject: invoice 12\r\n\r\nPay.\r\n",
            "me@example.test",
        ),
        "Inbox",
    )
    .unwrap();
    assert_eq!(
        action_ids(&plan),
        vec![
            "seen",
            "flagged",
            "copy:Invoices",
            "forward:accounting@example.test",
            "delete",
        ]
    );
}

#[test]
fn legacy_rule_claim_stays_finished_after_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let store = Store::open_plaintext(&path).unwrap();
    let account = store.add_account(&cfg()).unwrap();
    drop(store);

    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "DROP INDEX IF EXISTS rule_runs_due;
         ALTER TABLE rule_runs DROP COLUMN status;
         ALTER TABLE rule_runs DROP COLUMN folder_id;
         ALTER TABLE rule_runs DROP COLUMN uid;
         ALTER TABLE rule_runs DROP COLUMN done_actions;
         ALTER TABLE rule_runs DROP COLUMN attempts;
         ALTER TABLE rule_runs DROP COLUMN next_try;
         ALTER TABLE rule_runs DROP COLUMN last_error;
         ALTER TABLE rule_runs DROP COLUMN reported;",
    )
    .unwrap();
    db.execute(
        "INSERT INTO rule_runs (account_id, message_key, at) VALUES (?1, ?2, 10)",
        rusqlite::params![account, "mid:<legacy@example.test>"],
    )
    .unwrap();
    drop(db);

    let store = Store::open_plaintext(&path).unwrap();
    assert!(matches!(
        store
            .admit_rule_run(account, "mid:<legacy@example.test>", 1, 2, 50)
            .unwrap(),
        RuleAdmission::Finished
    ));
    // A message the old cache never saw is still eligible, and its progress
    // round-trips through the migrated table.
    match store
        .admit_rule_run(account, "mid:<new@example.test>", 1, 8, 50)
        .unwrap()
    {
        RuleAdmission::Proceed { done } => assert!(done.is_empty()),
        other => panic!("{other:?}"),
    }
    store
        .record_rule_action(account, "mid:<new@example.test>", "copy:Invoices")
        .unwrap();
    drop(store);
    let store = Store::open_plaintext(&path).unwrap();
    match store
        .admit_rule_run(account, "mid:<new@example.test>", 1, 8, 50)
        .unwrap()
    {
        RuleAdmission::Proceed { done } => assert_eq!(done, vec!["copy:Invoices".to_string()]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn sieve_hash_survives_other_settings_and_legacy_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let store = Store::open_plaintext(&path).unwrap();
    let account = store.add_account(&cfg()).unwrap();
    let saved = store
        .set_account_settings(
            account,
            &AccountSettings {
                signature_html: "<p>Hi</p>".into(),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .set_sieve_settings(
            account,
            SieveSettings {
                port: 4190,
                active: true,
                script_hash: Some("abc".into()),
            },
        )
        .unwrap();
    assert_eq!(
        store.account_settings(account).unwrap().signature_html,
        saved.signature_html
    );
    store.set_account_settings(account, &saved).unwrap();
    assert_eq!(
        store
            .sieve_settings(account)
            .unwrap()
            .script_hash
            .as_deref(),
        Some("abc")
    );
    drop(store);

    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("ALTER TABLE account_settings DROP COLUMN sieve_hash")
        .unwrap();
    drop(db);

    let store = Store::open_plaintext(&path).unwrap();
    let settings = store.sieve_settings(account).unwrap();
    assert!(settings.active);
    assert_eq!(settings.port, 4190);
    assert_eq!(settings.script_hash, None);
    assert_eq!(
        store.account_settings(account).unwrap().signature_html,
        saved.signature_html
    );
    assert!(neutron_core::sieve_state::is_stale(&settings, "keep;\n"));
    store
        .set_sieve_settings(
            account,
            SieveSettings {
                port: 4190,
                active: false,
                script_hash: Some("abc".into()),
            },
        )
        .unwrap();
    let off = store.sieve_settings(account).unwrap();
    assert!(!off.active);
    assert_eq!(off.script_hash, None);
}
