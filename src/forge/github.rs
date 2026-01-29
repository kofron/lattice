//! forge::github
//!
//! GitHub forge implementation using REST and GraphQL APIs.
//!
//! # Design
//!
//! This module implements the `Forge` trait for GitHub. It uses:
//! - REST API for most operations (create/update/get/merge PRs, request reviewers)
//! - GraphQL API for draft status toggling (required by GitHub)
//!
//! # Authentication
//!
//! The preferred authentication method is via [`TokenProvider`] which handles
//! automatic token refresh. Legacy static token authentication is also supported
//! for backwards compatibility.
//!
//! Per SPEC.md §4.4 and ARCHITECTURE.md §11.3:
//! - Tokens are refreshed automatically when near expiry
//! - Auth lock prevents concurrent refresh races
//! - 401/403 errors trigger one retry with fresh token
//!
//! # Rate Limiting
//!
//! GitHub has rate limits. This implementation:
//! - Returns `ForgeError::RateLimited` when limits are hit
//! - Does not implement automatic retry for rate limits (caller's responsibility)
//!
//! # Example
//!
//! ```ignore
//! use latticework::forge::github::GitHubForge;
//! use latticework::forge::{Forge, CreatePrRequest};
//! use latticework::auth::{GitHubAuthManager, TokenProvider};
//! use std::sync::Arc;
//!
//! // Preferred: Use TokenProvider for automatic refresh
//! let auth_manager = Arc::new(GitHubAuthManager::new("github.com", store));
//! let forge = GitHubForge::new_with_provider(auth_manager, "owner", "repo");
//!
//! let pr = forge.create_pr(CreatePrRequest {
//!     head: "feature".to_string(),
//!     base: "main".to_string(),
//!     title: "Add feature".to_string(),
//!     body: None,
//!     draft: false,
//! }).await?;
//! ```
//!
//! [`TokenProvider`]: crate::auth::TokenProvider

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};

use super::traits::{
    CreatePrRequest, Forge, ForgeError, ListPullsOpts, ListPullsResult, MergeMethod, PrState,
    PullRequest, PullRequestSummary, Reviewers, UpdatePrRequest,
};
use crate::auth::TokenProvider;

/// Default GitHub API base URL.
const DEFAULT_API_BASE: &str = "https://api.github.com";

/// Default GitHub GraphQL endpoint.
const GRAPHQL_ENDPOINT: &str = "https://api.github.com/graphql";

/// User-Agent header value for API requests.
const USER_AGENT_VALUE: &str = "lattice-cli";

/// GitHub forge implementation.
///
/// Implements the `Forge` trait for GitHub using REST and GraphQL APIs.
///
/// # Authentication
///
/// Two authentication modes are supported:
///
/// 1. **TokenProvider (preferred)**: Use [`new_with_provider`] for automatic token refresh.
///    The provider is called for each API request, ensuring tokens are always fresh.
///
/// 2. **Static token (deprecated)**: Use [`new`] for backwards compatibility.
///    The token is stored directly and never refreshed.
///
/// [`new_with_provider`]: GitHubForge::new_with_provider
/// [`new`]: GitHubForge::new
pub struct GitHubForge {
    /// HTTP client for making requests
    client: Client,
    /// Token provider for automatic refresh (preferred)
    token_provider: Option<Arc<dyn TokenProvider>>,
    /// Static token for backwards compatibility (deprecated)
    static_token: Option<String>,
    /// Repository owner (user or organization)
    owner: String,
    /// Repository name
    repo: String,
    /// API base URL (configurable for GitHub Enterprise)
    api_base: String,
}

// Custom Debug to avoid exposing static_token
impl std::fmt::Debug for GitHubForge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubForge")
            .field("has_token_provider", &self.token_provider.is_some())
            .field("has_static_token", &self.static_token.is_some())
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("api_base", &self.api_base)
            .finish()
    }
}

impl GitHubForge {
    /// Create a new GitHub forge with a TokenProvider for automatic token refresh.
    ///
    /// This is the **preferred constructor** for production use. The TokenProvider
    /// is called for each API request, ensuring tokens are always fresh. If a
    /// 401 or 403 error occurs, the request is retried once with a refreshed token.
    ///
    /// # Arguments
    ///
    /// * `provider` - Token provider for automatic refresh
    /// * `owner` - Repository owner
    /// * `repo` - Repository name
    ///
    /// # Example
    ///
    /// ```ignore
    /// use latticework::auth::{GitHubAuthManager, TokenProvider};
    /// use std::sync::Arc;
    ///
    /// let store = secrets::create_store(secrets::DEFAULT_PROVIDER)?;
    /// let provider: Arc<dyn TokenProvider> = Arc::new(GitHubAuthManager::new("github.com", store));
    /// let forge = GitHubForge::new_with_provider(provider, "octocat", "hello-world");
    /// ```
    pub fn new_with_provider(
        provider: Arc<dyn TokenProvider>,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            token_provider: Some(provider),
            static_token: None,
            owner: owner.into(),
            repo: repo.into(),
            api_base: DEFAULT_API_BASE.to_string(),
        }
    }

    /// Create a new GitHub forge with a TokenProvider and custom API base URL.
    ///
    /// Use this for GitHub Enterprise installations with automatic token refresh.
    ///
    /// # Arguments
    ///
    /// * `provider` - Token provider for automatic refresh
    /// * `owner` - Repository owner
    /// * `repo` - Repository name
    /// * `api_base` - Custom API base URL (e.g., `https://github.example.com/api/v3`)
    pub fn new_with_provider_and_api_base(
        provider: Arc<dyn TokenProvider>,
        owner: impl Into<String>,
        repo: impl Into<String>,
        api_base: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            token_provider: Some(provider),
            static_token: None,
            owner: owner.into(),
            repo: repo.into(),
            api_base: api_base.into(),
        }
    }

    /// Create a new GitHub forge with a static token.
    ///
    /// # Deprecated
    ///
    /// This constructor is deprecated. Use [`new_with_provider`] for production
    /// to enable automatic token refresh and retry on auth failures.
    ///
    /// This method is retained for:
    /// - Tests that don't need token refresh
    /// - Backwards compatibility during migration
    ///
    /// # Arguments
    ///
    /// * `token` - Personal access token or GitHub App token
    /// * `owner` - Repository owner
    /// * `repo` - Repository name
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[allow(deprecated)]
    /// let forge = GitHubForge::new("ghp_xxx", "octocat", "hello-world");
    /// ```
    ///
    /// [`new_with_provider`]: GitHubForge::new_with_provider
    #[deprecated(
        since = "0.7.0",
        note = "Use new_with_provider for automatic token refresh"
    )]
    pub fn new(
        token: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            token_provider: None,
            static_token: Some(token.into()),
            owner: owner.into(),
            repo: repo.into(),
            api_base: DEFAULT_API_BASE.to_string(),
        }
    }

    /// Create a GitHub forge with a static token and custom API base URL.
    ///
    /// # Deprecated
    ///
    /// Use [`new_with_provider_and_api_base`] for GitHub Enterprise with automatic
    /// token refresh.
    ///
    /// [`new_with_provider_and_api_base`]: GitHubForge::new_with_provider_and_api_base
    #[deprecated(
        since = "0.7.0",
        note = "Use new_with_provider_and_api_base for automatic token refresh"
    )]
    pub fn with_api_base(
        token: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
        api_base: impl Into<String>,
    ) -> Self {
        Self {
            client: Client::new(),
            token_provider: None,
            static_token: Some(token.into()),
            owner: owner.into(),
            repo: repo.into(),
            api_base: api_base.into(),
        }
    }

    /// Create a GitHub forge from a remote URL with a TokenProvider.
    ///
    /// Parses the remote URL to extract owner and repo.
    ///
    /// # Arguments
    ///
    /// * `url` - Git remote URL (SSH or HTTPS format)
    /// * `provider` - Token provider for automatic refresh
    ///
    /// # Returns
    ///
    /// `Some(GitHubForge)` if URL is parseable, `None` otherwise.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use latticework::forge::github::GitHubForge;
    /// use latticework::auth::{GitHubAuthManager, TokenProvider};
    /// use std::sync::Arc;
    ///
    /// let provider: Arc<dyn TokenProvider> = Arc::new(auth_manager);
    /// let forge = GitHubForge::from_remote_url_with_provider(
    ///     "git@github.com:owner/repo.git",
    ///     provider,
    /// );
    /// ```
    pub fn from_remote_url_with_provider(
        url: &str,
        provider: Arc<dyn TokenProvider>,
    ) -> Option<Self> {
        let (owner, repo) = parse_github_url(url)?;
        Some(Self::new_with_provider(provider, owner, repo))
    }

    /// Create a GitHub forge from a remote URL with a static token.
    ///
    /// # Deprecated
    ///
    /// Use [`from_remote_url_with_provider`] for automatic token refresh.
    ///
    /// [`from_remote_url_with_provider`]: GitHubForge::from_remote_url_with_provider
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::github::GitHubForge;
    ///
    /// // SSH format
    /// #[allow(deprecated)]
    /// let forge = GitHubForge::from_remote_url("git@github.com:owner/repo.git", "token");
    /// assert!(forge.is_some());
    /// ```
    #[deprecated(
        since = "0.7.0",
        note = "Use from_remote_url_with_provider for automatic token refresh"
    )]
    #[allow(deprecated)]
    pub fn from_remote_url(url: &str, token: impl Into<String>) -> Option<Self> {
        let (owner, repo) = parse_github_url(url)?;
        Some(Self::new(token, owner, repo))
    }

    /// Get the repository owner.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Get the repository name.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Check if this forge has a TokenProvider (enables retry on auth failure).
    pub fn has_token_provider(&self) -> bool {
        self.token_provider.is_some()
    }

    /// Get the current bearer token, refreshing if needed.
    ///
    /// If a TokenProvider is configured, calls `bearer_token()` which may
    /// refresh the token if near expiry. Otherwise, returns the static token.
    ///
    /// # Errors
    ///
    /// Returns `ForgeError::AuthRequired` if no token source is configured.
    /// Returns `ForgeError::AuthFailed` if the TokenProvider fails.
    async fn get_bearer_token(&self) -> Result<String, ForgeError> {
        if let Some(ref provider) = self.token_provider {
            provider
                .bearer_token()
                .await
                .map_err(|e| ForgeError::AuthFailed(e.to_string()))
        } else if let Some(ref token) = self.static_token {
            Ok(token.clone())
        } else {
            Err(ForgeError::AuthRequired)
        }
    }

    /// Build common headers for API requests (async to allow token refresh).
    async fn headers(&self) -> Result<HeaderMap, ForgeError> {
        let token = self.get_bearer_token().await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token)).expect("Invalid token format"),
        );
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
        Ok(headers)
    }

    /// Check if an error is an auth failure that might be resolved by token refresh.
    fn is_retryable_auth_error(err: &ForgeError) -> bool {
        matches!(err, ForgeError::AuthFailed(_))
    }

    /// Build URL for a repository endpoint.
    fn repo_url(&self, path: &str) -> String {
        format!(
            "{}/repos/{}/{}/{}",
            self.api_base, self.owner, self.repo, path
        )
    }

    /// Handle API response, mapping errors appropriately.
    async fn handle_response<T: for<'de> Deserialize<'de>>(
        &self,
        response: Response,
    ) -> Result<T, ForgeError> {
        let status = response.status();

        if status.is_success() {
            response.json().await.map_err(|e| ForgeError::ApiError {
                status: status.as_u16(),
                message: format!("Failed to parse response: {}", e),
            })
        } else {
            self.handle_error_response(response, status).await
        }
    }

    /// Handle an error response from the API.
    async fn handle_error_response<T>(
        &self,
        response: Response,
        status: StatusCode,
    ) -> Result<T, ForgeError> {
        // Extract permission headers before consuming response body.
        // GitHub Apps use X-Accepted-GitHub-Permissions, classic OAuth uses X-Accepted-OAuth-Scopes.
        let headers = response.headers();
        let required_permissions = headers
            .get("X-Accepted-GitHub-Permissions")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let required_scopes = headers
            .get("X-Accepted-OAuth-Scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let granted_scopes = headers
            .get("X-OAuth-Scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Try to get error message from body
        let message = match response.json::<GitHubErrorResponse>().await {
            Ok(err) => err.message,
            Err(_) => "Unknown error".to_string(),
        };

        Err(match status {
            StatusCode::UNAUTHORIZED => ForgeError::AuthFailed("Invalid or expired token".into()),
            StatusCode::FORBIDDEN => {
                let mut err_msg = format!("Permission denied: {}", message);

                // For GitHub Apps, show the fine-grained permissions required
                if let Some(perms) = required_permissions {
                    if !perms.is_empty() {
                        err_msg.push_str(&format!(" [required: {}]", perms));
                    }
                }
                // For classic OAuth, show scopes
                else if let Some(scopes) = required_scopes {
                    if !scopes.is_empty() {
                        err_msg.push_str(&format!(" [required scopes: {}]", scopes));
                        if let Some(granted) = granted_scopes {
                            err_msg.push_str(&format!(" [granted: {}]", granted));
                        }
                    }
                }

                ForgeError::AuthFailed(err_msg)
            }
            StatusCode::NOT_FOUND => ForgeError::NotFound(message),
            StatusCode::UNPROCESSABLE_ENTITY => ForgeError::ApiError {
                status: status.as_u16(),
                message,
            },
            StatusCode::TOO_MANY_REQUESTS => ForgeError::RateLimited,
            _ if status.is_server_error() => ForgeError::ApiError {
                status: status.as_u16(),
                message: format!("GitHub server error: {}", message),
            },
            _ => ForgeError::ApiError {
                status: status.as_u16(),
                message,
            },
        })
    }

    /// Execute a GraphQL mutation for draft status toggle.
    async fn graphql_set_draft(&self, node_id: &str, draft: bool) -> Result<(), ForgeError> {
        let mutation = if draft {
            r#"mutation($id: ID!) {
                convertPullRequestToDraft(input: {pullRequestId: $id}) {
                    pullRequest { id }
                }
            }"#
        } else {
            r#"mutation($id: ID!) {
                markPullRequestReadyForReview(input: {pullRequestId: $id}) {
                    pullRequest { id }
                }
            }"#
        };

        let body = serde_json::json!({
            "query": mutation,
            "variables": { "id": node_id }
        });

        // Helper to execute graphql and handle response
        let execute_graphql = |headers: HeaderMap| {
            let client = &self.client;
            let body = &body;
            async move {
                let response = client
                    .post(GRAPHQL_ENDPOINT)
                    .headers(headers)
                    .json(body)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

                let status = response.status();
                if status.is_success() {
                    let result: GraphQLResponse =
                        response.json().await.map_err(|e| ForgeError::ApiError {
                            status: status.as_u16(),
                            message: format!("Failed to parse GraphQL response: {}", e),
                        })?;

                    if let Some(errors) = result.errors {
                        if !errors.is_empty() {
                            return Err(ForgeError::ApiError {
                                status: 200,
                                message: errors[0].message.clone(),
                            });
                        }
                    }
                    Ok(())
                } else {
                    // For non-success, we need to parse the error
                    // Since we can't call self.handle_error_response in the closure,
                    // return a special error that we can retry
                    match status {
                        StatusCode::UNAUTHORIZED => {
                            Err(ForgeError::AuthFailed("Invalid or expired token".into()))
                        }
                        StatusCode::FORBIDDEN => {
                            Err(ForgeError::AuthFailed("Permission denied".into()))
                        }
                        _ => {
                            let message = response
                                .json::<GitHubErrorResponse>()
                                .await
                                .map(|e| e.message)
                                .unwrap_or_else(|_| "Unknown error".to_string());
                            Err(ForgeError::ApiError {
                                status: status.as_u16(),
                                message,
                            })
                        }
                    }
                }
            }
        };

        // First attempt
        let result = execute_graphql(self.headers().await?).await;

        // Retry once on auth failure if we have a TokenProvider
        match result {
            Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                execute_graphql(self.headers().await?).await
            }
            other => other,
        }
    }
}

#[async_trait]
impl Forge for GitHubForge {
    fn name(&self) -> &'static str {
        "github"
    }

    async fn create_pr(&self, request: CreatePrRequest) -> Result<PullRequest, ForgeError> {
        let url = self.repo_url("pulls");

        let body = CreatePrBody {
            head: &request.head,
            base: &request.base,
            title: &request.title,
            body: request.body.as_deref(),
            draft: request.draft,
        };

        // First attempt
        let response = self
            .client
            .post(&url)
            .headers(self.headers().await?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        let result: Result<GitHubPullRequest, ForgeError> = self.handle_response(response).await;

        // Retry once on auth failure if we have a TokenProvider
        let pr: GitHubPullRequest = match result {
            Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                let response = self
                    .client
                    .post(&url)
                    .headers(self.headers().await?)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                self.handle_response(response).await?
            }
            other => other?,
        };

        Ok(pr.into())
    }

    async fn update_pr(&self, request: UpdatePrRequest) -> Result<PullRequest, ForgeError> {
        let url = self.repo_url(&format!("pulls/{}", request.number));

        let body = UpdatePrBody {
            title: request.title.as_deref(),
            body: request.body.as_deref(),
            base: request.base.as_deref(),
        };

        // First attempt
        let response = self
            .client
            .patch(&url)
            .headers(self.headers().await?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        let result: Result<GitHubPullRequest, ForgeError> = self.handle_response(response).await;

        // Retry once on auth failure if we have a TokenProvider
        let pr: GitHubPullRequest = match result {
            Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                let response = self
                    .client
                    .patch(&url)
                    .headers(self.headers().await?)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                self.handle_response(response).await?
            }
            other => other?,
        };

        Ok(pr.into())
    }

    async fn get_pr(&self, number: u64) -> Result<PullRequest, ForgeError> {
        let url = self.repo_url(&format!("pulls/{}", number));

        // First attempt
        let response = self
            .client
            .get(&url)
            .headers(self.headers().await?)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        let result: Result<GitHubPullRequest, ForgeError> = self.handle_response(response).await;

        // Retry once on auth failure if we have a TokenProvider
        let pr: GitHubPullRequest = match result {
            Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                let response = self
                    .client
                    .get(&url)
                    .headers(self.headers().await?)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                self.handle_response(response).await?
            }
            other => other?,
        };

        Ok(pr.into())
    }

    async fn find_pr_by_head(&self, head: &str) -> Result<Option<PullRequest>, ForgeError> {
        // GitHub API requires owner:branch format for cross-fork PRs
        // For same-repo, just the branch name works
        let head_param = if head.contains(':') {
            head.to_string()
        } else {
            format!("{}:{}", self.owner, head)
        };

        let url = format!(
            "{}/repos/{}/{}/pulls?head={}&state=open",
            self.api_base, self.owner, self.repo, head_param
        );

        // First attempt
        let response = self
            .client
            .get(&url)
            .headers(self.headers().await?)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        let result: Result<Vec<GitHubPullRequest>, ForgeError> =
            self.handle_response(response).await;

        // Retry once on auth failure if we have a TokenProvider
        let prs: Vec<GitHubPullRequest> = match result {
            Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                let response = self
                    .client
                    .get(&url)
                    .headers(self.headers().await?)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                self.handle_response(response).await?
            }
            other => other?,
        };

        Ok(prs.into_iter().next().map(Into::into))
    }

    async fn set_draft(&self, number: u64, draft: bool) -> Result<(), ForgeError> {
        // First, get the PR to retrieve its node_id
        let pr = self.get_pr(number).await?;

        let node_id = pr.node_id.ok_or_else(|| ForgeError::ApiError {
            status: 0,
            message: "PR is missing node_id required for draft toggle".into(),
        })?;

        // Use GraphQL to toggle draft status
        self.graphql_set_draft(&node_id, draft).await
    }

    async fn request_reviewers(&self, number: u64, reviewers: Reviewers) -> Result<(), ForgeError> {
        if reviewers.is_empty() {
            return Ok(());
        }

        let url = self.repo_url(&format!("pulls/{}/requested_reviewers", number));

        let body = RequestReviewersBody {
            reviewers: &reviewers.users,
            team_reviewers: &reviewers.teams,
        };

        // First attempt
        let response = self
            .client
            .post(&url)
            .headers(self.headers().await?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        let status = response.status();
        let result: Result<(), ForgeError> = if status.is_success() {
            Ok(())
        } else {
            self.handle_error_response(response, status).await
        };

        // Retry once on auth failure if we have a TokenProvider
        match result {
            Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                let response = self
                    .client
                    .post(&url)
                    .headers(self.headers().await?)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                let status = response.status();
                if status.is_success() {
                    Ok(())
                } else {
                    self.handle_error_response(response, status).await
                }
            }
            other => other,
        }
    }

    async fn merge_pr(&self, number: u64, method: MergeMethod) -> Result<(), ForgeError> {
        let url = self.repo_url(&format!("pulls/{}/merge", number));

        let merge_method = match method {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        };

        let body = MergePrBody { merge_method };

        // First attempt
        let response = self
            .client
            .put(&url)
            .headers(self.headers().await?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        let status = response.status();
        let result: Result<(), ForgeError> = if status.is_success() {
            Ok(())
        } else {
            self.handle_error_response(response, status).await
        };

        // Retry once on auth failure if we have a TokenProvider
        match result {
            Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                let response = self
                    .client
                    .put(&url)
                    .headers(self.headers().await?)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                let status = response.status();
                if status.is_success() {
                    Ok(())
                } else {
                    self.handle_error_response(response, status).await
                }
            }
            other => other,
        }
    }

    async fn list_open_prs(&self, opts: ListPullsOpts) -> Result<ListPullsResult, ForgeError> {
        let limit = opts.effective_limit();
        let per_page: u32 = 100; // GitHub's max per page

        let mut all_prs: Vec<PullRequestSummary> = Vec::with_capacity(limit.min(100));
        let mut page: u32 = 1;
        let mut truncated = false;

        loop {
            // Fetch a page of PRs sorted by updated_at descending
            let url = format!(
                "{}/repos/{}/{}/pulls?state=open&sort=updated&direction=desc&per_page={}&page={}",
                self.api_base, self.owner, self.repo, per_page, page
            );

            // First attempt
            let response = self
                .client
                .get(&url)
                .headers(self.headers().await?)
                .send()
                .await
                .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

            let result: Result<Vec<GitHubPullRequestListItem>, ForgeError> =
                self.handle_response(response).await;

            // Retry once on auth failure if we have a TokenProvider
            let page_prs: Vec<GitHubPullRequestListItem> = match result {
                Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                    let response = self
                        .client
                        .get(&url)
                        .headers(self.headers().await?)
                        .send()
                        .await
                        .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                    self.handle_response(response).await?
                }
                other => other?,
            };

            let page_count = page_prs.len();

            for pr in page_prs {
                if all_prs.len() >= limit {
                    truncated = true;
                    break;
                }
                all_prs.push(pr.into());
            }

            // Stop if we hit the limit or no more pages
            if all_prs.len() >= limit || page_count < per_page as usize {
                break;
            }

            page += 1;
        }

        // If we stopped exactly at limit and the last page was full, we're likely truncated
        if all_prs.len() == limit && !truncated {
            // We may or may not be truncated - be conservative
            // The only way to know for sure is to fetch one more item
            // For simplicity, we don't mark as truncated unless we explicitly stopped early
        }

        Ok(ListPullsResult {
            pulls: all_prs,
            truncated,
        })
    }

    async fn list_closed_prs_targeting(
        &self,
        opts: super::ListClosedPrsOpts,
    ) -> Result<ListPullsResult, ForgeError> {
        let limit = opts.effective_limit();
        let per_page: u32 = 100; // GitHub's max per page

        let mut all_prs: Vec<PullRequestSummary> = Vec::with_capacity(limit.min(100));
        let mut page: u32 = 1;
        let mut truncated = false;

        loop {
            // Fetch a page of closed PRs filtered by base branch
            // Note: GitHub's base filter works for closed PRs too
            // Branch names typically don't need URL encoding for basic ASCII chars
            let url = format!(
                "{}/repos/{}/{}/pulls?state=closed&base={}&sort=updated&direction=desc&per_page={}&page={}",
                self.api_base,
                self.owner,
                self.repo,
                &opts.base,
                per_page,
                page
            );

            // First attempt
            let response = self
                .client
                .get(&url)
                .headers(self.headers().await?)
                .send()
                .await
                .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

            let result: Result<Vec<GitHubPullRequestListItem>, ForgeError> =
                self.handle_response(response).await;

            // Retry once on auth failure if we have a TokenProvider
            let page_prs: Vec<GitHubPullRequestListItem> = match result {
                Err(ref e) if Self::is_retryable_auth_error(e) && self.has_token_provider() => {
                    let response = self
                        .client
                        .get(&url)
                        .headers(self.headers().await?)
                        .send()
                        .await
                        .map_err(|e| ForgeError::NetworkError(e.to_string()))?;
                    self.handle_response(response).await?
                }
                other => other?,
            };

            let page_count = page_prs.len();

            for pr in page_prs {
                if all_prs.len() >= limit {
                    truncated = true;
                    break;
                }
                all_prs.push(pr.into());
            }

            // Stop if we hit the limit or no more pages
            if all_prs.len() >= limit || page_count < per_page as usize {
                break;
            }

            page += 1;
        }

        Ok(ListPullsResult {
            pulls: all_prs,
            truncated,
        })
    }
}

// --------------------------------------------------------------------------
// API Request/Response Types
// --------------------------------------------------------------------------

/// Request body for creating a PR.
#[derive(Serialize)]
struct CreatePrBody<'a> {
    head: &'a str,
    base: &'a str,
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    draft: bool,
}

/// Request body for updating a PR.
#[derive(Serialize)]
struct UpdatePrBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<&'a str>,
}

/// Request body for requesting reviewers.
#[derive(Serialize)]
struct RequestReviewersBody<'a> {
    reviewers: &'a [String],
    team_reviewers: &'a [String],
}

/// Request body for merging a PR.
#[derive(Serialize)]
struct MergePrBody<'a> {
    merge_method: &'a str,
}

/// GitHub error response format.
#[derive(Deserialize)]
struct GitHubErrorResponse {
    message: String,
}

/// GitHub PR response format.
#[derive(Deserialize)]
struct GitHubPullRequest {
    number: u64,
    html_url: String,
    state: String,
    draft: bool,
    head: GitHubRef,
    base: GitHubRef,
    title: String,
    body: Option<String>,
    node_id: String,
    merged: Option<bool>,
}

/// GitHub ref (head/base) format.
#[derive(Deserialize)]
struct GitHubRef {
    #[serde(rename = "ref")]
    ref_name: String,
}

/// GitHub PR list item response (subset of full PR for list endpoint).
///
/// Used by `list_open_prs` to avoid parsing unused fields.
#[derive(Deserialize)]
struct GitHubPullRequestListItem {
    number: u64,
    html_url: String,
    draft: bool,
    head: GitHubHeadRefWithRepo,
    base: GitHubRef,
    updated_at: String,
}

/// GitHub head ref with repository info (for fork detection).
#[derive(Deserialize)]
struct GitHubHeadRefWithRepo {
    #[serde(rename = "ref")]
    ref_name: String,
    /// Repository info (None for deleted forks)
    repo: Option<GitHubRepoInfo>,
}

/// Minimal GitHub repository info.
#[derive(Deserialize)]
struct GitHubRepoInfo {
    owner: GitHubOwnerInfo,
}

/// Minimal GitHub owner info.
#[derive(Deserialize)]
struct GitHubOwnerInfo {
    login: String,
}

impl From<GitHubPullRequestListItem> for PullRequestSummary {
    fn from(gh: GitHubPullRequestListItem) -> Self {
        let head_repo_owner = gh.head.repo.map(|r| r.owner.login);

        PullRequestSummary {
            number: gh.number,
            head_ref: gh.head.ref_name,
            head_repo_owner,
            base_ref: gh.base.ref_name,
            is_draft: gh.draft,
            url: gh.html_url,
            updated_at: gh.updated_at,
        }
    }
}

/// GraphQL response wrapper.
#[derive(Deserialize)]
struct GraphQLResponse {
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
    errors: Option<Vec<GraphQLError>>,
}

/// GraphQL error format.
#[derive(Deserialize)]
struct GraphQLError {
    message: String,
}

impl From<GitHubPullRequest> for PullRequest {
    fn from(pr: GitHubPullRequest) -> Self {
        let state = if pr.merged.unwrap_or(false) {
            PrState::Merged
        } else if pr.state == "closed" {
            PrState::Closed
        } else {
            PrState::Open
        };

        PullRequest {
            number: pr.number,
            url: pr.html_url,
            state,
            is_draft: pr.draft,
            head: pr.head.ref_name,
            base: pr.base.ref_name,
            title: pr.title,
            body: pr.body,
            node_id: Some(pr.node_id),
        }
    }
}

// --------------------------------------------------------------------------
// URL Parsing
// --------------------------------------------------------------------------

/// Parse a GitHub remote URL to extract owner and repo.
///
/// Supports both SSH and HTTPS formats:
/// - `git@github.com:owner/repo.git`
/// - `https://github.com/owner/repo.git`
/// - `https://github.com/owner/repo`
///
/// # Returns
///
/// `Some((owner, repo))` if the URL is a valid GitHub URL, `None` otherwise.
///
/// # Example
///
/// ```
/// use latticework::forge::github::parse_github_url;
///
/// let (owner, repo) = parse_github_url("git@github.com:octocat/hello-world.git").unwrap();
/// assert_eq!(owner, "octocat");
/// assert_eq!(repo, "hello-world");
/// ```
pub fn parse_github_url(url: &str) -> Option<(String, String)> {
    // SSH format: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // HTTPS format: https://github.com/owner/repo.git
    if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        let rest = rest.strip_suffix(".git").unwrap_or(rest);
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 && !parts[1].is_empty() {
            return Some((parts[0].to_string(), parts[1].to_string()));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_github_url {
        use super::*;

        #[test]
        fn ssh_with_git_suffix() {
            let result = parse_github_url("git@github.com:octocat/hello-world.git");
            assert_eq!(
                result,
                Some(("octocat".to_string(), "hello-world".to_string()))
            );
        }

        #[test]
        fn ssh_without_git_suffix() {
            let result = parse_github_url("git@github.com:octocat/hello-world");
            assert_eq!(
                result,
                Some(("octocat".to_string(), "hello-world".to_string()))
            );
        }

        #[test]
        fn https_with_git_suffix() {
            let result = parse_github_url("https://github.com/octocat/hello-world.git");
            assert_eq!(
                result,
                Some(("octocat".to_string(), "hello-world".to_string()))
            );
        }

        #[test]
        fn https_without_git_suffix() {
            let result = parse_github_url("https://github.com/octocat/hello-world");
            assert_eq!(
                result,
                Some(("octocat".to_string(), "hello-world".to_string()))
            );
        }

        #[test]
        fn http_format() {
            let result = parse_github_url("http://github.com/octocat/hello-world.git");
            assert_eq!(
                result,
                Some(("octocat".to_string(), "hello-world".to_string()))
            );
        }

        #[test]
        fn non_github_url() {
            assert!(parse_github_url("git@gitlab.com:owner/repo.git").is_none());
            assert!(parse_github_url("https://gitlab.com/owner/repo").is_none());
            assert!(parse_github_url("https://bitbucket.org/owner/repo").is_none());
        }

        #[test]
        fn invalid_format() {
            assert!(parse_github_url("not a url").is_none());
            assert!(parse_github_url("github.com/owner/repo").is_none());
            assert!(parse_github_url("https://github.com/").is_none());
            assert!(parse_github_url("https://github.com/owner").is_none());
        }

        #[test]
        fn repo_with_dots() {
            let result = parse_github_url("git@github.com:owner/repo.name.git");
            assert_eq!(result, Some(("owner".to_string(), "repo.name".to_string())));
        }

        #[test]
        fn repo_with_hyphens() {
            let result = parse_github_url("git@github.com:my-org/my-repo.git");
            assert_eq!(result, Some(("my-org".to_string(), "my-repo".to_string())));
        }
    }

    mod github_forge {
        use super::*;

        #[test]
        #[allow(deprecated)]
        fn new_creates_forge() {
            let forge = GitHubForge::new("token", "owner", "repo");
            assert_eq!(forge.name(), "github");
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.repo(), "repo");
            assert!(!forge.has_token_provider());
        }

        #[test]
        #[allow(deprecated)]
        fn from_remote_url_ssh() {
            let forge = GitHubForge::from_remote_url("git@github.com:owner/repo.git", "token");
            assert!(forge.is_some());
            let forge = forge.unwrap();
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.repo(), "repo");
            assert!(!forge.has_token_provider());
        }

        #[test]
        #[allow(deprecated)]
        fn from_remote_url_https() {
            let forge = GitHubForge::from_remote_url("https://github.com/owner/repo.git", "token");
            assert!(forge.is_some());
            let forge = forge.unwrap();
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.repo(), "repo");
        }

        #[test]
        #[allow(deprecated)]
        fn from_remote_url_invalid() {
            let forge = GitHubForge::from_remote_url("https://gitlab.com/owner/repo", "token");
            assert!(forge.is_none());
        }

        #[test]
        #[allow(deprecated)]
        fn with_api_base() {
            let forge = GitHubForge::with_api_base(
                "token",
                "owner",
                "repo",
                "https://github.example.com/api/v3",
            );
            assert_eq!(forge.api_base, "https://github.example.com/api/v3");
            assert!(!forge.has_token_provider());
        }

        #[test]
        #[allow(deprecated)]
        fn repo_url_format() {
            let forge = GitHubForge::new("token", "octocat", "hello-world");
            assert_eq!(
                forge.repo_url("pulls"),
                "https://api.github.com/repos/octocat/hello-world/pulls"
            );
            assert_eq!(
                forge.repo_url("pulls/123"),
                "https://api.github.com/repos/octocat/hello-world/pulls/123"
            );
        }

        #[test]
        #[allow(deprecated)]
        fn debug_redacts_static_token() {
            let forge = GitHubForge::new("secret_token_abc123", "owner", "repo");
            let debug_output = format!("{:?}", forge);
            assert!(!debug_output.contains("secret_token_abc123"));
            assert!(debug_output.contains("has_static_token"));
            assert!(debug_output.contains("owner"));
        }
    }

    mod github_forge_with_provider {
        use super::*;
        use std::sync::Arc;

        /// Mock TokenProvider for testing
        struct MockTokenProvider {
            token: String,
            host: String,
        }

        impl MockTokenProvider {
            fn new(token: &str, host: &str) -> Self {
                Self {
                    token: token.to_string(),
                    host: host.to_string(),
                }
            }
        }

        #[async_trait]
        impl crate::auth::TokenProvider for MockTokenProvider {
            async fn bearer_token(&self) -> Result<String, crate::auth::AuthError> {
                Ok(self.token.clone())
            }

            fn is_authenticated(&self) -> bool {
                true
            }

            fn host(&self) -> &str {
                &self.host
            }
        }

        #[test]
        fn new_with_provider_creates_forge() {
            let provider: Arc<dyn crate::auth::TokenProvider> =
                Arc::new(MockTokenProvider::new("test_token", "github.com"));
            let forge = GitHubForge::new_with_provider(provider, "owner", "repo");

            assert_eq!(forge.name(), "github");
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.repo(), "repo");
            assert!(forge.has_token_provider());
        }

        #[test]
        fn new_with_provider_and_api_base() {
            let provider: Arc<dyn crate::auth::TokenProvider> =
                Arc::new(MockTokenProvider::new("test_token", "github.example.com"));
            let forge = GitHubForge::new_with_provider_and_api_base(
                provider,
                "owner",
                "repo",
                "https://github.example.com/api/v3",
            );

            assert_eq!(forge.api_base, "https://github.example.com/api/v3");
            assert!(forge.has_token_provider());
        }

        #[test]
        fn from_remote_url_with_provider() {
            let provider: Arc<dyn crate::auth::TokenProvider> =
                Arc::new(MockTokenProvider::new("test_token", "github.com"));
            let forge = GitHubForge::from_remote_url_with_provider(
                "git@github.com:owner/repo.git",
                provider,
            );

            assert!(forge.is_some());
            let forge = forge.unwrap();
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.repo(), "repo");
            assert!(forge.has_token_provider());
        }

        #[test]
        fn from_remote_url_with_provider_invalid_url() {
            let provider: Arc<dyn crate::auth::TokenProvider> =
                Arc::new(MockTokenProvider::new("test_token", "github.com"));
            let forge = GitHubForge::from_remote_url_with_provider(
                "https://gitlab.com/owner/repo",
                provider,
            );

            assert!(forge.is_none());
        }

        #[test]
        fn debug_does_not_expose_token_provider() {
            let provider: Arc<dyn crate::auth::TokenProvider> =
                Arc::new(MockTokenProvider::new("secret_token_xyz", "github.com"));
            let forge = GitHubForge::new_with_provider(provider, "owner", "repo");

            let debug_output = format!("{:?}", forge);
            assert!(!debug_output.contains("secret_token_xyz"));
            assert!(debug_output.contains("has_token_provider"));
            assert!(debug_output.contains("owner"));
        }

        #[tokio::test]
        async fn get_bearer_token_uses_provider() {
            let provider: Arc<dyn crate::auth::TokenProvider> =
                Arc::new(MockTokenProvider::new("my_test_token", "github.com"));
            let forge = GitHubForge::new_with_provider(provider, "owner", "repo");

            let token = forge.get_bearer_token().await.unwrap();
            assert_eq!(token, "my_test_token");
        }

        #[tokio::test]
        #[allow(deprecated)]
        async fn get_bearer_token_uses_static_token() {
            let forge = GitHubForge::new("static_token_123", "owner", "repo");

            let token = forge.get_bearer_token().await.unwrap();
            assert_eq!(token, "static_token_123");
        }

        #[test]
        fn is_retryable_auth_error_returns_true_for_auth_failed() {
            let err = ForgeError::AuthFailed("token expired".into());
            assert!(GitHubForge::is_retryable_auth_error(&err));
        }

        #[test]
        fn is_retryable_auth_error_returns_false_for_other_errors() {
            assert!(!GitHubForge::is_retryable_auth_error(
                &ForgeError::RateLimited
            ));
            assert!(!GitHubForge::is_retryable_auth_error(
                &ForgeError::NotFound("PR not found".into())
            ));
            assert!(!GitHubForge::is_retryable_auth_error(
                &ForgeError::NetworkError("connection failed".into())
            ));
            assert!(!GitHubForge::is_retryable_auth_error(
                &ForgeError::ApiError {
                    status: 500,
                    message: "server error".into(),
                }
            ));
        }
    }

    mod github_pull_request {
        use super::*;

        #[test]
        fn from_open_pr() {
            let gh_pr = GitHubPullRequest {
                number: 42,
                html_url: "https://github.com/owner/repo/pull/42".to_string(),
                state: "open".to_string(),
                draft: false,
                head: GitHubRef {
                    ref_name: "feature".to_string(),
                },
                base: GitHubRef {
                    ref_name: "main".to_string(),
                },
                title: "Add feature".to_string(),
                body: Some("PR description".to_string()),
                node_id: "PR_123".to_string(),
                merged: Some(false),
            };

            let pr: PullRequest = gh_pr.into();
            assert_eq!(pr.number, 42);
            assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
            assert_eq!(pr.state, PrState::Open);
            assert!(!pr.is_draft);
            assert_eq!(pr.head, "feature");
            assert_eq!(pr.base, "main");
            assert_eq!(pr.title, "Add feature");
            assert_eq!(pr.body, Some("PR description".to_string()));
            assert_eq!(pr.node_id, Some("PR_123".to_string()));
        }

        #[test]
        fn from_draft_pr() {
            let gh_pr = GitHubPullRequest {
                number: 42,
                html_url: "https://github.com/owner/repo/pull/42".to_string(),
                state: "open".to_string(),
                draft: true,
                head: GitHubRef {
                    ref_name: "feature".to_string(),
                },
                base: GitHubRef {
                    ref_name: "main".to_string(),
                },
                title: "WIP: Add feature".to_string(),
                body: None,
                node_id: "PR_123".to_string(),
                merged: None,
            };

            let pr: PullRequest = gh_pr.into();
            assert!(pr.is_draft);
            assert_eq!(pr.state, PrState::Open);
            assert!(pr.body.is_none());
        }

        #[test]
        fn from_merged_pr() {
            let gh_pr = GitHubPullRequest {
                number: 42,
                html_url: "https://github.com/owner/repo/pull/42".to_string(),
                state: "closed".to_string(),
                draft: false,
                head: GitHubRef {
                    ref_name: "feature".to_string(),
                },
                base: GitHubRef {
                    ref_name: "main".to_string(),
                },
                title: "Add feature".to_string(),
                body: Some("Merged!".to_string()),
                node_id: "PR_123".to_string(),
                merged: Some(true),
            };

            let pr: PullRequest = gh_pr.into();
            assert_eq!(pr.state, PrState::Merged);
        }

        #[test]
        fn from_closed_pr() {
            let gh_pr = GitHubPullRequest {
                number: 42,
                html_url: "https://github.com/owner/repo/pull/42".to_string(),
                state: "closed".to_string(),
                draft: false,
                head: GitHubRef {
                    ref_name: "feature".to_string(),
                },
                base: GitHubRef {
                    ref_name: "main".to_string(),
                },
                title: "Add feature".to_string(),
                body: None,
                node_id: "PR_123".to_string(),
                merged: Some(false),
            };

            let pr: PullRequest = gh_pr.into();
            assert_eq!(pr.state, PrState::Closed);
        }
    }
}
