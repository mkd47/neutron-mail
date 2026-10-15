//! Local-only conditions must still run when an account enables Sieve.
use neutron_core::model::{AccountConfig, Security};
use neutron_core::rules::{Action, Condition, Field, Op, Rule};
use neutron_core::rules_exec::local_rules;
use neutron_core::store::{SieveSettings, Store};

#[test]
fn sieve_keeps_unexportable_rules_local_and_disabled_rules_off() {
    let store = Store::open_in_memory().unwrap();
    let account = store
        .add_account(&AccountConfig {
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
        })
        .unwrap();
    let portable = Rule {
        id: 0,
        name: "portable".into(),
        enabled: true,
        match_all: true,
        conditions: vec![Condition {
            field: Field::Subject,
            op: Op::Contains,
            value: "invoice".into(),
            header: String::new(),
        }],
        actions: vec![Action::Flag],
        server_side: true,
    };
    let mut body_equals = portable.clone();
    body_equals.name = "body_equals".into();
    body_equals.conditions[0] = Condition {
        field: Field::Body,
        op: Op::Equals,
        value: "invoice".into(),
        header: String::new(),
    };
    let mut local = portable.clone();
    local.name = "local".into();
    local.server_side = false;
    let mut disabled = body_equals.clone();
    disabled.name = "disabled".into();
    disabled.enabled = false;
    store
        .save_rules(&[portable, body_equals, local, disabled])
        .unwrap();

    let names = || {
        local_rules(&store, account)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(), ["portable", "body_equals", "local"]);
    store
        .set_sieve_settings(
            account,
            SieveSettings {
                port: 4190,
                active: true,
            },
        )
        .unwrap();
    assert_eq!(names(), ["body_equals", "local"]);
    store
        .set_sieve_settings(
            account,
            SieveSettings {
                port: 4190,
                active: false,
            },
        )
        .unwrap();
    assert_eq!(names(), ["portable", "body_equals", "local"]);
}
