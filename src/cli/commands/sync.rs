//! cli::commands::sync
//!
//! Sync with remote (fetch, update trunk, detect merged PRs).
//!
//! # Design
//!
//! Per SPEC.md Section 8E.3, the sync command:
//! - Fetches from remote
//! - Fast-forwards trunk (or errors if diverged without --force)
//! - Detects merged/closed PRs and prompts to delete local branches
//! - Updates stack comments in PR descriptions
//! - Optionally restacks after syncing
//!
//! # Architecture
//!
//! The sync command implements `AsyncCommand` per the Phase 6 command migration.
//! It uses mode dispatch (`SyncMode`) for bare repository handling:
//!
//! - `WithRestack`: May restack after sync, requires working directory
//! - `NoRestack`: Bare-repo compatible, skips restack
//!
//! Per SPEC.md Section 4.6.7, in bare repositories:
//! - `lattice sync` MUST refuse unless `--no-restack` is provided
//! - With `--no-restack`: may fetch, trunk FF, PR checks, branch deletion prompts
//! - MUST NOT attempt any rebase/restack
//!
//! # Example
//!
//! ```bash
//! # Sync with remote
//! lattice sync
//!
//! # Force reset trunk to remote
//! lattice sync --force
//!
//! # Restack after syncing
//! lattice sync --restack
//!
//! # Sync from bare repo (no restack)
//! lattice sync --no-restack
//! ```

use crate::core::ops::journal::OpId;
use crate::core::types::BranchName;
use crate::engine::command::{AsyncCommand, CommandOutput, PlanFut};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::modes::{ModeError, SyncMode};
use crate::engine::plan::{Plan, PlanStep};
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

use super::stack_comment_ops::update_stack_comments_for_branches;

/// Result of a sync operation.
#[derive(Debug)]
#[allow(dead_code)]
pub struct SyncResult {
    /// Whether trunk was updated.
    pub trunk_updated: bool,
    /// Number of branches restacked.
    pub branches_restacked: usize,
    /// Branches with merged/closed PRs detected.
    pub merged_prs: Vec<BranchName>,
}

/// Sync command arguments.
#[derive(Debug, Clone)]
pub struct SyncArgs {
    /// Force reset trunk even if diverged.
    pub force: bool,
    /// Restack branches after syncing.
    pub restack: bool,
    /// Quiet mode.
    pub quiet: bool,
    /// Verify commits with hooks.
    pub verify: bool,
}

/// The sync command for WithRestack mode.
pub struct SyncWithRestackCommand {
    #[allow(dead_code)]
    args: SyncArgs,
}

impl SyncWithRestackCommand {
    /// Create a new sync command with restack mode.
    pub fn new(args: SyncArgs) -> Self {
        Self { args }
    }
}

impl AsyncCommand for SyncWithRestackCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE;
    type Output = SyncResult;

    fn plan<'a>(&'a self, _ready: &'a ReadyContext) -> PlanFut<'a> {
        Box::pin(async move {
            // Build a plan with ForgeFetch step
            let plan = Plan::new(OpId::new(), "sync").with_step(PlanStep::ForgeFetch {
                remote: "origin".to_string(),
                refspec: None,
            });

            Ok(plan)
        })
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        match result {
            ExecuteResult::Success { .. } => CommandOutput::Success(SyncResult {
                trunk_updated: false,
                branches_restacked: 0,
                merged_prs: vec![],
            }),
            ExecuteResult::Paused { branch, .. } => CommandOutput::Paused {
                message: format!("Sync paused at '{}'. This shouldn't happen.", branch),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

/// The sync command for NoRestack mode (bare repo compatible).
pub struct SyncNoRestackCommand {
    #[allow(dead_code)]
    args: SyncArgs,
}

impl SyncNoRestackCommand {
    /// Create a new sync command without restack mode.
    pub fn new(args: SyncArgs) -> Self {
        Self { args }
    }
}

impl AsyncCommand for SyncNoRestackCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE_BARE_ALLOWED;
    type Output = SyncResult;

    fn plan<'a>(&'a self, _ready: &'a ReadyContext) -> PlanFut<'a> {
        Box::pin(async move {
            // Build a plan with ForgeFetch step
            let plan = Plan::new(OpId::new(), "sync").with_step(PlanStep::ForgeFetch {
                remote: "origin".to_string(),
                refspec: None,
            });

            Ok(plan)
        })
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        match result {
            ExecuteResult::Success { .. } => CommandOutput::Success(SyncResult {
                trunk_updated: false,
                branches_restacked: 0,
                merged_prs: vec![],
            }),
            ExecuteResult::Paused { branch, .. } => CommandOutput::Paused {
                message: format!("Sync paused at '{}'. This shouldn't happen.", branch),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

/// Run the sync command.
///
/// This is a synchronous wrapper that uses tokio to run the async implementation.
/// It uses mode dispatch for bare repository handling per SPEC.md §4.6.7.
pub fn sync(ctx: &Context, force: bool, restack: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // Resolve mode from flags and repo context
    // Note: sync uses --restack flag (opt-in restack), so no_restack = !restack
    let is_bare = git.info()?.work_dir.is_none();
    let mode = SyncMode::resolve(!restack, is_bare).map_err(|e| match e {
        ModeError::BareRepoRequiresFlag { command, required_flag } => {
            anyhow::anyhow!(
                "This is a bare repository. The `{}` command cannot restack without a working directory.\n\n\
                 To sync without restacking (fetch, trunk FF, PR checks only), use:\n\n\
                     lattice sync {}",
                command,
                required_flag
            )
        }
    })?;

    let args = SyncArgs {
        force,
        restack,
        quiet: ctx.quiet,
        verify: ctx.verify,
    };

    let rt = tokio::runtime::Runtime::new()?;
    match mode {
        SyncMode::WithRestack => rt.block_on(sync_with_restack_impl(&git, ctx, args)),
        SyncMode::NoRestack => rt.block_on(sync_no_restack_impl(&git, ctx, args)),
    }
}

/// Async implementation for WithRestack mode.
async fn sync_with_restack_impl(git: &Git, ctx: &Context, args: SyncArgs) -> Result<()> {
    use crate::engine::runner::run_async_command;

    let command = SyncWithRestackCommand::new(args.clone());

    // Run through async command lifecycle for gating
    let result = run_async_command(&command, git, ctx).await;

    match result {
        Ok(output) => match output {
            CommandOutput::Success(_) => {
                // Gating passed, now execute sync operations
                execute_sync(git, ctx, &args).await
            }
            CommandOutput::Paused { message } => bail!("Unexpected pause: {}", message),
            CommandOutput::Failed { error } => bail!("{}", error),
        },
        Err(e) => bail!("Sync failed: {}", e),
    }
}

/// Async implementation for NoRestack mode.
async fn sync_no_restack_impl(git: &Git, ctx: &Context, args: SyncArgs) -> Result<()> {
    use crate::engine::runner::run_async_command;

    let command = SyncNoRestackCommand::new(args.clone());

    // Run through async command lifecycle for gating
    let result = run_async_command(&command, git, ctx).await;

    match result {
        Ok(output) => match output {
            CommandOutput::Success(_) => {
                // Gating passed, now execute sync operations (without restack)
                execute_sync(git, ctx, &args).await
            }
            CommandOutput::Paused { message } => bail!("Unexpected pause: {}", message),
            CommandOutput::Failed { error } => bail!("{}", error),
        },
        Err(e) => bail!("Sync failed: {}", e),
    }
}

/// Execute the sync operations after gating succeeds.
async fn execute_sync(git: &Git, ctx: &Context, args: &SyncArgs) -> Result<()> {
    use crate::cli::commands::auth::get_github_token;
    use crate::core::metadata::schema::PrState;
    use crate::engine::scan::scan;
    use crate::forge::PrState as ForgePrState;
    use std::process::Command;

    let cwd = git
        .info()?
        .git_dir
        .parent()
        .unwrap_or(&git.info()?.git_dir)
        .to_path_buf();
    let snapshot = scan(git)?;

    // Get trunk
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Fetch from remote
    if !args.quiet {
        println!("Fetching from origin...");
    }

    let fetch_status = Command::new("git")
        .current_dir(&cwd)
        .args(["fetch", "origin"])
        .status()?;

    if !fetch_status.success() {
        bail!("git fetch failed");
    }

    // Check trunk state
    let local_trunk = format!("refs/heads/{}", trunk);
    let remote_trunk = format!("refs/remotes/origin/{}", trunk);

    let local_oid = git.resolve_ref(&local_trunk)?;
    let remote_oid = match git.resolve_ref(&remote_trunk) {
        Ok(oid) => oid,
        Err(_) => {
            if !ctx.quiet {
                println!("Remote trunk not found. Nothing to sync.");
            }
            return Ok(());
        }
    };

    if local_oid != remote_oid {
        // Check if we can fast-forward
        let is_ancestor = git.is_ancestor(&local_oid, &remote_oid)?;

        if is_ancestor {
            // Fast-forward
            if !args.quiet {
                println!("Fast-forwarding {} to origin/{}...", trunk, trunk);
            }

            let ff_status = Command::new("git")
                .current_dir(&cwd)
                .args(["checkout", trunk.as_str()])
                .status()?;

            if !ff_status.success() {
                bail!("git checkout failed");
            }

            let remote_ref = format!("origin/{}", trunk);
            let mut merge_args = vec!["merge"];
            if !args.verify {
                merge_args.push("--no-verify");
            }
            merge_args.extend(["--ff-only", &remote_ref]);
            let merge_status = Command::new("git")
                .current_dir(&cwd)
                .args(&merge_args)
                .status()?;

            if !merge_status.success() {
                bail!("git merge --ff-only failed");
            }
        } else if args.force {
            // Force reset
            if !args.quiet {
                println!(
                    "Force resetting {} to origin/{} (diverged)...",
                    trunk, trunk
                );
            }

            let checkout_status = Command::new("git")
                .current_dir(&cwd)
                .args(["checkout", trunk.as_str()])
                .status()?;

            if !checkout_status.success() {
                bail!("git checkout failed");
            }

            let reset_status = Command::new("git")
                .current_dir(&cwd)
                .args(["reset", "--hard", &format!("origin/{}", trunk)])
                .status()?;

            if !reset_status.success() {
                bail!("git reset --hard failed");
            }
        } else {
            bail!(
                "Trunk '{}' has diverged from origin. Use --force to reset.",
                trunk
            );
        }
    } else if !args.quiet {
        println!("Trunk '{}' is up to date.", trunk);
    }

    // Check PR states for tracked branches and update stack comments (requires auth)
    if let Ok(token) = get_github_token() {
        let remote_url = git.remote_url("origin")?;
        if let Some(url) = remote_url {
            if let Ok(forge) = crate::forge::create_forge(&url, &token, None) {
                let mut open_branches = Vec::new();

                for (branch, scanned) in &snapshot.metadata {
                    if let PrState::Linked { number, .. } = &scanned.metadata.pr {
                        match forge.get_pr(*number).await {
                            Ok(pr) => {
                                if pr.state == ForgePrState::Merged
                                    || pr.state == ForgePrState::Closed
                                {
                                    if !args.quiet {
                                        println!(
                                            "PR #{} for '{}' is {}.",
                                            number, branch, pr.state
                                        );
                                        // Would prompt to delete in interactive mode
                                    }
                                } else {
                                    // PR is still open, track for stack comment update
                                    open_branches.push(branch.clone());
                                }
                            }
                            Err(e) => {
                                if !args.quiet {
                                    eprintln!(
                                        "Warning: Could not check PR #{} for '{}': {}",
                                        number, branch, e
                                    );
                                }
                            }
                        }
                    }
                }

                // Update stack comments for all open PRs
                // This keeps PR descriptions in sync after merges/changes
                if !open_branches.is_empty() {
                    if !args.quiet {
                        println!("Updating stack comments...");
                    }

                    let updated = update_stack_comments_for_branches(
                        forge.as_ref(),
                        &snapshot,
                        &open_branches,
                        args.quiet,
                    )
                    .await?;

                    if updated > 0 && !args.quiet {
                        println!("  Updated {} PR description(s)", updated);
                    }
                }
            }
        }
    }

    // Restack if requested (per SPEC.md 8E.3)
    // "If --restack enabled: restack all restackable branches; skip those that conflict and report"
    if args.restack {
        if !args.quiet {
            println!("Restacking branches...");
        }

        // Restack from trunk to catch all branches that may need realignment
        // after trunk was updated. This reuses the full restack implementation:
        // - Lock acquisition
        // - Journal management for crash safety
        // - Conflict detection and pause/continue model
        // - Frozen branch skipping
        // - Topological ordering for correct rebase sequence
        super::restack::restack(ctx, Some(trunk.as_str()), false, false)?;
    }

    if !args.quiet {
        println!("Sync complete.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sync_command_compiles() {
        // Basic compilation test - verifies module structure
    }
}
