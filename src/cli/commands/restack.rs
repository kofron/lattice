//! restack command - Rebase tracked branches to align with parent tips
//!
//! This is the template command for conflict handling. It demonstrates
//! the full engine lifecycle with journaling and pause/resume capability.

use crate::core::metadata::schema::BaseInfo;
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpPhase, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::types::{BranchName, Oid};
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::{Git, GitState};
use anyhow::{bail, Context as _, Result};
use std::process::Command;

/// Rebase tracked branches to align with parent tips.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Specific branch to restack
/// * `only` - Only restack this branch
/// * `downstack` - Restack this branch and its ancestors
pub fn restack(ctx: &Context, branch: Option<&str>, only: bool, downstack: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let git_dir = git.git_dir();

    // Check for in-progress operation
    if let Some(op_state) = OpState::read(git_dir)? {
        bail!(
            "Another operation is in progress: {} ({}). Use 'lattice continue' or 'lattice abort'.",
            op_state.command,
            op_state.op_id
        );
    }

    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Ensure trunk is configured
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Resolve target branch
    let target = if let Some(name) = branch {
        BranchName::new(name).context("Invalid branch name")?
    } else if let Some(ref current) = snapshot.current_branch {
        current.clone()
    } else {
        bail!("Not on any branch and no branch specified");
    };

    // Check if tracked
    if !snapshot.metadata.contains_key(&target) {
        bail!("Branch '{}' is not tracked", target);
    }

    // Determine scope
    let branches_to_restack = if only {
        vec![target.clone()]
    } else if downstack {
        // Target and all ancestors (bottom-up order)
        get_ancestors_inclusive(&target, &snapshot)
    } else {
        // Default: target and all descendants (bottom-up order for correct rebase order)
        get_descendants_inclusive(&target, &snapshot)
    };

    if branches_to_restack.is_empty() {
        if !ctx.quiet {
            println!("No branches to restack.");
        }
        return Ok(());
    }

    // Sort in topological order (bottom-up: parents before children)
    let ordered = topological_sort(&branches_to_restack, &snapshot);

    // Check which branches need restacking
    let mut needs_restack = Vec::new();
    for branch in &ordered {
        let metadata = snapshot
            .metadata
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

        // Skip frozen branches
        if metadata.metadata.freeze.is_frozen() {
            if !ctx.quiet {
                println!("Skipping frozen branch '{}'", branch);
            }
            continue;
        }

        // Get parent tip
        let parent_tip = get_parent_tip(branch, &snapshot, trunk)?;

        // Check if already aligned
        if metadata.metadata.base.oid.as_str() == parent_tip.as_str() {
            if ctx.debug {
                eprintln!("[debug] '{}' is already aligned", branch);
            }
            continue;
        }

        needs_restack.push((
            branch.clone(),
            metadata.metadata.base.oid.clone(),
            parent_tip,
        ));
    }

    if needs_restack.is_empty() {
        if !ctx.quiet {
            println!("All branches are already aligned.");
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!("Restacking {} branch(es):", needs_restack.len());
        for (branch, _, _) in &needs_restack {
            println!("  - {}", branch);
        }
    }

    // Acquire lock
    let _lock = RepoLock::acquire(git_dir).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("restack");

    // Write op-state
    let op_state = OpState::from_journal(&journal);
    op_state.write(git_dir)?;

    // Execute restacks
    for (branch, old_base, new_base) in &needs_restack {
        if ctx.debug {
            eprintln!(
                "[debug] Restacking '{}': {} -> {}",
                branch,
                &old_base.as_str()[..7],
                &new_base.as_str()[..7]
            );
        }

        // Record checkpoint
        journal.record_checkpoint(format!("restack-{}", branch));

        // Run git rebase
        let status = Command::new("git")
            .args([
                "rebase",
                "--onto",
                new_base.as_str(),
                old_base.as_str(),
                branch.as_str(),
            ])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git rebase")?;

        if !status.success() {
            // Check if it's a conflict
            let git_state = git.state();
            if matches!(git_state, GitState::Rebase { .. }) {
                // Conflict - pause
                journal.record_conflict_paused(
                    branch.as_str(),
                    "rebase",
                    needs_restack
                        .iter()
                        .skip_while(|(b, _, _)| b != branch)
                        .skip(1)
                        .map(|(b, _, _)| b.to_string())
                        .collect(),
                );
                journal.pause();
                journal.write(git_dir)?;

                let mut op_state = OpState::from_journal(&journal);
                op_state.phase = OpPhase::Paused;
                op_state.write(git_dir)?;

                println!();
                println!("Conflict while restacking '{}'.", branch);
                println!("Resolve conflicts, then run 'lattice continue'.");
                println!("To abort, run 'lattice abort'.");
                return Ok(());
            } else {
                // Some other error
                OpState::remove(git_dir)?;
                bail!("git rebase failed for '{}'", branch);
            }
        }

        // Update metadata with new base
        let store = MetadataStore::new(&git);
        let scanned = snapshot
            .metadata
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

        let mut updated = scanned.metadata.clone();
        updated.base = BaseInfo {
            oid: new_base.to_string(),
        };
        updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

        store
            .write_cas(branch, Some(&scanned.ref_oid), &updated)
            .with_context(|| format!("Failed to update metadata for '{}'", branch))?;

        journal.record_metadata_write(
            branch.as_str(),
            Some(scanned.ref_oid.to_string()),
            "updated".to_string(),
        );

        if !ctx.quiet {
            println!("  Restacked '{}'", branch);
        }
    }

    // Mark journal as committed
    journal.commit();
    journal.write(git_dir)?;

    // Clear op-state
    OpState::remove(git_dir)?;

    if !ctx.quiet {
        println!("Restack complete.");
    }

    Ok(())
}

/// Get ancestors of a branch including itself (bottom-up order).
pub fn get_ancestors_inclusive(
    branch: &BranchName,
    snapshot: &crate::engine::scan::RepoSnapshot,
) -> Vec<BranchName> {
    let mut result = vec![branch.clone()];
    let mut current = branch.clone();

    while let Some(parent) = snapshot.graph.parent(&current) {
        if snapshot.metadata.contains_key(parent) {
            result.push(parent.clone());
            current = parent.clone();
        } else {
            break;
        }
    }

    result.reverse(); // Bottom-up: parents first
    result
}

/// Get descendants of a branch including itself (for bottom-up processing).
pub fn get_descendants_inclusive(
    branch: &BranchName,
    snapshot: &crate::engine::scan::RepoSnapshot,
) -> Vec<BranchName> {
    let mut result = vec![branch.clone()];
    let mut stack = vec![branch.clone()];

    while let Some(current) = stack.pop() {
        if let Some(children) = snapshot.graph.children(&current) {
            for child in children {
                result.push(child.clone());
                stack.push(child.clone());
            }
        }
    }

    result
}

/// Sort branches in topological order (parents before children).
pub fn topological_sort(
    branches: &[BranchName],
    snapshot: &crate::engine::scan::RepoSnapshot,
) -> Vec<BranchName> {
    let branch_set: std::collections::HashSet<_> = branches.iter().collect();
    let mut result = Vec::new();
    let mut visited = std::collections::HashSet::new();

    fn visit(
        branch: &BranchName,
        snapshot: &crate::engine::scan::RepoSnapshot,
        branch_set: &std::collections::HashSet<&BranchName>,
        visited: &mut std::collections::HashSet<BranchName>,
        result: &mut Vec<BranchName>,
    ) {
        if visited.contains(branch) {
            return;
        }
        visited.insert(branch.clone());

        // Visit parent first
        if let Some(parent) = snapshot.graph.parent(branch) {
            if branch_set.contains(parent) {
                visit(parent, snapshot, branch_set, visited, result);
            }
        }

        result.push(branch.clone());
    }

    for branch in branches {
        visit(branch, snapshot, &branch_set, &mut visited, &mut result);
    }

    result
}

/// Get the tip OID of a branch's parent.
pub fn get_parent_tip(
    branch: &BranchName,
    snapshot: &crate::engine::scan::RepoSnapshot,
    trunk: &BranchName,
) -> Result<Oid> {
    let metadata = snapshot
        .metadata
        .get(branch)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

    let parent_name_str = metadata.metadata.parent.name();
    let parent_name = if metadata.metadata.parent.is_trunk() {
        trunk.clone()
    } else {
        BranchName::new(parent_name_str)
            .map_err(|e| anyhow::anyhow!("Invalid parent name: {}", e))?
    };

    snapshot
        .branches
        .get(&parent_name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Parent branch '{}' not found", parent_name))
}
