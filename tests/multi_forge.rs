//! Integration tests for multi-forge support.
//!
//! These tests verify:
//! - Forge provider detection from URLs
//! - Forge factory error handling
//! - GitLab stub behavior (when feature enabled)
//! - Configuration-driven forge selection

use lattice::forge::{create_forge, detect_provider, valid_forge_names, ForgeError, ForgeProvider};

mod provider_detection {
    use super::*;

    #[test]
    fn detects_github_ssh_url() {
        let result = detect_provider("git@github.com:owner/repo.git");
        assert_eq!(result, Some(ForgeProvider::GitHub));
    }

    #[test]
    fn detects_github_https_url() {
        let result = detect_provider("https://github.com/owner/repo.git");
        assert_eq!(result, Some(ForgeProvider::GitHub));
    }

    #[test]
    fn detects_github_https_without_suffix() {
        let result = detect_provider("https://github.com/owner/repo");
        assert_eq!(result, Some(ForgeProvider::GitHub));
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn detects_gitlab_ssh_url() {
        let result = detect_provider("git@gitlab.com:owner/project.git");
        assert_eq!(result, Some(ForgeProvider::GitLab));
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn detects_gitlab_https_url() {
        let result = detect_provider("https://gitlab.com/owner/project.git");
        assert_eq!(result, Some(ForgeProvider::GitLab));
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn detects_gitlab_nested_groups() {
        let result = detect_provider("git@gitlab.com:group/subgroup/project.git");
        assert_eq!(result, Some(ForgeProvider::GitLab));
    }

    #[test]
    fn unknown_host_returns_none() {
        assert_eq!(detect_provider("git@bitbucket.org:owner/repo.git"), None);
        assert_eq!(detect_provider("https://bitbucket.org/owner/repo"), None);
        assert_eq!(
            detect_provider("git@unknown.example.com:owner/repo.git"),
            None
        );
    }

    #[test]
    fn invalid_url_returns_none() {
        assert_eq!(detect_provider("not-a-url"), None);
        assert_eq!(detect_provider(""), None);
        assert_eq!(detect_provider("github.com/owner/repo"), None);
    }
}

mod forge_factory {
    use super::*;

    #[test]
    fn creates_github_forge_from_url() {
        let result = create_forge("git@github.com:owner/repo.git", "token", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "github");
    }

    #[test]
    fn creates_github_forge_with_explicit_override() {
        let result = create_forge("git@github.com:owner/repo.git", "token", Some("github"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "github");
    }

    #[test]
    fn unknown_url_without_override_returns_error() {
        let result = create_forge("git@unknown.example.com:owner/repo.git", "token", None);
        assert!(matches!(result, Err(ForgeError::NotFound(_))));

        // Error message should list available providers
        if let Err(ForgeError::NotFound(msg)) = result {
            assert!(
                msg.contains("github"),
                "Error should mention github: {}",
                msg
            );
        }
    }

    #[test]
    fn unknown_provider_override_returns_error() {
        let result = create_forge("git@github.com:owner/repo.git", "token", Some("unknown"));
        assert!(matches!(result, Err(ForgeError::NotFound(_))));

        // Error message should list available providers
        if let Err(ForgeError::NotFound(msg)) = result {
            assert!(
                msg.contains("Available providers"),
                "Error should list providers: {}",
                msg
            );
        }
    }

    #[cfg(not(feature = "gitlab"))]
    #[test]
    fn gitlab_override_without_feature_returns_not_implemented() {
        let result = create_forge("git@github.com:owner/repo.git", "token", Some("gitlab"));
        assert!(matches!(result, Err(ForgeError::NotImplemented(_))));

        // Error message should mention the feature flag
        if let Err(ForgeError::NotImplemented(msg)) = result {
            assert!(
                msg.contains("--features gitlab"),
                "Error should mention feature flag: {}",
                msg
            );
        }
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn creates_gitlab_forge_from_url() {
        let result = create_forge("git@gitlab.com:owner/project.git", "token", None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "gitlab");
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn creates_gitlab_forge_with_explicit_override() {
        let result = create_forge("git@gitlab.com:owner/project.git", "token", Some("gitlab"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().name(), "gitlab");
    }
}

mod valid_forges {
    use super::*;

    #[test]
    fn includes_github() {
        assert!(valid_forge_names().contains(&"github"));
    }

    #[test]
    fn includes_gitlab_for_config_validation() {
        // GitLab should always be in valid names for config validation,
        // even without the feature enabled, so users can pre-configure it
        assert!(valid_forge_names().contains(&"gitlab"));
    }

    #[test]
    fn does_not_include_unknown_forges() {
        assert!(!valid_forge_names().contains(&"bitbucket"));
        assert!(!valid_forge_names().contains(&"gitea"));
    }
}

#[cfg(feature = "gitlab")]
mod gitlab_stub {
    use lattice::forge::gitlab::GitLabForge;
    use lattice::forge::{
        CreatePrRequest, Forge, ForgeError, MergeMethod, Reviewers, UpdatePrRequest,
    };

    #[tokio::test]
    async fn create_pr_returns_not_implemented() {
        let forge = GitLabForge::new("token", "owner", "project");
        let result = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test MR".into(),
                body: None,
                draft: false,
            })
            .await;

        assert!(matches!(result, Err(ForgeError::NotImplemented(_))));

        // Verify error message is actionable
        if let Err(ForgeError::NotImplemented(msg)) = result {
            assert!(
                msg.contains("not yet implemented"),
                "Should mention not implemented: {}",
                msg
            );
            assert!(
                msg.contains("http"),
                "Should include link for updates: {}",
                msg
            );
        }
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
    fn forge_name_is_gitlab() {
        let forge = GitLabForge::new("token", "owner", "project");
        assert_eq!(forge.name(), "gitlab");
    }

    #[test]
    fn from_remote_url_parses_correctly() {
        let forge = GitLabForge::from_remote_url("git@gitlab.com:mygroup/myproject.git", "token");
        assert!(forge.is_some());
        let forge = forge.unwrap();
        assert_eq!(forge.owner(), "mygroup");
        assert_eq!(forge.project(), "myproject");
    }

    #[test]
    fn from_remote_url_handles_nested_groups() {
        let forge = GitLabForge::from_remote_url("git@gitlab.com:a/b/c/project.git", "token");
        assert!(forge.is_some());
        let forge = forge.unwrap();
        assert_eq!(forge.owner(), "a/b/c");
        assert_eq!(forge.project(), "project");
    }

    #[test]
    fn rejects_non_gitlab_url() {
        let forge = GitLabForge::from_remote_url("git@github.com:owner/repo.git", "token");
        assert!(forge.is_none());
    }
}

mod forge_provider_enum {
    use super::*;

    #[test]
    fn all_includes_at_least_github() {
        let all = ForgeProvider::all();
        assert!(!all.is_empty());
        assert!(all.contains(&ForgeProvider::GitHub));
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(ForgeProvider::parse("github"), Some(ForgeProvider::GitHub));
        assert_eq!(ForgeProvider::parse("GitHub"), Some(ForgeProvider::GitHub));
        assert_eq!(ForgeProvider::parse("GITHUB"), Some(ForgeProvider::GitHub));
    }

    #[test]
    fn parse_returns_none_for_unknown() {
        assert_eq!(ForgeProvider::parse("unknown"), None);
        assert_eq!(ForgeProvider::parse(""), None);
        assert_eq!(ForgeProvider::parse("bitbucket"), None);
    }

    #[test]
    fn name_returns_lowercase() {
        assert_eq!(ForgeProvider::GitHub.name(), "github");
    }

    #[test]
    fn display_matches_name() {
        assert_eq!(format!("{}", ForgeProvider::GitHub), "github");
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn all_includes_gitlab_when_feature_enabled() {
        let all = ForgeProvider::all();
        assert!(all.contains(&ForgeProvider::GitLab));
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn parse_parses_gitlab() {
        assert_eq!(ForgeProvider::parse("gitlab"), Some(ForgeProvider::GitLab));
        assert_eq!(ForgeProvider::parse("GitLab"), Some(ForgeProvider::GitLab));
    }

    #[cfg(feature = "gitlab")]
    #[test]
    fn gitlab_name_is_lowercase() {
        assert_eq!(ForgeProvider::GitLab.name(), "gitlab");
    }
}

/// Tests that verify the architecture boundary:
/// Commands should work with any forge, not just GitHub
mod architecture_boundary {
    use lattice::forge::mock::MockForge;
    use lattice::forge::{CreatePrRequest, Forge, PrState};

    #[tokio::test]
    async fn mock_forge_works_as_trait_object() {
        // This test verifies that forges can be used as trait objects,
        // which is required for the factory pattern
        let forge: Box<dyn Forge> = Box::new(MockForge::new());

        assert_eq!(forge.name(), "mock");

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test PR".into(),
                body: None,
                draft: false,
            })
            .await
            .expect("Should create PR");

        assert_eq!(pr.number, 1);
        assert_eq!(pr.state, PrState::Open);
    }

    #[test]
    fn forge_provider_covers_all_supported() {
        use super::*;

        // Verify that ForgeProvider::all() matches valid_forge_names()
        // minus any that are config-only (like gitlab without feature)
        let all_providers: Vec<_> = ForgeProvider::all().iter().map(|p| p.name()).collect();

        // GitHub must always be available
        assert!(all_providers.contains(&"github"));

        // When gitlab feature is enabled, it must be in all()
        #[cfg(feature = "gitlab")]
        assert!(all_providers.contains(&"gitlab"));
    }
}
