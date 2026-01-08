//! secrets
//!
//! Secret storage abstraction for tokens and credentials.
//!
//! # Architecture
//!
//! Secrets are stored through the `SecretStore` trait, which has
//! multiple implementations:
//!
//! - [`FileSecretStore`]: Stores in `~/.lattice/secrets.toml` (default)
//! - [`KeychainSecretStore`]: Uses OS keychain (optional, feature-gated)
//!
//! # Security
//!
//! All secret store implementations follow these security rules:
//!
//! - Secrets are **never** logged or included in error messages
//! - File store uses 0600 permissions on Unix (owner read/write only)
//! - All writes are atomic (temp file + rename)
//!
//! # Provider Selection
//!
//! Use [`create_store`] to create a secret store based on configuration:
//!
//! ```ignore
//! use lattice::secrets::create_store;
//!
//! // Default file-based store
//! let store = create_store("file")?;
//!
//! // Keychain store (requires feature flag)
//! let store = create_store("keychain")?;
//! ```
//!
//! # Example
//!
//! ```ignore
//! use lattice::secrets::{FileSecretStore, SecretStore};
//!
//! let store = FileSecretStore::new()?;
//!
//! // Store a GitHub token
//! store.set("github.pat", "ghp_xxxx...")?;
//!
//! // Retrieve it later
//! if let Some(token) = store.get("github.pat")? {
//!     // Use token (never print it!)
//! }
//!
//! // Delete when done or compromised
//! store.delete("github.pat")?;
//! ```

mod file_store;
mod keychain_store;
mod traits;

pub use file_store::FileSecretStore;
pub use keychain_store::KeychainSecretStore;
pub use traits::{SecretError, SecretStore};

/// Create a secret store based on the provider name.
///
/// # Providers
///
/// - `"file"` (default): [`FileSecretStore`] storing in `~/.lattice/secrets.toml`
/// - `"keychain"`: [`KeychainSecretStore`] using OS keychain (requires feature)
///
/// # Errors
///
/// - Unknown provider name
/// - Keychain provider without `keychain` feature enabled
/// - Initialization errors from the store
///
/// # Example
///
/// ```ignore
/// use lattice::secrets::create_store;
///
/// // From configuration
/// let provider = config.secrets.provider.as_deref().unwrap_or("file");
/// let store = create_store(provider)?;
///
/// // Use the store
/// store.set("github.pat", token)?;
/// ```
pub fn create_store(provider: &str) -> Result<Box<dyn SecretStore>, SecretError> {
    match provider {
        "file" => Ok(Box::new(FileSecretStore::new()?)),
        #[cfg(feature = "keychain")]
        "keychain" => Ok(Box::new(KeychainSecretStore::new()?)),
        #[cfg(not(feature = "keychain"))]
        "keychain" => Err(SecretError::ProviderNotAvailable(
            "keychain support not enabled (compile with --features keychain)".into(),
        )),
        other => Err(SecretError::ProviderNotAvailable(format!(
            "unknown secret provider: '{}' (valid: file, keychain)",
            other
        ))),
    }
}

/// The default secret store provider name.
pub const DEFAULT_PROVIDER: &str = "file";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_file_store() {
        let store = create_store("file").expect("create file store");
        // Just verify it's created - actual operations tested in file_store module
        assert!(store.get("nonexistent").expect("get").is_none());
    }

    #[test]
    fn create_unknown_provider() {
        let result = create_store("unknown");
        match result {
            Err(SecretError::ProviderNotAvailable(msg)) => {
                assert!(msg.contains("unknown"));
            }
            Err(e) => panic!("unexpected error type: {:?}", e),
            Ok(_) => panic!("expected error"),
        }
    }

    #[cfg(not(feature = "keychain"))]
    #[test]
    fn create_keychain_without_feature() {
        let result = create_store("keychain");
        match result {
            Err(e) => {
                let msg = e.to_string();
                assert!(msg.contains("keychain"), "error should mention keychain");
                assert!(
                    msg.contains("not enabled"),
                    "error should mention not enabled"
                );
            }
            Ok(_) => panic!("expected error"),
        }
    }

    #[cfg(feature = "keychain")]
    #[test]
    fn create_keychain_with_feature() {
        // This may fail if keychain is not available on the system
        // but it should at least try to create the store
        let result = create_store("keychain");
        // We don't assert success because keychain may not be available
        // in CI environments, but if it fails it should be the right error
        if let Err(e) = result {
            assert!(matches!(e, SecretError::ReadError(_)));
        }
    }

    #[test]
    fn default_provider_constant() {
        assert_eq!(DEFAULT_PROVIDER, "file");
    }
}
