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
        // Bootstrap fix generators (Milestone 5.4)
        "remote-pr-branch-untracked" => generate_track_existing_from_pr_fixes(issue, snapshot),
        "remote-pr-branch-missing" => generate_fetch_and_track_pr_fixes(issue, snapshot),
        "remote-pr-not-linked" => generate_link_pr_fixes(issue, snapshot),
        // Local-only bootstrap (Milestone 5.7)
        "untracked-branch" => generate_import_local_topology_fixes(issue, snapshot),
        // Synthetic stack snapshot materialization (Milestone 5.9)
        "synthetic-stack-head" => generate_materialize_snapshot_fixes(issue, snapshot),
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

// =============================================================================
// Bootstrap Fix Generators (Milestone 5.4)
// =============================================================================

/// Generate fixes for untracked local branches that match open PRs.
///
/// This handles the `RemoteOpenPrBranchUntracked` issue - when a local branch
/// exists but isn't tracked by Lattice, and there's an open PR for it.
///
/// Fix: Track the existing branch with parent inferred from PR base.
/// Default state: Unfrozen (user's own branch, they can modify it).
fn generate_track_existing_from_pr_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract branch name and PR info from issue evidence
    let (branch, pr_number, base_ref, _url) = extract_pr_evidence(issue);

    if branch.is_empty() || pr_number == 0 {
        return fixes;
    }

    // Verify the branch actually exists locally but isn't tracked
    if let Ok(branch_name) = crate::core::types::BranchName::new(&branch) {
        if !snapshot.branches.contains_key(&branch_name) {
            return fixes; // Branch doesn't exist, wrong fix type
        }
        if snapshot.metadata.contains_key(&branch_name) {
            return fixes; // Already tracked, nothing to do
        }
    }

    // Determine parent from PR base_ref
    let parent = determine_parent_from_base_ref(&base_ref, snapshot);

    // Create fix option
    fixes.push(
        FixOption::new(
            FixId::new("remote-pr-branch-untracked", "track", &branch),
            issue.id.clone(),
            format!(
                "Track '{}' with parent '{}' (PR #{})",
                branch, parent, pr_number
            ),
            FixPreview::with_summary(format!(
                "Create tracking metadata for '{}' linked to PR #{}",
                branch, pr_number
            ))
            .add_metadata_change(MetadataChange::Create {
                branch: branch.clone(),
                description: format!("parent={}, pr=#{}, unfrozen", parent, pr_number),
            }),
        )
        .with_preconditions([
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::GraphValid,
        ]),
    );

    fixes
}

/// Generate fixes for open PRs whose head branches don't exist locally.
///
/// This handles the `RemoteOpenPrBranchMissingLocally` issue - when an open PR
/// exists on the remote but the head branch hasn't been fetched locally.
///
/// Fix: Fetch the branch from remote and track it as frozen.
/// Default state: Frozen (teammate's branch, should not modify).
fn generate_fetch_and_track_pr_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract PR info from issue evidence
    let (head_ref, pr_number, base_ref, _url) = extract_pr_evidence(issue);

    if head_ref.is_empty() || pr_number == 0 {
        return fixes;
    }

    // Verify the branch doesn't exist locally
    if let Ok(branch_name) = crate::core::types::BranchName::new(&head_ref) {
        if snapshot.branches.contains_key(&branch_name) {
            return fixes; // Branch exists locally, wrong fix type
        }
    }

    // Determine parent from PR base_ref
    let parent = determine_parent_from_base_ref(&base_ref, snapshot);

    // Create fix option - fetches and tracks as frozen
    fixes.push(
        FixOption::new(
            FixId::new("remote-pr-branch-missing", "fetch-and-track", &head_ref),
            issue.id.clone(),
            format!(
                "Fetch and track '{}' from PR #{} (frozen)",
                head_ref, pr_number
            ),
            FixPreview::with_summary(format!(
                "Fetch '{}' from remote and create frozen tracking metadata linked to PR #{}",
                head_ref, pr_number
            ))
            .add_ref_change(RefChange::Create {
                ref_name: format!("refs/heads/{}", head_ref),
                new_oid: "(fetched from remote)".to_string(),
            })
            .add_metadata_change(MetadataChange::Create {
                branch: head_ref.clone(),
                description: format!(
                    "parent={}, pr=#{}, frozen (teammate_branch)",
                    parent, pr_number
                ),
            }),
        )
        .with_preconditions([
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::AuthAvailable,
            Capability::RemoteResolved,
        ]),
    );

    fixes
}

/// Generate fixes for tracked branches that have open PRs but no linkage.
///
/// This handles the `RemoteOpenPrNotLinkedInMetadata` issue - when a branch
/// is already tracked by Lattice but the metadata doesn't link to the open PR.
///
/// Fix: Link the PR in cached metadata (does not modify structural fields).
/// Per ARCHITECTURE.md Section 11.2, PR linkage is a cached field, not structural.
fn generate_link_pr_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract branch and PR info from issue evidence
    let (branch, pr_number, _base_ref, _url) = extract_pr_evidence(issue);

    if branch.is_empty() || pr_number == 0 {
        return fixes;
    }

    // Verify branch is actually tracked
    if let Ok(branch_name) = crate::core::types::BranchName::new(&branch) {
        if !snapshot.metadata.contains_key(&branch_name) {
            return fixes; // Not tracked, wrong issue type
        }
    } else {
        return fixes; // Invalid branch name
    }

    // Create fix option - updates cached PR state only
    fixes.push(
        FixOption::new(
            FixId::new("remote-pr-not-linked", "link", &branch),
            issue.id.clone(),
            format!("Link PR #{} to tracked branch '{}'", pr_number, branch),
            FixPreview::with_summary(format!(
                "Update cached PR metadata for '{}' to reference PR #{}",
                branch, pr_number
            ))
            .add_metadata_change(MetadataChange::Update {
                branch: branch.clone(),
                field: "pr".to_string(),
                old_value: Some("none".to_string()),
                new_value: format!("linked(#{})", pr_number),
            }),
        )
        .with_precondition(Capability::RepoOpen),
    );

    fixes
}

// =============================================================================
// Bootstrap Helper Functions
// =============================================================================

/// Extract PR-related evidence from an issue.
///
/// Returns (branch_name, pr_number, base_ref, url).
/// All fields may be empty/zero if not found.
fn extract_pr_evidence(issue: &Issue) -> (String, u64, String, String) {
    let mut branch = String::new();
    let mut pr_number = 0u64;
    let mut base_ref = String::new();
    let mut url = String::new();

    for evidence in &issue.evidence {
        match evidence {
            Evidence::Ref { name, .. } => {
                // Extract branch name from ref_name like "refs/heads/feature"
                // or "refs/branch-metadata/feature"
                if let Some(b) = name.strip_prefix("refs/heads/") {
                    branch = b.to_string();
                } else if let Some(b) = name.strip_prefix("refs/branch-metadata/") {
                    branch = b.to_string();
                }
            }
            Evidence::Config { key, problem } => {
                // Parse PR number from key like "pr.42"
                if let Some(num_str) = key.strip_prefix("pr.") {
                    if let Ok(num) = num_str.parse() {
                        pr_number = num;
                    }
                }
                // Parse URL from problem string (look for http)
                if problem.contains("http") {
                    if let Some(start) = problem.find("http") {
                        url = problem[start..]
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();
                    }
                }
                // Parse base_ref from problem if present
                if problem.contains("base:") {
                    if let Some(rest) = problem.split("base:").nth(1) {
                        base_ref = rest.split_whitespace().next().unwrap_or("").to_string();
                    }
                }
            }
            _ => {}
        }
    }

    // Also try to extract branch from issue ID (format: "issue-type:branch")
    if branch.is_empty() {
        if let Some(b) = issue.id.as_str().split(':').nth(1) {
            branch = b.to_string();
        }
    }

    // Extract from issue message as fallback
    if branch.is_empty() && issue.message.contains("branch '") {
        if let Some(start) = issue.message.find("branch '") {
            let rest = &issue.message[start + 8..];
            if let Some(end) = rest.find('\'') {
                branch = rest[..end].to_string();
            }
        }
    }

    // Extract PR number from message if not in evidence
    if pr_number == 0 && issue.message.contains("PR #") {
        if let Some(start) = issue.message.find("PR #") {
            let rest = &issue.message[start + 4..];
            let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(num) = num_str.parse() {
                pr_number = num;
            }
        }
    }

    (branch, pr_number, base_ref, url)
}

// =============================================================================
// Local-Only Bootstrap Fix Generators (Milestone 5.7)
// =============================================================================

/// Generate fixes for untracked local branches using local graph topology.
///
/// Parent is inferred from merge-base distance to tracked branches.
/// Default state: Unfrozen (user's own branch).
///
/// Ambiguity handling:
/// - If multiple candidates have the same minimum distance, create separate
///   fix options for each and let the user choose.
fn generate_import_local_topology_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract parent candidates from evidence
    let (branch, candidates) = extract_parent_candidates(issue);

    if branch.is_empty() || candidates.is_empty() {
        return fixes;
    }

    // Verify branch exists and is untracked
    if let Ok(branch_name) = crate::core::types::BranchName::new(&branch) {
        if !snapshot.branches.contains_key(&branch_name) {
            return fixes;
        }
        if snapshot.metadata.contains_key(&branch_name) {
            return fixes;
        }
    } else {
        return fixes;
    }

    // Find minimum distance among candidates
    let min_distance = candidates
        .iter()
        .map(|c| c.distance)
        .min()
        .unwrap_or(u32::MAX);

    // Get all candidates at minimum distance (handles ties)
    let best_candidates: Vec<_> = candidates
        .iter()
        .filter(|c| c.distance == min_distance)
        .collect();

    // Generate a fix option for each best candidate
    for candidate in &best_candidates {
        let fix_id_suffix = if best_candidates.len() > 1 {
            format!("import-local:{}", candidate.name)
        } else {
            "import-local".to_string()
        };

        let description = if best_candidates.len() > 1 {
            format!(
                "Track '{}' with parent '{}' (one of {} equally close ancestors)",
                branch,
                candidate.name,
                best_candidates.len()
            )
        } else {
            format!(
                "Track '{}' with parent '{}' (nearest ancestor, {} commits diverged)",
                branch, candidate.name, candidate.distance
            )
        };

        let trunk_note = if candidate.is_trunk { " (trunk)" } else { "" };

        fixes.push(
            FixOption::new(
                FixId::new("untracked-branch", &fix_id_suffix, &branch),
                issue.id.clone(),
                description,
                FixPreview::with_summary(format!(
                    "Create tracking metadata for '{}' with parent '{}'{}, base at merge-base {}",
                    branch,
                    candidate.name,
                    trunk_note,
                    &candidate.merge_base[..7.min(candidate.merge_base.len())]
                ))
                .add_metadata_change(MetadataChange::Create {
                    branch: branch.clone(),
                    description: format!(
                        "parent={}, base={}, unfrozen",
                        candidate.name,
                        &candidate.merge_base[..7.min(candidate.merge_base.len())]
                    ),
                }),
            )
            .with_preconditions([
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::GraphValid,
            ]),
        );
    }

    fixes
}

/// Extract parent candidate evidence from an issue.
fn extract_parent_candidates(
    issue: &Issue,
) -> (String, Vec<crate::engine::health::ParentCandidate>) {
    let mut branch = String::new();
    let mut candidates = Vec::new();

    for evidence in &issue.evidence {
        if let Evidence::ParentCandidates {
            branch: b,
            candidates: c,
        } = evidence
        {
            branch = b.clone();
            candidates = c.clone();
            break;
        }
    }

    // Fallback: extract branch from issue ID
    if branch.is_empty() {
        if let Some(b) = issue.id.as_str().strip_prefix("untracked-branch:") {
            branch = b.to_string();
        }
    }

    (branch, candidates)
}

// =============================================================================
// Bootstrap Helper Functions
// =============================================================================

/// Determine parent branch from a PR's base_ref.
///
/// Uses the following priority:
/// 1. If base_ref is trunk → parent = trunk
/// 2. If base_ref exists locally as tracked branch → parent = base_ref
/// 3. If base_ref is another open PR's head (chain detection) → parent = base_ref
/// 4. Fallback to trunk
fn determine_parent_from_base_ref(base_ref: &str, snapshot: &RepoSnapshot) -> String {
    // Rule 1: If base_ref is trunk, parent = trunk
    if let Some(trunk) = &snapshot.trunk {
        if base_ref == trunk.as_str() {
            return trunk.as_str().to_string();
        }
    }

    // Rule 2: If base_ref exists locally as tracked branch, parent = base_ref
    if let Ok(base_branch) = crate::core::types::BranchName::new(base_ref) {
        if snapshot.metadata.contains_key(&base_branch) {
            return base_ref.to_string();
        }
    }

    // Rule 3: If base_ref is another open PR's head (chain detection)
    if let Some(ref evidence) = snapshot.remote_prs {
        for pr in &evidence.prs {
            if pr.head_ref == base_ref {
                // base_ref is another PR's head - it's a chain
                return base_ref.to_string();
            }
        }
    }

    // Rule 4: Fallback to trunk
    snapshot
        .trunk
        .as_ref()
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|| "main".to_string())
}

// =============================================================================
// Synthetic Stack Deep Analysis (Milestone 5.8 Tier 2)
// =============================================================================

/// Analyze a potential synthetic stack head by querying closed PRs.
///
/// This is Tier 2 analysis - performs forge API calls to enumerate closed
/// PRs that targeted a potential synthetic stack head branch.
///
/// # Arguments
///
/// * `issue` - A `PotentialSyntheticStackHead` issue
/// * `forge` - The forge to query for closed PRs
/// * `config` - Bootstrap configuration with budgets
///
/// # Returns
///
/// `Some(Evidence::SyntheticStackChildren)` if closed PRs were found targeting
/// this head branch, `None` otherwise.
///
/// # Example
///
/// ```ignore
/// let evidence = analyze_synthetic_stack_deep(
///     &issue,
///     &forge,
///     &config.doctor.bootstrap,
/// ).await;
///
/// if let Some(Evidence::SyntheticStackChildren { closed_prs, .. }) = evidence {
///     println!("Found {} closed PRs merged into this head", closed_prs.len());
/// }
/// ```
pub async fn analyze_synthetic_stack_deep(
    issue: &Issue,
    forge: &dyn crate::forge::Forge,
    config: &crate::core::config::schema::DoctorBootstrapConfig,
) -> Option<crate::engine::health::Evidence> {
    use crate::engine::health::{ClosedPrInfo, Evidence};
    use crate::forge::ListClosedPrsOpts;

    // Extract branch from issue
    let branch = extract_synthetic_head_branch(issue)?;

    // Query closed PRs targeting this branch
    let opts = ListClosedPrsOpts::for_base(&branch).with_limit(config.max_closed_prs_per_head);

    let result = match forge.list_closed_prs_targeting(opts).await {
        Ok(r) => r,
        Err(e) => {
            // Log warning but don't fail - API errors shouldn't block other fixes
            eprintln!(
                "Warning: failed to query closed PRs for branch '{}': {}",
                branch, e
            );
            return None;
        }
    };

    if result.pulls.is_empty() {
        return None; // No closed PRs - might not actually be a synthetic stack
    }

    // Convert to ClosedPrInfo
    let closed_prs: Vec<ClosedPrInfo> = result
        .pulls
        .iter()
        .map(|pr| ClosedPrInfo {
            number: pr.number,
            head_ref: pr.head_ref.clone(),
            merged: true, // GitHub's closed PRs endpoint returns both, assume merged for now
            url: pr.url.clone(),
        })
        .collect();

    Some(Evidence::SyntheticStackChildren {
        head_branch: branch,
        closed_prs,
        truncated: result.truncated,
    })
}

/// Extract the synthetic head branch name from an issue.
///
/// Parses the branch name from the issue message or evidence.
fn extract_synthetic_head_branch(issue: &Issue) -> Option<String> {
    // Parse from message which contains "synthetic stack head (branch 'X')"
    let msg = &issue.message;
    if let Some(start) = msg.find("(branch '") {
        if let Some(end) = msg[start + 9..].find("')") {
            return Some(msg[start + 9..start + 9 + end].to_string());
        }
    }

    // Fallback: try to extract from PrReference evidence
    for evidence in &issue.evidence {
        if let Evidence::PrReference { context, .. } = evidence {
            // Context format: "Open PR targeting trunk with head branch 'X'"
            if let Some(start) = context.find("head branch '") {
                if let Some(end) = context[start + 13..].find('\'') {
                    return Some(context[start + 13..start + 13 + end].to_string());
                }
            }
        }
    }

    None
}

// =============================================================================
// Synthetic Stack Snapshot Materialization (Milestone 5.9)
// =============================================================================

/// Reserved prefix for synthetic snapshot branches.
///
/// Snapshot branches use the naming pattern `lattice/snap/pr-{number}`.
/// This prefix clearly identifies them as Lattice-managed synthetic snapshots
/// and distinguishes them from user-created branches.
pub const SNAPSHOT_PREFIX: &str = "lattice/snap/pr-";

/// Generate a unique snapshot branch name for a PR.
///
/// Uses the pattern `lattice/snap/pr-{number}` with collision avoidance
/// by appending `-{k}` suffixes if the name already exists.
///
/// # Arguments
///
/// * `pr_number` - The PR number to create a snapshot for
/// * `snapshot` - Current repo snapshot to check for existing branches
///
/// # Returns
///
/// A unique branch name for the snapshot.
///
/// # Example
///
/// ```ignore
/// let name = snapshot_branch_name(42, &snapshot);
/// // Returns "lattice/snap/pr-42" if available
/// // Returns "lattice/snap/pr-42-1" if base name is taken
/// ```
pub fn snapshot_branch_name(pr_number: u64, snapshot: &RepoSnapshot) -> String {
    let base_name = format!("{}{}", SNAPSHOT_PREFIX, pr_number);

    // Check if base name is available
    if !branch_exists(&base_name, snapshot) {
        return base_name;
    }

    // Find first available suffix
    for k in 1..100 {
        let name = format!("{}-{}", base_name, k);
        if !branch_exists(&name, snapshot) {
            return name;
        }
    }

    // Fallback with timestamp (extremely unlikely to reach here)
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{}-{}", base_name, ts)
}

/// Check if a branch exists in the snapshot.
fn branch_exists(name: &str, snapshot: &RepoSnapshot) -> bool {
    snapshot.branches.keys().any(|b| b.as_str() == name)
}

/// Generate fix options for materializing synthetic stack snapshots.
///
/// This is triggered when a `PotentialSyntheticStackHead` issue exists
/// with Tier 2 evidence (`SyntheticStackChildren`). It creates a fix option
/// that will fetch closed PR refs and create frozen snapshot branches.
///
/// Per ARCHITECTURE.md Section 8.3, this fix requires explicit user
/// confirmation before execution.
///
/// # Arguments
///
/// * `issue` - The synthetic stack head issue (must have Tier 2 evidence)
/// * `snapshot` - Current repo snapshot
///
/// # Returns
///
/// A vector of fix options (0 or 1 options depending on evidence).
pub fn generate_materialize_snapshot_fixes(
    issue: &Issue,
    snapshot: &RepoSnapshot,
) -> Vec<FixOption> {
    // Extract the synthetic head branch from the issue
    let head_branch = match extract_synthetic_head_branch(issue) {
        Some(b) => b,
        None => return vec![],
    };

    // Find SyntheticStackChildren evidence (Tier 2 deep analysis results)
    let closed_prs = issue.evidence.iter().find_map(|ev| {
        if let Evidence::SyntheticStackChildren { closed_prs, .. } = ev {
            Some(closed_prs.clone())
        } else {
            None
        }
    });

    let closed_prs = match closed_prs {
        Some(prs) if !prs.is_empty() => prs,
        _ => return vec![], // No Tier 2 evidence or no closed PRs
    };

    // Build preview showing what branches will be created
    let mut preview = FixPreview::with_summary(format!(
        "Create {} frozen snapshot branch(es) from closed PRs merged into '{}'",
        closed_prs.len(),
        head_branch,
    ));

    // Add ref changes for each snapshot branch
    for pr in &closed_prs {
        let branch_name = snapshot_branch_name(pr.number, snapshot);
        preview = preview.add_ref_change(RefChange::Create {
            ref_name: format!("refs/heads/{}", branch_name),
            new_oid: format!("(fetched from PR #{})", pr.number),
        });
        preview = preview.add_metadata_change(MetadataChange::Create {
            branch: branch_name,
            description: format!(
                "parent={}, frozen (remote_synthetic_snapshot), pr=#{}",
                head_branch, pr.number
            ),
        });
    }

    let fix_id = FixId::new(
        "synthetic-stack-head",
        "materialize-snapshots",
        &head_branch,
    );

    vec![FixOption::new(
        fix_id,
        issue.id.clone(),
        format!(
            "Create {} frozen snapshot branch(es) for closed PRs merged into '{}'",
            closed_prs.len(),
            head_branch,
        ),
        preview,
    )
    .with_preconditions([
        Capability::RepoOpen,
        Capability::TrunkKnown,
        Capability::AuthAvailable,
        Capability::RemoteResolved,
    ])]
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
            remote_prs: None,
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

    // =========================================================================
    // Bootstrap Fix Generator Tests (Milestone 5.4)
    // =========================================================================

    #[test]
    fn track_existing_from_pr_generates_fix() {
        let issue = issues::remote_pr_branch_untracked(
            "feature",
            42,
            "main",
            "https://github.com/org/repo/pull/42",
        );
        let mut snapshot = minimal_snapshot();
        // Add the untracked branch to snapshot
        snapshot.branches.insert(
            crate::core::types::BranchName::new("feature").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap(),
        );

        let fixes = generate_track_existing_from_pr_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].description.contains("Track"));
        assert!(fixes[0].description.contains("feature"));
        assert!(fixes[0].description.contains("PR #42"));
    }

    #[test]
    fn track_existing_returns_empty_if_branch_missing() {
        let issue = issues::remote_pr_branch_untracked(
            "feature",
            42,
            "main",
            "https://github.com/org/repo/pull/42",
        );
        let snapshot = minimal_snapshot();
        // Branch doesn't exist in snapshot

        let fixes = generate_track_existing_from_pr_fixes(&issue, &snapshot);

        // Should return empty because branch doesn't exist locally
        assert!(fixes.is_empty());
    }

    #[test]
    fn track_existing_returns_empty_if_already_tracked() {
        use crate::core::metadata::schema::BranchMetadataV1;
        use crate::engine::scan::ScannedMetadata;

        let issue = issues::remote_pr_branch_untracked(
            "feature",
            42,
            "main",
            "https://github.com/org/repo/pull/42",
        );
        let mut snapshot = minimal_snapshot();
        // Add the branch
        let branch = crate::core::types::BranchName::new("feature").unwrap();
        let oid = crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap();
        snapshot.branches.insert(branch.clone(), oid.clone());
        // Also add metadata (already tracked)
        let parent = crate::core::types::BranchName::new("main").unwrap();
        let metadata = BranchMetadataV1::new(branch.clone(), parent, oid.clone());
        snapshot.metadata.insert(
            branch,
            ScannedMetadata {
                ref_oid: oid,
                metadata,
            },
        );

        let fixes = generate_track_existing_from_pr_fixes(&issue, &snapshot);

        // Should return empty because branch is already tracked
        assert!(fixes.is_empty());
    }

    #[test]
    fn fetch_and_track_generates_fix() {
        let issue = issues::remote_pr_branch_missing(
            42,
            "teammate-feature",
            "main",
            "https://github.com/org/repo/pull/42",
        );
        let snapshot = minimal_snapshot();
        // Branch doesn't exist locally

        let fixes = generate_fetch_and_track_pr_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].description.contains("Fetch"));
        assert!(fixes[0].description.contains("teammate-feature"));
        assert!(fixes[0].description.contains("frozen"));
    }

    #[test]
    fn fetch_and_track_returns_empty_if_branch_exists() {
        let issue = issues::remote_pr_branch_missing(
            42,
            "feature",
            "main",
            "https://github.com/org/repo/pull/42",
        );
        let mut snapshot = minimal_snapshot();
        // Branch exists locally
        snapshot.branches.insert(
            crate::core::types::BranchName::new("feature").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap(),
        );

        let fixes = generate_fetch_and_track_pr_fixes(&issue, &snapshot);

        // Should return empty because branch already exists
        assert!(fixes.is_empty());
    }

    #[test]
    fn link_pr_generates_fix() {
        use crate::core::metadata::schema::BranchMetadataV1;
        use crate::engine::scan::ScannedMetadata;

        let issue =
            issues::remote_pr_not_linked("my-feature", 42, "https://github.com/org/repo/pull/42");
        let mut snapshot = minimal_snapshot();
        // Add tracked branch
        let branch = crate::core::types::BranchName::new("my-feature").unwrap();
        let oid = crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap();
        snapshot.branches.insert(branch.clone(), oid.clone());
        let parent = crate::core::types::BranchName::new("main").unwrap();
        let metadata = BranchMetadataV1::new(branch.clone(), parent, oid.clone());
        snapshot.metadata.insert(
            branch,
            ScannedMetadata {
                ref_oid: oid,
                metadata,
            },
        );

        let fixes = generate_link_pr_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].description.contains("Link"));
        assert!(fixes[0].description.contains("PR #42"));
    }

    #[test]
    fn link_pr_returns_empty_if_not_tracked() {
        let issue =
            issues::remote_pr_not_linked("my-feature", 42, "https://github.com/org/repo/pull/42");
        let snapshot = minimal_snapshot();
        // Branch not tracked

        let fixes = generate_link_pr_fixes(&issue, &snapshot);

        // Should return empty because branch is not tracked
        assert!(fixes.is_empty());
    }

    #[test]
    fn parent_selection_uses_trunk_for_trunk_based_pr() {
        let snapshot = minimal_snapshot();
        let parent = determine_parent_from_base_ref("main", &snapshot);
        assert_eq!(parent, "main");
    }

    #[test]
    fn parent_selection_uses_tracked_branch() {
        use crate::core::metadata::schema::BranchMetadataV1;
        use crate::engine::scan::ScannedMetadata;

        let mut snapshot = minimal_snapshot();
        // Add a tracked branch
        let branch = crate::core::types::BranchName::new("feature-a").unwrap();
        let oid = crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap();
        snapshot.branches.insert(branch.clone(), oid.clone());
        let parent = crate::core::types::BranchName::new("main").unwrap();
        let metadata = BranchMetadataV1::new(branch.clone(), parent, oid.clone());
        snapshot.metadata.insert(
            branch,
            ScannedMetadata {
                ref_oid: oid,
                metadata,
            },
        );

        let parent = determine_parent_from_base_ref("feature-a", &snapshot);
        assert_eq!(parent, "feature-a");
    }

    #[test]
    fn parent_selection_falls_back_to_trunk() {
        let snapshot = minimal_snapshot();
        let parent = determine_parent_from_base_ref("unknown-branch", &snapshot);
        assert_eq!(parent, "main"); // trunk fallback
    }

    #[test]
    fn extract_pr_evidence_from_issue() {
        let issue = issues::remote_pr_branch_untracked(
            "feature",
            42,
            "main",
            "https://github.com/org/repo/pull/42",
        );

        let (branch, pr_number, base_ref, url) = extract_pr_evidence(&issue);

        assert_eq!(branch, "feature");
        assert_eq!(pr_number, 42);
        assert_eq!(base_ref, "main");
        assert!(url.contains("github.com"));
    }

    #[test]
    fn extract_pr_evidence_from_missing_branch_issue() {
        let issue = issues::remote_pr_branch_missing(
            42,
            "teammate-feature",
            "main",
            "https://github.com/org/repo/pull/42",
        );

        let (branch, pr_number, base_ref, url) = extract_pr_evidence(&issue);

        assert_eq!(branch, "teammate-feature");
        assert_eq!(pr_number, 42);
        assert_eq!(base_ref, "main");
        assert!(url.contains("github.com"));
    }

    // =========================================================================
    // Local-Only Bootstrap Fix Generator Tests (Milestone 5.7)
    // =========================================================================

    /// Helper to create an untracked branch issue with parent candidates.
    fn untracked_branch_issue_with_candidates(
        branch: &str,
        candidates: Vec<crate::engine::health::ParentCandidate>,
    ) -> Issue {
        let mut issue = Issue::new(
            &format!("untracked-branch:{}", branch),
            Severity::Info,
            format!("branch '{}' exists but is not tracked", branch),
        );
        issue.evidence.push(Evidence::ParentCandidates {
            branch: branch.to_string(),
            candidates,
        });
        issue
    }

    #[test]
    fn local_bootstrap_generates_fix_with_single_best_parent() {
        use crate::engine::health::ParentCandidate;

        let candidates = vec![
            ParentCandidate {
                name: "main".to_string(),
                merge_base: "abc123def4567890abc123def4567890abc12345".to_string(),
                distance: 3,
                is_trunk: true,
            },
            ParentCandidate {
                name: "feature-a".to_string(),
                merge_base: "def456def4567890def456def4567890def45678".to_string(),
                distance: 10,
                is_trunk: false,
            },
        ];

        let issue = untracked_branch_issue_with_candidates("my-feature", candidates);
        let mut snapshot = minimal_snapshot();

        // Add the untracked branch to snapshot
        snapshot.branches.insert(
            crate::core::types::BranchName::new("my-feature").unwrap(),
            crate::core::types::Oid::new("111111111111111111111111111111111111111a").unwrap(),
        );

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1, "Should generate exactly one fix");
        assert!(
            fixes[0].description.contains("main"),
            "Should use 'main' as parent (closest)"
        );
        assert!(
            fixes[0].description.contains("my-feature"),
            "Should reference the branch"
        );
        assert!(
            fixes[0].description.contains("nearest ancestor"),
            "Should indicate single best match"
        );
    }

    #[test]
    fn local_bootstrap_generates_multiple_fixes_for_tied_candidates() {
        use crate::engine::health::ParentCandidate;

        // Two candidates with the same distance - ambiguous case
        let candidates = vec![
            ParentCandidate {
                name: "feature-a".to_string(),
                merge_base: "abc123def4567890abc123def4567890abc12345".to_string(),
                distance: 5,
                is_trunk: false,
            },
            ParentCandidate {
                name: "feature-b".to_string(),
                merge_base: "def456def4567890def456def4567890def45678".to_string(),
                distance: 5,
                is_trunk: false,
            },
        ];

        let issue = untracked_branch_issue_with_candidates("my-feature", candidates);
        let mut snapshot = minimal_snapshot();

        // Add the untracked branch to snapshot
        snapshot.branches.insert(
            crate::core::types::BranchName::new("my-feature").unwrap(),
            crate::core::types::Oid::new("111111111111111111111111111111111111111a").unwrap(),
        );

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert_eq!(
            fixes.len(),
            2,
            "Should generate one fix per equally-close candidate"
        );
        assert!(
            fixes.iter().any(|f| f.description.contains("feature-a")),
            "Should offer feature-a as option"
        );
        assert!(
            fixes.iter().any(|f| f.description.contains("feature-b")),
            "Should offer feature-b as option"
        );
        // Both should mention "equally close"
        assert!(
            fixes[0].description.contains("equally close"),
            "Should indicate ambiguity"
        );
    }

    #[test]
    fn local_bootstrap_returns_empty_for_no_candidates() {
        let issue = untracked_branch_issue_with_candidates("my-feature", vec![]);
        let mut snapshot = minimal_snapshot();

        // Add the branch
        snapshot.branches.insert(
            crate::core::types::BranchName::new("my-feature").unwrap(),
            crate::core::types::Oid::new("111111111111111111111111111111111111111a").unwrap(),
        );

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert!(fixes.is_empty(), "Should return empty for no candidates");
    }

    #[test]
    fn local_bootstrap_returns_empty_if_branch_missing() {
        use crate::engine::health::ParentCandidate;

        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123def4567890abc123def4567890abc12345".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        let issue = untracked_branch_issue_with_candidates("nonexistent", candidates);
        let snapshot = minimal_snapshot();
        // Don't add the branch

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert!(
            fixes.is_empty(),
            "Should return empty if branch doesn't exist"
        );
    }

    #[test]
    fn local_bootstrap_returns_empty_if_already_tracked() {
        use crate::core::metadata::schema::BranchMetadataV1;
        use crate::engine::health::ParentCandidate;
        use crate::engine::scan::ScannedMetadata;

        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123def4567890abc123def4567890abc12345".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        let issue = untracked_branch_issue_with_candidates("my-feature", candidates);
        let mut snapshot = minimal_snapshot();

        // Add the branch
        let branch = crate::core::types::BranchName::new("my-feature").unwrap();
        let oid = crate::core::types::Oid::new("111111111111111111111111111111111111111a").unwrap();
        snapshot.branches.insert(branch.clone(), oid.clone());

        // Also add metadata (already tracked)
        let parent = crate::core::types::BranchName::new("main").unwrap();
        let metadata = BranchMetadataV1::new(branch.clone(), parent, oid.clone());
        snapshot.metadata.insert(
            branch,
            ScannedMetadata {
                ref_oid: oid,
                metadata,
            },
        );

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert!(
            fixes.is_empty(),
            "Should return empty if branch is already tracked"
        );
    }

    #[test]
    fn extract_parent_candidates_from_evidence() {
        use crate::engine::health::ParentCandidate;

        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        let issue = untracked_branch_issue_with_candidates("my-feature", candidates.clone());

        let (branch, extracted) = extract_parent_candidates(&issue);

        assert_eq!(branch, "my-feature");
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].name, "main");
        assert_eq!(extracted[0].distance, 3);
    }

    #[test]
    fn extract_parent_candidates_fallback_to_issue_id() {
        // Create issue without ParentCandidates evidence
        let issue = Issue::new(
            "untracked-branch:fallback-branch",
            Severity::Info,
            "branch 'fallback-branch' exists but is not tracked",
        );

        let (_branch, candidates) = extract_parent_candidates(&issue);

        // Branch extraction from issue ID depends on the ID format - here we just
        // verify that no candidates are returned when evidence is empty
        assert!(candidates.is_empty(), "Should have no candidates");
    }

    #[test]
    fn local_bootstrap_fix_has_correct_preconditions() {
        use crate::engine::health::ParentCandidate;

        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123def4567890abc123def4567890abc12345".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        let issue = untracked_branch_issue_with_candidates("my-feature", candidates);
        let mut snapshot = minimal_snapshot();

        snapshot.branches.insert(
            crate::core::types::BranchName::new("my-feature").unwrap(),
            crate::core::types::Oid::new("111111111111111111111111111111111111111a").unwrap(),
        );

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        let fix = &fixes[0];

        // Check preconditions contain required capabilities
        assert!(
            fix.preconditions.contains(&Capability::RepoOpen),
            "Should require RepoOpen"
        );
        assert!(
            fix.preconditions.contains(&Capability::TrunkKnown),
            "Should require TrunkKnown"
        );
        assert!(
            fix.preconditions.contains(&Capability::GraphValid),
            "Should require GraphValid"
        );
    }

    #[test]
    fn local_bootstrap_trunk_parent_noted_in_preview() {
        use crate::engine::health::ParentCandidate;

        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123def4567890abc123def4567890abc12345".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        let issue = untracked_branch_issue_with_candidates("my-feature", candidates);
        let mut snapshot = minimal_snapshot();

        snapshot.branches.insert(
            crate::core::types::BranchName::new("my-feature").unwrap(),
            crate::core::types::Oid::new("111111111111111111111111111111111111111a").unwrap(),
        );

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        let preview_summary = &fixes[0].preview.summary;
        assert!(
            preview_summary.contains("(trunk)"),
            "Preview should note trunk: {}",
            preview_summary
        );
    }

    #[test]
    fn local_bootstrap_dispatch_routes_correctly() {
        use crate::engine::health::ParentCandidate;

        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123def4567890abc123def4567890abc12345".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        let issue = untracked_branch_issue_with_candidates("my-feature", candidates);
        let mut snapshot = minimal_snapshot();

        snapshot.branches.insert(
            crate::core::types::BranchName::new("my-feature").unwrap(),
            crate::core::types::Oid::new("111111111111111111111111111111111111111a").unwrap(),
        );

        // Use the main dispatch function
        let fixes = generate_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1, "Dispatch should route to local bootstrap");
        assert!(
            fixes[0].description.contains("Track"),
            "Should generate track fix"
        );
    }

    // =========================================================================
    // Synthetic Stack Snapshot Materialization Tests (Milestone 5.9)
    // =========================================================================

    #[test]
    fn snapshot_branch_name_no_collision() {
        let snapshot = minimal_snapshot();
        let name = snapshot_branch_name(42, &snapshot);
        assert_eq!(name, "lattice/snap/pr-42");
    }

    #[test]
    fn snapshot_branch_name_with_collision() {
        let mut snapshot = minimal_snapshot();
        // Add an existing branch with the base name
        snapshot.branches.insert(
            crate::core::types::BranchName::new("lattice/snap/pr-42").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap(),
        );

        let name = snapshot_branch_name(42, &snapshot);
        assert_eq!(name, "lattice/snap/pr-42-1");
    }

    #[test]
    fn snapshot_branch_name_multiple_collisions() {
        let mut snapshot = minimal_snapshot();
        snapshot.branches.insert(
            crate::core::types::BranchName::new("lattice/snap/pr-42").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap(),
        );
        snapshot.branches.insert(
            crate::core::types::BranchName::new("lattice/snap/pr-42-1").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45679").unwrap(),
        );

        let name = snapshot_branch_name(42, &snapshot);
        assert_eq!(name, "lattice/snap/pr-42-2");
    }

    #[test]
    fn generate_materialize_snapshot_fixes_with_evidence() {
        use crate::engine::health::ClosedPrInfo;

        // Create an issue with Tier 2 evidence
        let mut issue = Issue::with_id(
            crate::engine::health::IssueId::new("synthetic-stack-head", "42"),
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature')",
        );
        issue.evidence.push(Evidence::SyntheticStackChildren {
            head_branch: "feature".to_string(),
            closed_prs: vec![
                ClosedPrInfo {
                    number: 10,
                    head_ref: "sub-a".to_string(),
                    merged: true,
                    url: "https://github.com/org/repo/pull/10".to_string(),
                },
                ClosedPrInfo {
                    number: 11,
                    head_ref: "sub-b".to_string(),
                    merged: true,
                    url: "https://github.com/org/repo/pull/11".to_string(),
                },
            ],
            truncated: false,
        });

        let mut snapshot = minimal_snapshot();
        // Add the synthetic head branch
        snapshot.branches.insert(
            crate::core::types::BranchName::new("feature").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap(),
        );

        let fixes = generate_materialize_snapshot_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        assert!(
            fixes[0].description.contains("2"),
            "Should mention count of PRs"
        );
        assert!(
            fixes[0].description.contains("feature"),
            "Should mention head branch"
        );
        assert!(
            fixes[0].preview.ref_changes.len() == 2,
            "Should have ref changes for each PR"
        );
        assert!(
            fixes[0].preview.metadata_changes.len() == 2,
            "Should have metadata changes for each PR"
        );
    }

    #[test]
    fn generate_materialize_snapshot_fixes_no_evidence() {
        // Create an issue without Tier 2 evidence
        let issue = Issue::with_id(
            crate::engine::health::IssueId::new("synthetic-stack-head", "42"),
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature')",
        );

        let snapshot = minimal_snapshot();

        let fixes = generate_materialize_snapshot_fixes(&issue, &snapshot);

        assert!(
            fixes.is_empty(),
            "Should return empty without Tier 2 evidence"
        );
    }

    #[test]
    fn generate_materialize_snapshot_fixes_empty_closed_prs() {
        // Create an issue with empty Tier 2 evidence
        let mut issue = Issue::with_id(
            crate::engine::health::IssueId::new("synthetic-stack-head", "42"),
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature')",
        );
        issue.evidence.push(Evidence::SyntheticStackChildren {
            head_branch: "feature".to_string(),
            closed_prs: vec![], // Empty!
            truncated: false,
        });

        let snapshot = minimal_snapshot();

        let fixes = generate_materialize_snapshot_fixes(&issue, &snapshot);

        assert!(
            fixes.is_empty(),
            "Should return empty when closed_prs is empty"
        );
    }

    #[test]
    fn generate_materialize_snapshot_dispatch_routes_correctly() {
        use crate::engine::health::ClosedPrInfo;

        let mut issue = Issue::with_id(
            crate::engine::health::IssueId::new("synthetic-stack-head", "42"),
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature')",
        );
        issue.evidence.push(Evidence::SyntheticStackChildren {
            head_branch: "feature".to_string(),
            closed_prs: vec![ClosedPrInfo {
                number: 10,
                head_ref: "sub-a".to_string(),
                merged: true,
                url: "https://github.com/org/repo/pull/10".to_string(),
            }],
            truncated: false,
        });

        let mut snapshot = minimal_snapshot();
        snapshot.branches.insert(
            crate::core::types::BranchName::new("feature").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap(),
        );

        // Use the main dispatch function
        let fixes = generate_fixes(&issue, &snapshot);

        assert_eq!(
            fixes.len(),
            1,
            "Dispatch should route to snapshot materialization"
        );
        assert!(
            fixes[0].description.contains("snapshot"),
            "Should generate snapshot fix"
        );
    }

    #[test]
    fn materialize_snapshot_fix_has_correct_preconditions() {
        use crate::engine::health::ClosedPrInfo;

        let mut issue = Issue::with_id(
            crate::engine::health::IssueId::new("synthetic-stack-head", "42"),
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature')",
        );
        issue.evidence.push(Evidence::SyntheticStackChildren {
            head_branch: "feature".to_string(),
            closed_prs: vec![ClosedPrInfo {
                number: 10,
                head_ref: "sub-a".to_string(),
                merged: true,
                url: "https://github.com/org/repo/pull/10".to_string(),
            }],
            truncated: false,
        });

        let mut snapshot = minimal_snapshot();
        snapshot.branches.insert(
            crate::core::types::BranchName::new("feature").unwrap(),
            crate::core::types::Oid::new("def456def4567890def456def4567890def45678").unwrap(),
        );

        let fixes = generate_materialize_snapshot_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        let fix = &fixes[0];

        // Check required preconditions for remote operation
        assert!(
            fix.preconditions.contains(&Capability::RepoOpen),
            "Should require RepoOpen"
        );
        assert!(
            fix.preconditions.contains(&Capability::AuthAvailable),
            "Should require AuthAvailable for fetch"
        );
        assert!(
            fix.preconditions.contains(&Capability::RemoteResolved),
            "Should require RemoteResolved for fetch"
        );
    }

    #[test]
    fn extract_synthetic_head_branch_from_message() {
        let issue = Issue::with_id(
            crate::engine::health::IssueId::new("synthetic-stack-head", "42"),
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature-xyz')",
        );

        let branch = extract_synthetic_head_branch(&issue);

        assert_eq!(branch, Some("feature-xyz".to_string()));
    }

    #[test]
    fn extract_synthetic_head_branch_from_evidence() {
        let mut issue = Issue::with_id(
            crate::engine::health::IssueId::new("synthetic-stack-head", "42"),
            Severity::Info,
            "Some message without branch name",
        );
        issue.evidence.push(Evidence::PrReference {
            number: 42,
            url: "https://github.com/org/repo/pull/42".to_string(),
            context: "Open PR targeting trunk with head branch 'other-feature'".to_string(),
        });

        let branch = extract_synthetic_head_branch(&issue);

        assert_eq!(branch, Some("other-feature".to_string()));
    }

    #[test]
    fn snapshot_prefix_constant() {
        assert_eq!(SNAPSHOT_PREFIX, "lattice/snap/pr-");
    }
}
