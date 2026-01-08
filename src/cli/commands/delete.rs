//! delete command - Delete a branch
//!
//! Per SPEC.md 8D.10:
//!
//! - Deletes local branch and metadata
//! - Re-parents children to deleted branch's parent
//! - Does not close PRs or delete remote branches
//! - --upstack deletes descendants too
//! - --downstack deletes ancestors (never trunk)
//!
//! # Integrity Contract
//!
//! - Must never delete frozen branches
//! - Must re-parent children before deleting
//! - Metadata updated only after refs succeed

use std::io::{self, Write};
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::commands::phase3_helpers::{check_freeze_affected_set, reparent_children};
use crate::cli::commands::restack::get_ancestors_inclusive;
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Delete a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to delete (defaults to current)
/// * `upstack` - Also delete all descendants
/// * `downstack` - Also delete all ancestors (not trunk)
/// * `force` - Skip confirmation prompts
pub fn delete(
    ctx: &Context,
    branch: Option<&str>,
    upstack: bool,
    downstack: bool,
    force: bool,
) -> Result<()> {
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

    // Cannot delete trunk
    if &target == trunk {
        bail!("Cannot delete trunk branch");
    }

    // Check if tracked
    if !snapshot.metadata.contains_key(&target) {
        bail!(
            "Branch '{}' is not tracked. Use 'git branch -d' for untracked branches.",
            target
        );
    }

    // Determine branches to delete based on flags
    let mut to_delete = vec![target.clone()];

    if upstack {
        // Add all descendants
        let mut stack = vec![target.clone()];
        while let Some(current) = stack.pop() {
            if let Some(children) = snapshot.graph.children(&current) {
                for child in children {
                    if !to_delete.contains(child) {
                        to_delete.push(child.clone());
                        stack.push(child.clone());
                    }
                }
            }
        }
    }

    if downstack {
        // Add all ancestors except trunk
        let ancestors = get_ancestors_inclusive(&target, &snapshot);
        for ancestor in ancestors {
            if &ancestor != trunk && !to_delete.contains(&ancestor) {
                to_delete.push(ancestor);
            }
        }
    }

    // Remove trunk from list if somehow added
    to_delete.retain(|b| b != trunk);

    // Check freeze policy on all branches to delete
    check_freeze_affected_set(&to_delete, &snapshot)?;

    // Show what will be deleted
    if !ctx.quiet {
        println!("Will delete {} branch(es):", to_delete.len());
        for branch in &to_delete {
            println!("  - {}", branch);
        }
    }

    // Confirm unless --force
    if !force && ctx.interactive {
        print!("Continue? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Acquire lock
    let _lock = RepoLock::acquire(git_dir).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("delete");

    // Write op-state
    let op_state = OpState::from_journal(&journal);
    op_state.write(git_dir)?;

    // Get parent of primary target for reparenting
    let target_meta = snapshot
        .metadata
        .get(&target)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", target))?;

    let parent_name = if target_meta.metadata.parent.is_trunk() {
        trunk.clone()
    } else {
        BranchName::new(target_meta.metadata.parent.name())
            .context("Invalid parent name in metadata")?
    };

    // For each branch being deleted (if not upstack mode), reparent its children
    // to the branch's parent. If upstack, children are being deleted anyway.
    if !upstack {
        for branch_to_delete in &to_delete {
            // Find the parent of this branch
            let meta = snapshot
                .metadata
                .get(branch_to_delete)
                .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch_to_delete))?;

            let branch_parent = if meta.metadata.parent.is_trunk() {
                trunk.clone()
            } else {
                BranchName::new(meta.metadata.parent.name()).context("Invalid parent name")?
            };

            // Reparent children of this branch
            let reparented = reparent_children(
                branch_to_delete,
                &branch_parent,
                &snapshot,
                &git,
                &mut journal,
            )?;

            if !ctx.quiet && !reparented.is_empty() {
                println!(
                    "  Reparented {} child(ren) of '{}' to '{}'",
                    reparented.len(),
                    branch_to_delete,
                    branch_parent
                );
            }
        }
    }

    // Check if we're on one of the branches being deleted
    let need_checkout = snapshot
        .current_branch
        .as_ref()
        .map(|c| to_delete.contains(c))
        .unwrap_or(false);

    // If so, checkout parent before deleting
    if need_checkout {
        if !ctx.quiet {
            println!("Checking out '{}' before delete...", parent_name);
        }

        let status = Command::new("git")
            .args(["checkout", parent_name.as_str()])
            .current_dir(&cwd)
            .status()
            .context("Failed to checkout parent")?;

        if !status.success() {
            OpState::remove(git_dir)?;
            bail!("git checkout failed");
        }
    }

    let store = MetadataStore::new(&git);

    // Delete branches in order (leaves first to avoid issues)
    // Sort so leaves come first
    let mut delete_order = to_delete.clone();
    delete_order.sort_by(|a, b| {
        // Count descendants - more descendants = delete later
        let count_a = count_descendants(a, &snapshot);
        let count_b = count_descendants(b, &snapshot);
        count_a.cmp(&count_b)
    });

    for branch_name in &delete_order {
        // Get branch OID for journal
        if let Some(oid) = snapshot.branches.get(branch_name) {
            // Delete git ref (force because it might not be merged)
            let status = Command::new("git")
                .args(["branch", "-D", branch_name.as_str()])
                .current_dir(&cwd)
                .status()
                .with_context(|| format!("Failed to delete branch '{}'", branch_name))?;

            if !status.success() {
                // Try to continue with others, but log warning
                eprintln!("Warning: Failed to delete git branch '{}'", branch_name);
                continue;
            }

            journal.record_ref_update(
                format!("refs/heads/{}", branch_name),
                Some(oid.to_string()),
                "0000000000000000000000000000000000000000".to_string(),
            );
        }

        // Delete metadata
        if let Some(scanned) = snapshot.metadata.get(branch_name) {
            store
                .delete_cas(branch_name, &scanned.ref_oid)
                .with_context(|| format!("Failed to delete metadata for '{}'", branch_name))?;

            journal.record_metadata_delete(branch_name.as_str(), scanned.ref_oid.to_string());
        }

        if !ctx.quiet {
            println!("  Deleted '{}'", branch_name);
        }
    }

    // Commit journal
    journal.commit();
    journal.write(git_dir)?;

    // Clear op-state
    OpState::remove(git_dir)?;

    if !ctx.quiet {
        println!("Delete complete.");
    }

    Ok(())
}

/// Count the number of descendants of a branch.
fn count_descendants(branch: &BranchName, snapshot: &crate::engine::scan::RepoSnapshot) -> usize {
    let mut count = 0;
    let mut stack = vec![branch.clone()];

    while let Some(current) = stack.pop() {
        if let Some(children) = snapshot.graph.children(&current) {
            for child in children {
                count += 1;
                stack.push(child.clone());
            }
        }
    }

    count
}
