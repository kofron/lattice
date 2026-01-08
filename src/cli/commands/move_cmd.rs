//! move command - Reparent branch onto another branch
//!
//! Per SPEC.md 8D.4:
//!
//! - Changes parent of source to onto
//! - Prevents cycles: cannot move onto a descendant
//! - Rebases source onto onto.tip using source.base as the from point
//! - Descendants remain descendants of source
//!
//! # Integrity Contract
//!
//! - Must prevent cycles (cannot move onto descendant)
//! - Must never rewrite frozen branches
//! - Metadata updated only after refs succeed

use anyhow::{bail, Context as _, Result};

use crate::cli::commands::phase3_helpers::{
    check_freeze_affected_set, is_descendant_of, rebase_onto_with_journal, RebaseOutcome,
};
use crate::cli::commands::restack::{get_descendants_inclusive, get_parent_tip, topological_sort};
use crate::core::metadata::schema::{BaseInfo, ParentInfo};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Move (reparent) a branch onto another branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `onto` - Target parent branch
/// * `source` - Branch to move (defaults to current)
pub fn move_branch(ctx: &Context, onto: &str, source: Option<&str>) -> Result<()> {
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

    // Resolve source branch
    let source_branch = if let Some(name) = source {
        BranchName::new(name).context("Invalid source branch name")?
    } else if let Some(ref current) = snapshot.current_branch {
        current.clone()
    } else {
        bail!("Not on any branch and no source specified");
    };

    // Check if source is tracked
    if !snapshot.metadata.contains_key(&source_branch) {
        bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            source_branch
        );
    }

    // Resolve onto branch
    let onto_branch = BranchName::new(onto).context("Invalid onto branch name")?;

    // Check if onto exists
    if !snapshot.branches.contains_key(&onto_branch) {
        bail!("Target branch '{}' does not exist", onto_branch);
    }

    // Prevent self-move
    if source_branch == onto_branch {
        bail!("Cannot move a branch onto itself");
    }

    // Cycle detection: ensure onto is not a descendant of source
    if is_descendant_of(&onto_branch, &source_branch, &snapshot) {
        bail!(
            "Cannot move '{}' onto '{}': would create a cycle (target is a descendant)",
            source_branch,
            onto_branch
        );
    }

    // Get descendants for freeze check
    let descendants = get_descendants_inclusive(&source_branch, &snapshot);

    // Check freeze policy on source and all descendants
    check_freeze_affected_set(&descendants, &snapshot)?;

    // Get source metadata
    let source_meta = snapshot
        .metadata
        .get(&source_branch)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", source_branch))?;

    // Get onto tip (new base for source)
    let onto_tip = snapshot
        .branches
        .get(&onto_branch)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", onto_branch))?;

    // Get source's current base
    let old_base = crate::core::types::Oid::new(&source_meta.metadata.base.oid)
        .context("Invalid base OID in source metadata")?;

    // If source is already a child of onto, check if already aligned
    let current_parent_name = source_meta.metadata.parent.name();
    if current_parent_name == onto_branch.as_str()
        && source_meta.metadata.base.oid == onto_tip.as_str()
    {
        if !ctx.quiet {
            println!(
                "'{}' is already a child of '{}' and aligned.",
                source_branch, onto_branch
            );
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!(
            "Moving '{}' onto '{}' (was child of '{}')...",
            source_branch, onto_branch, current_parent_name
        );
    }

    // Acquire lock
    let _lock = RepoLock::acquire(git_dir).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("move");

    // Write op-state
    let op_state = OpState::from_journal(&journal);
    op_state.write(git_dir)?;

    // Rebase source onto new parent
    let remaining_descendants: Vec<String> = descendants
        .iter()
        .filter(|b| *b != &source_branch)
        .map(|b| b.to_string())
        .collect();

    let outcome = rebase_onto_with_journal(
        &git,
        &cwd,
        &source_branch,
        &old_base,
        onto_tip,
        &mut journal,
        remaining_descendants.clone(),
        git_dir,
        ctx,
    )?;

    match outcome {
        RebaseOutcome::Success { new_tip: _ } => {
            // Update source metadata with new parent and base
            let store = MetadataStore::new(&git);

            let new_parent = if &onto_branch == trunk {
                ParentInfo::Trunk {
                    name: onto_branch.to_string(),
                }
            } else {
                ParentInfo::Branch {
                    name: onto_branch.to_string(),
                }
            };

            let mut updated = source_meta.metadata.clone();
            updated.parent = new_parent;
            updated.base = BaseInfo {
                oid: onto_tip.to_string(),
            };
            updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            let new_ref_oid = store
                .write_cas(&source_branch, Some(&source_meta.ref_oid), &updated)
                .with_context(|| format!("Failed to update metadata for '{}'", source_branch))?;

            journal.record_metadata_write(
                source_branch.as_str(),
                Some(source_meta.ref_oid.to_string()),
                new_ref_oid,
            );

            if !ctx.quiet {
                println!("  Moved '{}'", source_branch);
            }
        }
        RebaseOutcome::Conflict => {
            println!();
            println!("Conflict while moving '{}'.", source_branch);
            println!("Resolve conflicts, then run 'lattice continue'.");
            println!("To abort, run 'lattice abort'.");
            return Ok(());
        }
        RebaseOutcome::NoOp => {
            // Still need to update parent pointer even if no rebase needed
            let store = MetadataStore::new(&git);

            let new_parent = if &onto_branch == trunk {
                ParentInfo::Trunk {
                    name: onto_branch.to_string(),
                }
            } else {
                ParentInfo::Branch {
                    name: onto_branch.to_string(),
                }
            };

            let mut updated = source_meta.metadata.clone();
            updated.parent = new_parent;
            updated.base = BaseInfo {
                oid: onto_tip.to_string(),
            };
            updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            let new_ref_oid = store
                .write_cas(&source_branch, Some(&source_meta.ref_oid), &updated)
                .with_context(|| format!("Failed to update metadata for '{}'", source_branch))?;

            journal.record_metadata_write(
                source_branch.as_str(),
                Some(source_meta.ref_oid.to_string()),
                new_ref_oid,
            );
        }
    }

    // Restack descendants if any
    let descendants_to_restack: Vec<_> = descendants
        .iter()
        .filter(|b| *b != &source_branch)
        .cloned()
        .collect();

    if !descendants_to_restack.is_empty() {
        if !ctx.quiet {
            println!(
                "Restacking {} descendant(s)...",
                descendants_to_restack.len()
            );
        }

        let ordered = topological_sort(&descendants_to_restack, &snapshot);

        // Re-scan to get updated state
        let updated_snapshot = scan(&git).context("Failed to re-scan repository")?;

        for (idx, branch) in ordered.iter().enumerate() {
            let branch_meta = updated_snapshot
                .metadata
                .get(branch)
                .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

            // Skip frozen branches
            if branch_meta.metadata.freeze.is_frozen() {
                if !ctx.quiet {
                    println!("  Skipping frozen branch '{}'", branch);
                }
                continue;
            }

            // Get parent tip
            let parent_tip = get_parent_tip(branch, &updated_snapshot, trunk)?;

            // Check if already aligned
            if branch_meta.metadata.base.oid.as_str() == parent_tip.as_str() {
                continue;
            }

            let branch_old_base = crate::core::types::Oid::new(&branch_meta.metadata.base.oid)
                .context("Invalid base OID")?;

            let remaining: Vec<String> = ordered
                .iter()
                .skip(idx + 1)
                .map(|b| b.to_string())
                .collect();

            let outcome = rebase_onto_with_journal(
                &git,
                &cwd,
                branch,
                &branch_old_base,
                &parent_tip,
                &mut journal,
                remaining,
                git_dir,
                ctx,
            )?;

            match outcome {
                RebaseOutcome::Success { new_tip: _ } => {
                    // Update metadata
                    let store = MetadataStore::new(&git);
                    let fresh_snapshot = scan(&git)?;
                    let fresh_meta = fresh_snapshot
                        .metadata
                        .get(branch)
                        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

                    let mut updated = fresh_meta.metadata.clone();
                    updated.base = BaseInfo {
                        oid: parent_tip.to_string(),
                    };
                    updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

                    let new_ref_oid =
                        store.write_cas(branch, Some(&fresh_meta.ref_oid), &updated)?;

                    journal.record_metadata_write(
                        branch.as_str(),
                        Some(fresh_meta.ref_oid.to_string()),
                        new_ref_oid,
                    );

                    if !ctx.quiet {
                        println!("  Restacked '{}'", branch);
                    }
                }
                RebaseOutcome::Conflict => {
                    println!();
                    println!("Conflict while restacking '{}'.", branch);
                    println!("Resolve conflicts, then run 'lattice continue'.");
                    println!("To abort, run 'lattice abort'.");
                    return Ok(());
                }
                RebaseOutcome::NoOp => {}
            }
        }
    }

    // Commit journal
    journal.commit();
    journal.write(git_dir)?;

    // Clear op-state
    OpState::remove(git_dir)?;

    if !ctx.quiet {
        println!("Move complete.");
    }

    Ok(())
}
