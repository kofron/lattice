//! forge::traits
//!
//! Forge trait definition for interacting with remote hosting services.
//!
//! # Design
//!
//! The `Forge` trait is async because forge operations involve network I/O.
//! All methods return `Result` to handle API errors gracefully.
//!
//! Per ARCHITECTURE.md Section 11, host adapters:
//! - Are invoked only after local structural invariants are satisfied
//! - May fail without compromising local correctness
//! - Write results only to cached metadata fields
//!
//! # Example
//!
//! ```ignore
//! use latticework::forge::{Forge, CreatePrRequest};
//!
//! async fn submit_pr(forge: &dyn Forge) -> Result<(), ForgeError> {
//!     let request = CreatePrRequest {
//!         head: "feature-branch".to_string(),
//!         base: "main".to_string(),
//!         title: "Add feature".to_string(),
//!         body: Some("Description".to_string()),
//!         draft: false,
//!     };
//!     let pr = forge.create_pr(request).await?;
//!     println!("Created PR #{}: {}", pr.number, pr.url);
//!     Ok(())
//! }
//! ```

use async_trait::async_trait;
use thiserror::Error;

/// Errors from forge operations.
///
/// These error types map to common failure modes when interacting
/// with remote hosting services like GitHub.
#[derive(Debug, Clone, Error)]
pub enum ForgeError {
    /// Authentication is required but not available.
    #[error("authentication required")]
    AuthRequired,

    /// Authentication failed (invalid token, expired, insufficient permissions).
    #[error("authentication failed: {0}")]
    AuthFailed(String),

    /// The requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Rate limit exceeded.
    #[error("rate limited")]
    RateLimited,

    /// API returned an error.
    #[error("API error: {status} - {message}")]
    ApiError {
        /// HTTP status code
        status: u16,
        /// Error message from the API
        message: String,
    },

    /// Network or connection error.
    #[error("network error: {0}")]
    NetworkError(String),

    /// The operation is not supported by this forge.
    #[error("not implemented: {0}")]
    NotImplemented(String),
}

/// Request to create a pull request.
#[derive(Debug, Clone)]
pub struct CreatePrRequest {
    /// Head branch name (the branch with changes)
    pub head: String,
    /// Base branch name (the branch to merge into)
    pub base: String,
    /// PR title
    pub title: String,
    /// PR body/description
    pub body: Option<String>,
    /// Create as draft
    pub draft: bool,
}

/// Request to update a pull request.
#[derive(Debug, Clone, Default)]
pub struct UpdatePrRequest {
    /// PR number
    pub number: u64,
    /// New title (if changing)
    pub title: Option<String>,
    /// New body (if changing)
    pub body: Option<String>,
    /// New base branch (if changing)
    pub base: Option<String>,
}

/// Pull request information returned from the forge.
#[derive(Debug, Clone)]
pub struct PullRequest {
    /// PR number
    pub number: u64,
    /// PR URL (web URL for viewing)
    pub url: String,
    /// PR state (open, closed, merged)
    pub state: PrState,
    /// Whether the PR is a draft
    pub is_draft: bool,
    /// Head branch name
    pub head: String,
    /// Base branch name
    pub base: String,
    /// PR title
    pub title: String,
    /// PR body/description
    pub body: Option<String>,
    /// GraphQL node ID (for draft toggle mutations)
    pub node_id: Option<String>,
}

/// PR state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    /// PR is open and awaiting review/merge
    Open,
    /// PR is closed without being merged
    Closed,
    /// PR has been merged
    Merged,
}

impl std::fmt::Display for PrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrState::Open => write!(f, "open"),
            PrState::Closed => write!(f, "closed"),
            PrState::Merged => write!(f, "merged"),
        }
    }
}

/// Reviewers to request on a PR.
#[derive(Debug, Clone, Default)]
pub struct Reviewers {
    /// Individual reviewers (usernames)
    pub users: Vec<String>,
    /// Team reviewers (team slugs)
    pub teams: Vec<String>,
}

impl Reviewers {
    /// Check if there are any reviewers to request.
    pub fn is_empty(&self) -> bool {
        self.users.is_empty() && self.teams.is_empty()
    }
}

/// Merge method for merging a PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeMethod {
    /// Create a merge commit
    Merge,
    /// Squash all commits and merge
    #[default]
    Squash,
    /// Rebase commits onto base branch
    Rebase,
}

impl std::fmt::Display for MergeMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeMethod::Merge => write!(f, "merge"),
            MergeMethod::Squash => write!(f, "squash"),
            MergeMethod::Rebase => write!(f, "rebase"),
        }
    }
}

/// Options for listing pull requests.
///
/// Controls pagination and filtering for bulk PR queries.
#[derive(Debug, Clone, Default)]
pub struct ListPullsOpts {
    /// Maximum number of PRs to return.
    ///
    /// Default: 200. GitHub returns max 100 per page, so this may require
    /// multiple paginated requests internally.
    pub max_results: Option<usize>,
}

impl ListPullsOpts {
    /// Create options with a specific limit.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::ListPullsOpts;
    ///
    /// let opts = ListPullsOpts::with_limit(50);
    /// assert_eq!(opts.effective_limit(), 50);
    /// ```
    pub fn with_limit(limit: usize) -> Self {
        Self {
            max_results: Some(limit),
        }
    }

    /// Get the effective limit (default 200).
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::ListPullsOpts;
    ///
    /// let opts = ListPullsOpts::default();
    /// assert_eq!(opts.effective_limit(), 200);
    /// ```
    pub fn effective_limit(&self) -> usize {
        self.max_results.unwrap_or(200)
    }
}

/// Options for listing closed PRs targeting a specific base branch.
///
/// Used for Tier 2 synthetic stack detection (Milestone 5.8) to query
/// closed/merged PRs that targeted a potential synthetic stack head branch.
///
/// # Example
///
/// ```
/// use latticework::forge::ListClosedPrsOpts;
///
/// let opts = ListClosedPrsOpts::for_base("feature-branch").with_limit(20);
/// assert_eq!(opts.base, "feature-branch");
/// assert_eq!(opts.effective_limit(), 20);
/// ```
#[derive(Debug, Clone)]
pub struct ListClosedPrsOpts {
    /// Base branch to filter by (PRs that targeted this branch).
    pub base: String,
    /// Maximum number of PRs to return.
    pub max_results: Option<usize>,
}

impl ListClosedPrsOpts {
    /// Create options for a specific base branch.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::ListClosedPrsOpts;
    ///
    /// let opts = ListClosedPrsOpts::for_base("main");
    /// assert_eq!(opts.base, "main");
    /// assert_eq!(opts.effective_limit(), 100); // default
    /// ```
    pub fn for_base(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            max_results: None,
        }
    }

    /// Set the maximum results.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::ListClosedPrsOpts;
    ///
    /// let opts = ListClosedPrsOpts::for_base("main").with_limit(50);
    /// assert_eq!(opts.effective_limit(), 50);
    /// ```
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.max_results = Some(limit);
        self
    }

    /// Get the effective limit (default 100).
    pub fn effective_limit(&self) -> usize {
        self.max_results.unwrap_or(100)
    }
}

/// Lightweight PR summary for bulk listing.
///
/// Contains only the fields needed for bootstrap matching and display.
/// Use [`Forge::get_pr`] for full PR details when needed.
///
/// # Example
///
/// ```
/// use latticework::forge::PullRequestSummary;
///
/// let summary = PullRequestSummary {
///     number: 42,
///     head_ref: "feature-branch".to_string(),
///     head_repo_owner: None,
///     base_ref: "main".to_string(),
///     is_draft: false,
///     url: "https://github.com/owner/repo/pull/42".to_string(),
///     updated_at: "2024-01-15T10:30:00Z".to_string(),
/// };
///
/// assert!(!summary.is_fork());
/// ```
#[derive(Debug, Clone)]
pub struct PullRequestSummary {
    /// PR number
    pub number: u64,
    /// Head branch name (the branch with changes)
    pub head_ref: String,
    /// Head repository owner (for fork PRs, None if same repo)
    pub head_repo_owner: Option<String>,
    /// Base branch name (the branch to merge into)
    pub base_ref: String,
    /// Whether the PR is a draft
    pub is_draft: bool,
    /// Web URL for the PR
    pub url: String,
    /// Last updated timestamp (ISO 8601)
    pub updated_at: String,
}

impl PullRequestSummary {
    /// Check if this PR is from a fork.
    ///
    /// Fork PRs have a `head_repo_owner` that differs from the base repo owner.
    pub fn is_fork(&self) -> bool {
        self.head_repo_owner.is_some()
    }
}

/// Result from listing pull requests.
///
/// Contains the PRs up to the requested limit, and indicates whether
/// more PRs exist beyond the limit.
#[derive(Debug, Clone)]
pub struct ListPullsResult {
    /// The pull requests (up to the requested limit).
    pub pulls: Vec<PullRequestSummary>,
    /// Whether there were more PRs than the limit allowed.
    ///
    /// When true, callers may want to inform users that the view is incomplete.
    pub truncated: bool,
}

/// The Forge trait for interacting with remote hosting services.
///
/// This trait provides the abstraction layer for PR operations.
/// v1 implements GitHub only; other forges (GitLab, etc.) are stubs
/// behind feature flags per ROADMAP.md Milestone 10.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow use across async tasks.
///
/// # Error Handling
///
/// All methods return `Result<T, ForgeError>`. Callers should handle:
/// - `AuthRequired` / `AuthFailed`: Prompt user to authenticate
/// - `NotFound`: Resource doesn't exist
/// - `RateLimited`: Back off and retry
/// - `ApiError`: Display error message to user
/// - `NetworkError`: Check connectivity
///
/// # Example
///
/// ```ignore
/// use latticework::forge::{Forge, CreatePrRequest, MergeMethod};
///
/// async fn workflow(forge: &dyn Forge) -> Result<(), ForgeError> {
///     // Create a PR
///     let pr = forge.create_pr(CreatePrRequest {
///         head: "feature".to_string(),
///         base: "main".to_string(),
///         title: "Add feature".to_string(),
///         body: None,
///         draft: true,
///     }).await?;
///
///     // Later, publish the draft
///     forge.set_draft(pr.number, false).await?;
///
///     // Request reviewers
///     forge.request_reviewers(pr.number, Reviewers {
///         users: vec!["reviewer1".to_string()],
///         teams: vec![],
///     }).await?;
///
///     // Finally, merge
///     forge.merge_pr(pr.number, MergeMethod::Squash).await?;
///
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait Forge: Send + Sync {
    /// Get the forge name (e.g., "github", "gitlab").
    fn name(&self) -> &'static str;

    /// Create a new pull request.
    ///
    /// # Arguments
    ///
    /// * `request` - The PR creation request with head, base, title, etc.
    ///
    /// # Returns
    ///
    /// The created `PullRequest` with its number and URL.
    ///
    /// # Errors
    ///
    /// - `AuthRequired` if no authentication is configured
    /// - `AuthFailed` if the token is invalid or lacks permissions
    /// - `ApiError` with status 422 if validation fails (e.g., head doesn't exist)
    async fn create_pr(&self, request: CreatePrRequest) -> Result<PullRequest, ForgeError>;

    /// Update an existing pull request.
    ///
    /// # Arguments
    ///
    /// * `request` - The update request with PR number and fields to change
    ///
    /// # Returns
    ///
    /// The updated `PullRequest`.
    ///
    /// # Errors
    ///
    /// - `NotFound` if the PR doesn't exist
    /// - `AuthFailed` if lacking permissions to update
    async fn update_pr(&self, request: UpdatePrRequest) -> Result<PullRequest, ForgeError>;

    /// Get a pull request by number.
    ///
    /// # Arguments
    ///
    /// * `number` - The PR number
    ///
    /// # Returns
    ///
    /// The `PullRequest` details.
    ///
    /// # Errors
    ///
    /// - `NotFound` if the PR doesn't exist
    async fn get_pr(&self, number: u64) -> Result<PullRequest, ForgeError>;

    /// Find a pull request by head branch.
    ///
    /// This searches for an open PR with the given head branch.
    /// Used for idempotent submit (link existing PR instead of creating duplicate).
    ///
    /// # Arguments
    ///
    /// * `head` - The head branch name to search for
    ///
    /// # Returns
    ///
    /// `Some(PullRequest)` if found, `None` if no matching PR exists.
    async fn find_pr_by_head(&self, head: &str) -> Result<Option<PullRequest>, ForgeError>;

    /// Set the draft status of a pull request.
    ///
    /// On GitHub, this requires GraphQL API for toggling draft status.
    ///
    /// # Arguments
    ///
    /// * `number` - The PR number
    /// * `draft` - `true` to convert to draft, `false` to mark ready for review
    ///
    /// # Errors
    ///
    /// - `NotFound` if the PR doesn't exist
    /// - `ApiError` if the operation fails (e.g., PR already merged)
    async fn set_draft(&self, number: u64, draft: bool) -> Result<(), ForgeError>;

    /// Request reviewers for a pull request.
    ///
    /// # Arguments
    ///
    /// * `number` - The PR number
    /// * `reviewers` - Users and/or teams to request reviews from
    ///
    /// # Errors
    ///
    /// - `NotFound` if the PR or reviewers don't exist
    /// - `ApiError` if the request fails
    async fn request_reviewers(&self, number: u64, reviewers: Reviewers) -> Result<(), ForgeError>;

    /// Merge a pull request.
    ///
    /// # Arguments
    ///
    /// * `number` - The PR number
    /// * `method` - The merge method (merge, squash, rebase)
    ///
    /// # Errors
    ///
    /// - `NotFound` if the PR doesn't exist
    /// - `ApiError` if merge fails (e.g., conflicts, required checks failing)
    async fn merge_pr(&self, number: u64, method: MergeMethod) -> Result<(), ForgeError>;

    /// List open pull requests.
    ///
    /// Returns open PRs up to the configured limit, ordered by most recently
    /// updated first. Pagination is handled internally by the implementation.
    ///
    /// This method is designed for bulk queries during bootstrap, where we need
    /// to discover existing PRs to match against local branches. Using this
    /// instead of repeated [`find_pr_by_head`](Self::find_pr_by_head) calls
    /// avoids N+1 query patterns and rate limit issues.
    ///
    /// # Arguments
    ///
    /// * `opts` - Options controlling the query (limit, etc.)
    ///
    /// # Returns
    ///
    /// A [`ListPullsResult`] containing the PRs and truncation status.
    ///
    /// # Errors
    ///
    /// - `AuthRequired` if no authentication is configured
    /// - `AuthFailed` if the token is invalid or lacks permissions
    /// - `RateLimited` if API rate limit is exceeded
    /// - `NetworkError` if the request fails
    async fn list_open_prs(&self, opts: ListPullsOpts) -> Result<ListPullsResult, ForgeError>;

    /// List closed PRs that targeted a specific base branch.
    ///
    /// Returns closed PRs (both merged and unmerged) that had the specified
    /// branch as their base. This is used for Tier 2 synthetic stack detection
    /// to find PRs that were merged into a potential synthetic stack head.
    ///
    /// # Arguments
    ///
    /// * `opts` - Options including the base branch to filter by and limit
    ///
    /// # Returns
    ///
    /// A [`ListPullsResult`] containing closed PRs (merged or unmerged).
    /// The `PullRequestSummary` entries will have PRs that targeted `opts.base`.
    ///
    /// # Errors
    ///
    /// - `AuthRequired` if no authentication is configured
    /// - `AuthFailed` if the token is invalid or lacks permissions
    /// - `RateLimited` if API rate limit is exceeded
    /// - `NetworkError` if the request fails
    async fn list_closed_prs_targeting(
        &self,
        opts: ListClosedPrsOpts,
    ) -> Result<ListPullsResult, ForgeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_state_display() {
        assert_eq!(format!("{}", PrState::Open), "open");
        assert_eq!(format!("{}", PrState::Closed), "closed");
        assert_eq!(format!("{}", PrState::Merged), "merged");
    }

    #[test]
    fn merge_method_display() {
        assert_eq!(format!("{}", MergeMethod::Merge), "merge");
        assert_eq!(format!("{}", MergeMethod::Squash), "squash");
        assert_eq!(format!("{}", MergeMethod::Rebase), "rebase");
    }

    #[test]
    fn merge_method_default_is_squash() {
        assert_eq!(MergeMethod::default(), MergeMethod::Squash);
    }

    #[test]
    fn reviewers_is_empty() {
        let empty = Reviewers::default();
        assert!(empty.is_empty());

        let with_users = Reviewers {
            users: vec!["alice".to_string()],
            teams: vec![],
        };
        assert!(!with_users.is_empty());

        let with_teams = Reviewers {
            users: vec![],
            teams: vec!["core-team".to_string()],
        };
        assert!(!with_teams.is_empty());
    }

    #[test]
    fn update_pr_request_default() {
        let req = UpdatePrRequest::default();
        assert_eq!(req.number, 0);
        assert!(req.title.is_none());
        assert!(req.body.is_none());
        assert!(req.base.is_none());
    }

    #[test]
    fn forge_error_display() {
        assert_eq!(
            format!("{}", ForgeError::AuthRequired),
            "authentication required"
        );
        assert_eq!(
            format!("{}", ForgeError::AuthFailed("expired token".into())),
            "authentication failed: expired token"
        );
        assert_eq!(
            format!("{}", ForgeError::NotFound("PR #123".into())),
            "not found: PR #123"
        );
        assert_eq!(format!("{}", ForgeError::RateLimited), "rate limited");
        assert_eq!(
            format!(
                "{}",
                ForgeError::ApiError {
                    status: 422,
                    message: "Validation failed".into()
                }
            ),
            "API error: 422 - Validation failed"
        );
        assert_eq!(
            format!("{}", ForgeError::NetworkError("connection refused".into())),
            "network error: connection refused"
        );
        assert_eq!(
            format!("{}", ForgeError::NotImplemented("GitLab".into())),
            "not implemented: GitLab"
        );
    }

    mod list_pulls_opts {
        use super::*;

        #[test]
        fn default_limit_is_200() {
            let opts = ListPullsOpts::default();
            assert_eq!(opts.effective_limit(), 200);
            assert!(opts.max_results.is_none());
        }

        #[test]
        fn with_limit_sets_max_results() {
            let opts = ListPullsOpts::with_limit(50);
            assert_eq!(opts.max_results, Some(50));
            assert_eq!(opts.effective_limit(), 50);
        }

        #[test]
        fn zero_limit_is_respected() {
            let opts = ListPullsOpts::with_limit(0);
            assert_eq!(opts.effective_limit(), 0);
        }
    }

    mod list_closed_prs_opts {
        use super::*;

        #[test]
        fn for_base_creates_opts() {
            let opts = ListClosedPrsOpts::for_base("feature-branch");
            assert_eq!(opts.base, "feature-branch");
            assert!(opts.max_results.is_none());
        }

        #[test]
        fn default_limit_is_100() {
            let opts = ListClosedPrsOpts::for_base("main");
            assert_eq!(opts.effective_limit(), 100);
        }

        #[test]
        fn with_limit_sets_max_results() {
            let opts = ListClosedPrsOpts::for_base("main").with_limit(20);
            assert_eq!(opts.max_results, Some(20));
            assert_eq!(opts.effective_limit(), 20);
        }

        #[test]
        fn chaining_works() {
            let opts = ListClosedPrsOpts::for_base("feature").with_limit(50);
            assert_eq!(opts.base, "feature");
            assert_eq!(opts.effective_limit(), 50);
        }
    }

    mod pull_request_summary {
        use super::*;

        #[test]
        fn is_fork_with_owner() {
            let summary = PullRequestSummary {
                number: 1,
                head_ref: "feature".into(),
                head_repo_owner: Some("forker".into()),
                base_ref: "main".into(),
                is_draft: false,
                url: "https://example.com".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
            };
            assert!(summary.is_fork());
        }

        #[test]
        fn is_fork_without_owner() {
            let summary = PullRequestSummary {
                number: 1,
                head_ref: "feature".into(),
                head_repo_owner: None,
                base_ref: "main".into(),
                is_draft: false,
                url: "https://example.com".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
            };
            assert!(!summary.is_fork());
        }
    }

    mod list_pulls_result {
        use super::*;

        #[test]
        fn empty_result() {
            let result = ListPullsResult {
                pulls: vec![],
                truncated: false,
            };
            assert!(result.pulls.is_empty());
            assert!(!result.truncated);
        }

        #[test]
        fn truncated_result() {
            let result = ListPullsResult {
                pulls: vec![PullRequestSummary {
                    number: 1,
                    head_ref: "feature".into(),
                    head_repo_owner: None,
                    base_ref: "main".into(),
                    is_draft: false,
                    url: "https://example.com".into(),
                    updated_at: "2024-01-01T00:00:00Z".into(),
                }],
                truncated: true,
            };
            assert_eq!(result.pulls.len(), 1);
            assert!(result.truncated);
        }
    }
}
