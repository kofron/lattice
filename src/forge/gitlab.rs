//! forge::gitlab
//!
//! GitLab forge stub implementation.
//!
//! # Design
//!
//! This module provides a stub implementation of the `Forge` trait for GitLab.
//! All operations return `ForgeError::NotImplemented` to demonstrate the
//! multi-forge architecture without implementing actual GitLab API calls.
//!
//! Per ROADMAP.md Milestone 10, this stub:
//! - Compiles and is selectable in config
//! - Returns stable, actionable errors
//! - Proves the architecture boundary (core depends on Forge trait, not GitHub)
//!
//! # Feature Flag
//!
//! This module is only available when the `gitlab` feature is enabled:
//!
//! ```toml
//! [dependencies]
//! lattice = { version = "0.1", features = ["gitlab"] }
//! ```
//!
//! # Example
//!
//! ```ignore
//! use latticework::forge::gitlab::GitLabForge;
//! use latticework::forge::{Forge, CreatePrRequest};
//!
//! let forge = GitLabForge::new("token", "owner", "repo");
//!
//! // All operations return NotImplemented
//! let result = forge.create_pr(request).await;
//! assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
//! ```

use async_trait::async_trait;

use super::traits::{
    CreatePrRequest, Forge, ForgeError, MergeMethod, PullRequest, Reviewers, UpdatePrRequest,
};

/// GitLab forge stub implementation.
///
/// This is a placeholder that returns `NotImplemented` for all operations.
/// It exists to demonstrate the multi-forge architecture and validate that
/// commands work correctly with forge selection.
#[derive(Debug, Clone)]
pub struct GitLabForge {
    /// Personal access token for authentication
    token: String,
    /// Project owner (user or group)
    owner: String,
    /// Project name
    project: String,
    /// API base URL (for self-hosted GitLab)
    api_base: String,
}

/// Default GitLab API base URL.
const DEFAULT_API_BASE: &str = "https://gitlab.com/api/v4";

impl GitLabForge {
    /// Create a new GitLab forge.
    ///
    /// # Arguments
    ///
    /// * `token` - Personal access token
    /// * `owner` - Project owner (user or group)
    /// * `project` - Project name
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::gitlab::GitLabForge;
    ///
    /// let forge = GitLabForge::new("glpat-xxx", "mygroup", "myproject");
    /// ```
    pub fn new(
        token: impl Into<String>,
        owner: impl Into<String>,
        project: impl Into<String>,
    ) -> Self {
        Self {
            token: token.into(),
            owner: owner.into(),
            project: project.into(),
            api_base: DEFAULT_API_BASE.to_string(),
        }
    }

    /// Create a GitLab forge with a custom API base URL.
    ///
    /// Use this for self-hosted GitLab installations.
    ///
    /// # Arguments
    ///
    /// * `token` - Personal access token
    /// * `owner` - Project owner
    /// * `project` - Project name
    /// * `api_base` - Custom API base URL (e.g., `https://gitlab.example.com/api/v4`)
    pub fn with_api_base(
        token: impl Into<String>,
        owner: impl Into<String>,
        project: impl Into<String>,
        api_base: impl Into<String>,
    ) -> Self {
        Self {
            token: token.into(),
            owner: owner.into(),
            project: project.into(),
            api_base: api_base.into(),
        }
    }

    /// Create a GitLab forge from a remote URL.
    ///
    /// Parses the remote URL to extract owner and project.
    ///
    /// # Arguments
    ///
    /// * `url` - Git remote URL (SSH or HTTPS format)
    /// * `token` - Personal access token
    ///
    /// # Returns
    ///
    /// `Some(GitLabForge)` if URL is parseable as GitLab, `None` otherwise.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::gitlab::GitLabForge;
    ///
    /// // SSH format
    /// let forge = GitLabForge::from_remote_url("git@gitlab.com:owner/project.git", "token");
    /// assert!(forge.is_some());
    ///
    /// // HTTPS format
    /// let forge = GitLabForge::from_remote_url("https://gitlab.com/owner/project.git", "token");
    /// assert!(forge.is_some());
    ///
    /// // Not a GitLab URL
    /// let forge = GitLabForge::from_remote_url("git@github.com:owner/repo.git", "token");
    /// assert!(forge.is_none());
    /// ```
    pub fn from_remote_url(url: &str, token: impl Into<String>) -> Option<Self> {
        let (owner, project) = parse_gitlab_url(url)?;
        Some(Self::new(token, owner, project))
    }

    /// Get the project owner.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Get the project name.
    pub fn project(&self) -> &str {
        &self.project
    }

    /// Get the API base URL.
    pub fn api_base(&self) -> &str {
        &self.api_base
    }

    /// Check if the forge has a token configured.
    ///
    /// Note: The token is stored but not used since all operations
    /// return NotImplemented.
    pub fn has_token(&self) -> bool {
        !self.token.is_empty()
    }
}

#[async_trait]
impl Forge for GitLabForge {
    fn name(&self) -> &'static str {
        "gitlab"
    }

    async fn create_pr(&self, _request: CreatePrRequest) -> Result<PullRequest, ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab merge request creation is not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn update_pr(&self, _request: UpdatePrRequest) -> Result<PullRequest, ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab merge request updates are not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn get_pr(&self, _number: u64) -> Result<PullRequest, ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab merge request retrieval is not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn find_pr_by_head(&self, _head: &str) -> Result<Option<PullRequest>, ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab merge request search is not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn set_draft(&self, _number: u64, _draft: bool) -> Result<(), ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab draft status toggling is not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn request_reviewers(
        &self,
        _number: u64,
        _reviewers: Reviewers,
    ) -> Result<(), ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab reviewer requests are not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn merge_pr(&self, _number: u64, _method: MergeMethod) -> Result<(), ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab merge request merging is not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn list_open_prs(
        &self,
        _opts: super::traits::ListPullsOpts,
    ) -> Result<super::traits::ListPullsResult, ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab open merge request listing is not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }

    async fn list_closed_prs_targeting(
        &self,
        _opts: super::traits::ListClosedPrsOpts,
    ) -> Result<super::traits::ListPullsResult, ForgeError> {
        Err(ForgeError::NotImplemented(
            "GitLab closed merge request listing is not yet implemented. \
             See https://github.com/lattice-cli/lattice for updates."
                .to_string(),
        ))
    }
}

// --------------------------------------------------------------------------
// URL Parsing
// --------------------------------------------------------------------------

/// Parse a GitLab remote URL to extract owner and project.
///
/// Supports both SSH and HTTPS formats:
/// - `git@gitlab.com:owner/project.git`
/// - `https://gitlab.com/owner/project.git`
/// - `https://gitlab.com/owner/project`
/// - Nested groups: `git@gitlab.com:group/subgroup/project.git`
///
/// # Returns
///
/// `Some((owner, project))` if the URL is a valid GitLab URL, `None` otherwise.
/// For nested groups, owner includes the full path (e.g., "group/subgroup").
///
/// # Example
///
/// ```
/// use latticework::forge::gitlab::parse_gitlab_url;
///
/// let (owner, project) = parse_gitlab_url("git@gitlab.com:mygroup/myproject.git").unwrap();
/// assert_eq!(owner, "mygroup");
/// assert_eq!(project, "myproject");
///
/// // Nested groups
/// let (owner, project) = parse_gitlab_url("git@gitlab.com:group/subgroup/project.git").unwrap();
/// assert_eq!(owner, "group/subgroup");
/// assert_eq!(project, "project");
/// ```
pub fn parse_gitlab_url(url: &str) -> Option<(String, String)> {
    // SSH format: git@gitlab.com:owner/project.git
    if let Some(rest) = url.strip_prefix("git@gitlab.com:") {
        return parse_gitlab_path(rest);
    }

    // HTTPS format: https://gitlab.com/owner/project.git
    if let Some(rest) = url
        .strip_prefix("https://gitlab.com/")
        .or_else(|| url.strip_prefix("http://gitlab.com/"))
    {
        return parse_gitlab_path(rest);
    }

    None
}

/// Parse the path portion of a GitLab URL.
///
/// Handles nested groups by treating all but the last segment as the owner.
fn parse_gitlab_path(path: &str) -> Option<(String, String)> {
    let path = path.strip_suffix(".git").unwrap_or(path);

    // Split by '/' and ensure at least owner/project
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return None;
    }

    // Last part is project, everything before is owner (handles nested groups)
    let project = parts.last()?.to_string();
    if project.is_empty() {
        return None;
    }

    let owner = parts[..parts.len() - 1].join("/");
    if owner.is_empty() {
        return None;
    }

    Some((owner, project))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod parse_gitlab_url {
        use super::*;

        #[test]
        fn ssh_with_git_suffix() {
            let result = parse_gitlab_url("git@gitlab.com:mygroup/myproject.git");
            assert_eq!(
                result,
                Some(("mygroup".to_string(), "myproject".to_string()))
            );
        }

        #[test]
        fn ssh_without_git_suffix() {
            let result = parse_gitlab_url("git@gitlab.com:mygroup/myproject");
            assert_eq!(
                result,
                Some(("mygroup".to_string(), "myproject".to_string()))
            );
        }

        #[test]
        fn https_with_git_suffix() {
            let result = parse_gitlab_url("https://gitlab.com/mygroup/myproject.git");
            assert_eq!(
                result,
                Some(("mygroup".to_string(), "myproject".to_string()))
            );
        }

        #[test]
        fn https_without_git_suffix() {
            let result = parse_gitlab_url("https://gitlab.com/mygroup/myproject");
            assert_eq!(
                result,
                Some(("mygroup".to_string(), "myproject".to_string()))
            );
        }

        #[test]
        fn http_format() {
            let result = parse_gitlab_url("http://gitlab.com/mygroup/myproject.git");
            assert_eq!(
                result,
                Some(("mygroup".to_string(), "myproject".to_string()))
            );
        }

        #[test]
        fn nested_groups() {
            let result = parse_gitlab_url("git@gitlab.com:group/subgroup/project.git");
            assert_eq!(
                result,
                Some(("group/subgroup".to_string(), "project".to_string()))
            );
        }

        #[test]
        fn deeply_nested_groups() {
            let result = parse_gitlab_url("https://gitlab.com/a/b/c/d/project.git");
            assert_eq!(result, Some(("a/b/c/d".to_string(), "project".to_string())));
        }

        #[test]
        fn non_gitlab_url() {
            assert!(parse_gitlab_url("git@github.com:owner/repo.git").is_none());
            assert!(parse_gitlab_url("https://github.com/owner/repo").is_none());
            assert!(parse_gitlab_url("https://bitbucket.org/owner/repo").is_none());
        }

        #[test]
        fn invalid_format() {
            assert!(parse_gitlab_url("not a url").is_none());
            assert!(parse_gitlab_url("gitlab.com/owner/project").is_none());
            assert!(parse_gitlab_url("https://gitlab.com/").is_none());
            assert!(parse_gitlab_url("https://gitlab.com/owner").is_none());
        }

        #[test]
        fn project_with_dots() {
            let result = parse_gitlab_url("git@gitlab.com:owner/project.name.git");
            assert_eq!(
                result,
                Some(("owner".to_string(), "project.name".to_string()))
            );
        }

        #[test]
        fn project_with_hyphens() {
            let result = parse_gitlab_url("git@gitlab.com:my-group/my-project.git");
            assert_eq!(
                result,
                Some(("my-group".to_string(), "my-project".to_string()))
            );
        }
    }

    mod gitlab_forge {
        use super::*;

        #[test]
        fn new_creates_forge() {
            let forge = GitLabForge::new("token", "owner", "project");
            assert_eq!(forge.name(), "gitlab");
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.project(), "project");
            assert!(forge.has_token());
        }

        #[test]
        fn from_remote_url_ssh() {
            let forge = GitLabForge::from_remote_url("git@gitlab.com:owner/project.git", "token");
            assert!(forge.is_some());
            let forge = forge.unwrap();
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.project(), "project");
        }

        #[test]
        fn from_remote_url_https() {
            let forge =
                GitLabForge::from_remote_url("https://gitlab.com/owner/project.git", "token");
            assert!(forge.is_some());
            let forge = forge.unwrap();
            assert_eq!(forge.owner(), "owner");
            assert_eq!(forge.project(), "project");
        }

        #[test]
        fn from_remote_url_invalid() {
            let forge = GitLabForge::from_remote_url("https://github.com/owner/repo", "token");
            assert!(forge.is_none());
        }

        #[test]
        fn with_api_base() {
            let forge = GitLabForge::with_api_base(
                "token",
                "owner",
                "project",
                "https://gitlab.example.com/api/v4",
            );
            assert_eq!(forge.api_base(), "https://gitlab.example.com/api/v4");
        }

        #[test]
        fn empty_token() {
            let forge = GitLabForge::new("", "owner", "project");
            assert!(!forge.has_token());
        }
    }

    mod forge_trait {
        use super::*;

        #[tokio::test]
        async fn create_pr_returns_not_implemented() {
            let forge = GitLabForge::new("token", "owner", "project");
            let result = forge
                .create_pr(CreatePrRequest {
                    head: "feature".into(),
                    base: "main".into(),
                    title: "Test".into(),
                    body: None,
                    draft: false,
                })
                .await;

            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
        }

        #[tokio::test]
        async fn update_pr_returns_not_implemented() {
            let forge = GitLabForge::new("token", "owner", "project");
            let result = forge
                .update_pr(UpdatePrRequest {
                    number: 1,
                    title: Some("New title".into()),
                    ..Default::default()
                })
                .await;

            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
        }

        #[tokio::test]
        async fn get_pr_returns_not_implemented() {
            let forge = GitLabForge::new("token", "owner", "project");
            let result = forge.get_pr(1).await;

            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
        }

        #[tokio::test]
        async fn find_pr_by_head_returns_not_implemented() {
            let forge = GitLabForge::new("token", "owner", "project");
            let result = forge.find_pr_by_head("feature").await;

            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
        }

        #[tokio::test]
        async fn set_draft_returns_not_implemented() {
            let forge = GitLabForge::new("token", "owner", "project");
            let result = forge.set_draft(1, true).await;

            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
        }

        #[tokio::test]
        async fn request_reviewers_returns_not_implemented() {
            let forge = GitLabForge::new("token", "owner", "project");
            let result = forge
                .request_reviewers(
                    1,
                    Reviewers {
                        users: vec!["alice".into()],
                        teams: vec![],
                    },
                )
                .await;

            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
        }

        #[tokio::test]
        async fn merge_pr_returns_not_implemented() {
            let forge = GitLabForge::new("token", "owner", "project");
            let result = forge.merge_pr(1, MergeMethod::Squash).await;

            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));
        }

        #[test]
        fn error_messages_are_actionable() {
            // Verify error messages include guidance
            let err = ForgeError::NotImplemented(
                "GitLab merge request creation is not yet implemented. \
                 See https://github.com/lattice-cli/lattice for updates."
                    .to_string(),
            );

            let msg = format!("{}", err);
            assert!(msg.contains("not yet implemented"));
            assert!(msg.contains("https://"));
        }
    }
}
