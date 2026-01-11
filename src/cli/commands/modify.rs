//! modify command - Amend commits or create first commit, auto-restack descendants
//!
//! This is the simplest Phase 3 rewriting command and establishes patterns
//! for the others. Per SPEC.md 8D.2:
//!
//! - Default: amend HEAD commit on current branch with staged changes
//! - If branch is empty (no commits unique beyond base), creates first commit
//! - After mutation, automatically restack descendants unless prevented by freeze
//! - Conflicts during descendant restack pause the operation
//!
//! # Integrity Contract
//!
//! - Must never rewrite frozen branches
//! - Metadata must be updated only after branch refs have moved successfully

use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::commands::phase3_helpers::{
    check_freeze_affected_set, count_commits_in_range, rebase_onto_with_journal, RebaseOutcome,
};
use crate::cli::commands::restack::{get_descendants_inclusive, get_parent_tip, topological_sort};
use crate::core::metadata::schema::BaseInfo;
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::paths::LatticePaths;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Amend commits or create first commit on current branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `create` - Force create new commit instead of amend
/// * `all` - Stage all changes (git add -A)
/// * `update` - Stage modified tracked files (git add -u)
/// * `patch` - Interactive patch staging (git add -p)
/// * `message` - Commit message
/// * `edit` - Open editor for commit message
pub fn modify(
    ctx: &Context,
    create: bool,
    all: bool,
    update: bool,
    patch: bool,
    message: Option<&str>,
    edit: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Check for in-progress operation
    if let Some(op_state) = OpState::read(&paths)? {
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

    // Get descendants for freeze check and potential restack
    let descendants = get_descendants_inclusive(&current, &snapshot);

    // Check freeze policy on current branch and all descendants
    check_freeze_affected_set(&descendants, &snapshot)?;

    // Get current branch metadata
    let scanned = snapshot
        .metadata
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    let base_oid = &scanned.metadata.base.oid;

    // Get current tip
    let current_tip = snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?;

    // Count commits unique to this branch
    let base_oid_parsed =
        crate::core::types::Oid::new(base_oid).context("Invalid base OID in metadata")?;
    let commit_count = count_commits_in_range(&cwd, &base_oid_parsed, current_tip)?;

    // Determine operation mode
    let is_empty_branch = commit_count == 0;

    // Stage changes if requested
    if all {
        let status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add -A")?;

        if !status.success() {
            bail!("git add -A failed");
        }
    } else if update {
        let status = Command::new("git")
            .args(["add", "-u"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add -u")?;

        if !status.success() {
            bail!("git add -u failed");
        }
    } else if patch {
        let status = Command::new("git")
            .args(["add", "-p"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add -p")?;

        if !status.success() {
            bail!("git add -p failed");
        }
    }

    // Check for staged changes
    let has_staged = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(&cwd)
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    // Build commit command
    let mut commit_args = vec!["commit"];

    if is_empty_branch || create {
        // Create new commit
        if !has_staged {
            bail!("No staged changes to commit. Use -a to stage all changes.");
        }
    } else {
        // Amend existing commit
        commit_args.push("--amend");

        // Allow empty if we're just changing the message
        if !has_staged {
            commit_args.push("--allow-empty");
        }
    }

    // Add message handling
    if let Some(msg) = message {
        commit_args.push("-m");
        commit_args.push(msg);
    } else if edit || is_empty_branch || create {
        // Open editor for message
        // (git commit without -m opens editor by default)
    } else {
        // Amend without changing message
        commit_args.push("--no-edit");
    }

    // Acquire lock before mutating
    let _lock = RepoLock::acquire(&paths).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("modify");

    // Write op-state
    let op_state = OpState::from_journal(&journal, &paths, info.work_dir.clone());
    op_state.write(&paths)?;

    // Record old tip for journal
    journal.record_ref_update(
        format!("refs/heads/{}", current),
        Some(current_tip.to_string()),
        "pending".to_string(), // Will be updated after commit
    );

    // Execute commit
    let status = Command::new("git")
        .args(&commit_args)
        .current_dir(&cwd)
        .status()
        .context("Failed to run git commit")?;

    if !status.success() {
        // Clean up op-state on failure
        OpState::remove(&paths)?;

        if !has_staged && !commit_args.contains(&"--amend") {
            bail!("No staged changes to commit");
        }
        bail!("git commit failed");
    }

    // Get new tip
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&cwd)
        .output()
        .context("Failed to get HEAD after commit")?;

    let new_tip = String::from_utf8_lossy(&output.stdout).trim().to_string();

    if !ctx.quiet {
        if is_empty_branch || create {
            println!("Created commit on '{}'", current);
        } else {
            println!("Amended commit on '{}'", current);
        }
    }

    // Update journal with actual new tip
    journal.record_ref_update(
        format!("refs/heads/{}", current),
        Some(current_tip.to_string()),
        new_tip.clone(),
    );

    // Now restack descendants if any
    let descendants_to_restack: Vec<_> = descendants
        .iter()
        .filter(|b| *b != &current)
        .cloned()
        .collect();

    if !descendants_to_restack.is_empty() {
        if !ctx.quiet {
            println!(
                "Restacking {} descendant(s)...",
                descendants_to_restack.len()
            );
        }

        // Sort in topological order (parents before children)
        let ordered = topological_sort(&descendants_to_restack, &snapshot);

        // Re-scan to get updated state after our commit
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

            // Get parent tip (may be the branch we just modified or another descendant)
            let parent_tip = get_parent_tip(branch, &updated_snapshot, trunk)?;

            // Check if already aligned
            if branch_meta.metadata.base.oid.as_str() == parent_tip.as_str() {
                if ctx.debug {
                    eprintln!("[debug] '{}' is already aligned", branch);
                }
                continue;
            }

            let old_base = crate::core::types::Oid::new(&branch_meta.metadata.base.oid)
                .context("Invalid base OID")?;

            // Calculate remaining branches for conflict handling
            let remaining: Vec<String> = ordered
                .iter()
                .skip(idx + 1)
                .map(|b| b.to_string())
                .collect();

            // Rebase with journal integration
            let outcome = rebase_onto_with_journal(
                &git,
                &cwd,
                branch,
                &old_base,
                &parent_tip,
                &mut journal,
                remaining,
                &paths,
                ctx,
            )?;

            match outcome {
                RebaseOutcome::Success { new_tip: _ } => {
                    // Update metadata with new base
                    let store = MetadataStore::new(&git);

                    // Re-fetch metadata after rebase
                    let fresh_snapshot = scan(&git).context("Failed to re-scan after rebase")?;
                    let fresh_meta = fresh_snapshot
                        .metadata
                        .get(branch)
                        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

                    let mut updated = fresh_meta.metadata.clone();
                    updated.base = BaseInfo {
                        oid: parent_tip.to_string(),
                    };
                    updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

                    let new_ref_oid = store
                        .write_cas(branch, Some(&fresh_meta.ref_oid), &updated)
                        .with_context(|| format!("Failed to update metadata for '{}'", branch))?;

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
                    // Already paused by rebase_onto_with_journal
                    println!();
                    println!("Conflict while restacking '{}'.", branch);
                    println!("Resolve conflicts, then run 'lattice continue'.");
                    println!("To abort, run 'lattice abort'.");
                    return Ok(());
                }
                RebaseOutcome::NoOp => {
                    if ctx.debug {
                        eprintln!("[debug] '{}' no-op", branch);
                    }
                }
            }
        }
    }

    // Mark journal as committed
    journal.commit();
    journal.write(&paths)?;

    // Clear op-state
    OpState::remove(&paths)?;

    if !ctx.quiet {
        println!("Modify complete.");
    }

    Ok(())
}
