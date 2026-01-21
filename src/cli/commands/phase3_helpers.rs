//! phase3_helpers - Shared helpers for Phase 3 commands
//!
//! This module provides reusable functions for the Phase 3 advanced
//! rewriting commands. Following the **Reuse** principle from CLAUDE.md,
//! common patterns are centralized here.
//!
//! # Key Functions
//!
//! - `check_freeze_affected_set` - Validates freeze policy for a set of branches
//! - `count_commits_in_range` - Counts commits unique to a branch
//! - `get_net_diff` - Gets the net diff between two commits
//! - `is_descendant_of` - Checks if a branch is a descendant of another

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::core::types::{BranchName, Oid};
use crate::engine::scan::RepoSnapshot;

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
