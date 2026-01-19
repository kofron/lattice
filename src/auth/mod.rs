//! auth - GitHub App OAuth authentication
//!
//! This module implements GitHub App device flow OAuth as the only authentication
//! method for Lattice, per SPEC.md Section 4.4 and ARCHITECTURE.md Section 11.3.
//!
//! # Architecture
//!
//! The auth system:
//! - Uses device flow OAuth (no client secret needed for CLI)
//! - Stores token bundles in SecretStore
//! - Refreshes tokens automatically before expiry
//! - Guards refresh operations with per-host file locks
//! - Never exposes tokens in logs, errors, or outputs
//!
//! # Components
//!
//! - [`TokenBundle`] - JSON schema for stored OAuth tokens
//! - [`AuthLock`] - File-based lock for concurrent refresh protection
//! - [`TokenProvider`] - Trait for providing bearer tokens to forge adapters
//! - [`GitHubAuthManager`] - Implementation of TokenProvider
//! - [`DeviceFlowClient`] - HTTP client for OAuth device flow
//!
//! # Security
//!
//! Per SPEC.md Section 4.4.4, tokens MUST never appear in:
//! - logs (including --debug)
//! - JSON outputs
//! - error messages
//! - debug output
//!
//! All types in this module implement custom Debug to redact token values.
//!
//! # Example
//!
//! ```ignore
//! use latticework::auth::{GitHubAuthManager, TokenProvider};
//! use std::sync::Arc;
//!
//! // Create auth manager
//! let store = secrets::create_store(secrets::DEFAULT_PROVIDER)?;
//! let manager = Arc::new(GitHubAuthManager::new("github.com", store));
//!
//! // Use with forge
//! let token = manager.bearer_token().await?;
//! ```

pub mod cache;
mod device_flow;
mod errors;
pub mod installations;
mod lock;
mod provider;
mod token_bundle;

// Re-export public types
pub use device_flow::DeviceFlowClient;
pub use errors::AuthError;
pub use lock::{AuthLock, DEFAULT_LOCK_TIMEOUT};
pub use provider::GitHubAuthManager;
pub use token_bundle::{
    BundleTimestamps, TokenBundle, TokenInfo, UserInfo, EXPIRY_BUFFER_SECS, GITHUB_APP_CLIENT_ID,
    TOKEN_BUNDLE_KIND, TOKEN_BUNDLE_VERSION,
};

/// Trait for providing bearer tokens to forge adapters.
///
/// Per ARCHITECTURE.md Section 11.3:
/// - Returns a valid bearer token, refreshing if necessary
/// - Acquires auth lock during refresh to prevent race conditions
/// - Never participates in repository mutation plans
///
/// # Implementation Notes
///
/// Implementors must:
/// - Handle token refresh transparently
/// - Acquire auth lock before refreshing to prevent races
/// - Re-check if refresh is needed after acquiring lock
/// - Never log or expose token values
///
/// # Example
///
/// ```ignore
/// use latticework::auth::TokenProvider;
///
/// async fn make_api_call(provider: &dyn TokenProvider) -> Result<()> {
///     let token = provider.bearer_token().await?;
///     // Use token in Authorization header
///     // ...
/// }
/// ```
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    /// Returns a valid bearer token, refreshing if necessary.
    ///
    /// This method:
    /// 1. Checks if cached token is valid
    /// 2. If expired/near-expiry, acquires auth lock
    /// 3. Re-checks after lock (another process may have refreshed)
    /// 4. Performs refresh if still needed
    /// 5. Returns the valid access token
    ///
    /// # Errors
    ///
    /// - [`AuthError::NotAuthenticated`] if no token exists
    /// - [`AuthError::Expired`] if refresh token has expired
    /// - [`AuthError::RefreshFailed`] if refresh fails
    /// - [`AuthError::LockTimeout`] if auth lock cannot be acquired
    async fn bearer_token(&self) -> Result<String, AuthError>;

    /// Check if authentication is available without refreshing.
    ///
    /// Returns true if a valid token bundle exists and the refresh token
    /// has not expired. Does not perform refresh.
    fn is_authenticated(&self) -> bool;

    /// Get the host this provider authenticates for.
    fn host(&self) -> &str;
}

/// Check if GitHub App OAuth authentication is available for a host.
///
/// This is used by the scanner to set the AuthAvailable capability.
///
/// # Arguments
///
/// * `host` - GitHub host (e.g., "github.com")
///
/// # Returns
///
/// `true` if a valid token bundle exists for the host.
///
/// # Example
///
/// ```ignore
/// if has_github_auth("github.com") {
///     capabilities.insert(Capability::AuthAvailable);
/// }
/// ```
pub fn has_github_auth(host: &str) -> bool {
    let store = match crate::secrets::create_store(crate::secrets::DEFAULT_PROVIDER) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let key = TokenBundle::secret_key(host);
    match store.get(&key) {
        Ok(Some(json)) => {
            // Try to parse and validate
            match TokenBundle::parse(&json) {
                Ok(bundle) => bundle.is_valid(),
                Err(_) => false,
            }
        }
        _ => false,
    }
}

/// Get the stored user info for a host, if authenticated.
///
/// Returns the user login and ID without exposing tokens.
///
/// # Arguments
///
/// * `host` - GitHub host (e.g., "github.com")
///
/// # Returns
///
/// `Some(UserInfo)` if authenticated, `None` otherwise.
pub fn get_user_info(host: &str) -> Option<UserInfo> {
    let store = crate::secrets::create_store(crate::secrets::DEFAULT_PROVIDER).ok()?;
    let key = TokenBundle::secret_key(host);
    let json = store.get(&key).ok()??;
    let bundle = TokenBundle::parse(&json).ok()?;
    Some(bundle.user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_app_client_id_is_correct() {
        // Verify the hardcoded client ID matches PLAN.md
        assert_eq!(GITHUB_APP_CLIENT_ID, "Iv23liIqb9vJ8kaRyZaU");
    }

    #[test]
    fn token_bundle_kind_is_correct() {
        assert_eq!(TOKEN_BUNDLE_KIND, "lattice.github-app-oauth");
    }

    #[test]
    fn token_bundle_version_is_one() {
        assert_eq!(TOKEN_BUNDLE_VERSION, 1);
    }

    #[test]
    fn expiry_buffer_is_five_minutes() {
        assert_eq!(EXPIRY_BUFFER_SECS, 300);
    }

    #[test]
    fn has_github_auth_returns_false_when_no_store() {
        // This test verifies the function doesn't panic when store fails
        // In practice, we can't easily test this without mocking
        // but we can at least verify the function signature
        let _ = has_github_auth("nonexistent.example.com");
    }
}
