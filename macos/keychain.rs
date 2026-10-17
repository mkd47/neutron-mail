//! Run only on a disposable macOS CI keychain, never on a user's keychain.

use neutron_core::secrets::{Backend, SecretStore};

#[test]
fn apple_keychain_persists_credentials_and_cache_key() {
    assert_eq!(
        std::env::var("NEUTRON_MACOS_TEST_KEYCHAIN").as_deref(),
        Ok("1")
    );
    let service = format!("dev.neutronmail.ci.{}", std::process::id());
    let entry = keyring::Entry::new(&service, "roundtrip").unwrap();
    entry.set_password("ci-only-no-mail-credentials").unwrap();
    assert_eq!(
        keyring::Entry::new(&service, "roundtrip")
            .unwrap()
            .get_password()
            .unwrap(),
        "ci-only-no-mail-credentials"
    );
    entry.delete_credential().unwrap();
    assert!(matches!(entry.get_password(), Err(keyring::Error::NoEntry)));

    let first = SecretStore::detect();
    assert_eq!(first.backend(), Backend::Keyring);
    let key = first.cache_key().unwrap().expect("real Keychain backend");
    let second = SecretStore::detect();
    assert_eq!(
        key.hex().as_str(),
        second.cache_key().unwrap().unwrap().hex().as_str()
    );
    // The packaged app is a different signing identity from this test.
    // Let its launch test create its own item, avoiding a Keychain ACL prompt.
    keyring::Entry::new("dev.neutronmail.NeutronMail", "cache-key")
        .unwrap()
        .delete_credential()
        .unwrap();
}
