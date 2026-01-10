//! auth::device_flow
//!
//! OAuth device flow client for GitHub App authentication.
//!
//! # Device Flow Overview
//!
//! Per SPEC.md Section 8A.1, the device flow works as follows:
//!
//! 1. Client requests a device code from GitHub
//! 2. User visits verification URL and enters the code
//! 3. Client polls GitHub until authorization completes
//! 4. Client receives access and refresh tokens
//!
//! This flow is ideal for CLI tools because:
//! - No client secret is required (safe for distribution)
//! - No callback server needed
//! - Works in headless environments
//!
//! # Polling States
//!
//! During polling, GitHub returns one of:
//! - `authorization_pending` - Continue polling
//! - `slow_down` - Increase polling interval by 5 seconds
//! - `expired_token` - Device code expired, restart flow
//! - `access_denied` - User denied authorization
//!
//! # Token Refresh
//!
//! Refresh tokens are single-use and rotate on each refresh.
//! The client must handle the new refresh token from each response.
//!
//! # Example
//!
//! ```ignore
//! use latticework::auth::DeviceFlowClient;
//!
//! let client = DeviceFlowClient::new("github.com");
//!
//! // Step 1: Get device code
//! let device_code = client.request_device_code().await?;
//! println!("Visit {} and enter code: {}", device_code.verification_uri, device_code.user_code);
//!
//! // Step 2: Poll for authorization
//! let tokens = client.poll_for_token(&device_code).await?;
//!
//! // Step 3: Later, refresh the token
//! let new_tokens = client.refresh_token(&tokens.refresh_token).await?;
//! ```

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::time::sleep;

use super::errors::AuthError;
use super::token_bundle::GITHUB_APP_CLIENT_ID;

/// Default scopes requested for the GitHub App.
const DEFAULT_SCOPES: &str = "repo";

/// User-Agent header for OAuth requests.
const USER_AGENT: &str = "lattice-cli";

/// Response from device code request.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    /// The device verification code.
    pub device_code: String,

    /// The user verification code to display.
    pub user_code: String,

    /// The verification URL the user should visit.
    pub verification_uri: String,

    /// Seconds until the device code expires.
    pub expires_in: u64,

    /// Minimum polling interval in seconds.
    pub interval: u64,
}

/// Successful token response from GitHub.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    /// The access token (starts with "ghu_").
    pub access_token: String,

    /// Token type (always "bearer").
    pub token_type: String,

    /// Seconds until access token expires.
    pub expires_in: u64,

    /// The refresh token (starts with "ghr_").
    pub refresh_token: String,

    /// Seconds until refresh token expires.
    pub refresh_token_expires_in: u64,

    /// Granted scopes.
    pub scope: String,
}

/// Error response from GitHub OAuth endpoints.
#[derive(Debug, Clone, Deserialize)]
struct OAuthError {
    /// Error code.
    error: String,

    /// Human-readable description.
    #[allow(dead_code)]
    error_description: Option<String>,
}

/// Request body for device code endpoint.
#[derive(Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
    scope: &'a str,
}

/// Request body for token polling/refresh.
#[derive(Serialize)]
struct TokenRequest<'a> {
    client_id: &'a str,
    device_code: Option<&'a str>,
    refresh_token: Option<&'a str>,
    grant_type: &'a str,
}

/// Client for GitHub OAuth device flow.
///
/// Handles device code requests, polling, and token refresh.
#[derive(Debug, Clone)]
pub struct DeviceFlowClient {
    /// HTTP client.
    client: Client,

    /// GitHub host (e.g., "github.com").
    host: String,

    /// Client ID for the GitHub App.
    client_id: String,
}

impl DeviceFlowClient {
    /// Create a new device flow client.
    ///
    /// Uses the canonical Lattice GitHub App client ID.
    ///
    /// # Arguments
    ///
    /// * `host` - GitHub host (e.g., "github.com")
    pub fn new(host: &str) -> Self {
        Self {
            client: Client::new(),
            host: host.to_string(),
            client_id: GITHUB_APP_CLIENT_ID.to_string(),
        }
    }

    /// Create a device flow client with a custom client ID.
    ///
    /// For testing or GitHub Enterprise with a different app.
    #[cfg(test)]
    pub fn with_client_id(host: &str, client_id: &str) -> Self {
        Self {
            client: Client::new(),
            host: host.to_string(),
            client_id: client_id.to_string(),
        }
    }

    /// Get the device code endpoint URL.
    fn device_code_url(&self) -> String {
        format!("https://{}/login/device/code", self.host)
    }

    /// Get the token endpoint URL.
    fn token_url(&self) -> String {
        format!("https://{}/login/oauth/access_token", self.host)
    }

    /// Build headers for OAuth requests.
    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_static(USER_AGENT),
        );
        headers
    }

    /// Request a device code to begin the authorization flow.
    ///
    /// # Returns
    ///
    /// A [`DeviceCodeResponse`] containing the device code, user code,
    /// and verification URL to display to the user.
    ///
    /// # Errors
    ///
    /// - [`AuthError::DeviceFlowError`] if the request fails
    /// - [`AuthError::Network`] if there's a network error
    ///
    /// # Example
    ///
    /// ```ignore
    /// let response = client.request_device_code().await?;
    /// println!("Go to {} and enter: {}", response.verification_uri, response.user_code);
    /// ```
    pub async fn request_device_code(&self) -> Result<DeviceCodeResponse, AuthError> {
        let request = DeviceCodeRequest {
            client_id: &self.client_id,
            scope: DEFAULT_SCOPES,
        };

        let response = self
            .client
            .post(self.device_code_url())
            .headers(self.headers())
            .form(&request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if status.is_success() {
            serde_json::from_str(&body).map_err(|e| {
                AuthError::DeviceFlowError(format!("failed to parse device code response: {}", e))
            })
        } else {
            // Try to parse as error
            if let Ok(err) = serde_json::from_str::<OAuthError>(&body) {
                Err(AuthError::DeviceFlowError(format!(
                    "{}: {}",
                    err.error,
                    err.error_description.unwrap_or_default()
                )))
            } else {
                Err(AuthError::GitHubApi {
                    status: status.as_u16(),
                    message: body,
                })
            }
        }
    }

    /// Poll for token completion after user authorizes.
    ///
    /// This method blocks (with async sleep) until:
    /// - Authorization succeeds (returns tokens)
    /// - User denies authorization (returns error)
    /// - Device code expires (returns error)
    ///
    /// # Arguments
    ///
    /// * `device_code` - The device code response from `request_device_code()`
    ///
    /// # Polling Behavior
    ///
    /// - Starts with the interval specified in the device code response
    /// - If GitHub returns `slow_down`, increases interval by 5 seconds
    /// - Continues until success, denial, or expiration
    ///
    /// # Errors
    ///
    /// - [`AuthError::Cancelled`] if user denies authorization
    /// - [`AuthError::DeviceFlowExpired`] if device code expires
    /// - [`AuthError::Network`] if there's a network error
    pub async fn poll_for_token(
        &self,
        device_code: &DeviceCodeResponse,
    ) -> Result<TokenResponse, AuthError> {
        let deadline = Instant::now() + Duration::from_secs(device_code.expires_in);
        let mut interval = Duration::from_secs(device_code.interval);

        loop {
            // Check if expired
            if Instant::now() >= deadline {
                return Err(AuthError::DeviceFlowExpired);
            }

            // Wait before polling
            sleep(interval).await;

            // Poll for token
            match self.poll_once(&device_code.device_code).await {
                Ok(tokens) => return Ok(tokens),
                Err(PollResult::Pending) => {
                    // Continue polling
                }
                Err(PollResult::SlowDown) => {
                    // Increase interval by 5 seconds
                    interval += Duration::from_secs(5);
                }
                Err(PollResult::Expired) => {
                    return Err(AuthError::DeviceFlowExpired);
                }
                Err(PollResult::AccessDenied) => {
                    return Err(AuthError::Cancelled);
                }
                Err(PollResult::Error(e)) => {
                    return Err(e);
                }
            }
        }
    }

    /// Internal: Single poll attempt.
    async fn poll_once(&self, device_code: &str) -> Result<TokenResponse, PollResult> {
        let request = TokenRequest {
            client_id: &self.client_id,
            device_code: Some(device_code),
            refresh_token: None,
            grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        };

        let response = self
            .client
            .post(self.token_url())
            .headers(self.headers())
            .form(&request)
            .send()
            .await
            .map_err(|e| PollResult::Error(AuthError::Network(e.to_string())))?;

        let body = response
            .text()
            .await
            .map_err(|e| PollResult::Error(AuthError::Network(e.to_string())))?;

        // Try to parse as success
        if let Ok(tokens) = serde_json::from_str::<TokenResponse>(&body) {
            return Ok(tokens);
        }

        // Try to parse as error
        if let Ok(err) = serde_json::from_str::<OAuthError>(&body) {
            match err.error.as_str() {
                "authorization_pending" => Err(PollResult::Pending),
                "slow_down" => Err(PollResult::SlowDown),
                "expired_token" => Err(PollResult::Expired),
                "access_denied" => Err(PollResult::AccessDenied),
                _ => Err(PollResult::Error(AuthError::DeviceFlowError(format!(
                    "{}: {}",
                    err.error,
                    err.error_description.unwrap_or_default()
                )))),
            }
        } else {
            Err(PollResult::Error(AuthError::DeviceFlowError(format!(
                "unexpected response: {}",
                body
            ))))
        }
    }

    /// Refresh an access token using a refresh token.
    ///
    /// # Important
    ///
    /// Refresh tokens are **single-use**. After calling this method,
    /// the old refresh token is invalidated and the response contains
    /// a new refresh token that must be stored.
    ///
    /// # Arguments
    ///
    /// * `refresh_token` - The current refresh token
    ///
    /// # Returns
    ///
    /// A new [`TokenResponse`] with fresh access and refresh tokens.
    ///
    /// # Errors
    ///
    /// - [`AuthError::RefreshFailed`] if the refresh token is invalid
    /// - [`AuthError::Expired`] if the refresh token has expired
    /// - [`AuthError::Network`] if there's a network error
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse, AuthError> {
        let request = TokenRequest {
            client_id: &self.client_id,
            device_code: None,
            refresh_token: Some(refresh_token),
            grant_type: "refresh_token",
        };

        let response = self
            .client
            .post(self.token_url())
            .headers(self.headers())
            .form(&request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if let Ok(tokens) = serde_json::from_str::<TokenResponse>(&body) {
            return Ok(tokens);
        }

        // Parse error
        if let Ok(err) = serde_json::from_str::<OAuthError>(&body) {
            match err.error.as_str() {
                "bad_refresh_token" | "invalid_grant" => Err(AuthError::Expired(self.host.clone())),
                _ => Err(AuthError::RefreshFailed(format!(
                    "{}: {}",
                    err.error,
                    err.error_description.unwrap_or_default()
                ))),
            }
        } else {
            Err(AuthError::GitHubApi {
                status: status.as_u16(),
                message: body,
            })
        }
    }

    /// Fetch user info using an access token.
    ///
    /// Calls `GET /user` to get the authenticated user's info.
    ///
    /// # Arguments
    ///
    /// * `access_token` - A valid access token
    ///
    /// # Returns
    ///
    /// A [`UserInfoResponse`] with the user's ID and login.
    pub async fn fetch_user_info(&self, access_token: &str) -> Result<UserInfoResponse, AuthError> {
        let url = format!("https://api.{}/user", self.host);

        let response = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", access_token),
            )
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if status.is_success() {
            serde_json::from_str(&body).map_err(|e| {
                AuthError::DeviceFlowError(format!("failed to parse user info: {}", e))
            })
        } else {
            Err(AuthError::GitHubApi {
                status: status.as_u16(),
                message: body,
            })
        }
    }
}

/// User info from GitHub API.
#[derive(Debug, Clone, Deserialize)]
pub struct UserInfoResponse {
    /// GitHub user ID.
    pub id: u64,

    /// GitHub login/username.
    pub login: String,
}

/// Internal polling result states.
enum PollResult {
    /// Authorization pending, continue polling.
    Pending,
    /// Slow down, increase polling interval.
    SlowDown,
    /// Device code expired.
    Expired,
    /// User denied access.
    AccessDenied,
    /// Other error.
    Error(AuthError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_flow_client_new() {
        let client = DeviceFlowClient::new("github.com");
        assert_eq!(client.host, "github.com");
        assert_eq!(client.client_id, GITHUB_APP_CLIENT_ID);
    }

    #[test]
    fn device_code_url_format() {
        let client = DeviceFlowClient::new("github.com");
        assert_eq!(
            client.device_code_url(),
            "https://github.com/login/device/code"
        );
    }

    #[test]
    fn token_url_format() {
        let client = DeviceFlowClient::new("github.com");
        assert_eq!(
            client.token_url(),
            "https://github.com/login/oauth/access_token"
        );
    }

    #[test]
    fn github_enterprise_urls() {
        let client = DeviceFlowClient::new("github.example.com");
        assert_eq!(
            client.device_code_url(),
            "https://github.example.com/login/device/code"
        );
        assert_eq!(
            client.token_url(),
            "https://github.example.com/login/oauth/access_token"
        );
    }

    #[test]
    fn headers_include_accept_json() {
        let client = DeviceFlowClient::new("github.com");
        let headers = client.headers();
        assert_eq!(
            headers.get(ACCEPT).map(|v| v.to_str().ok()),
            Some(Some("application/json"))
        );
    }

    #[test]
    fn device_code_response_deserialize() {
        let json = r#"{
            "device_code": "abc123",
            "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        }"#;

        let response: DeviceCodeResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(response.device_code, "abc123");
        assert_eq!(response.user_code, "ABCD-1234");
        assert_eq!(response.verification_uri, "https://github.com/login/device");
        assert_eq!(response.expires_in, 900);
        assert_eq!(response.interval, 5);
    }

    #[test]
    fn token_response_deserialize() {
        let json = r#"{
            "access_token": "ghu_test_token",
            "token_type": "bearer",
            "expires_in": 28800,
            "refresh_token": "ghr_test_refresh",
            "refresh_token_expires_in": 15552000,
            "scope": "repo"
        }"#;

        let response: TokenResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(response.access_token, "ghu_test_token");
        assert_eq!(response.token_type, "bearer");
        assert_eq!(response.expires_in, 28800);
        assert_eq!(response.refresh_token, "ghr_test_refresh");
        assert_eq!(response.refresh_token_expires_in, 15552000);
        assert_eq!(response.scope, "repo");
    }

    #[test]
    fn oauth_error_deserialize() {
        let json = r#"{
            "error": "authorization_pending",
            "error_description": "The authorization request is still pending."
        }"#;

        let error: OAuthError = serde_json::from_str(json).expect("parse");
        assert_eq!(error.error, "authorization_pending");
        assert_eq!(
            error.error_description,
            Some("The authorization request is still pending.".to_string())
        );
    }

    #[test]
    fn user_info_response_deserialize() {
        let json = r#"{
            "id": 12345,
            "login": "octocat",
            "name": "The Octocat"
        }"#;

        let response: UserInfoResponse = serde_json::from_str(json).expect("parse");
        assert_eq!(response.id, 12345);
        assert_eq!(response.login, "octocat");
    }

    #[test]
    fn default_scopes_include_repo() {
        assert!(DEFAULT_SCOPES.contains("repo"));
    }
}
