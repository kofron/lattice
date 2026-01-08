//! reorder command - Editor-driven branch reordering
//!
//! Per SPEC.md 8D.5:
//!
//! - Opens editor with list of branches between trunk and current (exclusive of trunk)
//! - User reorders lines
//! - Validates same set of branches, no duplicates, no missing entries
//! - Computes required rebase sequence to realize new ordering
//! - Journals each rebase step
//! - Conflicts pause
//!
//! # Integrity Contract
//!
//! - Must never reorder frozen branches
//! - Must validate edit result
//! - Metadata updated only after refs succeed

use std::fs;
use std::io::{self, Write};
use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::commands::phase3_helpers::{
    check_freeze_affected_set, rebase_onto_with_journal, RebaseOutcome,
};
use crate::cli::commands::restack::get_ancestors_inclusive;
use crate::core::metadata::schema::{BaseInfo, ParentInfo};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Reorder branches in current stack using editor.
///
/// # Arguments
///
/// * `ctx` - Execution context
pub fn reorder(ctx: &Context) -> Result<()> {
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

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    // Check if tracked
    if !snapshot.metadata.contains_key(&current) {
        bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            current
        );
    }

    // Get stack from trunk to current (ancestors including current, excluding trunk)
    let ancestors = get_ancestors_inclusive(&current, &snapshot);
    let stack: Vec<_> = ancestors.into_iter().filter(|b| b != trunk).collect();

    if stack.len() < 2 {
        if !ctx.quiet {
            println!(
                "Need at least 2 branches to reorder. Stack has {} tracked branch(es).",
                stack.len()
            );
        }
        return Ok(());
    }

    // Check freeze policy
    check_freeze_affected_set(&stack, &snapshot)?;

    // Create temp file with branch list
    let temp_file = git_dir.join("REORDER_BRANCHES");
    let content = format!(
        "# Reorder branches by moving lines. Lines starting with # are ignored.\n\
         # Do not add or remove branches.\n\
         # Original order (bottom to top, trunk-child first):\n\
         \n\
         {}\n",
        stack
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );

    fs::write(&temp_file, &content).context("Failed to write reorder file")?;

    // Get editor
    let editor = std::env::var("LATTICE_TEST_EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    if !ctx.quiet {
        println!("Opening editor to reorder {} branches...", stack.len());
    }

    // Open editor
    let status = Command::new(&editor)
        .arg(&temp_file)
        .status()
        .with_context(|| format!("Failed to open editor '{}'", editor))?;

    if !status.success() {
        fs::remove_file(&temp_file).ok();
        bail!("Editor exited with error");
    }

    // Read edited file
    let edited = fs::read_to_string(&temp_file).context("Failed to read edited file")?;
    fs::remove_file(&temp_file).ok();

    // Parse edited order
    let new_order: Vec<BranchName> = edited
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| BranchName::new(line.trim()))
        .collect::<Result<Vec<_>, _>>()
        .context("Invalid branch name in edited file")?;

    // Validate: same set, no duplicates
    if new_order.len() != stack.len() {
        bail!(
            "Invalid edit: expected {} branches, got {}. Do not add or remove branches.",
            stack.len(),
            new_order.len()
        );
    }

    let new_set: std::collections::HashSet<_> = new_order.iter().collect();
    if new_set.len() != new_order.len() {
        bail!("Invalid edit: duplicate branch names detected");
    }

    let old_set: std::collections::HashSet<_> = stack.iter().collect();
    if new_set != old_set {
        let missing: Vec<_> = old_set.difference(&new_set).collect();
        let added: Vec<_> = new_set.difference(&old_set).collect();

        if !missing.is_empty() {
            bail!("Invalid edit: missing branches: {:?}", missing);
        }
        if !added.is_empty() {
            bail!("Invalid edit: unknown branches: {:?}", added);
        }
    }

    // Check if order actually changed
    if new_order == stack {
        if !ctx.quiet {
            println!("No changes to branch order.");
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!("New order:");
        for (i, branch) in new_order.iter().enumerate() {
            let parent = if i == 0 {
                trunk.to_string()
            } else {
                new_order[i - 1].to_string()
            };
            println!("  {} (parent: {})", branch, parent);
        }
        println!();
    }

    // Confirm
    if ctx.interactive {
        print!("Apply this reorder? [y/N] ");
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
    let mut journal = Journal::new("reorder");

    // Write op-state
    let op_state = OpState::from_journal(&journal);
    op_state.write(git_dir)?;

    // Execute rebase sequence
    // We need to rebase each branch onto its new parent
    let store = MetadataStore::new(&git);

    for (i, branch) in new_order.iter().enumerate() {
        let new_parent = if i == 0 {
            trunk.clone()
        } else {
            new_order[i - 1].clone()
        };

        // Re-scan to get current state
        let current_snapshot = scan(&git).context("Failed to re-scan")?;

        let branch_meta = current_snapshot
            .metadata
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

        // Get new parent tip
        let new_parent_tip = current_snapshot
            .branches
            .get(&new_parent)
            .ok_or_else(|| anyhow::anyhow!("Parent '{}' not found", new_parent))?;

        // Check if branch is already in correct position
        let current_parent_name = branch_meta.metadata.parent.name();
        if current_parent_name == new_parent.as_str()
            && branch_meta.metadata.base.oid == new_parent_tip.as_str()
        {
            if ctx.debug {
                eprintln!("[debug] '{}' is already in position", branch);
            }
            continue;
        }

        if !ctx.quiet {
            println!("  Rebasing '{}' onto '{}'...", branch, new_parent);
        }

        let old_base =
            crate::core::types::Oid::new(&branch_meta.metadata.base.oid).context("Invalid base")?;

        let remaining: Vec<String> = new_order
            .iter()
            .skip(i + 1)
            .map(|b| b.to_string())
            .collect();

        let outcome = rebase_onto_with_journal(
            &git,
            &cwd,
            branch,
            &old_base,
            new_parent_tip,
            &mut journal,
            remaining,
            git_dir,
            ctx,
        )?;

        match outcome {
            RebaseOutcome::Success { new_tip: _ } => {
                // Update metadata
                let fresh_snapshot = scan(&git)?;
                let fresh_meta = fresh_snapshot
                    .metadata
                    .get(branch)
                    .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

                let new_parent_ref = if &new_parent == trunk {
                    ParentInfo::Trunk {
                        name: new_parent.to_string(),
                    }
                } else {
                    ParentInfo::Branch {
                        name: new_parent.to_string(),
                    }
                };

                let mut updated = fresh_meta.metadata.clone();
                updated.parent = new_parent_ref;
                updated.base = BaseInfo {
                    oid: new_parent_tip.to_string(),
                };
                updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

                let new_ref_oid = store.write_cas(branch, Some(&fresh_meta.ref_oid), &updated)?;

                journal.record_metadata_write(
                    branch.as_str(),
                    Some(fresh_meta.ref_oid.to_string()),
                    new_ref_oid,
                );
            }
            RebaseOutcome::Conflict => {
                println!();
                println!("Conflict while reordering '{}'.", branch);
                println!("Resolve conflicts, then run 'lattice continue'.");
                println!("To abort, run 'lattice abort'.");
                return Ok(());
            }
            RebaseOutcome::NoOp => {
                // Still update parent pointer
                let fresh_snapshot = scan(&git)?;
                let fresh_meta = fresh_snapshot
                    .metadata
                    .get(branch)
                    .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

                let new_parent_ref = if &new_parent == trunk {
                    ParentInfo::Trunk {
                        name: new_parent.to_string(),
                    }
                } else {
                    ParentInfo::Branch {
                        name: new_parent.to_string(),
                    }
                };

                let mut updated = fresh_meta.metadata.clone();
                updated.parent = new_parent_ref;
                updated.base = BaseInfo {
                    oid: new_parent_tip.to_string(),
                };
                updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

                let new_ref_oid = store.write_cas(branch, Some(&fresh_meta.ref_oid), &updated)?;

                journal.record_metadata_write(
                    branch.as_str(),
                    Some(fresh_meta.ref_oid.to_string()),
                    new_ref_oid,
                );
            }
        }
    }

    // Commit journal
    journal.commit();
    journal.write(git_dir)?;

    // Clear op-state
    OpState::remove(git_dir)?;

    if !ctx.quiet {
        println!("Reorder complete.");
    }

    Ok(())
}
