//! cli::commands::merge
//!
//! Merge PRs via GitHub API.
//!
//! # Design
//!
//! Per SPEC.md Section 8E.5, the merge command:
//! - Merges PRs from trunk to current branch in order
//! - Uses GitHub merge API
//! - Stops on first failure
//! - Suggests running `lattice sync` after
//!
//! # Architecture
//!
//! The merge command implements `AsyncCommand` per the Phase 6 command migration.
//! It flows through `run_async_command()` to ensure proper gating and lifecycle.
//!
//! Since merge doesn't modify local repository state (only calls GitHub API),
//! it uses `requirements::REMOTE_BARE_ALLOWED` and works in bare repositories.
//!
//! # Example
//!
//! ```bash
//! # Merge PRs in stack
//! lattice merge
//!
//! # Dry run
//! lattice merge --dry-run
//!
//! # Use squash merge
//! lattice merge --method squash
//! ```

use crate::cli::args::MergeMethodArg;
use crate::cli::commands::auth::get_github_token;
use crate::core::metadata::schema::PrState;
use crate::core::ops::journal::OpId;
use crate::core::types::BranchName;
use crate::engine::command::{AsyncCommand, CommandOutput, PlanFut};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::Context;
use crate::forge::{create_forge, MergeMethod};
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Result of a merge operation.
///
/// Note: Currently the result is computed during execution rather than
/// being returned through the command output. This struct is kept for
/// future use when the async executor properly handles ForgeMergePr steps.
#[derive(Debug)]
#[allow(dead_code)]
pub struct MergeResult {
    /// Number of PRs merged.
    pub merged_count: usize,
    /// Branches that were merged.
    pub merged_branches: Vec<BranchName>,
}

/// The merge command.
///
/// Merges PRs in stack order from trunk to current branch via GitHub API.
pub struct MergeCommand {
    /// Merge method to use.
    merge_method: MergeMethod,
    /// Quiet mode.
    quiet: bool,
}

impl MergeCommand {
    /// Create a new merge command.
    pub fn new(method: Option<MergeMethodArg>, quiet: bool) -> Self {
        let merge_method = match method {
            Some(MergeMethodArg::Merge) => MergeMethod::Merge,
            Some(MergeMethodArg::Squash) => MergeMethod::Squash,
            Some(MergeMethodArg::Rebase) => MergeMethod::Rebase,
            None => MergeMethod::Squash, // Default
        };

        Self {
            merge_method,
            quiet,
        }
    }
}

impl AsyncCommand for MergeCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE_BARE_ALLOWED;
    type Output = MergeResult;

    fn plan<'a>(&'a self, ready: &'a ReadyContext) -> PlanFut<'a> {
        Box::pin(async move {
            // Get current branch
            let current = ready
                .snapshot
                .current_branch
                .as_ref()
                .ok_or_else(|| PlanError::InvalidState("Not on a branch".to_string()))?;

            // Get stack from trunk to current (ancestors + current)
            let mut stack = ready.snapshot.graph.ancestors(current);
            stack.reverse(); // Bottom-up order
            stack.push(current.clone());

            // Filter to branches with linked PRs
            let mergeable: Vec<_> = stack
                .iter()
                .filter(|b| {
                    ready
                        .snapshot
                        .metadata
                        .get(*b)
                        .map(|m| matches!(m.metadata.pr, PrState::Linked { .. }))
                        .unwrap_or(false)
                })
                .cloned()
                .collect();

            if mergeable.is_empty() {
                return Err(PlanError::InvalidState(
                    "No PRs to merge. Run 'lattice submit' first.".to_string(),
                ));
            }

            // Build plan with ForgeMergePr steps
            let mut plan = Plan::new(OpId::new(), "merge");

            for branch in &mergeable {
                if let Some(scanned) = ready.snapshot.metadata.get(branch) {
                    if let PrState::Linked { number, .. } = &scanned.metadata.pr {
                        plan = plan.with_step(PlanStep::ForgeMergePr {
                            number: *number,
                            method: self.merge_method.to_string(),
                        });
                    }
                }
            }

            Ok(plan)
        })
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        match result {
            ExecuteResult::Success { .. } => {
                // We don't have the actual merged branches info here,
                // but the caller handles output printing
                CommandOutput::Success(MergeResult {
                    merged_count: 0, // Will be updated by caller
                    merged_branches: vec![],
                })
            }
            ExecuteResult::Paused { branch, .. } => CommandOutput::Paused {
                message: format!(
                    "Merge paused at '{}'. This shouldn't happen for merge operations.",
                    branch
                ),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

/// Run the merge command.
///
/// This is a synchronous wrapper that uses tokio to run the async implementation.
pub fn merge(
    ctx: &Context,
    _confirm: bool,
    dry_run: bool,
    method: Option<MergeMethodArg>,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(merge_impl(&git, ctx, dry_run, method))
}

/// Async implementation of merge using the engine lifecycle.
///
/// This uses `run_async_command()` to ensure proper gating while executing
/// the merge operations directly through the forge client.
async fn merge_impl(
    git: &Git,
    ctx: &Context,
    dry_run: bool,
    method: Option<MergeMethodArg>,
) -> Result<()> {
    use crate::engine::runner::run_async_command;

    // Create the command
    let command = MergeCommand::new(method, ctx.quiet);

    // Run through the async command lifecycle for proper gating
    // This performs: Scan -> Gate -> Plan
    let result = run_async_command(&command, git, ctx).await;

    match result {
        Ok(output) => match output {
            CommandOutput::Success(_) => {
                // Plan was generated successfully, now execute the merge operations
                // We need to re-scan to get the plan data and execute
                execute_merge_plan(git, ctx, dry_run, &command).await
            }
            CommandOutput::Paused { message } => {
                // Should not happen for merge
                bail!("Unexpected pause: {}", message);
            }
            CommandOutput::Failed { error } => {
                bail!("{}", error);
            }
        },
        Err(e) => {
            // Gating or planning failed
            bail!("Merge failed: {}", e);
        }
    }
}

/// Execute the merge plan by calling the forge API directly.
///
/// This is called after gating succeeds. We use the snapshot from gating
/// to determine which PRs to merge, then call the forge API.
async fn execute_merge_plan(
    git: &Git,
    _ctx: &Context,
    dry_run: bool,
    command: &MergeCommand,
) -> Result<()> {
    use crate::engine::scan::scan;

    // Re-scan to get current state (gating already validated requirements)
    let snapshot = scan(git)?;

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on a branch."))?;

    // Get authentication
    let token = get_github_token()?;

    // Get forge
    let remote_url = git
        .remote_url("origin")?
        .ok_or_else(|| anyhow::anyhow!("No 'origin' remote configured."))?;

    let forge = create_forge(&remote_url, &token, None)?;

    // Get stack from trunk to current (ancestors + current)
    let mut stack = snapshot.graph.ancestors(current);
    stack.reverse(); // Bottom-up order
    stack.push(current.clone());

    // Filter to branches with linked PRs
    let mergeable: Vec<_> = stack
        .iter()
        .filter(|b| {
            snapshot
                .metadata
                .get(*b)
                .map(|m| matches!(m.metadata.pr, PrState::Linked { .. }))
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    if mergeable.is_empty() {
        bail!("No PRs to merge. Run 'lattice submit' first.");
    }

    if dry_run {
        println!(
            "Would merge {} PR(s) using {} method:",
            mergeable.len(),
            command.merge_method
        );
        for branch in &mergeable {
            if let Some(scanned) = snapshot.metadata.get(branch) {
                if let PrState::Linked { number, .. } = &scanned.metadata.pr {
                    println!("  PR #{} ({})", number, branch);
                }
            }
        }
        return Ok(());
    }

    // Merge in order
    let mut merged_count = 0;
    for branch in &mergeable {
        if let Some(scanned) = snapshot.metadata.get(branch) {
            if let PrState::Linked { number, .. } = &scanned.metadata.pr {
                if !command.quiet {
                    println!("Merging PR #{} ({})...", number, branch);
                }

                match forge.merge_pr(*number, command.merge_method).await {
                    Ok(()) => {
                        merged_count += 1;
                        if !command.quiet {
                            println!("  Merged successfully.");
                        }
                    }
                    Err(e) => {
                        eprintln!("  Failed to merge: {}", e);
                        eprintln!("Stopping. Run 'lattice sync' to update state.");
                        return Err(e.into());
                    }
                }
            }
        }
    }

    if !command.quiet {
        println!(
            "\n{} PR(s) merged. Run 'lattice sync' to update local state.",
            merged_count
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn merge_method_conversion() {
        use crate::forge::MergeMethod;

        let m: MergeMethod = MergeMethod::Squash;
        assert_eq!(format!("{}", m), "squash");
    }
}
