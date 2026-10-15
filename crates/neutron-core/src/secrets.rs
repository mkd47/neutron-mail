//! Password storage. Uses the freedesktop Secret Service (GNOME Keyring) via
//! the `keyring` crate. If no Secret Service is reachable, falls back to an
//! in-process memory store: the password then works for the current session
//! only and is never written to disk in plain text.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{Error, Result};

const SERVICE: &str = "dev.neutronmail.NeutronMail";
const CACHE_KEY_USER: &str = "cache-key";

/// Raw 256-bit SQLCipher key, kept as hex and wiped from memory on drop.
pub struct CacheKey(zeroize::Zeroizing<String>);

impl CacheKey {
    pub fn random() -> Result<Self> {
        let mut b = zeroize::Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut b[..]).map_err(|e| Error::Secret(format!("random: {e}")))?;
        Ok(Self(zeroize::Zeroizing::new(
            b.iter().map(|x| format!("{x:02x}")).collect(),
        )))
    }
    pub fn from_hex(s: &str) -> Option<Self> {
        (s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            .then(|| Self(zeroize::Zeroizing::new(s.to_ascii_lowercase())))
    }
    pub fn hex(&self) -> &zeroize::Zeroizing<String> {
        &self.0
    }
    /// SQLCipher raw-key literal: x'<64 hex>' (no KDF needed for a random key).
    pub(crate) fn pragma_value(&self) -> zeroize::Zeroizing<String> {
        zeroize::Zeroizing::new(format!("\"x'{}'\"", self.0.as_str()))
    }
}

impl std::fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CacheKey(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Keyring,
    Memory,
}

pub struct SecretStore {
    backend: Backend,
    memory: Mutex<HashMap<String, String>>,
}

impl SecretStore {
    /// Probe the Secret Service once; choose memory fallback if unavailable.
    pub fn detect() -> Self {
        let backend = match keyring::Entry::new(SERVICE, "__probe__") {
            Ok(e) => match e.get_password() {
                Ok(_) | Err(keyring::Error::NoEntry) => Backend::Keyring,
                Err(err) => {
                    tracing::warn!("Secret Service unavailable, using memory fallback: {err}");
                    Backend::Memory
                }
            },
            Err(err) => {
                tracing::warn!("Secret Service unavailable, using memory fallback: {err}");
                Backend::Memory
            }
        };
        Self::with_backend(backend)
    }

    pub fn with_backend(backend: Backend) -> Self {
        Self {
            backend,
            memory: Mutex::new(HashMap::new()),
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    fn key(account_id: i64) -> String {
        format!("account-{account_id}")
    }

    pub fn set(&self, account_id: i64, password: &str) -> Result<()> {
        match self.backend {
            Backend::Keyring => keyring::Entry::new(SERVICE, &Self::key(account_id))
                .and_then(|e| e.set_password(password))
                .map_err(|e| Error::Secret(e.to_string())),
            Backend::Memory => {
                self.mem()
                    .insert(Self::key(account_id), password.to_owned());
                Ok(())
            }
        }
    }

    pub fn get(&self, account_id: i64) -> Result<String> {
        match self.backend {
            Backend::Keyring => keyring::Entry::new(SERVICE, &Self::key(account_id))
                .and_then(|e| e.get_password())
                .map_err(|e| match e {
                    keyring::Error::NoEntry => Error::NotFound("stored password".into()),
                    other => Error::Secret(other.to_string()),
                }),
            Backend::Memory => self
                .mem()
                .get(&Self::key(account_id))
                .cloned()
                .ok_or_else(|| {
                    Error::NotFound("password (memory store; re-enter after restart)".into())
                }),
        }
    }

    pub fn delete(&self, account_id: i64) -> Result<()> {
        match self.backend {
            Backend::Keyring => match keyring::Entry::new(SERVICE, &Self::key(account_id))
                .and_then(|e| e.delete_credential())
            {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(Error::Secret(e.to_string())),
            },
            Backend::Memory => {
                self.mem().remove(&Self::key(account_id));
                Ok(())
            }
        }
    }

    /// The 256-bit key of the encrypted mail cache, created on first use.
    /// Only available with a real keyring: with the memory fallback the key
    /// would be lost on exit, so callers keep the cache in memory instead.
    pub fn cache_key(&self) -> Result<Option<CacheKey>> {
        if self.backend != Backend::Keyring {
            return Ok(None);
        }
        let entry = keyring::Entry::new(SERVICE, CACHE_KEY_USER)
            .map_err(|e| Error::Secret(e.to_string()))?;
        match entry.get_password() {
            Ok(hex) => CacheKey::from_hex(&hex)
                .map(Some)
                .ok_or_else(|| Error::Secret("stored cache key is malformed".into())),
            Err(keyring::Error::NoEntry) => {
                let key = CacheKey::random()?;
                entry
                    .set_password(key.hex())
                    .map_err(|e| Error::Secret(e.to_string()))?;
                // Read back: never encrypt data with a key the keyring did not keep.
                match entry.get_password() {
                    Ok(h) if h.as_str() == key.hex().as_str() => Ok(Some(key)),
                    Ok(_) => Err(Error::Secret(
                        "keyring returned a different cache key".into(),
                    )),
                    Err(e) => Err(Error::Secret(e.to_string())),
                }
            }
            Err(e) => Err(Error::Secret(e.to_string())),
        }
    }

    fn mem(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.memory.lock().unwrap_or_else(|p| p.into_inner())
    }
}
