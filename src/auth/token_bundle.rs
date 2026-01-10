//! auth::token_bundle
//!
//! Token bundle schema for GitHub App OAuth tokens.
//!
//! # Design
//!
//! Per SPEC.md Section 4.4.2, the token bundle stores:
//! - kind, schema_version, host, client_id
//! - user (id, login)
//! - tokens (access_token, access_token_expires_at, refresh_token, refresh_token_expires_at)
//! - timestamps (created_at, updated_at)
//!
//! # Security
//!
//! Per SPEC.md Section 4.4.4, tokens MUST never appear in:
//! - logs (including --debug)
//! - JSON outputs
//! - error messages
//! - debug output
//!
//! This module implements custom Debug to redact token values.
//!
//! # Example
//!
//! ```ignore
//! use latticework::auth::TokenBundle;
//!
//! let bundle = TokenBundle::new(
//!     "github.com",
//!     UserInfo { id: 123, login: "octocat".to_string() },
//!     TokenInfo::new("ghu_xxx", expires_in, "ghr_yyy", refresh_expires_in),
//! );
//!
//! // Store in SecretStore
//! let json = bundle.to_json()?;
//! store.set(&TokenBundle::secret_key("github.com"), &json)?;
//! ```

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

use super::errors::AuthError;

/// Kind identifier for token bundles.
pub const TOKEN_BUNDLE_KIND: &str = "lattice.github-app-oauth";

/// Current schema version for token bundles.
pub const TOKEN_BUNDLE_VERSION: u32 = 1;

/// The canonical client ID for the Lattice GitHub App.
pub const GITHUB_APP_CLIENT_ID: &str = "Iv23liIqb9vJ8kaRyZaU";

/// Buffer before expiry to trigger proactive refresh (5 minutes).
pub const EXPIRY_BUFFER_SECS: i64 = 300;

/// Token bundle stored in SecretStore.
///
/// Contains all OAuth tokens and metadata for a single host.
///
/// # Security
///
/// This struct implements custom Debug to redact token values.
/// Never log or print this struct's tokens directly.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenBundle {
    /// Bundle type identifier.
    pub kind: String,

    /// Schema version for forward compatibility.
    pub schema_version: u32,

    /// GitHub host (e.g., "github.com").
    pub host: String,

    /// GitHub App client ID.
    pub client_id: String,

    /// Authenticated user info.
    pub user: UserInfo,

    /// OAuth tokens with expiration times.
    pub tokens: TokenInfo,

    /// Bundle timestamps.
    pub timestamps: BundleTimestamps,
}

/// Authenticated GitHub user information.
///
/// Stored for display purposes (status command) without needing API calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    /// GitHub user ID (durable identifier).
    pub id: u64,

    /// GitHub login/username.
    pub login: String,
}

/// OAuth token information.
///
/// # Security
///
/// This struct implements custom Debug to redact token values.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    /// OAuth access token (starts with "ghu_").
    pub access_token: String,

    /// When the access token expires.
    pub access_token_expires_at: DateTime<Utc>,

    /// OAuth refresh token (starts with "ghr_").
    pub refresh_token: String,

    /// When the refresh token expires.
    pub refresh_token_expires_at: DateTime<Utc>,
}

/// Bundle creation and update timestamps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleTimestamps {
    /// When the bundle was first created.
    pub created_at: DateTime<Utc>,

    /// When the bundle was last updated (token refresh).
    pub updated_at: DateTime<Utc>,
}

impl TokenBundle {
    /// Create a new token bundle.
    ///
    /// # Arguments
    ///
    /// * `host` - GitHub host (e.g., "github.com")
    /// * `user` - Authenticated user info
    /// * `tokens` - OAuth tokens
    pub fn new(host: &str, user: UserInfo, tokens: TokenInfo) -> Self {
        let now = Utc::now();
        Self {
            kind: TOKEN_BUNDLE_KIND.to_string(),
            schema_version: TOKEN_BUNDLE_VERSION,
            host: host.to_string(),
            client_id: GITHUB_APP_CLIENT_ID.to_string(),
            user,
            tokens,
            timestamps: BundleTimestamps {
                created_at: now,
                updated_at: now,
            },
        }
    }

    /// Get the SecretStore key for a host.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::auth::TokenBundle;
    ///
    /// assert_eq!(
    ///     TokenBundle::secret_key("github.com"),
    ///     "github_app.oauth.github.com"
    /// );
    /// ```
    pub fn secret_key(host: &str) -> String {
        format!("github_app.oauth.{}", host)
    }

    /// Check if the access token has expired.
    ///
    /// Returns true if the access token expiration time has passed.
    pub fn is_access_token_expired(&self) -> bool {
        Utc::now() >= self.tokens.access_token_expires_at
    }

    /// Check if the access token needs refresh.
    ///
    /// Returns true if the access token will expire within the buffer period
    /// (5 minutes by default). This allows proactive refresh before expiry.
    pub fn needs_refresh(&self) -> bool {
        let buffer = Duration::seconds(EXPIRY_BUFFER_SECS);
        Utc::now() >= self.tokens.access_token_expires_at - buffer
    }

    /// Check if the refresh token has expired.
    ///
    /// If this returns true, the user must re-authenticate via device flow.
    pub fn is_refresh_token_expired(&self) -> bool {
        Utc::now() >= self.tokens.refresh_token_expires_at
    }

    /// Check if authentication is still valid.
    ///
    /// Returns true if either:
    /// - Access token is still valid, or
    /// - Refresh token is still valid (can refresh)
    pub fn is_valid(&self) -> bool {
        !self.is_refresh_token_expired()
    }

    /// Parse a token bundle from JSON.
    ///
    /// # Errors
    ///
    /// Returns `AuthError::InvalidBundle` if parsing fails or schema is invalid.
    pub fn parse(json: &str) -> Result<Self, AuthError> {
        let bundle: Self = serde_json::from_str(json)?;

        // Validate schema
        if bundle.kind != TOKEN_BUNDLE_KIND {
            return Err(AuthError::InvalidBundle(format!(
                "unexpected kind '{}', expected '{}'",
                bundle.kind, TOKEN_BUNDLE_KIND
            )));
        }

        if bundle.schema_version != TOKEN_BUNDLE_VERSION {
            return Err(AuthError::InvalidBundle(format!(
                "unsupported schema version {}, expected {}",
                bundle.schema_version, TOKEN_BUNDLE_VERSION
            )));
        }

        Ok(bundle)
    }

    /// Serialize the token bundle to JSON.
    ///
    /// # Errors
    ///
    /// Returns `AuthError::InvalidBundle` if serialization fails.
    pub fn to_json(&self) -> Result<String, AuthError> {
        serde_json::to_string_pretty(self).map_err(|e| AuthError::InvalidBundle(e.to_string()))
    }

    /// Update tokens after a refresh.
    ///
    /// Creates a new bundle with updated tokens and timestamp.
    pub fn with_refreshed_tokens(&self, tokens: TokenInfo) -> Self {
        Self {
            kind: self.kind.clone(),
            schema_version: self.schema_version,
            host: self.host.clone(),
            client_id: self.client_id.clone(),
            user: self.user.clone(),
            tokens,
            timestamps: BundleTimestamps {
                created_at: self.timestamps.created_at,
                updated_at: Utc::now(),
            },
        }
    }
}

impl TokenInfo {
    /// Create new token info from OAuth response.
    ///
    /// # Arguments
    ///
    /// * `access_token` - OAuth access token
    /// * `access_expires_in` - Seconds until access token expires
    /// * `refresh_token` - OAuth refresh token
    /// * `refresh_expires_in` - Seconds until refresh token expires
    pub fn new(
        access_token: String,
        access_expires_in: u64,
        refresh_token: String,
        refresh_expires_in: u64,
    ) -> Self {
        let now = Utc::now();
        Self {
            access_token,
            access_token_expires_at: now + Duration::seconds(access_expires_in as i64),
            refresh_token,
            refresh_token_expires_at: now + Duration::seconds(refresh_expires_in as i64),
        }
    }
}

// Custom Debug implementations to redact tokens

impl fmt::Debug for TokenBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenBundle")
            .field("kind", &self.kind)
            .field("schema_version", &self.schema_version)
            .field("host", &self.host)
            .field("client_id", &self.client_id)
            .field("user", &self.user)
            .field("tokens", &self.tokens) // TokenInfo has its own redacting Debug
            .field("timestamps", &self.timestamps)
            .finish()
    }
}

impl fmt::Debug for TokenInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenInfo")
            .field("access_token", &"[REDACTED]")
            .field("access_token_expires_at", &self.access_token_expires_at)
            .field("refresh_token", &"[REDACTED]")
            .field("refresh_token_expires_at", &self.refresh_token_expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_bundle() -> TokenBundle {
        TokenBundle::new(
            "github.com",
            UserInfo {
                id: 12345,
                login: "octocat".to_string(),
            },
            TokenInfo::new(
                "ghu_test_access_token".to_string(),
                3600,
                "ghr_test_refresh_token".to_string(),
                15_552_000, // 180 days
            ),
        )
    }

    #[test]
    fn secret_key_format() {
        assert_eq!(
            TokenBundle::secret_key("github.com"),
            "github_app.oauth.github.com"
        );
        assert_eq!(
            TokenBundle::secret_key("github.example.com"),
            "github_app.oauth.github.example.com"
        );
    }

    #[test]
    fn new_bundle_has_correct_fields() {
        let bundle = make_test_bundle();
        assert_eq!(bundle.kind, TOKEN_BUNDLE_KIND);
        assert_eq!(bundle.schema_version, TOKEN_BUNDLE_VERSION);
        assert_eq!(bundle.host, "github.com");
        assert_eq!(bundle.client_id, GITHUB_APP_CLIENT_ID);
        assert_eq!(bundle.user.id, 12345);
        assert_eq!(bundle.user.login, "octocat");
    }

    #[test]
    fn serialize_deserialize_roundtrip() {
        let bundle = make_test_bundle();
        let json = bundle.to_json().expect("serialization failed");
        let parsed = TokenBundle::parse(&json).expect("parsing failed");

        assert_eq!(parsed.kind, bundle.kind);
        assert_eq!(parsed.host, bundle.host);
        assert_eq!(parsed.user.id, bundle.user.id);
        assert_eq!(parsed.tokens.access_token, bundle.tokens.access_token);
    }

    #[test]
    fn parse_rejects_wrong_kind() {
        let json = r#"{
            "kind": "wrong.kind",
            "schema_version": 1,
            "host": "github.com",
            "client_id": "test",
            "user": {"id": 1, "login": "test"},
            "tokens": {
                "access_token": "ghu_xxx",
                "access_token_expires_at": "2026-01-10T12:00:00Z",
                "refresh_token": "ghr_yyy",
                "refresh_token_expires_at": "2026-07-10T12:00:00Z"
            },
            "timestamps": {
                "created_at": "2026-01-10T12:00:00Z",
                "updated_at": "2026-01-10T12:00:00Z"
            }
        }"#;

        let result = TokenBundle::parse(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unexpected kind"));
    }

    #[test]
    fn parse_rejects_wrong_version() {
        let json = r#"{
            "kind": "lattice.github-app-oauth",
            "schema_version": 999,
            "host": "github.com",
            "client_id": "test",
            "user": {"id": 1, "login": "test"},
            "tokens": {
                "access_token": "ghu_xxx",
                "access_token_expires_at": "2026-01-10T12:00:00Z",
                "refresh_token": "ghr_yyy",
                "refresh_token_expires_at": "2026-07-10T12:00:00Z"
            },
            "timestamps": {
                "created_at": "2026-01-10T12:00:00Z",
                "updated_at": "2026-01-10T12:00:00Z"
            }
        }"#;

        let result = TokenBundle::parse(json);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unsupported schema version"));
    }

    #[test]
    fn access_token_expiry_check() {
        // Token that expires in 1 hour - not expired
        let bundle = make_test_bundle();
        assert!(!bundle.is_access_token_expired());

        // Token that expired - create with past expiry
        let mut expired_bundle = bundle.clone();
        expired_bundle.tokens.access_token_expires_at = Utc::now() - Duration::hours(1);
        assert!(expired_bundle.is_access_token_expired());
    }

    #[test]
    fn needs_refresh_with_buffer() {
        let bundle = make_test_bundle();
        // Fresh token (1 hour to expiry) - doesn't need refresh
        assert!(!bundle.needs_refresh());

        // Token expiring in 4 minutes - needs refresh (within 5 min buffer)
        let mut almost_expired = bundle.clone();
        almost_expired.tokens.access_token_expires_at = Utc::now() + Duration::minutes(4);
        assert!(almost_expired.needs_refresh());

        // Token expiring in 6 minutes - doesn't need refresh yet
        let mut still_ok = bundle.clone();
        still_ok.tokens.access_token_expires_at = Utc::now() + Duration::minutes(6);
        assert!(!still_ok.needs_refresh());
    }

    #[test]
    fn refresh_token_expiry_check() {
        let bundle = make_test_bundle();
        // Fresh refresh token - not expired
        assert!(!bundle.is_refresh_token_expired());

        // Expired refresh token
        let mut expired = bundle.clone();
        expired.tokens.refresh_token_expires_at = Utc::now() - Duration::days(1);
        assert!(expired.is_refresh_token_expired());
    }

    #[test]
    fn is_valid_checks_refresh_token() {
        let bundle = make_test_bundle();
        assert!(bundle.is_valid());

        // Even if access token expired, still valid if refresh token is good
        let mut access_expired = bundle.clone();
        access_expired.tokens.access_token_expires_at = Utc::now() - Duration::hours(1);
        assert!(access_expired.is_valid());

        // If refresh token expired, not valid
        let mut refresh_expired = bundle.clone();
        refresh_expired.tokens.refresh_token_expires_at = Utc::now() - Duration::days(1);
        assert!(!refresh_expired.is_valid());
    }

    #[test]
    fn with_refreshed_tokens_preserves_metadata() {
        let bundle = make_test_bundle();
        let new_tokens = TokenInfo::new(
            "ghu_new_access".to_string(),
            7200,
            "ghr_new_refresh".to_string(),
            15_552_000,
        );

        let refreshed = bundle.with_refreshed_tokens(new_tokens);

        assert_eq!(refreshed.host, bundle.host);
        assert_eq!(refreshed.user.id, bundle.user.id);
        assert_eq!(
            refreshed.timestamps.created_at,
            bundle.timestamps.created_at
        );
        assert!(refreshed.timestamps.updated_at > bundle.timestamps.updated_at);
        assert_eq!(refreshed.tokens.access_token, "ghu_new_access");
    }

    #[test]
    fn debug_output_redacts_tokens() {
        let bundle = make_test_bundle();
        let debug_output = format!("{:?}", bundle);

        // Should NOT contain actual tokens
        assert!(
            !debug_output.contains("ghu_test_access_token"),
            "Debug output contains access token"
        );
        assert!(
            !debug_output.contains("ghr_test_refresh_token"),
            "Debug output contains refresh token"
        );

        // Should contain redaction markers
        assert!(debug_output.contains("[REDACTED]"));

        // Should contain non-sensitive info
        assert!(debug_output.contains("github.com"));
        assert!(debug_output.contains("octocat"));
    }

    #[test]
    fn token_info_debug_redacts() {
        let tokens = TokenInfo::new(
            "ghu_secret_access".to_string(),
            3600,
            "ghr_secret_refresh".to_string(),
            15_552_000,
        );

        let debug_output = format!("{:?}", tokens);

        assert!(!debug_output.contains("ghu_secret_access"));
        assert!(!debug_output.contains("ghr_secret_refresh"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    #[test]
    fn json_output_does_contain_tokens() {
        // Note: JSON output DOES contain tokens (for storage)
        // The redaction is only for Debug output
        let bundle = make_test_bundle();
        let json = bundle.to_json().expect("serialization failed");

        // JSON must contain tokens for storage to work
        assert!(json.contains("ghu_test_access_token"));
        assert!(json.contains("ghr_test_refresh_token"));
    }
}
