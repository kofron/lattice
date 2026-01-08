//! squash command - Squash all commits in current branch into one
//!
//! Per SPEC.md 8D.7:
//!
//! - Squash all commits unique to current branch into one
//! - Preserve parent relation
//! - Restack descendants
//! - Respect freeze
//!
//! # Integrity Contract
//!
//! - Must never rewrite frozen branches
//! - Metadata updated only after refs succeed

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
use crate::core::types::Oid;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Squash all commits in current branch into one.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `message` - Commit message for squashed commit
/// * `edit` - Open editor for commit message
pub fn squash(ctx: &Context, message: Option<&str>, edit: bool) -> Result<()> {
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

    // Get descendants for freeze check
    let descendants = get_descendants_inclusive(&current, &snapshot);

    // Check freeze policy
    check_freeze_affected_set(&descendants, &snapshot)?;

    // Get current metadata
    let scanned = snapshot
        .metadata
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    let base_oid = Oid::new(&scanned.metadata.base.oid).context("Invalid base OID")?;

    // Get current tip
    let current_tip = snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?;

    // Count commits
    let commit_count = count_commits_in_range(&cwd, &base_oid, current_tip)?;

    if commit_count <= 1 {
        if !ctx.quiet {
            println!(
                "Branch '{}' has {} commit(s). Nothing to squash.",
                current, commit_count
            );
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!("Squashing {} commits on '{}'...", commit_count, current);
    }

    // Acquire lock
    let _lock = RepoLock::acquire(git_dir).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("squash");

    // Write op-state
    let op_state = OpState::from_journal(&journal);
    op_state.write(git_dir)?;

    // Record old tip
    journal.record_ref_update(
        format!("refs/heads/{}", current),
        Some(current_tip.to_string()),
        "pending".to_string(),
    );

    // Get commit messages for default squash message
    let log_output = Command::new("git")
        .args([
            "log",
            "--reverse",
            "--format=%B%n---",
            &format!("{}..{}", base_oid.as_str(), current_tip.as_str()),
        ])
        .current_dir(&cwd)
        .output()
        .context("Failed to get commit messages")?;

    let combined_messages = String::from_utf8_lossy(&log_output.stdout).to_string();

    // Soft reset to base
    let status = Command::new("git")
        .args(["reset", "--soft", base_oid.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to soft reset")?;

    if !status.success() {
        OpState::remove(git_dir)?;
        bail!("git reset --soft failed");
    }

    // Create new squashed commit
    if let Some(msg) = message {
        let status = Command::new("git")
            .args(["commit", "-m", msg])
            .current_dir(&cwd)
            .status()
            .context("Failed to create squashed commit")?;

        if !status.success() {
            // Restore original state
            let _ = Command::new("git")
                .args(["reset", "--soft", current_tip.as_str()])
                .current_dir(&cwd)
                .status();
            OpState::remove(git_dir)?;
            bail!("git commit failed");
        }
    } else if edit {
        let mut commit_args = vec!["commit"];
        // Use combined messages as template
        // Write to temp file for editor
        let temp_msg_file = git_dir.join("SQUASH_MSG");
        std::fs::write(&temp_msg_file, &combined_messages)
            .context("Failed to write squash message template")?;
        commit_args.push("-F");
        // Need to keep the path alive
        let temp_path_str = temp_msg_file.to_string_lossy().to_string();
        commit_args.push(&temp_path_str);
        commit_args.push("-e");

        let status = Command::new("git")
            .args(&commit_args)
            .current_dir(&cwd)
            .status()
            .context("Failed to create squashed commit")?;

        if !status.success() {
            // Restore original state
            let _ = Command::new("git")
                .args(["reset", "--soft", current_tip.as_str()])
                .current_dir(&cwd)
                .status();
            OpState::remove(git_dir)?;
            bail!("git commit failed or was aborted");
        }
    } else {
        // Use first commit's message as default
        let first_msg = combined_messages
            .split("---")
            .next()
            .unwrap_or("Squashed commits")
            .trim();

        let status = Command::new("git")
            .args(["commit", "-m", first_msg])
            .current_dir(&cwd)
            .status()
            .context("Failed to create squashed commit")?;

        if !status.success() {
            // Restore original state
            let _ = Command::new("git")
                .args(["reset", "--soft", current_tip.as_str()])
                .current_dir(&cwd)
                .status();
            OpState::remove(git_dir)?;
            bail!("git commit failed");
        }
    }

    // Get new tip
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&cwd)
        .output()
        .context("Failed to get HEAD")?;

    let new_tip = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Update journal with actual new tip
    journal.record_ref_update(
        format!("refs/heads/{}", current),
        Some(current_tip.to_string()),
        new_tip.clone(),
    );

    if !ctx.quiet {
        println!("  Squashed to commit {}", &new_tip[..7]);
    }

    // Restack descendants
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

        let ordered = topological_sort(&descendants_to_restack, &snapshot);
        let updated_snapshot = scan(&git).context("Failed to re-scan repository")?;

        for (idx, branch) in ordered.iter().enumerate() {
            let branch_meta = updated_snapshot
                .metadata
                .get(branch)
                .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

            if branch_meta.metadata.freeze.is_frozen() {
                if !ctx.quiet {
                    println!("  Skipping frozen branch '{}'", branch);
                }
                continue;
            }

            let parent_tip = get_parent_tip(branch, &updated_snapshot, trunk)?;

            if branch_meta.metadata.base.oid.as_str() == parent_tip.as_str() {
                continue;
            }

            let old_base = Oid::new(&branch_meta.metadata.base.oid).context("Invalid base OID")?;

            let remaining: Vec<String> = ordered
                .iter()
                .skip(idx + 1)
                .map(|b| b.to_string())
                .collect();

            let outcome = rebase_onto_with_journal(
                &git,
                &cwd,
                branch,
                &old_base,
                &parent_tip,
                &mut journal,
                remaining,
                git_dir,
                ctx,
            )?;

            match outcome {
                RebaseOutcome::Success { new_tip: _ } => {
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
        println!("Squash complete.");
    }

    Ok(())
}
