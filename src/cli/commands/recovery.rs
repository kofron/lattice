//! continue and abort commands - Resume or cancel paused operations

use crate::core::ops::journal::{OpPhase, OpState};
use crate::engine::Context;
use crate::git::{Git, GitState};
use anyhow::{bail, Context as _, Result};
use std::process::Command;

/// Continue a paused operation after resolving conflicts.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `all` - Stage all changes before continuing
pub fn continue_op(ctx: &Context, all: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let git_dir = git.git_dir();

    // Check for in-progress operation
    let op_state =
        OpState::read(git_dir)?.ok_or_else(|| anyhow::anyhow!("No operation in progress"))?;

    if op_state.phase != OpPhase::Paused {
        bail!(
            "Operation '{}' is not paused (phase: {:?})",
            op_state.command,
            op_state.phase
        );
    }

    // Check git state
    let git_state = git.state();
    if !git_state.is_in_progress() {
        // Git operation already completed somehow
        if !ctx.quiet {
            println!("Git operation appears to be complete. Cleaning up...");
        }
        OpState::remove(git_dir)?;
        return Ok(());
    }

    // Stage all if requested
    if all {
        let status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add")?;

        if !status.success() {
            bail!("git add failed");
        }
    }

    // Continue the git operation
    let continue_args = match git_state {
        GitState::Rebase { .. } => vec!["rebase", "--continue"],
        GitState::Merge => vec!["merge", "--continue"],
        GitState::CherryPick => vec!["cherry-pick", "--continue"],
        GitState::Revert => vec!["revert", "--continue"],
        GitState::Bisect => bail!("Cannot continue a bisect operation with lattice"),
        GitState::ApplyMailbox => vec!["am", "--continue"],
        GitState::Clean => bail!("No git operation in progress"),
    };

    if !ctx.quiet {
        println!("Continuing {}...", op_state.command);
    }

    let status = Command::new("git")
        .args(&continue_args)
        .current_dir(&cwd)
        .status()
        .context("Failed to continue git operation")?;

    if !status.success() {
        // Check if still in conflict
        let new_state = git.state();
        if new_state.is_in_progress() {
            println!();
            println!("Conflicts remain. Resolve them and run 'lattice continue' again.");
            return Ok(());
        }
        bail!("git {} failed", continue_args.join(" "));
    }

    // Git operation completed - check if there are more steps
    // For now, we assume the operation is complete
    // A full implementation would read the journal and continue remaining steps

    // Clear op-state
    OpState::remove(git_dir)?;

    if !ctx.quiet {
        println!("Operation '{}' completed.", op_state.command);
    }

    Ok(())
}

/// Abort a paused operation and restore pre-operation state.
pub fn abort(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let git_dir = git.git_dir();

    // Check for in-progress operation
    let op_state =
        OpState::read(git_dir)?.ok_or_else(|| anyhow::anyhow!("No operation in progress"))?;

    if !ctx.quiet {
        println!("Aborting {}...", op_state.command);
    }

    // Abort the git operation if any
    let git_state = git.state();
    let abort_args: Option<Vec<&str>> = match git_state {
        GitState::Rebase { .. } => Some(vec!["rebase", "--abort"]),
        GitState::Merge => Some(vec!["merge", "--abort"]),
        GitState::CherryPick => Some(vec!["cherry-pick", "--abort"]),
        GitState::Revert => Some(vec!["revert", "--abort"]),
        GitState::Bisect => Some(vec!["bisect", "reset"]),
        GitState::ApplyMailbox => Some(vec!["am", "--abort"]),
        GitState::Clean => None,
    };

    if let Some(args) = abort_args {
        let status = Command::new("git")
            .args(&args)
            .current_dir(&cwd)
            .status()
            .context("Failed to abort git operation")?;

        if !status.success() {
            eprintln!("Warning: git {} may have failed", args.join(" "));
        }
    }

    // Read journal to rollback ref changes
    // For now, we just clear the op-state
    // A full implementation would use journal.ref_updates_for_rollback()

    // Clear op-state
    OpState::remove(git_dir)?;

    if !ctx.quiet {
        println!("Operation '{}' aborted.", op_state.command);
    }

    Ok(())
}
