//! Secret storage for connection passwords.
//!
//! [`KeyringStore`] keeps them in the OS keychain (service `"tulkabd"`,
//! account = connection id). [`MemoryStore`] and [`UnavailableStore`] exist
//! for tests.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Keychain service name under which all secrets are stored.
pub const KEYRING_SERVICE: &str = "tulkabd";

/// Errors reported by a [`SecretStore`].
///
/// Serialized adjacently tagged: `{"kind": "unavailable", "value": "..."}`.
#[derive(Error, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum SecretError {
    /// There is no usable keychain (no backend, locked or denied).
    #[error("secret store unavailable: {0}")]
    Unavailable(String),
    #[error("secret store error: {0}")]
    Other(String),
}

/// Storage for secrets, keyed by connection id.
pub trait SecretStore: Send + Sync {
    fn set(&self, id: &str, secret: &str) -> Result<(), SecretError>;
    /// `Ok(None)` when no secret is stored for `id`.
    fn get(&self, id: &str) -> Result<Option<String>, SecretError>;
    /// Deleting a secret that does not exist is not an error.
    fn delete(&self, id: &str) -> Result<(), SecretError>;
}

/// The OS keychain (Secret Service on Linux, Keychain on macOS, Credential
/// Manager on Windows).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyringStore;

impl KeyringStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(id: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(KEYRING_SERVICE, id).map_err(map_keyring_error)
    }
}

impl SecretStore for KeyringStore {
    fn set(&self, id: &str, secret: &str) -> Result<(), SecretError> {
        Self::entry(id)?
            .set_password(secret)
            .map_err(map_keyring_error)
    }

    fn get(&self, id: &str) -> Result<Option<String>, SecretError> {
        match Self::entry(id)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring_error(e)),
        }
    }

    fn delete(&self, id: &str) -> Result<(), SecretError> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring_error(e)),
        }
    }
}

/// Platform and storage-access failures mean the keychain cannot be used;
/// everything else is a plain error.
fn map_keyring_error(e: keyring::Error) -> SecretError {
    match e {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
            SecretError::Unavailable(e.to_string())
        }
        other => SecretError::Other(other.to_string()),
    }
}

/// In-memory store, for tests.
#[derive(Debug, Default)]
pub struct MemoryStore {
    secrets: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn set(&self, id: &str, secret: &str) -> Result<(), SecretError> {
        self.secrets
            .lock()
            .unwrap()
            .insert(id.to_owned(), secret.to_owned());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<String>, SecretError> {
        Ok(self.secrets.lock().unwrap().get(id).cloned())
    }

    fn delete(&self, id: &str) -> Result<(), SecretError> {
        self.secrets.lock().unwrap().remove(id);
        Ok(())
    }
}

/// A store that is never available, for testing the no-keychain path.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableStore;

impl UnavailableStore {
    fn err() -> SecretError {
        SecretError::Unavailable("no secret store available".into())
    }
}

impl SecretStore for UnavailableStore {
    fn set(&self, _id: &str, _secret: &str) -> Result<(), SecretError> {
        Err(Self::err())
    }

    fn get(&self, _id: &str) -> Result<Option<String>, SecretError> {
        Err(Self::err())
    }

    fn delete(&self, _id: &str) -> Result<(), SecretError> {
        Err(Self::err())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_set_get_delete() {
        let s = MemoryStore::default();
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "pw").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("pw"));
        s.set("a", "pw2").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("pw2"));
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
        // Deleting a missing secret is not an error.
        s.delete("a").unwrap();
    }

    #[test]
    fn unavailable_store_always_unavailable() {
        let s = UnavailableStore;
        assert!(matches!(s.set("a", "x"), Err(SecretError::Unavailable(_))));
        assert!(matches!(s.get("a"), Err(SecretError::Unavailable(_))));
        assert!(matches!(s.delete("a"), Err(SecretError::Unavailable(_))));
    }

    #[test]
    fn keyring_errors_map_to_unavailable() {
        use keyring::Error as K;
        let platform = K::PlatformFailure("no dbus".into());
        assert!(matches!(
            map_keyring_error(platform),
            SecretError::Unavailable(_)
        ));
        let access = K::NoStorageAccess("locked".into());
        assert!(matches!(
            map_keyring_error(access),
            SecretError::Unavailable(_)
        ));
        assert!(matches!(
            map_keyring_error(K::BadEncoding(vec![0xff])),
            SecretError::Other(_)
        ));
    }

    // Touches the real OS keychain; run explicitly with `--ignored`.
    #[test]
    #[ignore]
    fn keyring_store_real_roundtrip() {
        let s = KeyringStore::new();
        let id = format!("test-{}", uuid::Uuid::new_v4());
        s.set(&id, "pw").unwrap();
        assert_eq!(s.get(&id).unwrap().as_deref(), Some("pw"));
        s.delete(&id).unwrap();
        assert_eq!(s.get(&id).unwrap(), None);
    }
}
