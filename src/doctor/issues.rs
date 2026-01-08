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
}
