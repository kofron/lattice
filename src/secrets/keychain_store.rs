//! secrets::keychain_store
//!
//! Keychain-based secret storage using the OS keychain.
//!
//! # Platform Support
//!
//! This module uses the `keyring` crate which supports:
//! - macOS: Keychain
//! - Windows: Credential Manager
//! - Linux: Secret Service (via D-Bus)
//!
//! # Feature Flag
//!
//! This module is only available with the `keychain` feature flag:
//!
//! ```toml
//! lattice = { version = "0.1", features = ["keychain"] }
//! ```
//!
//! # Example
//!
//! ```ignore
//! use latticework::secrets::{KeychainSecretStore, SecretStore};
//!
//! let store = KeychainSecretStore::new()?;
//! store.set("github.pat", "ghp_xxxx...")?;
//!
//! if let Some(token) = store.get("github.pat")? {
//!     // Use token...
//! }
//! ```

#[cfg(feature = "keychain")]
use keyring::Entry;

use super::traits::{SecretError, SecretStore};

/// Keychain-based secret storage.
///
/// Uses the OS keychain (macOS Keychain, Windows Credential Manager,
/// Linux Secret Service) via the `keyring` crate.
///
/// This is only available when compiled with the `keychain` feature.
///
/// # Example
///
/// ```ignore
/// use latticework::secrets::{KeychainSecretStore, SecretStore};
///
/// let store = KeychainSecretStore::new()?;
///
/// // Store a token
/// store.set("github.pat", "ghp_xxxxx...")?;
///
/// // Retrieve it
/// if let Some(token) = store.get("github.pat")? {
///     println!("Token found (not printing it!)");
/// }
///
/// // Delete when done
/// store.delete("github.pat")?;
/// ```
#[cfg(feature = "keychain")]
#[derive(Debug)]
pub struct KeychainSecretStore {
    /// Service name for keychain entries
    service: String,
}

#[cfg(feature = "keychain")]
impl KeychainSecretStore {
    /// Create a new keychain secret store.
    ///
    /// Uses "lattice" as the service name for all keychain entries.
    pub fn new() -> Result<Self, SecretError> {
        Ok(Self {
            service: "lattice".to_string(),
        })
    }

    /// Create a new keychain secret store with a custom service name.
    ///
    /// This is primarily useful for testing to avoid conflicts.
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// Get the service name.
    pub fn service(&self) -> &str {
        &self.service
    }

    /// Create a keyring entry for the given key.
    fn entry(&self, key: &str) -> Result<Entry, SecretError> {
        Entry::new(&self.service, key)
            .map_err(|e| SecretError::ReadError(format!("cannot create keyring entry: {}", e)))
    }
}

#[cfg(feature = "keychain")]
impl Default for KeychainSecretStore {
    fn default() -> Self {
        Self::new().expect("failed to create KeychainSecretStore")
    }
}

#[cfg(feature = "keychain")]
impl SecretStore for KeychainSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let entry = self.entry(key)?;

        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(keyring::Error::Ambiguous(_)) => {
                // Multiple entries found - this shouldn't happen with our usage
                Err(SecretError::ReadError(
                    "ambiguous keychain entry".to_string(),
                ))
            }
            Err(e) => Err(SecretError::ReadError(format!(
                "cannot read from keychain: {}",
                e
            ))),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let entry = self.entry(key)?;

        entry
            .set_password(value)
            .map_err(|e| SecretError::WriteError(format!("cannot write to keychain: {}", e)))
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        let entry = self.entry(key)?;

        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already gone, that's fine
            Err(e) => Err(SecretError::DeleteError(format!(
                "cannot delete from keychain: {}",
                e
            ))),
        }
    }
}

// Stub implementation when keychain feature is disabled
#[cfg(not(feature = "keychain"))]
#[derive(Debug)]
pub struct KeychainSecretStore {
    _private: (),
}

#[cfg(not(feature = "keychain"))]
impl KeychainSecretStore {
    /// Create a new keychain secret store.
    ///
    /// Always fails when compiled without the `keychain` feature.
    pub fn new() -> Result<Self, SecretError> {
        Err(SecretError::ReadError(
            "keychain support not enabled (compile with --features keychain)".into(),
        ))
    }
}

#[cfg(not(feature = "keychain"))]
impl SecretStore for KeychainSecretStore {
    fn get(&self, _key: &str) -> Result<Option<String>, SecretError> {
        Err(SecretError::ReadError("keychain not available".into()))
    }

    fn set(&self, _key: &str, _value: &str) -> Result<(), SecretError> {
        Err(SecretError::WriteError("keychain not available".into()))
    }

    fn delete(&self, _key: &str) -> Result<(), SecretError> {
        Err(SecretError::DeleteError("keychain not available".into()))
    }
}

#[cfg(all(test, feature = "keychain"))]
mod tests {
    use super::*;

    // Note: These tests interact with the real system keychain.
    // They use a unique service name to avoid conflicts.

    fn test_service() -> String {
        format!("lattice-test-{}", std::process::id())
    }

    fn cleanup_test_entry(service: &str, key: &str) {
        if let Ok(entry) = Entry::new(service, key) {
            let _ = entry.delete_credential();
        }
    }

    #[test]
    fn service_accessor() {
        let store = KeychainSecretStore::with_service("test-service");
        assert_eq!(store.service(), "test-service");
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let service = test_service();
        let store = KeychainSecretStore::with_service(&service);

        // Clean up first in case of previous failed test
        cleanup_test_entry(&service, "nonexistent");

        let result = store.get("nonexistent").expect("get");
        assert!(result.is_none());
    }

    #[test]
    fn set_and_get() {
        let service = test_service();
        let store = KeychainSecretStore::with_service(&service);
        let key = "test-key";

        // Clean up first
        cleanup_test_entry(&service, key);

        // Set and get
        store.set(key, "test_value").expect("set");
        let result = store.get(key).expect("get");
        assert_eq!(result, Some("test_value".to_string()));

        // Clean up
        cleanup_test_entry(&service, key);
    }

    #[test]
    fn delete_existing() {
        let service = test_service();
        let store = KeychainSecretStore::with_service(&service);
        let key = "test-delete";

        // Clean up first
        cleanup_test_entry(&service, key);

        // Set, then delete
        store.set(key, "value").expect("set");
        store.delete(key).expect("delete");

        let result = store.get(key).expect("get after delete");
        assert!(result.is_none());
    }

    #[test]
    fn delete_nonexistent_ok() {
        let service = test_service();
        let store = KeychainSecretStore::with_service(&service);

        // Clean up first
        cleanup_test_entry(&service, "nonexistent");

        // Should not error
        store.delete("nonexistent").expect("delete nonexistent");
    }
}

#[cfg(all(test, not(feature = "keychain")))]
mod tests {
    use super::*;

    #[test]
    fn new_fails_without_feature() {
        let result = KeychainSecretStore::new();
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("keychain"));
        assert!(err_str.contains("not enabled"));
    }
}
