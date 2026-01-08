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
//! use lattice::forge::{Forge, CreatePrRequest};
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
/// use lattice::forge::{Forge, CreatePrRequest, MergeMethod};
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
}
