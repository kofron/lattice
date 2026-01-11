//! doctor::generators
//!
//! Fix generators for the Doctor framework.
//!
//! # Architecture
//!
//! Each issue type has one or more fix generators that produce `FixOption`s.
//! Fix generators are pure functions: they take an issue and snapshot,
//! and return possible fixes without performing any I/O.
//!
//! Per ARCHITECTURE.md Section 8.2, fix options must contain:
//! - Preconditions (capabilities required at apply time)
//! - A plan preview (what changes will be made)
//! - A concrete repair plan (generated when the fix is selected)

use crate::engine::capabilities::Capability;
use crate::engine::health::{Evidence, Issue};
use crate::engine::scan::RepoSnapshot;

use super::fixes::{ConfigChange, FixId, FixOption, FixPreview, MetadataChange, RefChange};

/// Generate fix options for an issue.
///
/// This is the main entry point for fix generation. It examines the issue
/// and dispatches to the appropriate generator.
///
/// # Example
///
/// ```ignore
/// use latticework::doctor::generators::generate_fixes;
/// use latticework::engine::health::Issue;
///
/// let issue = Issue::new("trunk-not-configured", Severity::Blocking, "...");
/// let fixes = generate_fixes(&issue, &snapshot);
/// for fix in fixes {
///     println!("{}: {}", fix.id, fix.description);
/// }
/// ```
pub fn generate_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let issue_type = extract_issue_type(issue.id.as_str());

    match issue_type {
        "trunk-not-configured" => generate_trunk_fixes(issue, snapshot),
        "metadata-parse-error" => generate_metadata_parse_fixes(issue, snapshot),
        "parent-missing" => generate_parent_missing_fixes(issue, snapshot),
        "graph-cycle" => generate_cycle_fixes(issue, snapshot),
        "base-not-ancestor" => generate_base_ancestry_fixes(issue, snapshot),
        "orphaned-metadata" => generate_orphaned_metadata_fixes(issue, snapshot),
        "lattice-op-in-progress" => generate_lattice_op_fixes(issue, snapshot),
        "git-op-in-progress" => generate_git_op_fixes(issue, snapshot),
        "config-migration" => generate_config_migration_fixes(issue, snapshot),
        _ => Vec::new(), // Unknown issue type
    }
}

/// Extract the issue type prefix from an issue ID.
///
/// Issue IDs are formatted as `type:hash` or just `type` for singletons.
fn extract_issue_type(id: &str) -> &str {
    id.split(':').next().unwrap_or(id)
}

/// Generate fixes for missing trunk configuration.
///
/// Fix options:
/// 1. Set trunk to 'main' (if branch exists)
/// 2. Set trunk to 'master' (if branch exists)
/// 3. Set trunk to detected default branch
fn generate_trunk_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Common trunk branch names to try
    let candidates = ["main", "master", "develop", "trunk"];

    for candidate in candidates {
        if let Ok(branch) = crate::core::types::BranchName::new(candidate) {
            if snapshot.branches.contains_key(&branch) {
                fixes.push(
                    FixOption::new(
                        FixId::new("trunk-not-configured", "set-trunk", candidate),
                        issue.id.clone(),
                        format!("Set trunk to '{}'", candidate),
                        FixPreview::with_summary(format!(
                            "Configure '{}' as the trunk branch",
                            candidate
                        ))
                        .add_config_change(ConfigChange::Set {
                            key: "trunk".to_string(),
                            value: candidate.to_string(),
                        }),
                    )
                    .with_precondition(Capability::RepoOpen),
                );
            }
        }
    }

    // If no common candidates found, offer to use current branch
    if fixes.is_empty() {
        if let Some(current) = &snapshot.current_branch {
            fixes.push(
                FixOption::new(
                    FixId::new("trunk-not-configured", "set-trunk", current.as_str()),
                    issue.id.clone(),
                    format!("Set trunk to current branch '{}'", current),
                    FixPreview::with_summary(format!(
                        "Configure '{}' as the trunk branch",
                        current
                    ))
                    .add_config_change(ConfigChange::Set {
                        key: "trunk".to_string(),
                        value: current.as_str().to_string(),
                    }),
                )
                .with_precondition(Capability::RepoOpen),
            );
        }
    }

    fixes
}

/// Generate fixes for metadata parse errors.
///
/// Fix options:
/// 1. Delete the invalid metadata ref (untrack the branch)
/// 2. Re-initialize metadata with defaults
fn generate_metadata_parse_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract branch name from evidence
    let branch = issue
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::ParseError { ref_name, .. } => {
                // Extract branch name from ref_name like "refs/branch-metadata/feature"
                ref_name.strip_prefix("refs/branch-metadata/")
            }
            _ => None,
        })
        .unwrap_or("unknown");

    // Option 1: Delete the invalid metadata
    fixes.push(
        FixOption::new(
            FixId::new("metadata-parse-error", "delete-metadata", branch),
            issue.id.clone(),
            format!("Delete invalid metadata for '{}'", branch),
            FixPreview::with_summary(format!(
                "Remove the corrupted metadata ref for '{}', making it untracked",
                branch
            ))
            .add_metadata_change(MetadataChange::Delete {
                branch: branch.to_string(),
            }),
        )
        .with_precondition(Capability::RepoOpen),
    );

    // Option 2: Re-initialize if branch exists
    if let Ok(branch_name) = crate::core::types::BranchName::new(branch) {
        if snapshot.branches.contains_key(&branch_name) {
            let parent = snapshot
                .trunk
                .as_ref()
                .map(|t| t.as_str())
                .unwrap_or("main");

            fixes.push(
                FixOption::new(
                    FixId::new("metadata-parse-error", "reinit-metadata", branch),
                    issue.id.clone(),
                    format!(
                        "Re-initialize metadata for '{}' with parent '{}'",
                        branch, parent
                    ),
                    FixPreview::with_summary(format!(
                        "Create fresh metadata for '{}' tracking it under '{}'",
                        branch, parent
                    ))
                    .add_metadata_change(MetadataChange::Create {
                        branch: branch.to_string(),
                        description: format!("parent={}", parent),
                    }),
                )
                .with_preconditions([Capability::RepoOpen, Capability::TrunkKnown]),
            );
        }
    }

    fixes
}

/// Generate fixes for missing parent branch.
///
/// Fix options:
/// 1. Reparent to trunk
/// 2. Reparent to nearest ancestor that exists
/// 3. Untrack the orphaned branch
fn generate_parent_missing_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract child branch from evidence
    let child = issue.evidence.iter().find_map(|e| match e {
        Evidence::MissingBranch { name } => Some(name.as_str()),
        _ => None,
    });

    // We need to find which branch has this missing parent
    // The child is actually in the issue message, extract from issue ID
    let child_branch = issue
        .id
        .as_str()
        .strip_prefix("parent-missing:")
        .and_then(|s| s.split(':').next())
        .or(child)
        .unwrap_or("unknown");

    // Option 1: Reparent to trunk
    if let Some(trunk) = &snapshot.trunk {
        fixes.push(
            FixOption::new(
                FixId::new("parent-missing", "reparent-trunk", child_branch),
                issue.id.clone(),
                format!("Reparent '{}' to trunk ('{}')", child_branch, trunk),
                FixPreview::with_summary(format!(
                    "Update '{}' to have '{}' as its parent",
                    child_branch, trunk
                ))
                .add_metadata_change(MetadataChange::Update {
                    branch: child_branch.to_string(),
                    field: "parent".to_string(),
                    old_value: None,
                    new_value: trunk.as_str().to_string(),
                }),
            )
            .with_preconditions([Capability::RepoOpen, Capability::TrunkKnown]),
        );
    }

    // Option 2: Untrack the branch
    fixes.push(
        FixOption::new(
            FixId::new("parent-missing", "untrack", child_branch),
            issue.id.clone(),
            format!("Untrack '{}'", child_branch),
            FixPreview::with_summary(format!(
                "Remove tracking metadata for '{}', keeping the branch itself",
                child_branch
            ))
            .add_metadata_change(MetadataChange::Delete {
                branch: child_branch.to_string(),
            }),
        )
        .with_precondition(Capability::RepoOpen),
    );

    fixes
}

/// Generate fixes for graph cycles.
///
/// Fix options:
/// 1. Break cycle by untracking one of the branches
/// 2. Reparent one branch to trunk to break the cycle
fn generate_cycle_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract branches involved in cycle
    let cycle_branches: Vec<&str> = issue
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::Cycle { branches } => {
                Some(branches.iter().map(|s| s.as_str()).collect::<Vec<_>>())
            }
            _ => None,
        })
        .unwrap_or_default();

    if cycle_branches.is_empty() {
        return fixes;
    }

    // For each branch in the cycle, offer to reparent to trunk
    if let Some(trunk) = &snapshot.trunk {
        for branch in &cycle_branches {
            fixes.push(
                FixOption::new(
                    FixId::new("graph-cycle", "reparent-trunk", branch),
                    issue.id.clone(),
                    format!("Break cycle by reparenting '{}' to trunk", branch),
                    FixPreview::with_summary(format!(
                        "Set '{}' parent to '{}' to break the cycle",
                        branch, trunk
                    ))
                    .add_metadata_change(MetadataChange::Update {
                        branch: branch.to_string(),
                        field: "parent".to_string(),
                        old_value: None,
                        new_value: trunk.as_str().to_string(),
                    }),
                )
                .with_preconditions([Capability::RepoOpen, Capability::TrunkKnown]),
            );
        }
    }

    // Offer to untrack one branch to break the cycle
    if let Some(first) = cycle_branches.first() {
        fixes.push(
            FixOption::new(
                FixId::new("graph-cycle", "untrack", first),
                issue.id.clone(),
                format!("Break cycle by untracking '{}'", first),
                FixPreview::with_summary(format!(
                    "Remove '{}' from tracking to break the cycle",
                    first
                ))
                .add_metadata_change(MetadataChange::Delete {
                    branch: first.to_string(),
                }),
            )
            .with_precondition(Capability::RepoOpen),
        );
    }

    fixes
}

/// Generate fixes for base ancestry violations.
///
/// Fix options:
/// 1. Recompute base from parent tip
/// 2. Force update base to current value
fn generate_base_ancestry_fixes(issue: &Issue, _snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract branch and base info from evidence
    let (branch, _base_oid, _tip_oid) = issue
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::BaseAncestry {
                branch,
                base_oid,
                tip_oid,
            } => Some((branch.as_str(), base_oid.as_str(), tip_oid.as_str())),
            _ => None,
        })
        .unwrap_or(("unknown", "", ""));

    // Option 1: Recompute base from parent
    fixes.push(
        FixOption::new(
            FixId::new("base-not-ancestor", "recompute-base", branch),
            issue.id.clone(),
            format!("Recompute base for '{}' from parent", branch),
            FixPreview::with_summary(format!(
                "Recalculate the base commit for '{}' using the parent branch tip",
                branch
            ))
            .add_metadata_change(MetadataChange::Update {
                branch: branch.to_string(),
                field: "base".to_string(),
                old_value: None,
                new_value: "(computed from parent)".to_string(),
            }),
        )
        .with_preconditions([Capability::RepoOpen, Capability::GraphValid]),
    );

    // Option 2: Trigger a restack
    fixes.push(
        FixOption::new(
            FixId::new("base-not-ancestor", "restack", branch),
            issue.id.clone(),
            format!("Restack '{}' to fix ancestry", branch),
            FixPreview::with_summary(format!(
                "Rebase '{}' onto its parent to restore proper ancestry",
                branch
            )),
        )
        .with_preconditions([
            Capability::RepoOpen,
            Capability::NoLatticeOpInProgress,
            Capability::NoExternalGitOpInProgress,
        ]),
    );

    fixes
}

/// Generate fixes for orphaned metadata.
///
/// Fix options:
/// 1. Delete the orphaned metadata ref
fn generate_orphaned_metadata_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let _ = snapshot; // Not needed for this simple fix

    let mut fixes = Vec::new();

    // Extract branch name from evidence
    let branch = issue
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::Ref { name, .. } => name.strip_prefix("refs/branch-metadata/"),
            _ => None,
        })
        .unwrap_or("unknown");

    fixes.push(
        FixOption::new(
            FixId::new("orphaned-metadata", "delete", branch),
            issue.id.clone(),
            format!("Delete orphaned metadata for '{}'", branch),
            FixPreview::with_summary(format!(
                "Remove the metadata ref for '{}' since the branch no longer exists",
                branch
            ))
            .add_ref_change(RefChange::Delete {
                ref_name: format!("refs/branch-metadata/{}", branch),
                old_oid: "(current)".to_string(),
            }),
        )
        .with_precondition(Capability::RepoOpen),
    );

    fixes
}

/// Generate fixes for Lattice operation in progress.
///
/// Fix options:
/// 1. Continue the operation
/// 2. Abort the operation
fn generate_lattice_op_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let _ = snapshot;

    let mut fixes = Vec::new();

    // Extract operation info from the issue message
    let op_info = issue.message.clone();

    fixes.push(
        FixOption::new(
            FixId::simple("lattice-op-in-progress", "continue"),
            issue.id.clone(),
            "Continue the in-progress operation",
            FixPreview::with_summary(format!("Resume: {}", op_info)),
        )
        .with_precondition(Capability::RepoOpen),
    );

    fixes.push(
        FixOption::new(
            FixId::simple("lattice-op-in-progress", "abort"),
            issue.id.clone(),
            "Abort the in-progress operation",
            FixPreview::with_summary("Cancel the operation and restore previous state"),
        )
        .with_precondition(Capability::RepoOpen),
    );

    fixes
}

/// Generate fixes for external Git operation in progress.
///
/// Fix options:
/// 1. Abort via git (rebase --abort, merge --abort, etc.)
fn generate_git_op_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let _ = snapshot;

    let mut fixes = Vec::new();

    // Extract Git state from evidence
    let state = issue
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::GitState { state } => Some(state.as_str()),
            _ => None,
        })
        .unwrap_or("operation");

    let abort_cmd = match state {
        "rebase" => "git rebase --abort",
        "merge" => "git merge --abort",
        "cherry-pick" => "git cherry-pick --abort",
        "revert" => "git revert --abort",
        _ => "git <operation> --abort",
    };

    fixes.push(
        FixOption::new(
            FixId::new("git-op-in-progress", "abort", state),
            issue.id.clone(),
            format!("Abort the Git {} (run `{}`)", state, abort_cmd),
            FixPreview::with_summary(format!(
                "Execute `{}` to cancel the in-progress operation",
                abort_cmd
            )),
        )
        .with_precondition(Capability::RepoOpen),
    );

    // Also suggest completing it
    let continue_cmd = match state {
        "rebase" => "git rebase --continue",
        "merge" => "git commit",
        "cherry-pick" => "git cherry-pick --continue",
        "revert" => "git revert --continue",
        _ => "git <operation> --continue",
    };

    fixes.push(
        FixOption::new(
            FixId::new("git-op-in-progress", "continue", state),
            issue.id.clone(),
            format!("Complete the Git {} (run `{}`)", state, continue_cmd),
            FixPreview::with_summary(format!(
                "Resolve conflicts and run `{}` to complete the operation",
                continue_cmd
            )),
        )
        .with_precondition(Capability::RepoOpen),
    );

    fixes
}

/// Generate fixes for config migration.
///
/// Fix options:
/// 1. Migrate config to canonical location
fn generate_config_migration_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let _ = snapshot;

    let mut fixes = Vec::new();

    // Extract paths from evidence
    let (old_path, new_path) = issue
        .evidence
        .iter()
        .find_map(|e| match e {
            Evidence::Config { key, problem } => {
                // Parse "legacy location, should be at {new_path}" from problem
                let new = problem
                    .strip_prefix("legacy location, should be at ")
                    .unwrap_or(".git/lattice/config.toml");
                Some((key.as_str(), new))
            }
            _ => None,
        })
        .unwrap_or(("unknown", ".git/lattice/config.toml"));

    fixes.push(
        FixOption::new(
            FixId::simple("config-migration", "migrate"),
            issue.id.clone(),
            format!("Migrate config from '{}' to '{}'", old_path, new_path),
            FixPreview::with_summary("Move config to canonical location").add_config_change(
                ConfigChange::Migrate {
                    from: old_path.to_string(),
                    to: new_path.to_string(),
                },
            ),
        )
        .with_precondition(Capability::RepoOpen),
    );

    fixes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::health::{issues, Severity};

    // Helper to create a minimal snapshot for testing
    fn minimal_snapshot() -> RepoSnapshot {
        use crate::core::graph::StackGraph;
        use crate::core::types::{BranchName, Fingerprint, Oid};
        use crate::engine::health::RepoHealthReport;
        use crate::git::{GitState, RepoInfo, WorktreeStatus};
        use std::collections::HashMap;
        use std::path::PathBuf;

        let mut branches = HashMap::new();
        branches.insert(
            BranchName::new("main").unwrap(),
            Oid::new("abc123def4567890abc123def4567890abc12345").unwrap(),
        );

        RepoSnapshot {
            info: RepoInfo {
                git_dir: PathBuf::from(".git"),
                common_dir: PathBuf::from(".git"),
                work_dir: Some(PathBuf::from(".")),
                context: crate::git::RepoContext::Normal,
            },
            git_state: GitState::Clean,
            worktree_status: WorktreeStatus::default(),
            current_branch: Some(BranchName::new("main").unwrap()),
            branches,
            metadata: HashMap::new(),
            repo_config: None,
            trunk: Some(BranchName::new("main").unwrap()),
            graph: StackGraph::new(),
            fingerprint: Fingerprint::compute(&[]),
            health: RepoHealthReport::new(),
        }
    }

    #[test]
    fn extract_issue_type_singleton() {
        assert_eq!(
            extract_issue_type("trunk-not-configured"),
            "trunk-not-configured"
        );
    }

    #[test]
    fn extract_issue_type_with_hash() {
        assert_eq!(
            extract_issue_type("metadata-parse-error:abc123"),
            "metadata-parse-error"
        );
    }

    #[test]
    fn trunk_fixes_includes_main() {
        let issue = issues::trunk_not_configured();
        let snapshot = minimal_snapshot();

        let fixes = generate_trunk_fixes(&issue, &snapshot);

        assert!(!fixes.is_empty());
        assert!(fixes.iter().any(|f| f.description.contains("main")));
    }

    #[test]
    fn metadata_parse_fixes_offers_delete() {
        let issue = issues::metadata_parse_error("feature", "invalid json");
        let snapshot = minimal_snapshot();

        let fixes = generate_metadata_parse_fixes(&issue, &snapshot);

        assert!(!fixes.is_empty());
        assert!(fixes.iter().any(|f| f.description.contains("Delete")));
    }

    #[test]
    fn orphaned_metadata_fixes() {
        let issue = issues::orphaned_metadata("old-branch");
        let snapshot = minimal_snapshot();

        let fixes = generate_orphaned_metadata_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].description.contains("Delete"));
    }

    #[test]
    fn lattice_op_fixes_offers_continue_and_abort() {
        let issue = issues::lattice_operation_in_progress("restack", "op-123");
        let snapshot = minimal_snapshot();

        let fixes = generate_lattice_op_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 2);
        assert!(fixes.iter().any(|f| f.description.contains("Continue")));
        assert!(fixes.iter().any(|f| f.description.contains("Abort")));
    }

    #[test]
    fn git_op_fixes_offers_abort_and_continue() {
        let issue = issues::git_operation_in_progress("rebase");
        let snapshot = minimal_snapshot();

        let fixes = generate_git_op_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 2);
        assert!(fixes.iter().any(|f| f.description.contains("Abort")));
        assert!(fixes.iter().any(|f| f.description.contains("Complete")));
    }

    #[test]
    fn generate_fixes_dispatches_correctly() {
        let issue = issues::trunk_not_configured();
        let snapshot = minimal_snapshot();

        let fixes = generate_fixes(&issue, &snapshot);

        assert!(!fixes.is_empty());
    }

    #[test]
    fn generate_fixes_returns_empty_for_unknown() {
        let issue = Issue::new("unknown-issue-type", Severity::Info, "Test");
        let snapshot = minimal_snapshot();

        let fixes = generate_fixes(&issue, &snapshot);

        assert!(fixes.is_empty());
    }
}
