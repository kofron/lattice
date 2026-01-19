//! cli::commands::stack_comment_ops
//!
//! Shared helpers for generating and updating stack comments in PR descriptions.
//!
//! # Design
//!
//! This module provides shared functionality used by both `submit` and `sync`
//! commands to keep stack comments in PR descriptions up to date. The stack
//! comment shows the full context of stacked PRs with visual indicators.
//!
//! Per CLAUDE.md principles:
//! - **Purity**: Core generation logic is in `ui::stack_comment` (pure functions)
//! - **Reuse**: This module wraps that logic for use by multiple commands
//!
//! # Example
//!
//! ```ignore
//! use latticework::cli::commands::stack_comment_ops::update_pr_stack_comment;
//!
//! // Update stack comment for a single PR
//! update_pr_stack_comment(&forge, &snapshot, &branch, quiet).await?;
//! ```

use anyhow::Result;

use crate::core::metadata::schema::PrState;
use crate::core::types::BranchName;
use crate::engine::scan::RepoSnapshot;
use crate::forge::{Forge, UpdatePrRequest};
use crate::ui::stack_comment::{
    generate_stack_comment, merge_stack_comment, StackBranchInfo, StackCommentInput, StackPosition,
};

/// Build stack comment input for a branch.
///
/// This collects all branches in the stack (ancestors and descendants)
/// and their PR linkage status.
///
/// # Arguments
///
/// * `snapshot` - Repository snapshot with metadata and graph
/// * `current_branch` - The branch to build the stack for
///
/// # Returns
///
/// A `StackCommentInput` ready for `generate_stack_comment`.
pub fn build_stack_comment_input(
    snapshot: &RepoSnapshot,
    current_branch: &BranchName,
) -> StackCommentInput {
    // Get ancestors (from current toward trunk, then reverse for display order)
    let mut ancestors = snapshot.graph.ancestors(current_branch);
    ancestors.reverse(); // Now ordered from trunk toward current

    // Get descendants
    let descendants = snapshot.graph.descendants(current_branch);

    // Build ordered list: ancestors, current, descendants
    let mut branches = Vec::new();

    // Add ancestors (skip trunk - it's not part of the tracked stack)
    for ancestor in &ancestors {
        // Skip if this is not tracked (likely trunk)
        if !snapshot.is_tracked(ancestor) {
            continue;
        }

        let (pr_number, pr_url) = get_pr_info(snapshot, ancestor);
        branches.push(StackBranchInfo {
            name: ancestor.to_string(),
            pr_number,
            pr_url,
            position: StackPosition::Ancestor,
        });
    }

    // Add current branch
    let (pr_number, pr_url) = get_pr_info(snapshot, current_branch);
    branches.push(StackBranchInfo {
        name: current_branch.to_string(),
        pr_number,
        pr_url,
        position: StackPosition::Current,
    });

    // Add descendants (ordered by depth from current)
    let mut desc_ordered: Vec<_> = descendants.into_iter().collect();
    // Sort by depth (number of ancestors) to get consistent ordering
    desc_ordered.sort_by_key(|b| snapshot.graph.ancestors(b).len());

    for descendant in desc_ordered {
        let (pr_number, pr_url) = get_pr_info(snapshot, &descendant);
        branches.push(StackBranchInfo {
            name: descendant.to_string(),
            pr_number,
            pr_url,
            position: StackPosition::Descendant,
        });
    }

    StackCommentInput { branches }
}

/// Extract PR number and URL from metadata.
fn get_pr_info(snapshot: &RepoSnapshot, branch: &BranchName) -> (Option<u64>, Option<String>) {
    snapshot
        .metadata
        .get(branch)
        .and_then(|m| match &m.metadata.pr {
            PrState::Linked { number, url, .. } => Some((Some(*number), Some(url.clone()))),
            PrState::None => None,
        })
        .unwrap_or((None, None))
}

/// Build stack comment input by fetching PR info from the forge.
///
/// This variant fetches PR info directly from the forge API instead of
/// relying on metadata. Use this after creating PRs when metadata hasn't
/// been persisted yet.
///
/// # Arguments
///
/// * `forge` - The forge to fetch PR info from
/// * `snapshot` - Repository snapshot with graph structure
/// * `current_branch` - The branch to build the stack for
///
/// # Returns
///
/// A `StackCommentInput` with PR info fetched from the forge.
pub async fn build_stack_comment_input_from_forge(
    forge: &dyn Forge,
    snapshot: &RepoSnapshot,
    current_branch: &BranchName,
) -> StackCommentInput {
    // Get ancestors (from current toward trunk, then reverse for display order)
    let mut ancestors = snapshot.graph.ancestors(current_branch);
    ancestors.reverse(); // Now ordered from trunk toward current

    // Get descendants
    let descendants = snapshot.graph.descendants(current_branch);

    // Build ordered list: ancestors, current, descendants
    let mut branches = Vec::new();

    // Add ancestors (skip trunk - it's not part of the tracked stack)
    for ancestor in &ancestors {
        // Skip if this is not tracked (likely trunk)
        if !snapshot.is_tracked(ancestor) {
            continue;
        }

        let (pr_number, pr_url) = get_pr_info_from_forge(forge, ancestor).await;
        branches.push(StackBranchInfo {
            name: ancestor.to_string(),
            pr_number,
            pr_url,
            position: StackPosition::Ancestor,
        });
    }

    // Add current branch
    let (pr_number, pr_url) = get_pr_info_from_forge(forge, current_branch).await;
    branches.push(StackBranchInfo {
        name: current_branch.to_string(),
        pr_number,
        pr_url,
        position: StackPosition::Current,
    });

    // Add descendants (ordered by depth from current)
    let mut desc_ordered: Vec<_> = descendants.into_iter().collect();
    // Sort by depth (number of ancestors) to get consistent ordering
    desc_ordered.sort_by_key(|b| snapshot.graph.ancestors(b).len());

    for descendant in desc_ordered {
        let (pr_number, pr_url) = get_pr_info_from_forge(forge, &descendant).await;
        branches.push(StackBranchInfo {
            name: descendant.to_string(),
            pr_number,
            pr_url,
            position: StackPosition::Descendant,
        });
    }

    StackCommentInput { branches }
}

/// Fetch PR info for a branch from the forge.
async fn get_pr_info_from_forge(
    forge: &dyn Forge,
    branch: &BranchName,
) -> (Option<u64>, Option<String>) {
    match forge.find_pr_by_head(branch.as_str()).await {
        Ok(Some(pr)) => (Some(pr.number), Some(pr.url)),
        _ => (None, None),
    }
}

/// Generate a stack comment body for a branch.
///
/// This is a convenience function that combines building input and generating output.
/// Uses metadata for PR info - prefer `generate_stack_comment_for_branch_from_forge`
/// when metadata may be stale.
///
/// # Arguments
///
/// * `snapshot` - Repository snapshot
/// * `branch` - The branch to generate the stack comment for
///
/// # Returns
///
/// The generated stack comment string (including markers).
pub fn generate_stack_comment_for_branch(snapshot: &RepoSnapshot, branch: &BranchName) -> String {
    let input = build_stack_comment_input(snapshot, branch);
    generate_stack_comment(&input)
}

/// Generate a stack comment body for a branch, fetching PR info from forge.
///
/// Use this variant after creating PRs when metadata hasn't been persisted yet.
///
/// # Arguments
///
/// * `forge` - The forge to fetch PR info from
/// * `snapshot` - Repository snapshot
/// * `branch` - The branch to generate the stack comment for
///
/// # Returns
///
/// The generated stack comment string (including markers).
pub async fn generate_stack_comment_for_branch_from_forge(
    forge: &dyn Forge,
    snapshot: &RepoSnapshot,
    branch: &BranchName,
) -> String {
    let input = build_stack_comment_input_from_forge(forge, snapshot, branch).await;
    generate_stack_comment(&input)
}

/// Generate a merged PR body with updated stack comment.
///
/// This fetches the existing PR body, merges the new stack comment,
/// and returns the result.
///
/// # Arguments
///
/// * `existing_body` - The current PR body (may be None)
/// * `snapshot` - Repository snapshot
/// * `branch` - The branch this PR is for
///
/// # Returns
///
/// The merged body string with updated stack comment.
pub fn generate_merged_body(
    existing_body: Option<&str>,
    snapshot: &RepoSnapshot,
    branch: &BranchName,
) -> String {
    let stack_comment = generate_stack_comment_for_branch(snapshot, branch);
    merge_stack_comment(existing_body, &stack_comment)
}

/// Generate a merged PR body with updated stack comment, fetching PR info from forge.
///
/// Use this variant after creating PRs when metadata hasn't been persisted yet.
///
/// # Arguments
///
/// * `forge` - The forge to fetch PR info from
/// * `existing_body` - The current PR body (may be None)
/// * `snapshot` - Repository snapshot
/// * `branch` - The branch this PR is for
///
/// # Returns
///
/// The merged body string with updated stack comment.
pub async fn generate_merged_body_from_forge(
    forge: &dyn Forge,
    existing_body: Option<&str>,
    snapshot: &RepoSnapshot,
    branch: &BranchName,
) -> String {
    let stack_comment = generate_stack_comment_for_branch_from_forge(forge, snapshot, branch).await;
    merge_stack_comment(existing_body, &stack_comment)
}

/// Update the stack comment for a single PR.
///
/// This fetches the current PR body, generates an updated stack comment,
/// merges it with the existing body, and updates the PR.
///
/// # Arguments
///
/// * `forge` - The forge implementation to use
/// * `snapshot` - Repository snapshot with current state
/// * `branch` - The branch whose PR should be updated
/// * `quiet` - If true, suppress progress output
///
/// # Returns
///
/// `Ok(true)` if the PR was updated, `Ok(false)` if skipped (no PR linked),
/// or an error if the update failed.
pub async fn update_pr_stack_comment(
    forge: &dyn Forge,
    snapshot: &RepoSnapshot,
    branch: &BranchName,
    quiet: bool,
) -> Result<bool> {
    // Get PR number from metadata
    let pr_number = match snapshot.metadata.get(branch) {
        Some(scanned) => match &scanned.metadata.pr {
            PrState::Linked { number, .. } => *number,
            PrState::None => return Ok(false), // No PR to update
        },
        None => return Ok(false), // Not tracked
    };

    // Fetch current PR body
    let existing_body = match forge.get_pr(pr_number).await {
        Ok(pr) => pr.body,
        Err(e) => {
            if !quiet {
                eprintln!(
                    "  Warning: Could not fetch PR #{} for '{}': {}",
                    pr_number, branch, e
                );
            }
            return Ok(false);
        }
    };

    // Generate merged body
    let new_body = generate_merged_body(existing_body.as_deref(), snapshot, branch);

    // Update PR
    let update_req = UpdatePrRequest {
        number: pr_number,
        title: None,
        body: Some(new_body),
        base: None,
    };

    match forge.update_pr(update_req).await {
        Ok(_) => {
            if !quiet {
                println!("  Updated stack comment for PR #{} ({})", pr_number, branch);
            }
            Ok(true)
        }
        Err(e) => {
            if !quiet {
                eprintln!(
                    "  Warning: Could not update stack comment for PR #{}: {}",
                    pr_number, e
                );
            }
            Ok(false)
        }
    }
}

/// Update stack comments for all PRs in a set of branches.
///
/// This is used by `sync` to refresh stack comments after detecting changes.
/// Uses metadata for PR info.
///
/// # Arguments
///
/// * `forge` - The forge implementation to use
/// * `snapshot` - Repository snapshot with current state
/// * `branches` - The branches whose PRs should be updated
/// * `quiet` - If true, suppress progress output
///
/// # Returns
///
/// The number of PRs that were successfully updated.
pub async fn update_stack_comments_for_branches(
    forge: &dyn Forge,
    snapshot: &RepoSnapshot,
    branches: &[BranchName],
    quiet: bool,
) -> Result<usize> {
    let mut updated_count = 0;

    for branch in branches {
        if update_pr_stack_comment(forge, snapshot, branch, quiet).await? {
            updated_count += 1;
        }
    }

    Ok(updated_count)
}

/// Update stack comments for all PRs in a set of branches, fetching PR info from forge.
///
/// Use this after creating PRs when metadata hasn't been persisted yet.
/// This variant looks up PR info directly from the forge API.
///
/// # Arguments
///
/// * `forge` - The forge implementation to use
/// * `snapshot` - Repository snapshot with graph structure
/// * `branches` - The branches whose PRs should be updated
/// * `quiet` - If true, suppress progress output
///
/// # Returns
///
/// The number of PRs that were successfully updated.
pub async fn update_stack_comments_for_branches_from_forge(
    forge: &dyn Forge,
    snapshot: &RepoSnapshot,
    branches: &[BranchName],
    quiet: bool,
) -> Result<usize> {
    let mut updated_count = 0;

    for branch in branches {
        // Look up PR by head branch name
        let pr = match forge.find_pr_by_head(branch.as_str()).await {
            Ok(Some(pr)) => pr,
            Ok(None) => continue, // No PR for this branch
            Err(e) => {
                if !quiet {
                    eprintln!("  Warning: Could not find PR for '{}': {}", branch, e);
                }
                continue;
            }
        };

        // Fetch current PR body
        let existing_body = match forge.get_pr(pr.number).await {
            Ok(fetched_pr) => fetched_pr.body,
            Err(e) => {
                if !quiet {
                    eprintln!(
                        "  Warning: Could not fetch PR #{} for '{}': {}",
                        pr.number, branch, e
                    );
                }
                continue;
            }
        };

        // Generate merged body using forge-based lookup
        let new_body =
            generate_merged_body_from_forge(forge, existing_body.as_deref(), snapshot, branch)
                .await;

        // Update PR
        let update_req = UpdatePrRequest {
            number: pr.number,
            title: None,
            body: Some(new_body),
            base: None,
        };

        match forge.update_pr(update_req).await {
            Ok(_) => {
                if !quiet {
                    println!("  Updated stack comment for PR #{} ({})", pr.number, branch);
                }
                updated_count += 1;
            }
            Err(e) => {
                if !quiet {
                    eprintln!(
                        "  Warning: Could not update stack comment for PR #{}: {}",
                        pr.number, e
                    );
                }
            }
        }
    }

    Ok(updated_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::graph::StackGraph;
    use crate::core::metadata::schema::BranchMetadataV1;
    use crate::core::types::Oid;
    use crate::engine::health::RepoHealthReport;
    use crate::engine::scan::{compute_fingerprint, ScannedMetadata};
    use crate::git::{GitState, RepoInfo, WorktreeStatus};
    use std::collections::HashMap;

    fn make_test_snapshot() -> RepoSnapshot {
        RepoSnapshot {
            info: RepoInfo {
                git_dir: std::path::PathBuf::from("/repo/.git"),
                common_dir: std::path::PathBuf::from("/repo/.git"),
                work_dir: Some(std::path::PathBuf::from("/repo")),
                context: crate::git::RepoContext::Normal,
            },
            git_state: GitState::Clean,
            worktree_status: WorktreeStatus::default(),
            current_branch: Some(BranchName::new("current").unwrap()),
            branches: HashMap::new(),
            metadata: HashMap::new(),
            repo_config: None,
            trunk: Some(BranchName::new("main").unwrap()),
            graph: StackGraph::new(),
            fingerprint: compute_fingerprint(&HashMap::new(), &HashMap::new(), None),
            health: RepoHealthReport::new(),
            remote_prs: None,
        }
    }

    fn add_tracked_branch(
        snapshot: &mut RepoSnapshot,
        name: &str,
        parent: &str,
        pr_number: Option<u64>,
    ) {
        let branch = BranchName::new(name).unwrap();
        let parent_branch = BranchName::new(parent).unwrap();
        let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();

        let mut metadata =
            BranchMetadataV1::new(branch.clone(), parent_branch.clone(), oid.clone());

        if let Some(num) = pr_number {
            metadata.pr = PrState::Linked {
                forge: "github".to_string(),
                number: num,
                url: format!("https://github.com/org/repo/pull/{}", num),
                last_known: None,
            };
        }

        snapshot.metadata.insert(
            branch.clone(),
            ScannedMetadata {
                ref_oid: oid,
                metadata,
            },
        );

        // Add to graph
        snapshot.graph.add_edge(branch, parent_branch);
    }

    // =============================================================
    // Build stack comment input tests
    // =============================================================

    #[test]
    fn build_input_orders_ancestors_correctly() {
        let mut snapshot = make_test_snapshot();

        // Create a chain: main <- feature-a <- feature-b <- feature-c
        add_tracked_branch(&mut snapshot, "feature-a", "main", Some(1));
        add_tracked_branch(&mut snapshot, "feature-b", "feature-a", Some(2));
        add_tracked_branch(&mut snapshot, "feature-c", "feature-b", Some(3));

        let input = build_stack_comment_input(&snapshot, &BranchName::new("feature-c").unwrap());

        // Should have: feature-a (ancestor), feature-b (ancestor), feature-c (current)
        assert_eq!(input.branches.len(), 3);
        assert_eq!(input.branches[0].name, "feature-a");
        assert_eq!(input.branches[0].position, StackPosition::Ancestor);
        assert_eq!(input.branches[1].name, "feature-b");
        assert_eq!(input.branches[1].position, StackPosition::Ancestor);
        assert_eq!(input.branches[2].name, "feature-c");
        assert_eq!(input.branches[2].position, StackPosition::Current);
    }

    #[test]
    fn build_input_marks_current_correctly() {
        let mut snapshot = make_test_snapshot();
        add_tracked_branch(&mut snapshot, "feature", "main", Some(1));

        let input = build_stack_comment_input(&snapshot, &BranchName::new("feature").unwrap());

        assert_eq!(input.branches.len(), 1);
        assert_eq!(input.branches[0].name, "feature");
        assert_eq!(input.branches[0].position, StackPosition::Current);
    }

    #[test]
    fn build_input_orders_descendants_by_depth() {
        let mut snapshot = make_test_snapshot();

        // Create: main <- feature-a <- feature-b, feature-c (both children of feature-a)
        add_tracked_branch(&mut snapshot, "feature-a", "main", Some(1));
        add_tracked_branch(&mut snapshot, "feature-b", "feature-a", Some(2));
        add_tracked_branch(&mut snapshot, "feature-c", "feature-a", Some(3));

        let input = build_stack_comment_input(&snapshot, &BranchName::new("feature-a").unwrap());

        // Current + 2 descendants
        assert_eq!(input.branches.len(), 3);
        assert_eq!(input.branches[0].name, "feature-a");
        assert_eq!(input.branches[0].position, StackPosition::Current);

        // Both descendants should be marked as such
        assert!(input.branches[1..]
            .iter()
            .all(|b| b.position == StackPosition::Descendant));
    }

    #[test]
    fn build_input_extracts_pr_info() {
        let mut snapshot = make_test_snapshot();
        add_tracked_branch(&mut snapshot, "feature", "main", Some(42));

        let input = build_stack_comment_input(&snapshot, &BranchName::new("feature").unwrap());

        assert_eq!(input.branches[0].pr_number, Some(42));
        assert!(input.branches[0].pr_url.is_some());
        assert!(input.branches[0].pr_url.as_ref().unwrap().contains("42"));
    }

    #[test]
    fn build_input_handles_missing_pr() {
        let mut snapshot = make_test_snapshot();
        add_tracked_branch(&mut snapshot, "feature", "main", None);

        let input = build_stack_comment_input(&snapshot, &BranchName::new("feature").unwrap());

        assert_eq!(input.branches[0].pr_number, None);
        assert_eq!(input.branches[0].pr_url, None);
    }

    // =============================================================
    // Generate stack comment for branch tests
    // =============================================================

    #[test]
    fn generate_stack_comment_for_branch_produces_valid_output() {
        let mut snapshot = make_test_snapshot();
        add_tracked_branch(&mut snapshot, "feature", "main", Some(1));

        let comment =
            generate_stack_comment_for_branch(&snapshot, &BranchName::new("feature").unwrap());

        assert!(comment.contains("<!-- lattice:stack:start -->"));
        assert!(comment.contains("<!-- lattice:stack:end -->"));
        assert!(comment.contains("feature"));
        assert!(comment.contains("[#1]"));
    }

    // =============================================================
    // Generate merged body tests
    // =============================================================

    #[test]
    fn generate_merged_body_with_existing_content() {
        let mut snapshot = make_test_snapshot();
        add_tracked_branch(&mut snapshot, "feature", "main", Some(1));

        let existing = "My PR description.";
        let result = generate_merged_body(
            Some(existing),
            &snapshot,
            &BranchName::new("feature").unwrap(),
        );

        assert!(result.contains("My PR description."));
        assert!(result.contains("<!-- lattice:stack:start -->"));
    }

    #[test]
    fn generate_merged_body_replaces_existing_stack() {
        let mut snapshot = make_test_snapshot();
        add_tracked_branch(&mut snapshot, "feature", "main", Some(1));

        let existing =
            "Description\n\n<!-- lattice:stack:start -->\nold\n<!-- lattice:stack:end -->";
        let result = generate_merged_body(
            Some(existing),
            &snapshot,
            &BranchName::new("feature").unwrap(),
        );

        assert!(result.contains("Description"));
        assert!(result.contains("feature"));
        assert!(!result.contains("old"));
    }
}
