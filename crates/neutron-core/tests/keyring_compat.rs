//! Passwords and the SQLCipher key written by keyring 3.6.3 must still be
//! found by keyring 4. Skipped unless `NEUTRON_KEYRING=1` (see
//! `testserver/keyring-test.sh`, which unlocks a throwaway Secret Service).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use neutron_core::secrets::{Backend, SecretStore};
use neutron_core::Error;

const SERVICE: &str = "dev.neutronmail.NeutronMail";

fn enabled() -> bool {
    let on = std::env::var("NEUTRON_KEYRING").as_deref() == Ok("1");
    if !on {
        eprintln!("skipping: set NEUTRON_KEYRING=1 inside an unlocked Secret Service session");
    }
    on
}

fn unique_account() -> i64 {
    static N: AtomicU64 = AtomicU64::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // Stay positive and clear of any account id a real profile would use.
    ((nanos ^ (n << 32) ^ (std::process::id() as u64)) & 0x7fff_ffff_ffff) as i64
}

/// An item shaped the way keyring 3.6.3 (`sync-secret-service`) wrote one:
/// attributes `service`, `username`, `target=default`, `application=rust-keyring`,
/// label `{user}@{service}:default (keyring v…)`, secret content type `text/plain`.
fn plant_keyring3_item(username: &str, secret: &[u8]) {
    let username = username.to_owned();
    let secret = secret.to_vec();
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let ss = secret_service::blocking::SecretService::connect(
                    secret_service::EncryptionType::Dh,
                )
                .expect("connect to the Secret Service");
                let collection = ss
                    .get_default_collection()
                    .expect("default collection (login keyring)");
                if collection.is_locked().expect("lock state") {
                    panic!("default collection is locked; unlock it before planting an item");
                }
                let mut attributes = HashMap::new();
                attributes.insert("service", SERVICE);
                attributes.insert("username", username.as_str());
                attributes.insert("target", "default");
                attributes.insert("application", "rust-keyring");
                let label = format!("{username}@{SERVICE}:default (keyring v3)");
                collection
                    .create_item(&label, attributes, &secret, true, "text/plain")
                    .expect("create keyring 3 shaped item");
            })
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    });
}

fn delete_username(username: &str) {
    let username = username.to_owned();
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                if let Ok(entry) = keyring::v1::Entry::new(SERVICE, &username) {
                    let _ = entry.delete_credential();
                }
            })
            .join()
            .ok();
    });
}

struct DeleteOnDrop {
    username: String,
}

impl Drop for DeleteOnDrop {
    fn drop(&mut self) {
        delete_username(&self.username);
    }
}

#[test]
fn keyring3_cache_key_is_reused_not_replaced() {
    if !enabled() {
        return;
    }
    let already = std::thread::scope(|scope| {
        scope
            .spawn(|| keyring::v1::Entry::new(SERVICE, "cache-key").and_then(|e| e.get_password()))
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    });
    if already.is_ok() {
        panic!("a cache-key entry already exists; refusing to overwrite it");
    }
    if let Err(err) = &already {
        if !matches!(err, keyring::v1::Error::NoEntry) {
            panic!("Secret Service probe failed: {err}");
        }
    }
    let _cleanup = DeleteOnDrop {
        username: "cache-key".into(),
    };
    let hex = "ab".repeat(32);
    plant_keyring3_item("cache-key", hex.as_bytes());
    let store = SecretStore::detect();
    assert_eq!(store.backend(), Backend::Keyring);
    let key = store
        .cache_key()
        .expect("read the stored cache key")
        .expect("keyring backend returns a key");
    assert_eq!(
        key.hex().as_str(),
        hex,
        "an existing SQLCipher key must be returned unchanged"
    );
    // A second read must not rotate it.
    let again = store.cache_key().unwrap().unwrap();
    assert_eq!(again.hex().as_str(), hex);
}

#[tokio::test]
async fn keyring3_password_round_trips_from_the_async_runtime() {
    if !enabled() {
        return;
    }
    let id = unique_account();
    let username = format!("account-{id}");
    let _cleanup = DeleteOnDrop {
        username: username.clone(),
    };
    let password = format!("legacy-password-{id}");
    plant_keyring3_item(&username, password.as_bytes());

    let store = SecretStore::detect();
    assert_eq!(
        store.backend(),
        Backend::Keyring,
        "Secret Service probe failed inside the test runtime"
    );
    assert_eq!(store.get(id).unwrap(), password);

    store.set(id, "rotated-after-upgrade").unwrap();
    assert_eq!(store.get(id).unwrap(), "rotated-after-upgrade");

    store.delete(id).unwrap();
    match store.get(id) {
        Err(Error::NotFound(_)) => {}
        other => panic!("deleted password should be gone, got {other:?}"),
    }
}
