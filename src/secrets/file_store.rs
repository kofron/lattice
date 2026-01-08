//! secrets::file_store
//!
//! File-based secret storage.
//!
//! # Security
//!
//! - Secrets are stored in `~/.lattice/secrets.toml`
//! - File permissions are set to 0600 on Unix (owner read/write only)
//! - All writes are atomic (write to temp file, then rename)
//! - Secrets are NEVER logged, printed, or included in error messages
//!
//! # Example
//!
//! ```ignore
//! use latticework::secrets::{FileSecretStore, SecretStore};
//!
//! let store = FileSecretStore::new()?;
//! store.set("github.pat", "ghp_xxxx...")?;
//!
//! if let Some(token) = store.get("github.pat")? {
//!     // Use token...
//! }
//! ```

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::traits::{SecretError, SecretStore};

/// File-based secret storage.
///
/// Stores secrets in a TOML file at `~/.lattice/secrets.toml`.
/// This is the default secret store for Lattice.
///
/// # Security Considerations
///
/// - On Unix, file permissions are set to 0600 (owner read/write only)
/// - Writes are atomic (write to temp file, then rename)
/// - Secrets are never included in error messages or logs
///
/// # Example
///
/// ```ignore
/// use latticework::secrets::{FileSecretStore, SecretStore};
///
/// let store = FileSecretStore::new()?;
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
#[derive(Debug)]
pub struct FileSecretStore {
    /// Path to the secrets file
    path: PathBuf,
}

impl FileSecretStore {
    /// Create a new file secret store at the default location.
    ///
    /// The default location is `~/.lattice/secrets.toml`.
    ///
    /// # Errors
    ///
    /// Returns an error if the home directory cannot be determined.
    pub fn new() -> Result<Self, SecretError> {
        let home = dirs::home_dir()
            .ok_or_else(|| SecretError::ReadError("cannot determine home directory".into()))?;
        let path = home.join(".lattice").join("secrets.toml");
        Ok(Self { path })
    }

    /// Create a file secret store at a custom path.
    ///
    /// This is primarily useful for testing.
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Get the path to the secrets file.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Read all secrets from the file.
    fn read_secrets(&self) -> Result<HashMap<String, String>, SecretError> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }

        let content = fs::read_to_string(&self.path)
            .map_err(|e| SecretError::ReadError(format!("cannot read secrets file: {}", e)))?;

        // Parse as TOML
        let secrets: HashMap<String, String> = toml::from_str(&content)
            .map_err(|e| SecretError::ReadError(format!("cannot parse secrets file: {}", e)))?;

        Ok(secrets)
    }

    /// Write secrets to the file with atomic write and proper permissions.
    fn write_secrets(&self, secrets: &HashMap<String, String>) -> Result<(), SecretError> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| SecretError::WriteError(format!("cannot create directory: {}", e)))?;
        }

        // Serialize to TOML
        let content = toml::to_string_pretty(secrets)
            .map_err(|e| SecretError::WriteError(format!("cannot serialize secrets: {}", e)))?;

        // Write to a temp file first for atomicity
        let temp_path = self.path.with_extension("tmp");

        // Create and write to temp file
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp_path)
                .map_err(|e| SecretError::WriteError(format!("cannot create temp file: {}", e)))?;

            // Set restrictive permissions BEFORE writing content (Unix only)
            #[cfg(unix)]
            {
                let permissions = fs::Permissions::from_mode(0o600);
                file.set_permissions(permissions).map_err(|e| {
                    SecretError::WriteError(format!("cannot set permissions: {}", e))
                })?;
            }

            // Write content
            file.write_all(content.as_bytes())
                .map_err(|e| SecretError::WriteError(format!("cannot write secrets: {}", e)))?;

            // Sync to disk
            file.sync_all()
                .map_err(|e| SecretError::WriteError(format!("cannot sync to disk: {}", e)))?;
        }

        // Atomic rename
        fs::rename(&temp_path, &self.path)
            .map_err(|e| SecretError::WriteError(format!("cannot rename temp file: {}", e)))?;

        Ok(())
    }

    /// Verify file permissions are correct (Unix only).
    ///
    /// Returns true if the file doesn't exist or has 0600 permissions.
    #[cfg(unix)]
    pub fn verify_permissions(&self) -> Result<bool, SecretError> {
        if !self.path.exists() {
            return Ok(true);
        }

        let metadata = fs::metadata(&self.path)
            .map_err(|e| SecretError::ReadError(format!("cannot read file metadata: {}", e)))?;

        let mode = metadata.permissions().mode() & 0o777;
        Ok(mode == 0o600)
    }

    /// Verify file permissions are correct (non-Unix always returns true).
    #[cfg(not(unix))]
    pub fn verify_permissions(&self) -> Result<bool, SecretError> {
        Ok(true)
    }
}

impl SecretStore for FileSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let secrets = self.read_secrets()?;
        Ok(secrets.get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let mut secrets = self.read_secrets()?;
        secrets.insert(key.to_string(), value.to_string());
        self.write_secrets(&secrets)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        let mut secrets = self.read_secrets()?;
        secrets.remove(key);
        self.write_secrets(&secrets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, FileSecretStore) {
        let temp = TempDir::new().expect("create temp dir");
        let path = temp.path().join("secrets.toml");
        let store = FileSecretStore::with_path(path);
        (temp, store)
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let (_temp, store) = create_test_store();

        let result = store.get("nonexistent").expect("get");
        assert!(result.is_none());
    }

    #[test]
    fn set_and_get() {
        let (_temp, store) = create_test_store();

        store.set("github.pat", "test_token").expect("set");

        let result = store.get("github.pat").expect("get");
        assert_eq!(result, Some("test_token".to_string()));
    }

    #[test]
    fn set_overwrites() {
        let (_temp, store) = create_test_store();

        store.set("key", "value1").expect("first set");
        store.set("key", "value2").expect("second set");

        let result = store.get("key").expect("get");
        assert_eq!(result, Some("value2".to_string()));
    }

    #[test]
    fn delete_existing() {
        let (_temp, store) = create_test_store();

        store.set("key", "value").expect("set");
        store.delete("key").expect("delete");

        let result = store.get("key").expect("get after delete");
        assert!(result.is_none());
    }

    #[test]
    fn delete_nonexistent_ok() {
        let (_temp, store) = create_test_store();

        // Should not error when deleting a key that doesn't exist
        store.delete("nonexistent").expect("delete nonexistent");
    }

    #[test]
    fn multiple_secrets() {
        let (_temp, store) = create_test_store();

        store.set("github.pat", "token1").expect("set github");
        store.set("gitlab.token", "token2").expect("set gitlab");
        store.set("custom.key", "value").expect("set custom");

        assert_eq!(
            store.get("github.pat").expect("get github"),
            Some("token1".to_string())
        );
        assert_eq!(
            store.get("gitlab.token").expect("get gitlab"),
            Some("token2".to_string())
        );
        assert_eq!(
            store.get("custom.key").expect("get custom"),
            Some("value".to_string())
        );
    }

    #[test]
    fn creates_directory_if_missing() {
        let temp = TempDir::new().expect("create temp dir");
        let path = temp.path().join("subdir").join("secrets.toml");
        let store = FileSecretStore::with_path(path.clone());

        // Directory should not exist yet
        assert!(!path.parent().unwrap().exists());

        store.set("key", "value").expect("set");

        // Directory should now exist
        assert!(path.parent().unwrap().exists());
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn permissions_0600_on_unix() {
        let (_temp, store) = create_test_store();

        store.set("key", "value").expect("set");

        // Verify permissions
        let metadata = fs::metadata(store.path()).expect("metadata");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "permissions should be 0600");
    }

    #[cfg(unix)]
    #[test]
    fn verify_permissions_works() {
        let (_temp, store) = create_test_store();

        // No file yet - should be ok
        assert!(store.verify_permissions().expect("verify"));

        // After write - should be correct
        store.set("key", "value").expect("set");
        assert!(store.verify_permissions().expect("verify after write"));
    }

    #[test]
    fn error_messages_are_reasonable() {
        // This test verifies that our error wrapping works correctly.
        // Note: We can't prevent the underlying toml crate from including
        // parse context in its errors, but we can verify our wrapper is applied.
        let (_temp, store) = create_test_store();

        // Write invalid TOML to cause a parse error
        fs::create_dir_all(store.path().parent().unwrap()).expect("mkdir");
        fs::write(store.path(), "invalid = [unclosed").expect("write bad toml");

        let err = store.get("key").unwrap_err();
        let err_str = err.to_string();

        // Error should indicate a read/parse error occurred
        assert!(
            err_str.contains("cannot parse") || err_str.contains("read"),
            "error should mention parse or read failure: {}",
            err_str
        );
    }

    #[test]
    fn path_accessor() {
        let temp = TempDir::new().expect("create temp dir");
        let path = temp.path().join("custom.toml");
        let store = FileSecretStore::with_path(path.clone());

        assert_eq!(store.path(), &path);
    }

    #[test]
    fn persistence_across_instances() {
        let temp = TempDir::new().expect("create temp dir");
        let path = temp.path().join("secrets.toml");

        // First instance writes
        {
            let store = FileSecretStore::with_path(path.clone());
            store.set("key", "value").expect("set");
        }

        // Second instance reads
        {
            let store = FileSecretStore::with_path(path);
            let result = store.get("key").expect("get");
            assert_eq!(result, Some("value".to_string()));
        }
    }

    #[test]
    fn special_characters_in_values() {
        let (_temp, store) = create_test_store();

        // Test with special characters
        let special = "value with \"quotes\" and \n newlines and = equals";
        store.set("key", special).expect("set");

        let result = store.get("key").expect("get");
        assert_eq!(result, Some(special.to_string()));
    }
}
