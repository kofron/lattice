//! auth::provider
//!
//! GitHubAuthManager - TokenProvider implementation for GitHub App OAuth.
//!
//! # Architecture
//!
//! Per ARCHITECTURE.md Section 11.3, the auth manager:
//! - Loads token bundles from SecretStore
//! - Refreshes tokens when expired (with auth-scoped locking)
//! - Never participates in repository mutation plans
//! - Redacts secrets in all logs, errors, and outputs
//!
//! # Concurrency
//!
//! Token refresh uses the [`AuthLock`] to prevent race conditions when
//! multiple Lattice processes try to refresh simultaneously. The pattern is:
//!
//! 1. Check if refresh is needed
//! 2. If so, acquire the auth lock
//! 3. Re-check after acquiring lock (another process may have refreshed)
//! 4. Perform refresh if still needed
//! 5. Release lock
//!
//! # Example
//!
//! ```ignore
//! use latticework::auth::{GitHubAuthManager, TokenProvider};
//! use latticework::secrets;
//! use std::sync::Arc;
//!
//! let store = secrets::create_store(secrets::DEFAULT_PROVIDER)?;
//! let manager = GitHubAuthManager::new("github.com", store);
//!
//! // Get bearer token (refreshes if needed)
//! let token = manager.bearer_token().await?;
//!
//! // Check if authenticated without refreshing
//! if manager.is_authenticated() {
//!     println!("Authenticated!");
//! }
//! ```

use std::sync::RwLock;

use chrono::{DateTime, Utc};

use super::device_flow::DeviceFlowClient;
use super::errors::AuthError;
use super::lock::{AuthLock, DEFAULT_LOCK_TIMEOUT};
use super::token_bundle::{TokenBundle, TokenInfo, UserInfo};
use super::TokenProvider;
use crate::secrets::SecretStore;

/// GitHub authentication manager.
///
/// Implements [`TokenProvider`] to provide bearer tokens to forge adapters.
/// Handles loading tokens from SecretStore and refreshing when needed.
pub struct GitHubAuthManager {
    /// GitHub host (e.g., "github.com").
    host: String,

    /// Secret store for token persistence.
    store: Box<dyn SecretStore>,

    /// Cached token bundle (refreshed on demand).
    cache: RwLock<Option<TokenBundle>>,
}

impl GitHubAuthManager {
    /// Create a new GitHub auth manager.
    ///
    /// # Arguments
    ///
    /// * `host` - GitHub host (e.g., "github.com")
    /// * `store` - Secret store for token persistence
    pub fn new(host: &str, store: Box<dyn SecretStore>) -> Self {
        Self {
            host: host.to_string(),
            store,
            cache: RwLock::new(None),
        }
    }

    /// Get the SecretStore key for this host.
    fn secret_key(&self) -> String {
        TokenBundle::secret_key(&self.host)
    }

    /// Load the token bundle from SecretStore.
    ///
    /// Does not perform refresh.
    fn load_bundle(&self) -> Result<Option<TokenBundle>, AuthError> {
        let key = self.secret_key();
        match self.store.get(&key)? {
            Some(json) => {
                let bundle = TokenBundle::parse(&json)?;
                Ok(Some(bundle))
            }
            None => Ok(None),
        }
    }

    /// Save a token bundle to SecretStore.
    fn save_bundle(&self, bundle: &TokenBundle) -> Result<(), AuthError> {
        let key = self.secret_key();
        let json = bundle.to_json()?;
        self.store.set(&key, &json)?;
        Ok(())
    }

    /// Update the in-memory cache.
    fn update_cache(&self, bundle: TokenBundle) {
        if let Ok(mut cache) = self.cache.write() {
            *cache = Some(bundle);
        }
    }

    /// Get the cached bundle, loading from store if needed.
    fn get_or_load_bundle(&self) -> Result<Option<TokenBundle>, AuthError> {
        // Check cache first
        if let Ok(cache) = self.cache.read() {
            if let Some(ref bundle) = *cache {
                return Ok(Some(bundle.clone()));
            }
        }

        // Load from store
        let bundle = self.load_bundle()?;
        if let Some(ref b) = bundle {
            self.update_cache(b.clone());
        }
        Ok(bundle)
    }

    /// Refresh tokens with lock protection.
    ///
    /// Acquires the auth lock, re-checks if refresh is needed, and
    /// performs the refresh if still necessary.
    async fn refresh_with_lock(&self) -> Result<TokenBundle, AuthError> {
        // Acquire auth lock
        let _lock = AuthLock::acquire(&self.host, DEFAULT_LOCK_TIMEOUT)?;

        // Re-load bundle and re-check (another process may have refreshed)
        let bundle = self
            .load_bundle()?
            .ok_or_else(|| AuthError::NotAuthenticated(self.host.clone()))?;

        // Check if still needs refresh
        if !bundle.needs_refresh() {
            // Another process refreshed, use the updated bundle
            self.update_cache(bundle.clone());
            return Ok(bundle);
        }

        // Check if refresh token is still valid
        if bundle.is_refresh_token_expired() {
            return Err(AuthError::Expired(self.host.clone()));
        }

        // Perform refresh
        let client = DeviceFlowClient::new(&self.host);
        let token_response = client.refresh_token(&bundle.tokens.refresh_token).await?;

        // Create updated bundle with new tokens
        let new_tokens = TokenInfo::new(
            token_response.access_token,
            token_response.expires_in,
            token_response.refresh_token,
            token_response.refresh_token_expires_in,
        );
        let new_bundle = bundle.with_refreshed_tokens(new_tokens);

        // Save to store
        self.save_bundle(&new_bundle)?;

        // Update cache
        self.update_cache(new_bundle.clone());

        Ok(new_bundle)
    }

    /// Get user info from the cached bundle.
    ///
    /// Returns `None` if not authenticated.
    pub fn user_info(&self) -> Option<UserInfo> {
        self.get_or_load_bundle().ok().flatten().map(|b| b.user)
    }

    /// Get access token expiry time from the cached bundle.
    ///
    /// Returns `None` if not authenticated.
    pub fn access_token_expires_at(&self) -> Option<DateTime<Utc>> {
        self.get_or_load_bundle()
            .ok()
            .flatten()
            .map(|b| b.tokens.access_token_expires_at)
    }

    /// Get refresh token expiry time from the cached bundle.
    ///
    /// Returns `None` if not authenticated.
    pub fn refresh_token_expires_at(&self) -> Option<DateTime<Utc>> {
        self.get_or_load_bundle()
            .ok()
            .flatten()
            .map(|b| b.tokens.refresh_token_expires_at)
    }

    /// Store a new token bundle (after device flow login).
    ///
    /// This is called by the auth login command after successful device flow.
    pub fn store_tokens(
        &self,
        user: UserInfo,
        tokens: TokenInfo,
    ) -> Result<TokenBundle, AuthError> {
        let bundle = TokenBundle::new(&self.host, user, tokens);
        self.save_bundle(&bundle)?;
        self.update_cache(bundle.clone());
        Ok(bundle)
    }

    /// Delete stored tokens (logout).
    pub fn delete_tokens(&self) -> Result<(), AuthError> {
        let key = self.secret_key();
        self.store.delete(&key)?;

        // Clear cache
        if let Ok(mut cache) = self.cache.write() {
            *cache = None;
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl TokenProvider for GitHubAuthManager {
    async fn bearer_token(&self) -> Result<String, AuthError> {
        // Get or load bundle
        let bundle = self
            .get_or_load_bundle()?
            .ok_or_else(|| AuthError::NotAuthenticated(self.host.clone()))?;

        // Check if refresh token expired
        if bundle.is_refresh_token_expired() {
            return Err(AuthError::Expired(self.host.clone()));
        }

        // Check if needs refresh
        if bundle.needs_refresh() {
            let refreshed = self.refresh_with_lock().await?;
            return Ok(refreshed.tokens.access_token);
        }

        Ok(bundle.tokens.access_token)
    }

    fn is_authenticated(&self) -> bool {
        match self.get_or_load_bundle() {
            Ok(Some(bundle)) => bundle.is_valid(),
            _ => false,
        }
    }

    fn host(&self) -> &str {
        &self.host
    }
}

// Custom Debug to avoid exposing tokens
impl std::fmt::Debug for GitHubAuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubAuthManager")
            .field("host", &self.host)
            .field("is_authenticated", &self.is_authenticated())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SecretError;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Mock secret store for testing.
    struct MockSecretStore {
        data: Mutex<HashMap<String, String>>,
    }

    impl MockSecretStore {
        fn new() -> Self {
            Self {
                data: Mutex::new(HashMap::new()),
            }
        }

        fn with_bundle(bundle: &TokenBundle) -> Self {
            let store = Self::new();
            let key = TokenBundle::secret_key(&bundle.host);
            let json = bundle.to_json().expect("serialize bundle");
            store.data.lock().unwrap().insert(key, json);
            store
        }
    }

    impl SecretStore for MockSecretStore {
        fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
            Ok(self.data.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
            self.data
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecretError> {
            self.data.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn make_test_bundle() -> TokenBundle {
        TokenBundle::new(
            "github.com",
            UserInfo {
                id: 12345,
                login: "testuser".to_string(),
            },
            TokenInfo::new(
                "ghu_test_access".to_string(),
                3600, // 1 hour
                "ghr_test_refresh".to_string(),
                15_552_000, // 180 days
            ),
        )
    }

    #[test]
    fn new_manager_not_authenticated() {
        let store = Box::new(MockSecretStore::new());
        let manager = GitHubAuthManager::new("github.com", store);

        assert!(!manager.is_authenticated());
        assert_eq!(manager.host(), "github.com");
    }

    #[test]
    fn manager_with_valid_bundle_is_authenticated() {
        let bundle = make_test_bundle();
        let store = Box::new(MockSecretStore::with_bundle(&bundle));
        let manager = GitHubAuthManager::new("github.com", store);

        assert!(manager.is_authenticated());
    }

    #[test]
    fn user_info_returns_cached_user() {
        let bundle = make_test_bundle();
        let store = Box::new(MockSecretStore::with_bundle(&bundle));
        let manager = GitHubAuthManager::new("github.com", store);

        let user = manager.user_info().expect("should have user");
        assert_eq!(user.id, 12345);
        assert_eq!(user.login, "testuser");
    }

    #[test]
    fn user_info_none_when_not_authenticated() {
        let store = Box::new(MockSecretStore::new());
        let manager = GitHubAuthManager::new("github.com", store);

        assert!(manager.user_info().is_none());
    }

    #[test]
    fn store_tokens_creates_bundle() {
        let store = Box::new(MockSecretStore::new());
        let manager = GitHubAuthManager::new("github.com", store);

        let user = UserInfo {
            id: 999,
            login: "newuser".to_string(),
        };
        let tokens = TokenInfo::new(
            "ghu_new".to_string(),
            3600,
            "ghr_new".to_string(),
            15_552_000,
        );

        manager.store_tokens(user, tokens).expect("store tokens");

        assert!(manager.is_authenticated());
        let info = manager.user_info().expect("user info");
        assert_eq!(info.login, "newuser");
    }

    #[test]
    fn delete_tokens_clears_auth() {
        let bundle = make_test_bundle();
        let store = Box::new(MockSecretStore::with_bundle(&bundle));
        let manager = GitHubAuthManager::new("github.com", store);

        assert!(manager.is_authenticated());

        manager.delete_tokens().expect("delete tokens");

        assert!(!manager.is_authenticated());
    }

    #[test]
    fn debug_output_does_not_expose_tokens() {
        let bundle = make_test_bundle();
        let store = Box::new(MockSecretStore::with_bundle(&bundle));
        let manager = GitHubAuthManager::new("github.com", store);

        let debug_output = format!("{:?}", manager);

        assert!(debug_output.contains("github.com"));
        assert!(!debug_output.contains("ghu_"));
        assert!(!debug_output.contains("ghr_"));
    }

    #[test]
    fn secret_key_format() {
        let store = Box::new(MockSecretStore::new());
        let manager = GitHubAuthManager::new("github.com", store);

        assert_eq!(manager.secret_key(), "github_app.oauth.github.com");
    }

    #[tokio::test]
    async fn bearer_token_not_authenticated_error() {
        let store = Box::new(MockSecretStore::new());
        let manager = GitHubAuthManager::new("github.com", store);

        let result = manager.bearer_token().await;
        assert!(matches!(result, Err(AuthError::NotAuthenticated(_))));
    }

    #[tokio::test]
    async fn bearer_token_returns_access_token_when_valid() {
        let bundle = make_test_bundle();
        let store = Box::new(MockSecretStore::with_bundle(&bundle));
        let manager = GitHubAuthManager::new("github.com", store);

        let token = manager.bearer_token().await.expect("get token");
        assert_eq!(token, "ghu_test_access");
    }
}
