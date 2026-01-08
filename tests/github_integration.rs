//! Integration tests for GitHub/Forge functionality.
//!
//! These tests verify the GitHub integration works correctly using MockForge.
//! Live GitHub API tests are behind the `live_github_tests` feature flag.

use latticework::forge::mock::{FailOn, MockForge, MockOperation};
use latticework::forge::{
    CreatePrRequest, Forge, ForgeError, MergeMethod, PrState as ForgePrState, PullRequest,
    Reviewers, UpdatePrRequest,
};

// =============================================================================
// MockForge Unit Tests
// =============================================================================

mod mock_forge_tests {
    use super::*;

    #[tokio::test]
    async fn create_pr_returns_pr() {
        let forge = MockForge::new();

        let req = CreatePrRequest {
            head: "feature".to_string(),
            base: "main".to_string(),
            title: "Add feature".to_string(),
            body: Some("Feature description".to_string()),
            draft: false,
        };

        let pr = forge.create_pr(req).await.unwrap();

        assert_eq!(pr.number, 1);
        assert_eq!(pr.head, "feature");
        assert_eq!(pr.base, "main");
        assert_eq!(pr.title, "Add feature");
        assert_eq!(pr.state, ForgePrState::Open);
        assert!(!pr.is_draft);
    }

    #[tokio::test]
    async fn create_pr_increments_number() {
        let forge = MockForge::new();

        let pr1 = forge
            .create_pr(CreatePrRequest {
                head: "feature1".to_string(),
                base: "main".to_string(),
                title: "PR 1".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let pr2 = forge
            .create_pr(CreatePrRequest {
                head: "feature2".to_string(),
                base: "main".to_string(),
                title: "PR 2".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        assert_eq!(pr1.number, 1);
        assert_eq!(pr2.number, 2);
    }

    #[tokio::test]
    async fn create_draft_pr() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Draft PR".to_string(),
                body: None,
                draft: true,
            })
            .await
            .unwrap();

        assert!(pr.is_draft);
    }

    #[tokio::test]
    async fn update_pr_changes_fields() {
        let forge = MockForge::new();

        // Create a PR first
        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Original title".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Update it
        let updated = forge
            .update_pr(UpdatePrRequest {
                number: pr.number,
                base: Some("develop".to_string()),
                title: Some("Updated title".to_string()),
                body: Some("Updated body".to_string()),
            })
            .await
            .unwrap();

        assert_eq!(updated.number, pr.number);
        assert_eq!(updated.base, "develop");
        assert_eq!(updated.title, "Updated title");
    }

    #[tokio::test]
    async fn update_nonexistent_pr_fails() {
        let forge = MockForge::new();

        let result = forge
            .update_pr(UpdatePrRequest {
                number: 999,
                base: None,
                title: Some("New title".to_string()),
                body: None,
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_pr_returns_pr() {
        let forge = MockForge::new();

        let created = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Test PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let fetched = forge.get_pr(created.number).await.unwrap();

        assert_eq!(fetched.number, created.number);
        assert_eq!(fetched.title, "Test PR");
    }

    #[tokio::test]
    async fn get_nonexistent_pr_fails() {
        let forge = MockForge::new();

        let result = forge.get_pr(999).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn find_pr_by_head() {
        let forge = MockForge::new();

        // Create PR
        forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Feature PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Find by head
        let found = forge.find_pr_by_head("feature").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().head, "feature");

        // Non-existent head
        let not_found = forge.find_pr_by_head("other").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn find_pr_by_head_excludes_closed() {
        let forge = MockForge::new();

        // Create and merge PR
        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Feature PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        forge.merge_pr(pr.number, MergeMethod::Merge).await.unwrap();

        // Should not find merged PR
        let not_found = forge.find_pr_by_head("feature").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn set_draft_toggles_state() {
        let forge = MockForge::new();

        // Create non-draft PR
        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        assert!(!pr.is_draft);

        // Convert to draft
        forge.set_draft(pr.number, true).await.unwrap();
        let pr = forge.get_pr(pr.number).await.unwrap();
        assert!(pr.is_draft);

        // Publish
        forge.set_draft(pr.number, false).await.unwrap();
        let pr = forge.get_pr(pr.number).await.unwrap();
        assert!(!pr.is_draft);
    }

    #[tokio::test]
    async fn merge_pr_closes_pr() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        forge
            .merge_pr(pr.number, MergeMethod::Squash)
            .await
            .unwrap();

        let merged = forge.get_pr(pr.number).await.unwrap();
        assert_eq!(merged.state, ForgePrState::Merged);
    }

    #[tokio::test]
    async fn merge_already_merged_pr_fails() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Merge once - should succeed
        forge.merge_pr(pr.number, MergeMethod::Merge).await.unwrap();

        // Merge again - should fail
        let result = forge.merge_pr(pr.number, MergeMethod::Merge).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn request_reviewers() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let reviewers = Reviewers {
            users: vec!["alice".to_string(), "bob".to_string()],
            teams: vec!["core-team".to_string()],
        };

        forge.request_reviewers(pr.number, reviewers).await.unwrap();

        // Verify operation was recorded
        let ops = forge.operations();
        assert!(ops.iter().any(|op| matches!(
            op,
            MockOperation::RequestReviewers { number, .. } if *number == pr.number
        )));
    }
}

// =============================================================================
// MockForge Failure Injection Tests
// =============================================================================

mod failure_injection_tests {
    use super::*;

    #[tokio::test]
    async fn fail_on_create_pr() {
        let forge = MockForge::new().fail_on(FailOn::CreatePr(ForgeError::RateLimited));

        let result = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fail_on_update_pr() {
        let forge = MockForge::new();

        // Create PR first (should succeed)
        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Set up failure - need a new forge since fail_on consumes self
        let forge_with_prs = MockForge::with_prs(vec![PullRequest {
            number: pr.number,
            url: "https://github.com/test/test/pull/1".to_string(),
            state: ForgePrState::Open,
            is_draft: false,
            head: "feature".to_string(),
            base: "main".to_string(),
            title: "PR".to_string(),
            node_id: None,
            body: None,
        }])
        .fail_on(FailOn::UpdatePr(ForgeError::RateLimited));

        let result = forge_with_prs
            .update_pr(UpdatePrRequest {
                number: pr.number,
                base: None,
                title: Some("New title".to_string()),
                body: None,
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fail_on_merge() {
        let forge = MockForge::new();

        // Create PR first
        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Set up failure with a new forge containing the PR
        let forge_with_prs = MockForge::with_prs(vec![PullRequest {
            number: pr.number,
            url: "https://github.com/test/test/pull/1".to_string(),
            state: ForgePrState::Open,
            is_draft: false,
            head: "feature".to_string(),
            base: "main".to_string(),
            title: "PR".to_string(),
            node_id: None,
            body: None,
        }])
        .fail_on(FailOn::MergePr(ForgeError::RateLimited));

        let result = forge_with_prs.merge_pr(pr.number, MergeMethod::Merge).await;
        assert!(result.is_err());
    }
}

// =============================================================================
// Operation Recording Tests
// =============================================================================

mod operation_recording_tests {
    use super::*;

    #[tokio::test]
    async fn records_create_operation() {
        let forge = MockForge::new();

        forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let ops = forge.operations();
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            &ops[0],
            MockOperation::CreatePr { head, base, .. } if head == "feature" && base == "main"
        ));
    }

    #[tokio::test]
    async fn records_update_operation() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        forge
            .update_pr(UpdatePrRequest {
                number: pr.number,
                base: Some("develop".to_string()),
                title: None,
                body: None,
            })
            .await
            .unwrap();

        let ops = forge.operations();
        assert_eq!(ops.len(), 2);
        assert!(matches!(
            &ops[1],
            MockOperation::UpdatePr { number, base, .. } if *number == pr.number && base == &Some("develop".to_string())
        ));
    }

    #[tokio::test]
    async fn records_merge_operation() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        forge
            .merge_pr(pr.number, MergeMethod::Squash)
            .await
            .unwrap();

        let ops = forge.operations();
        assert_eq!(ops.len(), 2);
        assert!(matches!(
            &ops[1],
            MockOperation::MergePr { number, method } if *number == pr.number && *method == MergeMethod::Squash
        ));
    }

    #[tokio::test]
    async fn clear_operations() {
        let forge = MockForge::new();

        forge
            .create_pr(CreatePrRequest {
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "PR".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        assert_eq!(forge.operations().len(), 1);

        forge.clear_operations();
        assert_eq!(forge.operations().len(), 0);
    }
}

// =============================================================================
// Submit Flow Tests (using MockForge)
// =============================================================================

mod submit_flow_tests {
    use super::*;

    /// Simulates the submit command's PR creation logic
    async fn simulate_submit(
        forge: &MockForge,
        branches: &[(&str, &str)], // (branch, parent)
        draft: bool,
    ) -> Vec<PullRequest> {
        let mut prs = Vec::new();

        for (branch, parent) in branches {
            // Check if PR exists
            if let Some(existing) = forge.find_pr_by_head(branch).await.unwrap() {
                // Update existing PR
                let updated = forge
                    .update_pr(UpdatePrRequest {
                        number: existing.number,
                        base: Some(parent.to_string()),
                        title: None,
                        body: None,
                    })
                    .await
                    .unwrap();
                prs.push(updated);
            } else {
                // Create new PR
                let pr = forge
                    .create_pr(CreatePrRequest {
                        head: branch.to_string(),
                        base: parent.to_string(),
                        title: branch.to_string(),
                        body: None,
                        draft,
                    })
                    .await
                    .unwrap();
                prs.push(pr);
            }
        }

        prs
    }

    #[tokio::test]
    async fn submit_creates_prs_with_correct_bases() {
        let forge = MockForge::new();

        // Submit a stack: main -> feature1 -> feature2
        let prs = simulate_submit(
            &forge,
            &[("feature1", "main"), ("feature2", "feature1")],
            false,
        )
        .await;

        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].head, "feature1");
        assert_eq!(prs[0].base, "main");
        assert_eq!(prs[1].head, "feature2");
        assert_eq!(prs[1].base, "feature1");
    }

    #[tokio::test]
    async fn submit_updates_existing_prs_no_duplicates() {
        let forge = MockForge::new();

        // First submit
        let prs1 = simulate_submit(&forge, &[("feature1", "main")], false).await;

        // Second submit (should update, not create new)
        let prs2 = simulate_submit(&forge, &[("feature1", "main")], false).await;

        // Should have same PR number
        assert_eq!(prs1[0].number, prs2[0].number);

        // Verify operations: first submit does find + create, second does find + update
        let ops = forge.operations();
        // ops[0] = FindPrByHead (first submit, no result)
        // ops[1] = CreatePr (first submit)
        // ops[2] = FindPrByHead (second submit, finds PR)
        // ops[3] = UpdatePr (second submit)
        assert_eq!(ops.len(), 4);
        assert!(matches!(&ops[0], MockOperation::FindPrByHead { .. }));
        assert!(matches!(&ops[1], MockOperation::CreatePr { .. }));
        assert!(matches!(&ops[2], MockOperation::FindPrByHead { .. }));
        assert!(matches!(&ops[3], MockOperation::UpdatePr { .. }));
    }

    #[tokio::test]
    async fn submit_draft_creates_draft_prs() {
        let forge = MockForge::new();

        let prs = simulate_submit(&forge, &[("feature1", "main")], true).await;

        assert!(prs[0].is_draft);
    }
}

// =============================================================================
// Merge Flow Tests (using MockForge)
// =============================================================================

mod merge_flow_tests {
    use super::*;

    #[tokio::test]
    async fn merge_stack_in_order() {
        let forge = MockForge::new();

        // Create stack PRs
        let pr1 = forge
            .create_pr(CreatePrRequest {
                head: "feature1".to_string(),
                base: "main".to_string(),
                title: "PR 1".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let pr2 = forge
            .create_pr(CreatePrRequest {
                head: "feature2".to_string(),
                base: "feature1".to_string(),
                title: "PR 2".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Merge in order (bottom to top)
        forge
            .merge_pr(pr1.number, MergeMethod::Squash)
            .await
            .unwrap();
        forge
            .merge_pr(pr2.number, MergeMethod::Squash)
            .await
            .unwrap();

        // Verify both are merged
        let merged1 = forge.get_pr(pr1.number).await.unwrap();
        let merged2 = forge.get_pr(pr2.number).await.unwrap();

        assert_eq!(merged1.state, ForgePrState::Merged);
        assert_eq!(merged2.state, ForgePrState::Merged);

        // Verify merge order in operations
        let ops = forge.operations();
        let merge_ops: Vec<_> = ops
            .iter()
            .filter(|op| matches!(op, MockOperation::MergePr { .. }))
            .collect();

        assert_eq!(merge_ops.len(), 2);
        assert!(
            matches!(merge_ops[0], MockOperation::MergePr { number, .. } if *number == pr1.number)
        );
        assert!(
            matches!(merge_ops[1], MockOperation::MergePr { number, .. } if *number == pr2.number)
        );
    }

    #[tokio::test]
    async fn merge_stops_on_failure() {
        // Create PRs with a fresh forge
        let forge = MockForge::new();

        let pr1 = forge
            .create_pr(CreatePrRequest {
                head: "feature1".to_string(),
                base: "main".to_string(),
                title: "PR 1".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let _pr2 = forge
            .create_pr(CreatePrRequest {
                head: "feature2".to_string(),
                base: "feature1".to_string(),
                title: "PR 2".to_string(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Set up failure forge with these PRs
        let forge_fail = MockForge::with_prs(vec![
            PullRequest {
                number: pr1.number,
                url: "https://github.com/test/test/pull/1".to_string(),
                state: ForgePrState::Open,
                is_draft: false,
                head: "feature1".to_string(),
                base: "main".to_string(),
                title: "PR 1".to_string(),
                node_id: None,
                body: None,
            },
            PullRequest {
                number: 2,
                url: "https://github.com/test/test/pull/2".to_string(),
                state: ForgePrState::Open,
                is_draft: false,
                head: "feature2".to_string(),
                base: "feature1".to_string(),
                title: "PR 2".to_string(),
                node_id: None,
                body: None,
            },
        ])
        .fail_on(FailOn::MergePr(ForgeError::RateLimited));

        // First merge fails
        let result = forge_fail.merge_pr(pr1.number, MergeMethod::Merge).await;
        assert!(result.is_err());

        // PR should still be open (in the original forge)
        let pr1_state = forge.get_pr(pr1.number).await.unwrap();
        assert_eq!(pr1_state.state, ForgePrState::Open);
    }
}

// =============================================================================
// GitHub URL Parsing Tests
// =============================================================================

mod github_url_tests {
    use latticework::forge::github::parse_github_url;

    #[test]
    fn parse_https_url() {
        let (owner, repo) = parse_github_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_https_url_without_git_suffix() {
        let (owner, repo) = parse_github_url("https://github.com/owner/repo").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_ssh_url() {
        let (owner, repo) = parse_github_url("git@github.com:owner/repo.git").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_ssh_url_without_git_suffix() {
        let (owner, repo) = parse_github_url("git@github.com:owner/repo").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn parse_non_github_url_returns_none() {
        assert!(parse_github_url("https://gitlab.com/owner/repo.git").is_none());
        assert!(parse_github_url("git@gitlab.com:owner/repo.git").is_none());
    }

    #[test]
    fn parse_invalid_url_returns_none() {
        assert!(parse_github_url("not a url").is_none());
        assert!(parse_github_url("").is_none());
    }
}

// =============================================================================
// Auth Tests
// =============================================================================

mod auth_tests {
    use latticework::cli::commands::{get_github_token, has_github_token};

    #[test]
    fn has_github_token_returns_bool() {
        // In test environment, token is unlikely to be set
        // This is a basic sanity check
        let _ = has_github_token();
    }

    #[test]
    fn get_github_token_returns_result() {
        // Should return error if no token is configured
        // (Unless running in CI with GITHUB_TOKEN set)
        let result = get_github_token();
        // Either succeeds (CI) or fails (local without token)
        let _ = result;
    }
}

// =============================================================================
// Live GitHub API Tests (behind feature flag)
// =============================================================================

#[cfg(feature = "live_github_tests")]
mod live_tests {
    use super::*;
    use latticework::forge::github::GitHubForge;

    fn get_test_token() -> Option<String> {
        std::env::var("GITHUB_TOKEN").ok()
    }

    fn get_test_repo() -> Option<(String, String)> {
        let owner = std::env::var("LATTICE_TEST_OWNER").ok()?;
        let repo = std::env::var("LATTICE_TEST_REPO").ok()?;
        Some((owner, repo))
    }

    #[tokio::test]
    async fn live_get_nonexistent_pr() {
        let Some(token) = get_test_token() else {
            eprintln!("Skipping: GITHUB_TOKEN not set");
            return;
        };

        let Some((owner, repo)) = get_test_repo() else {
            eprintln!("Skipping: LATTICE_TEST_OWNER/LATTICE_TEST_REPO not set");
            return;
        };

        let forge = GitHubForge::new(owner, repo, token);

        // PR 999999999 should not exist
        let result = forge.get_pr(999999999).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn live_find_pr_by_nonexistent_head() {
        let Some(token) = get_test_token() else {
            eprintln!("Skipping: GITHUB_TOKEN not set");
            return;
        };

        let Some((owner, repo)) = get_test_repo() else {
            eprintln!("Skipping: LATTICE_TEST_OWNER/LATTICE_TEST_REPO not set");
            return;
        };

        let forge = GitHubForge::new(owner, repo, token);

        let result = forge
            .find_pr_by_head("definitely-does-not-exist-xyz-123")
            .await
            .unwrap();

        assert!(result.is_none());
    }
}
