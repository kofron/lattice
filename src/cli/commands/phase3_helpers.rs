// Legacy journal API - these commands will be migrated to executor pattern
#![allow(deprecated)]

//! phase3_helpers - Shared helpers for Phase 3 commands
//!
//! This module provides reusable functions for the Phase 3 advanced
//! rewriting commands. Following the **Reuse** principle from CLAUDE.md,
//! common patterns are centralized here.
//!
//! # Key Functions
//!
//! - `check_freeze_affected_set` - Validates freeze policy for a set of branches
//! - `rebase_onto_with_journal` - Wrapper around git rebase with journal integration
//! - `reparent_children` - Updates parent pointers when a branch is deleted/folded
//! - `get_net_diff` - Gets the net diff between two commits
//!
//! # Cross-Cutting Requirements
//!
//! Per ROADMAP.md Milestone 9, all Phase 3 commands must enforce:
//! 1. Freeze Policy Validation - Block if any affected branch is frozen
//! 2. Transactional Integrity - Journal before/after OIDs, update metadata after refs
//! 3. State Management - Use executor's plan/journal/lock system, CAS ref updates

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpPhase, OpState};
use crate::core::paths::LatticePaths;
use crate::core::types::{BranchName, Oid};
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::{Git, GitState};

/// Outcome of a rebase operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum RebaseOutcome {
    /// Rebase completed successfully.
    Success {
        /// New tip OID after rebase.
        new_tip: Oid,
    },
    /// Rebase resulted in a conflict and was paused.
    Conflict,
    /// No changes were needed (already aligned).
    NoOp,
}

/// Check freeze policy for a set of branches.
///
/// Returns an error if any branch in the set is frozen, otherwise Ok(()).
/// This implements the freeze policy validation required by ROADMAP.md
/// for all Phase 3 commands.
///
/// # Arguments
///
/// * `branches` - The set of branches to check
/// * `snapshot` - Current repository snapshot
///
/// # Returns
///
/// * `Ok(())` if no branches are frozen
/// * `Err` with descriptive message if any branch is frozen
pub fn check_freeze_affected_set(branches: &[BranchName], snapshot: &RepoSnapshot) -> Result<()> {
    let mut frozen_branches = Vec::new();

    for branch in branches {
        if let Some(scanned) = snapshot.metadata.get(branch) {
            if scanned.metadata.freeze.is_frozen() {
                frozen_branches.push(branch.to_string());
            }
        }
    }

    if frozen_branches.is_empty() {
        Ok(())
    } else if frozen_branches.len() == 1 {
        bail!(
            "Cannot proceed: branch '{}' is frozen. Use 'lattice unfreeze' first.",
            frozen_branches[0]
        );
    } else {
        bail!(
            "Cannot proceed: {} branches are frozen: {}. Use 'lattice unfreeze' first.",
            frozen_branches.len(),
            frozen_branches.join(", ")
        );
    }
}

/// Check freeze policy for a single branch.
///
/// Convenience wrapper around `check_freeze_affected_set` for single branch.
pub fn check_freeze(branch: &BranchName, snapshot: &RepoSnapshot) -> Result<()> {
    check_freeze_affected_set(std::slice::from_ref(branch), snapshot)
}

/// Wrapper around git rebase --onto with journal integration.
///
/// This function:
/// 1. Records a checkpoint in the journal
/// 2. Executes `git rebase --onto <new_base> <old_base> <branch>`
/// 3. On success, returns the new tip OID
/// 4. On conflict, pauses the operation and returns `Conflict`
///
/// # Arguments
///
/// * `git` - Git interface
/// * `cwd` - Current working directory
/// * `branch` - Branch being rebased
/// * `old_base` - Original base commit (--onto from)
/// * `new_base` - New base commit (--onto target)
/// * `journal` - Journal for recording state
/// * `remaining_branches` - Branches remaining to process after this one
/// * `paths` - LatticePaths for repository paths
/// * `ctx` - Execution context (for quiet/debug/verify flags)
///
/// # Returns
///
/// * `RebaseOutcome::Success` with new tip if rebase completed
/// * `RebaseOutcome::Conflict` if conflicts occurred (journal paused)
/// * `RebaseOutcome::NoOp` if no rebase was needed
#[allow(clippy::too_many_arguments)]
pub fn rebase_onto_with_journal(
    git: &Git,
    cwd: &Path,
    branch: &BranchName,
    old_base: &Oid,
    new_base: &Oid,
    journal: &mut Journal,
    remaining_branches: Vec<String>,
    paths: &LatticePaths,
    ctx: &Context,
) -> Result<RebaseOutcome> {
    // Record checkpoint before rebase
    journal.record_checkpoint(format!("rebase-{}", branch));

    // Record git process for audit trail
    journal.record_git_process(
        vec![
            "rebase".to_string(),
            "--onto".to_string(),
            new_base.as_str().to_string(),
            old_base.as_str().to_string(),
            branch.to_string(),
        ],
        format!(
            "Rebase {} onto {}",
            branch,
            &new_base.as_str()[..7.min(new_base.as_str().len())]
        ),
    );

    // Run git rebase
    let mut rebase_args = vec!["rebase"];
    if !ctx.verify {
        rebase_args.push("--no-verify");
    }
    rebase_args.extend([
        "--onto",
        new_base.as_str(),
        old_base.as_str(),
        branch.as_str(),
    ]);
    let status = Command::new("git")
        .args(&rebase_args)
        .current_dir(cwd)
        .status()
        .context("Failed to run git rebase")?;

    if status.success() {
        // Get new tip
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(cwd)
            .output()
            .context("Failed to get HEAD after rebase")?;

        let new_tip_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let new_tip = Oid::new(&new_tip_str).context("Invalid OID after rebase")?;

        Ok(RebaseOutcome::Success { new_tip })
    } else {
        // Check if it's a conflict
        let git_state = git.state();
        if matches!(git_state, GitState::Rebase { .. }) {
            // Conflict - pause operation
            journal.record_conflict_paused(branch.as_str(), "rebase", remaining_branches);
            journal.pause();
            journal.write(paths)?;

            #[allow(deprecated)]
            let mut op_state = OpState::from_journal_legacy(journal, paths, None);
            op_state.phase = OpPhase::Paused;
            op_state.write(paths)?;

            Ok(RebaseOutcome::Conflict)
        } else {
            // Some other error
            bail!("git rebase failed for '{}'", branch);
        }
    }
}

/// Update parent pointers for all children of a deleted/folded branch.
///
/// When a branch is deleted or folded, its children need to be reparented
/// to the deleted branch's parent. This function updates all child metadata
/// to point to the new parent.
///
/// # Arguments
///
/// * `old_parent` - The branch being deleted/folded
/// * `new_parent` - The new parent for children (typically old_parent's parent)
/// * `snapshot` - Current repository snapshot
/// * `git` - Git interface for metadata store
/// * `journal` - Journal for recording state
///
/// # Returns
///
/// * `Ok(Vec<BranchName>)` - List of branches that were reparented
pub fn reparent_children(
    old_parent: &BranchName,
    new_parent: &BranchName,
    snapshot: &RepoSnapshot,
    git: &Git,
    journal: &mut Journal,
) -> Result<Vec<BranchName>> {
    let store = MetadataStore::new(git);
    let mut reparented = Vec::new();

    // Find all children of old_parent
    if let Some(children) = snapshot.graph.children(old_parent) {
        for child in children {
            let scanned = snapshot
                .metadata
                .get(child)
                .ok_or_else(|| anyhow::anyhow!("Child '{}' metadata not found", child))?;

            // Update parent reference
            let mut updated = scanned.metadata.clone();
            updated.parent = if snapshot.trunk.as_ref() == Some(new_parent) {
                crate::core::metadata::schema::ParentInfo::Trunk {
                    name: new_parent.to_string(),
                }
            } else {
                crate::core::metadata::schema::ParentInfo::Branch {
                    name: new_parent.to_string(),
                }
            };
            updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            // Write with CAS
            let new_ref_oid = store
                .write_cas(child, Some(&scanned.ref_oid), &updated)
                .with_context(|| format!("Failed to reparent child '{}'", child))?;

            journal.record_metadata_write(
                child.as_str(),
                Some(scanned.ref_oid.to_string()),
                new_ref_oid,
            );

            reparented.push(child.clone());
        }
    }

    Ok(reparented)
}

/// Get the net diff between two commits as a patch.
///
/// This produces the combined diff that represents all changes from
/// `base` to `tip`, suitable for applying to another branch.
///
/// # Arguments
///
/// * `cwd` - Current working directory
/// * `base` - Base commit (start of range)
/// * `tip` - Tip commit (end of range)
///
/// # Returns
///
/// * `Ok(String)` - The patch content
pub fn get_net_diff(cwd: &Path, base: &Oid, tip: &Oid) -> Result<String> {
    let output = Command::new("git")
        .args(["diff", base.as_str(), tip.as_str()])
        .current_dir(cwd)
        .output()
        .context("Failed to get diff")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get the list of commits unique to a branch (base..tip).
///
/// Returns commits in chronological order (oldest first).
///
/// # Arguments
///
/// * `cwd` - Current working directory
/// * `base` - Base commit
/// * `tip` - Tip commit
///
/// # Returns
///
/// * `Ok(Vec<Oid>)` - List of commit OIDs
pub fn get_commits_in_range(cwd: &Path, base: &Oid, tip: &Oid) -> Result<Vec<Oid>> {
    let output = Command::new("git")
        .args([
            "rev-list",
            "--reverse",
            &format!("{}..{}", base.as_str(), tip.as_str()),
        ])
        .current_dir(cwd)
        .output()
        .context("Failed to list commits")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git rev-list failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| Oid::new(s).context("Invalid OID in rev-list"))
        .collect()
}

/// Count commits unique to a branch (base..tip).
///
/// # Arguments
///
/// * `cwd` - Current working directory
/// * `base` - Base commit
/// * `tip` - Tip commit
///
/// # Returns
///
/// * `Ok(usize)` - Number of commits
pub fn count_commits_in_range(cwd: &Path, base: &Oid, tip: &Oid) -> Result<usize> {
    let output = Command::new("git")
        .args([
            "rev-list",
            "--count",
            &format!("{}..{}", base.as_str(), tip.as_str()),
        ])
        .current_dir(cwd)
        .output()
        .context("Failed to count commits")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git rev-list --count failed: {}", stderr);
    }

    let count_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    count_str
        .parse::<usize>()
        .context("Failed to parse commit count")
}

/// Check if a branch is a descendant of another.
///
/// Used for cycle detection in move operations.
///
/// # Arguments
///
/// * `potential_descendant` - Branch to check
/// * `ancestor` - Potential ancestor
/// * `snapshot` - Repository snapshot
///
/// # Returns
///
/// * `true` if `potential_descendant` is a descendant of `ancestor`
pub fn is_descendant_of(
    potential_descendant: &BranchName,
    ancestor: &BranchName,
    snapshot: &RepoSnapshot,
) -> bool {
    let mut current = potential_descendant.clone();

    // Walk up the parent chain
    while let Some(parent) = snapshot.graph.parent(&current) {
        if parent == ancestor {
            return true;
        }
        current = parent.clone();
    }

    false
}

/// Apply a patch to the working tree.
///
/// # Arguments
///
/// * `cwd` - Current working directory
/// * `patch` - The patch content to apply
/// * `check_only` - If true, only check if patch applies (--check)
///
/// # Returns
///
/// * `Ok(())` if patch applied successfully
/// * `Err` if patch failed to apply
#[allow(dead_code)]
pub fn apply_patch(cwd: &Path, patch: &str, check_only: bool) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let mut args = vec!["apply"];
    if check_only {
        args.push("--check");
    }

    let mut child = Command::new("git")
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn git apply")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(patch.as_bytes())
            .context("Failed to write patch to git apply")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for git apply")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if check_only {
            bail!("Patch would not apply cleanly: {}", stderr);
        } else {
            bail!("Failed to apply patch: {}", stderr);
        }
    }

    Ok(())
}

/// Get the current HEAD commit.
///
/// # Arguments
///
/// * `cwd` - Current working directory
///
/// # Returns
///
/// * `Ok(Oid)` - Current HEAD commit
#[allow(dead_code)]
pub fn get_head(cwd: &Path) -> Result<Oid> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()
        .context("Failed to get HEAD")?;

    if !output.status.success() {
        bail!("Failed to get HEAD");
    }

    let oid_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Oid::new(&oid_str).context("Invalid HEAD OID")
}

/// Check if working tree is clean (no staged or unstaged changes).
///
/// # Arguments
///
/// * `cwd` - Current working directory
///
/// # Returns
///
/// * `Ok(true)` if working tree is clean
/// * `Ok(false)` if there are uncommitted changes
pub fn is_working_tree_clean(cwd: &Path) -> Result<bool> {
    // Check for staged changes
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(cwd)
        .status()
        .context("Failed to check staged changes")?;

    if !staged.success() {
        return Ok(false);
    }

    // Check for unstaged changes
    let unstaged = Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(cwd)
        .status()
        .context("Failed to check unstaged changes")?;

    Ok(unstaged.success())
}

/// Delete a branch (both ref and metadata).
///
/// # Arguments
///
/// * `branch` - Branch to delete
/// * `cwd` - Current working directory
/// * `git` - Git interface
/// * `snapshot` - Repository snapshot
/// * `journal` - Journal for recording state
/// * `force` - If true, force delete (-D) even if not merged
///
/// # Returns
///
/// * `Ok(())` on success
#[allow(dead_code)]
pub fn delete_branch(
    branch: &BranchName,
    cwd: &Path,
    git: &Git,
    snapshot: &RepoSnapshot,
    journal: &mut Journal,
    force: bool,
) -> Result<()> {
    // Get old branch OID for journal
    let old_oid = snapshot
        .branches
        .get(branch)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch))?;

    // Delete git ref
    let delete_flag = if force { "-D" } else { "-d" };
    let status = Command::new("git")
        .args(["branch", delete_flag, branch.as_str()])
        .current_dir(cwd)
        .status()
        .with_context(|| format!("Failed to delete branch '{}'", branch))?;

    if !status.success() {
        bail!("git branch {} {} failed", delete_flag, branch);
    }

    // Record ref deletion
    journal.record_ref_update(
        format!("refs/heads/{}", branch),
        Some(old_oid.to_string()),
        "0000000000000000000000000000000000000000".to_string(),
    );

    // Delete metadata if tracked
    if let Some(scanned) = snapshot.metadata.get(branch) {
        let store = MetadataStore::new(git);
        store
            .delete_cas(branch, &scanned.ref_oid)
            .with_context(|| format!("Failed to delete metadata for '{}'", branch))?;

        journal.record_metadata_delete(branch.as_str(), scanned.ref_oid.to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    mod check_freeze {
        // These tests would require setting up a mock snapshot
        // For now, we verify the function compiles and has correct signature
    }

    mod is_descendant_of {
        // These tests would require setting up a mock graph
        // For now, we verify the function compiles and has correct signature
    }
}
