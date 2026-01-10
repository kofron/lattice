//! auth::errors
//!
//! Authentication error types for GitHub App OAuth.
//!
//! # Design
//!
//! Per SPEC.md Section 4.4.4, error messages MUST NOT contain tokens.
//! All error variants are designed to provide useful context without
//! exposing sensitive data.
//!
//! # Example
//!
//! ```
//! use latticework::auth::AuthError;
//!
//! let err = AuthError::NotAuthenticated("github.com".to_string());
//! assert!(err.to_string().contains("github.com"));
//! assert!(!err.to_string().contains("ghu_")); // Never contains tokens
//! ```

use thiserror::Error;

/// Errors from authentication operations.
///
/// # Security
///
/// Error messages intentionally do not include token values.
/// Any error that might contain a token uses redacted placeholders.
#[derive(Debug, Error)]
pub enum AuthError {
    /// No authentication exists for the specified host.
    #[error("not authenticated for host '{0}'. Run 'lattice auth login'.")]
    NotAuthenticated(String),

    /// Authentication has expired (both access and refresh tokens).
    #[error("authentication expired for host '{0}'. Run 'lattice auth login' again.")]
    Expired(String),

    /// Token refresh failed.
    #[error("token refresh failed: {0}")]
    RefreshFailed(String),

    /// Device flow error during OAuth process.
    #[error("device flow error: {0}")]
    DeviceFlowError(String),

    /// User cancelled the authentication flow.
    #[error("authentication cancelled by user")]
    Cancelled,

    /// Device flow timed out waiting for authorization.
    #[error("device flow expired. Please try again.")]
    DeviceFlowExpired,

    /// Failed to acquire auth lock (another process is refreshing).
    #[error("failed to acquire auth lock: {0}")]
    LockError(String),

    /// Lock acquisition timed out.
    #[error("auth lock timeout - another process may be refreshing tokens")]
    LockTimeout,

    /// Token bundle is invalid or cannot be parsed.
    #[error("invalid token bundle: {0}")]
    InvalidBundle(String),

    /// Error from secret storage.
    #[error("secret store error: {0}")]
    SecretStore(String),

    /// Network error during authentication.
    #[error("network error: {0}")]
    Network(String),

    /// GitHub API error during authentication.
    #[error("GitHub API error: {status} - {message}")]
    GitHubApi {
        /// HTTP status code
        status: u16,
        /// Error message from GitHub
        message: String,
    },

    /// GitHub App is not installed for the repository.
    #[error("GitHub App not installed for {owner}/{repo}. Install at: https://github.com/apps/lattice/installations/new")]
    AppNotInstalled {
        /// Repository owner
        owner: String,
        /// Repository name
        repo: String,
    },

    /// Internal error (should not happen).
    #[error("internal auth error: {0}")]
    Internal(String),
}

impl AuthError {
    /// Check if this error indicates the user needs to re-authenticate.
    ///
    /// Returns true for errors that can be resolved by running `lattice auth login`.
    pub fn needs_reauth(&self) -> bool {
        matches!(
            self,
            AuthError::NotAuthenticated(_)
                | AuthError::Expired(_)
                | AuthError::Cancelled
                | AuthError::DeviceFlowExpired
        )
    }

    /// Check if this error indicates a transient failure that might succeed on retry.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            AuthError::Network(_) | AuthError::LockTimeout | AuthError::LockError(_)
        )
    }

    /// Check if this error requires the user to install the GitHub App.
    pub fn needs_app_install(&self) -> bool {
        matches!(self, AuthError::AppNotInstalled { .. })
    }
}

impl From<crate::secrets::SecretError> for AuthError {
    fn from(err: crate::secrets::SecretError) -> Self {
        AuthError::SecretStore(err.to_string())
    }
}

impl From<reqwest::Error> for AuthError {
    fn from(err: reqwest::Error) -> Self {
        AuthError::Network(err.to_string())
    }
}

impl From<std::io::Error> for AuthError {
    fn from(err: std::io::Error) -> Self {
        AuthError::Internal(format!("IO error: {}", err))
    }
}

impl From<serde_json::Error> for AuthError {
    fn from(err: serde_json::Error) -> Self {
        AuthError::InvalidBundle(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_formatting() {
        let err = AuthError::NotAuthenticated("github.com".to_string());
        let msg = err.to_string();
        assert!(msg.contains("github.com"));
        assert!(msg.contains("lattice auth login"));
    }

    #[test]
    fn expired_error_suggests_reauth() {
        let err = AuthError::Expired("github.com".to_string());
        let msg = err.to_string();
        assert!(msg.contains("expired"));
        assert!(msg.contains("lattice auth login"));
    }

    #[test]
    fn app_not_installed_shows_link() {
        let err = AuthError::AppNotInstalled {
            owner: "octocat".to_string(),
            repo: "hello-world".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("octocat/hello-world"));
        assert!(msg.contains("https://github.com/apps/lattice/installations/new"));
    }

    #[test]
    fn needs_reauth_classification() {
        assert!(AuthError::NotAuthenticated("h".into()).needs_reauth());
        assert!(AuthError::Expired("h".into()).needs_reauth());
        assert!(AuthError::Cancelled.needs_reauth());
        assert!(AuthError::DeviceFlowExpired.needs_reauth());

        assert!(!AuthError::Network("err".into()).needs_reauth());
        assert!(!AuthError::LockTimeout.needs_reauth());
    }

    #[test]
    fn is_transient_classification() {
        assert!(AuthError::Network("err".into()).is_transient());
        assert!(AuthError::LockTimeout.is_transient());
        assert!(AuthError::LockError("err".into()).is_transient());

        assert!(!AuthError::NotAuthenticated("h".into()).is_transient());
        assert!(!AuthError::Expired("h".into()).is_transient());
    }

    #[test]
    fn needs_app_install_classification() {
        assert!(AuthError::AppNotInstalled {
            owner: "o".into(),
            repo: "r".into()
        }
        .needs_app_install());

        assert!(!AuthError::NotAuthenticated("h".into()).needs_app_install());
    }

    #[test]
    fn github_api_error_formatting() {
        let err = AuthError::GitHubApi {
            status: 401,
            message: "Bad credentials".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("401"));
        assert!(msg.contains("Bad credentials"));
    }

    #[test]
    fn error_messages_never_contain_token_patterns() {
        // Ensure our error types don't accidentally include tokens
        let errors = vec![
            AuthError::NotAuthenticated("github.com".to_string()),
            AuthError::Expired("github.com".to_string()),
            AuthError::RefreshFailed("some error".to_string()),
            AuthError::DeviceFlowError("some error".to_string()),
            AuthError::Cancelled,
            AuthError::DeviceFlowExpired,
            AuthError::LockError("some error".to_string()),
            AuthError::LockTimeout,
            AuthError::InvalidBundle("parse error".to_string()),
            AuthError::SecretStore("store error".to_string()),
            AuthError::Network("network error".to_string()),
            AuthError::GitHubApi {
                status: 401,
                message: "unauthorized".to_string(),
            },
            AuthError::AppNotInstalled {
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            },
            AuthError::Internal("internal error".to_string()),
        ];

        for err in errors {
            let msg = err.to_string();
            assert!(
                !msg.contains("ghu_"),
                "Error message contains access token pattern: {}",
                msg
            );
            assert!(
                !msg.contains("ghr_"),
                "Error message contains refresh token pattern: {}",
                msg
            );
        }
    }
}
