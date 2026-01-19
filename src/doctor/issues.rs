//! doctor::issues
//!
//! Known issue types for the Doctor framework.
//!
//! # Architecture
//!
//! This module provides a type-safe enum of known issues. Each variant
//! maps to a specific issue constructor in `engine::health::issues`.
//!
//! The `KnownIssue` enum is useful for:
//! - Type-safe issue matching in fix generators
//! - Generating stable issue IDs
//! - Determining issue severity

use thiserror::Error;

use crate::engine::health::{Issue, IssueId, Severity};

/// Known issue types that Doctor can diagnose and repair.
///
/// Each variant corresponds to a specific repository problem
/// with associated fix options.
#[derive(Debug, Clone, Error)]
pub enum KnownIssue {
    /// Trunk branch not configured in repo config.
    #[error("trunk not configured")]
    TrunkNotConfigured,

    /// Metadata for a branch failed to parse.
    #[error("metadata parse error for branch '{branch}': {error}")]
    MetadataParseError {
        /// The branch with invalid metadata.
        branch: String,
        /// The parse error message.
        error: String,
    },

    /// A branch's parent reference points to a non-existent branch.
    #[error("parent branch '{parent}' missing (referenced by '{child}')")]
    ParentMissing {
        /// The missing parent branch.
        parent: String,
        /// The child branch referencing the missing parent.
        child: String,
    },

    /// The stack graph contains a cycle.
    #[error("cycle detected in stack graph: {trace}")]
    CycleDetected {
        /// Human-readable cycle trace (e.g., "a -> b -> c -> a").
        trace: String,
        /// Branches involved in the cycle.
        branches: Vec<String>,
    },

    /// Base commit is not an ancestor of the branch tip.
    #[error("base commit not ancestor of tip for branch '{branch}'")]
    BaseAncestryViolation {
        /// The branch with the violation.
        branch: String,
        /// The base OID.
        base_oid: String,
        /// The tip OID.
        tip_oid: String,
    },

    /// Metadata ref exists but the corresponding branch doesn't.
    #[error("metadata exists but branch '{branch}' is missing")]
    OrphanedMetadata {
        /// The branch name from the metadata ref.
        branch: String,
        /// The metadata ref OID.
        metadata_ref_oid: String,
    },

    /// Branch exists but has no tracking metadata.
    #[error("branch '{branch}' exists but is not tracked")]
    UntrackedBranch {
        /// The untracked branch name.
        branch: String,
    },

    /// A Lattice operation is in progress.
    #[error("lattice operation '{command}' in progress ({op_id})")]
    LatticeOpInProgress {
        /// The command that started the operation.
        command: String,
        /// The operation ID.
        op_id: String,
    },

    /// An external Git operation is in progress.
    #[error("external git operation in progress: {state}")]
    ExternalGitOpInProgress {
        /// The Git state (rebase, merge, cherry-pick, etc.).
        state: String,
    },

    /// Config file needs migration to canonical location.
    #[error("config file at '{old_path}' should be migrated to '{new_path}'")]
    ConfigMigrationNeeded {
        /// The legacy config path.
        old_path: String,
        /// The canonical config path.
        new_path: String,
    },

    // --- Bootstrap Issues (Remote Evidence) ---
    /// Remote forge reports open pull requests for this repository.
    /// This is informational - indicates bootstrap opportunity.
    #[error("remote has {count} open pull request(s)")]
    RemoteOpenPullRequestsDetected {
        /// Number of open PRs detected.
        count: usize,
        /// Whether the result was truncated (more PRs exist).
        truncated: bool,
    },

    /// An open PR exists on the remote but the head branch doesn't exist locally.
    /// User should fetch the branch to import the PR.
    #[error("open PR #{number} has head branch '{head_ref}' which doesn't exist locally")]
    RemoteOpenPrBranchMissingLocally {
        /// PR number.
        number: u64,
        /// Head branch name from the PR.
        head_ref: String,
        /// Base branch name (often trunk).
        base_ref: String,
        /// PR URL for reference.
        url: String,
    },

    /// A local branch exists that matches an open PR's head, but it's not tracked.
    /// User should track the branch to link it with the PR.
    #[error("branch '{branch}' matches open PR #{number} but is not tracked")]
    RemoteOpenPrBranchUntracked {
        /// Local branch name.
        branch: String,
        /// PR number.
        number: u64,
        /// PR's base ref (parent branch).
        base_ref: String,
        /// PR URL.
        url: String,
    },

    /// A tracked branch exists that matches an open PR, but the PR isn't linked in metadata.
    /// User should link the PR to the tracked branch.
    #[error(
        "tracked branch '{branch}' matches open PR #{number} but PR is not linked in metadata"
    )]
    RemoteOpenPrNotLinkedInMetadata {
        /// Branch name.
        branch: String,
        /// PR number.
        number: u64,
        /// PR URL.
        url: String,
    },

    // --- Synthetic Stack Detection (Milestone 5.8) ---
    /// A PR targeting trunk may be a synthetic stack head.
    /// This indicates prior work may have been merged into the branch.
    #[error("PR #{pr_number} targeting trunk may be a synthetic stack head (branch '{branch}')")]
    PotentialSyntheticStackHead {
        /// The branch that may be a synthetic stack head.
        branch: String,
        /// PR number targeting trunk.
        pr_number: u64,
        /// PR URL.
        pr_url: String,
    },
}

impl KnownIssue {
    /// Generate a stable issue ID for this issue.
    ///
    /// Issue IDs are deterministic and stable across runs,
    /// allowing them to be referenced in commands.
    pub fn issue_id(&self) -> IssueId {
        match self {
            KnownIssue::TrunkNotConfigured => IssueId::singleton("trunk-not-configured"),
            KnownIssue::MetadataParseError { branch, .. } => {
                IssueId::new("metadata-parse-error", branch)
            }
            KnownIssue::ParentMissing { child, .. } => IssueId::new("parent-missing", child),
            KnownIssue::CycleDetected { branches, .. } => {
                IssueId::new("graph-cycle", &branches.join(","))
            }
            KnownIssue::BaseAncestryViolation { branch, .. } => {
                IssueId::new("base-not-ancestor", branch)
            }
            KnownIssue::OrphanedMetadata { branch, .. } => {
                IssueId::new("orphaned-metadata", branch)
            }
            KnownIssue::UntrackedBranch { branch } => IssueId::new("untracked-branch", branch),
            KnownIssue::LatticeOpInProgress { op_id, .. } => {
                IssueId::new("lattice-op-in-progress", op_id)
            }
            KnownIssue::ExternalGitOpInProgress { state } => {
                IssueId::new("git-op-in-progress", state)
            }
            KnownIssue::ConfigMigrationNeeded { old_path, .. } => {
                IssueId::new("config-migration", old_path)
            }
            KnownIssue::RemoteOpenPullRequestsDetected { .. } => {
                IssueId::singleton("remote-open-prs-detected")
            }
            KnownIssue::RemoteOpenPrBranchMissingLocally { number, .. } => {
                IssueId::new("remote-pr-branch-missing", &number.to_string())
            }
            KnownIssue::RemoteOpenPrBranchUntracked { branch, .. } => {
                IssueId::new("remote-pr-branch-untracked", branch)
            }
            KnownIssue::RemoteOpenPrNotLinkedInMetadata { branch, .. } => {
                IssueId::new("remote-pr-not-linked", branch)
            }
            KnownIssue::PotentialSyntheticStackHead { pr_number, .. } => {
                IssueId::new("synthetic-stack-head", &pr_number.to_string())
            }
        }
    }

    /// Get the severity of this issue.
    pub fn severity(&self) -> Severity {
        match self {
            KnownIssue::TrunkNotConfigured => Severity::Blocking,
            KnownIssue::MetadataParseError { .. } => Severity::Blocking,
            KnownIssue::ParentMissing { .. } => Severity::Blocking,
            KnownIssue::CycleDetected { .. } => Severity::Blocking,
            KnownIssue::BaseAncestryViolation { .. } => Severity::Warning,
            KnownIssue::OrphanedMetadata { .. } => Severity::Warning,
            KnownIssue::UntrackedBranch { .. } => Severity::Info,
            KnownIssue::LatticeOpInProgress { .. } => Severity::Blocking,
            KnownIssue::ExternalGitOpInProgress { .. } => Severity::Blocking,
            KnownIssue::ConfigMigrationNeeded { .. } => Severity::Warning,
            KnownIssue::RemoteOpenPullRequestsDetected { .. } => Severity::Info,
            KnownIssue::RemoteOpenPrBranchMissingLocally { .. } => Severity::Warning,
            KnownIssue::RemoteOpenPrBranchUntracked { .. } => Severity::Warning,
            KnownIssue::RemoteOpenPrNotLinkedInMetadata { .. } => Severity::Info,
            KnownIssue::PotentialSyntheticStackHead { .. } => Severity::Info,
        }
    }

    /// Convert this known issue to an `engine::health::Issue`.
    ///
    /// This creates the appropriate Issue with evidence and blocked capabilities.
    pub fn to_issue(&self) -> Issue {
        use crate::engine::health::issues;

        match self {
            KnownIssue::TrunkNotConfigured => issues::trunk_not_configured(),
            KnownIssue::MetadataParseError { branch, error } => {
                issues::metadata_parse_error(branch, error)
            }
            KnownIssue::ParentMissing { parent, child } => issues::parent_missing(child, parent),
            KnownIssue::CycleDetected { branches, .. } => issues::graph_cycle(branches.clone()),
            KnownIssue::BaseAncestryViolation {
                branch,
                base_oid,
                tip_oid,
            } => issues::base_not_ancestor(branch, base_oid, tip_oid),
            KnownIssue::OrphanedMetadata { branch, .. } => issues::orphaned_metadata(branch),
            KnownIssue::UntrackedBranch { branch } => issues::missing_branch(branch),
            KnownIssue::LatticeOpInProgress { command, op_id } => {
                issues::lattice_operation_in_progress(command, op_id)
            }
            KnownIssue::ExternalGitOpInProgress { state } => {
                issues::git_operation_in_progress(state)
            }
            KnownIssue::ConfigMigrationNeeded { old_path, new_path } => {
                issues::config_migration_needed(old_path, new_path)
            }
            KnownIssue::RemoteOpenPullRequestsDetected { count, truncated } => {
                issues::remote_open_prs_detected(*count, *truncated)
            }
            KnownIssue::RemoteOpenPrBranchMissingLocally {
                number,
                head_ref,
                base_ref,
                url,
            } => issues::remote_pr_branch_missing(*number, head_ref, base_ref, url),
            KnownIssue::RemoteOpenPrBranchUntracked {
                branch,
                number,
                base_ref,
                url,
            } => issues::remote_pr_branch_untracked(branch, *number, base_ref, url),
            KnownIssue::RemoteOpenPrNotLinkedInMetadata {
                branch,
                number,
                url,
            } => issues::remote_pr_not_linked(branch, *number, url),
            KnownIssue::PotentialSyntheticStackHead {
                branch,
                pr_number,
                pr_url,
            } => issues::potential_synthetic_stack_head(branch, *pr_number, pr_url),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trunk_not_configured_issue_id() {
        let issue = KnownIssue::TrunkNotConfigured;
        assert_eq!(issue.issue_id().as_str(), "trunk-not-configured");
        assert_eq!(issue.severity(), Severity::Blocking);
    }

    #[test]
    fn metadata_parse_error_issue_id() {
        let issue = KnownIssue::MetadataParseError {
            branch: "feature".to_string(),
            error: "invalid json".to_string(),
        };
        assert!(issue
            .issue_id()
            .as_str()
            .starts_with("metadata-parse-error:"));
        assert_eq!(issue.severity(), Severity::Blocking);
    }

    #[test]
    fn parent_missing_issue_id() {
        let issue = KnownIssue::ParentMissing {
            parent: "parent-branch".to_string(),
            child: "child-branch".to_string(),
        };
        assert!(issue.issue_id().as_str().starts_with("parent-missing:"));
        assert_eq!(issue.severity(), Severity::Blocking);
    }

    #[test]
    fn cycle_detected_issue_id() {
        let issue = KnownIssue::CycleDetected {
            trace: "a -> b -> a".to_string(),
            branches: vec!["a".to_string(), "b".to_string()],
        };
        assert!(issue.issue_id().as_str().starts_with("graph-cycle:"));
        assert_eq!(issue.severity(), Severity::Blocking);
    }

    #[test]
    fn orphaned_metadata_severity() {
        let issue = KnownIssue::OrphanedMetadata {
            branch: "old".to_string(),
            metadata_ref_oid: "abc123".to_string(),
        };
        assert_eq!(issue.severity(), Severity::Warning);
    }

    #[test]
    fn untracked_branch_severity() {
        let issue = KnownIssue::UntrackedBranch {
            branch: "feature".to_string(),
        };
        assert_eq!(issue.severity(), Severity::Info);
    }

    #[test]
    fn to_issue_creates_health_issue() {
        let known = KnownIssue::TrunkNotConfigured;
        let issue = known.to_issue();
        assert!(issue.is_blocking());
    }

    // --- Bootstrap Issue Tests ---

    #[test]
    fn remote_open_prs_detected_issue_id() {
        let issue = KnownIssue::RemoteOpenPullRequestsDetected {
            count: 5,
            truncated: false,
        };
        assert_eq!(issue.issue_id().as_str(), "remote-open-prs-detected");
        assert_eq!(issue.severity(), Severity::Info);
    }

    #[test]
    fn remote_open_prs_detected_truncated() {
        let issue = KnownIssue::RemoteOpenPullRequestsDetected {
            count: 200,
            truncated: true,
        };
        let health_issue = issue.to_issue();
        assert!(!health_issue.is_blocking());
        assert!(health_issue.message.contains("200"));
        assert!(health_issue.message.contains("truncated"));
    }

    #[test]
    fn remote_pr_branch_missing_issue_id() {
        let issue = KnownIssue::RemoteOpenPrBranchMissingLocally {
            number: 42,
            head_ref: "feature".to_string(),
            base_ref: "main".to_string(),
            url: "https://github.com/org/repo/pull/42".to_string(),
        };
        assert!(issue
            .issue_id()
            .as_str()
            .starts_with("remote-pr-branch-missing:"));
        assert_eq!(issue.severity(), Severity::Warning);
    }

    #[test]
    fn remote_pr_branch_untracked_issue_id() {
        let issue = KnownIssue::RemoteOpenPrBranchUntracked {
            branch: "feature".to_string(),
            number: 42,
            base_ref: "main".to_string(),
            url: "https://github.com/org/repo/pull/42".to_string(),
        };
        assert!(issue
            .issue_id()
            .as_str()
            .starts_with("remote-pr-branch-untracked:"));
        assert_eq!(issue.severity(), Severity::Warning);
    }

    #[test]
    fn remote_pr_not_linked_issue_id() {
        let issue = KnownIssue::RemoteOpenPrNotLinkedInMetadata {
            branch: "feature".to_string(),
            number: 42,
            url: "https://github.com/org/repo/pull/42".to_string(),
        };
        assert!(issue
            .issue_id()
            .as_str()
            .starts_with("remote-pr-not-linked:"));
        assert_eq!(issue.severity(), Severity::Info);
    }

    #[test]
    fn remote_issues_to_issue_not_blocking() {
        // All bootstrap issues should be non-blocking
        let issues = vec![
            KnownIssue::RemoteOpenPullRequestsDetected {
                count: 3,
                truncated: false,
            },
            KnownIssue::RemoteOpenPrBranchMissingLocally {
                number: 1,
                head_ref: "f".to_string(),
                base_ref: "m".to_string(),
                url: "u".to_string(),
            },
            KnownIssue::RemoteOpenPrBranchUntracked {
                branch: "f".to_string(),
                number: 1,
                base_ref: "main".to_string(),
                url: "u".to_string(),
            },
            KnownIssue::RemoteOpenPrNotLinkedInMetadata {
                branch: "f".to_string(),
                number: 1,
                url: "u".to_string(),
            },
        ];

        for known in issues {
            let health_issue = known.to_issue();
            assert!(
                !health_issue.is_blocking(),
                "Bootstrap issue should not be blocking: {:?}",
                known
            );
        }
    }

    // --- Synthetic Stack Detection Tests (Milestone 5.8) ---

    #[test]
    fn potential_synthetic_stack_head_issue_id() {
        let issue = KnownIssue::PotentialSyntheticStackHead {
            branch: "feature".to_string(),
            pr_number: 42,
            pr_url: "https://github.com/org/repo/pull/42".to_string(),
        };
        assert!(issue
            .issue_id()
            .as_str()
            .starts_with("synthetic-stack-head:"));
        assert_eq!(issue.severity(), Severity::Info);
    }

    #[test]
    fn potential_synthetic_stack_head_to_issue() {
        let known = KnownIssue::PotentialSyntheticStackHead {
            branch: "feature".to_string(),
            pr_number: 42,
            pr_url: "https://github.com/org/repo/pull/42".to_string(),
        };
        let issue = known.to_issue();

        // Should be informational, not blocking
        assert!(!issue.is_blocking());
        assert_eq!(issue.severity, Severity::Info);

        // Message should contain branch and PR number
        assert!(issue.message.contains("feature"));
        assert!(issue.message.contains("42"));
    }

    #[test]
    fn potential_synthetic_stack_head_not_blocking() {
        let issue = KnownIssue::PotentialSyntheticStackHead {
            branch: "feature".to_string(),
            pr_number: 42,
            pr_url: "u".to_string(),
        };
        let health_issue = issue.to_issue();
        assert!(
            !health_issue.is_blocking(),
            "Synthetic stack head should be informational, not blocking"
        );
    }
}
