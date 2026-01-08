//! secrets::traits
//!
//! Secret storage trait definition.
//!
//! # Design
//!
//! The `SecretStore` trait defines a simple key-value interface for secrets.
//! Keys are namespaced (e.g., "github.pat") to avoid collisions.
//!
//! # Security
//!
//! Implementations MUST:
//! - Never log, print, or include secrets in error messages
//! - Use secure storage mechanisms appropriate to the platform
//! - Be thread-safe (Send + Sync)
//!
//! # Example
//!
//! ```ignore
//! use lattice::secrets::{SecretStore, SecretError};
//!
//! fn use_token(store: &dyn SecretStore) -> Result<(), SecretError> {
//!     if let Some(token) = store.get("github.pat")? {
//!         // Use token (never print it!)
//!         Ok(())
//!     } else {
//!         Err(SecretError::NotFound("github.pat".into()))
//!     }
//! }
//! ```

use thiserror::Error;

/// Errors from secret storage operations.
///
/// Note: Error messages intentionally do not include secret values.
#[derive(Debug, Error)]
pub enum SecretError {
    /// Secret not found for the given key.
    #[error("secret not found: {0}")]
    NotFound(String),

    /// Failed to read from secret storage.
    #[error("failed to read secret: {0}")]
    ReadError(String),

    /// Failed to write to secret storage.
    #[error("failed to write secret: {0}")]
    WriteError(String),

    /// Failed to delete from secret storage.
    #[error("failed to delete secret: {0}")]
    DeleteError(String),

    /// Permission denied accessing secret storage.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Provider not available or not configured.
    #[error("secret provider not available: {0}")]
    ProviderNotAvailable(String),
}

/// Trait for secret storage providers.
///
/// Implementations must be thread-safe (Send + Sync) and must never
/// log, print, or include secret values in error messages.
///
/// # Keys
///
/// Keys are namespaced strings like "github.pat" or "gitlab.token".
/// The implementation should store these as-is without interpretation.
///
/// # Example
///
/// ```ignore
/// use lattice::secrets::{SecretStore, FileSecretStore};
///
/// let store = FileSecretStore::new()?;
///
/// // Store a token
/// store.set("github.pat", "ghp_xxxxx...")?;
///
/// // Retrieve it
/// match store.get("github.pat")? {
///     Some(token) => println!("Token found (not printing value!)"),
///     None => println!("No token stored"),
/// }
///
/// // Delete when done
/// store.delete("github.pat")?;
/// ```
pub trait SecretStore: Send + Sync {
    /// Get a secret by key.
    ///
    /// Returns `Ok(Some(value))` if the secret exists.
    /// Returns `Ok(None)` if the secret does not exist.
    /// Returns `Err` if there was an error accessing the store.
    ///
    /// # Security
    ///
    /// The returned value is the raw secret. Do not log or print it.
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;

    /// Set a secret.
    ///
    /// Overwrites any existing value for the key.
    ///
    /// # Security
    ///
    /// The value is stored securely. The implementation must never
    /// log or include the value in error messages.
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;

    /// Delete a secret.
    ///
    /// Returns `Ok(())` even if the secret did not exist.
    /// This makes delete idempotent.
    fn delete(&self, key: &str) -> Result<(), SecretError>;

    /// Check if a secret exists.
    ///
    /// Default implementation uses `get()` and checks for `Some`.
    fn exists(&self, key: &str) -> Result<bool, SecretError> {
        Ok(self.get(key)?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_formatting() {
        let err = SecretError::NotFound("github.pat".into());
        assert!(err.to_string().contains("github.pat"));
        assert!(err.to_string().contains("not found"));

        let err = SecretError::ReadError("disk full".into());
        assert!(err.to_string().contains("read"));

        let err = SecretError::WriteError("permission denied".into());
        assert!(err.to_string().contains("write"));

        let err = SecretError::DeleteError("io error".into());
        assert!(err.to_string().contains("delete"));

        let err = SecretError::PermissionDenied("access denied".into());
        assert!(err.to_string().contains("permission"));

        let err = SecretError::ProviderNotAvailable("keychain".into());
        assert!(err.to_string().contains("provider"));
    }
}
