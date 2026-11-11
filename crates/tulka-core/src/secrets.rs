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

    fn entry(id: &str) -> Result<keyring_core::Entry, SecretError> {
        ensure_default_store()?;
        keyring_core::Entry::new(KEYRING_SERVICE, id).map_err(map_keyring_error)
    }
}

/// Installs the platform credential store as the `keyring_core` default the
/// first time it is needed. Connecting can fail (for example without a
/// session bus), so a failure is reported per call and retried on the next
/// one instead of being remembered.
///
/// The stores keep the entry layout of keyring 3: Secret Service items with
/// `service` and `username` attributes, a macOS generic password with that
/// service and account, and a Windows credential named `{user}.{service}`.
fn ensure_default_store() -> Result<(), SecretError> {
    static INSTALL: Mutex<()> = Mutex::new(());
    let _guard = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    if keyring_core::get_default_store().is_some() {
        return Ok(());
    }
    keyring_core::set_default_store(new_platform_store().map_err(map_keyring_error)?);
    Ok(())
}

#[cfg(target_os = "linux")]
fn new_platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn new_platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "windows")]
fn new_platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn new_platform_store() -> keyring_core::Result<std::sync::Arc<keyring_core::CredentialStore>> {
    Err(keyring_core::Error::NoDefaultStore)
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
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring_error(e)),
        }
    }

    fn delete(&self, id: &str) -> Result<(), SecretError> {
        match Self::entry(id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring_error(e)),
        }
    }
}

/// Platform and storage-access failures mean the keychain cannot be used;
/// everything else is a plain error.
fn map_keyring_error(e: keyring_core::Error) -> SecretError {
    match e {
        keyring_core::Error::PlatformFailure(_)
        | keyring_core::Error::NoStorageAccess(_)
        | keyring_core::Error::NoDefaultStore => SecretError::Unavailable(e.to_string()),
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
        use keyring_core::Error as K;
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
