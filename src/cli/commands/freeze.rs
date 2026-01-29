//! freeze and unfreeze commands - Mark branches as frozen/unfrozen
//!
//! # Architecture
//!
//! These commands implement the `Command` trait and use
//! `requirements::MUTATING_METADATA_ONLY`. They flow through `run_command`
//! to ensure proper gating and executor-based execution.
//!
//! The freeze/unfreeze operations:
//! 1. Plan metadata updates for target branch(es)
//! 2. Execute via the single transactional executor with CAS semantics
//! 3. This ensures consistency and enables proper journaling

use crate::core::metadata::schema::{FreezeScope, FreezeState};
use crate::core::ops::journal::OpId;
use crate::core::types::BranchName;
use crate::engine::command::{Command, CommandOutput, SimpleCommand};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::{run_command, RunError};
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Command to freeze a branch.
pub struct FreezeCommand<'a> {
    ctx: &'a Context,
    branch: Option<&'a str>,
    only: bool,
}

impl Command for FreezeCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING_METADATA_ONLY;
    type Output = ();

    fn plan(&self, ready: &ReadyContext) -> Result<Plan, PlanError> {
        plan_freeze_state(ready, self.branch, self.only, true, self.ctx)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        self.simple_finish(result)
    }
}

impl SimpleCommand for FreezeCommand<'_> {}

/// Command to unfreeze a branch.
pub struct UnfreezeCommand<'a> {
    ctx: &'a Context,
    branch: Option<&'a str>,
    only: bool,
}

impl Command for UnfreezeCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING_METADATA_ONLY;
    type Output = ();

    fn plan(&self, ready: &ReadyContext) -> Result<Plan, PlanError> {
        plan_freeze_state(ready, self.branch, self.only, false, self.ctx)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        self.simple_finish(result)
    }
}

impl SimpleCommand for UnfreezeCommand<'_> {}

/// Plan freeze state changes for a branch (and optionally its ancestors).
fn plan_freeze_state(
    ready: &ReadyContext,
    branch: Option<&str>,
    only: bool,
    frozen: bool,
    ctx: &Context,
) -> Result<Plan, PlanError> {
    let snapshot = &ready.snapshot;

    // Resolve target branch
    let target = if let Some(name) = branch {
        BranchName::new(name)
            .map_err(|e| PlanError::InvalidState(format!("Invalid branch name: {}", e)))?
    } else if let Some(ref current) = snapshot.current_branch {
        current.clone()
    } else {
        return Err(PlanError::InvalidState(
            "Not on any branch and no branch specified".to_string(),
        ));
    };

    // Check if tracked
    if !snapshot.metadata.contains_key(&target) {
        return Err(PlanError::InvalidState(format!(
            "Branch '{}' is not tracked",
            target
        )));
    }

    // Get branches to update
    let branches_to_update = if only {
        vec![target.clone()]
    } else {
        // Include all ancestors (downstack)
        let mut branches = vec![target.clone()];
        let mut current = target.clone();
        while let Some(parent) = snapshot.graph.parent(&current) {
            // Stop at trunk (which isn't tracked)
            if !snapshot.metadata.contains_key(parent) {
                break;
            }
            branches.push(parent.clone());
            current = parent.clone();
        }
        branches
    };

    let action = if frozen { "freeze" } else { "unfreeze" };
    let mut plan = Plan::new(OpId::new(), action);

    for branch in &branches_to_update {
        // Get current metadata
        let scanned = snapshot.metadata.get(branch).ok_or_else(|| {
            PlanError::InvalidState(format!("Metadata not found for '{}'", branch))
        })?;

        // Skip if already in desired state
        if scanned.metadata.freeze.is_frozen() == frozen {
            if !ctx.quiet {
                let state = if frozen { "frozen" } else { "unfrozen" };
                println!("'{}' is already {}", branch, state);
            }
            continue;
        }

        // Create updated metadata
        let mut updated = scanned.metadata.clone();
        updated.freeze = if frozen {
            FreezeState::frozen(FreezeScope::Single, None)
        } else {
            FreezeState::Unfrozen
        };
        updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

        // Add step to plan
        plan = plan.with_step(PlanStep::WriteMetadataCas {
            branch: branch.to_string(),
            old_ref_oid: Some(scanned.ref_oid.to_string()),
            metadata: Box::new(updated),
        });

        if !ctx.quiet {
            let action_past = if frozen { "Freezing" } else { "Unfreezing" };
            println!("{} '{}'", action_past, branch);
        }
    }

    Ok(plan)
}

/// Mark a branch as frozen.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to freeze (defaults to current)
/// * `only` - Only freeze this branch, not downstack
///
/// # Gating
///
/// Uses `requirements::MUTATING_METADATA_ONLY` via `Command` trait.
pub fn freeze(ctx: &Context, branch: Option<&str>, only: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = FreezeCommand { ctx, branch, only };

    let output = run_command(&cmd, &git, ctx).map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })?;

    output.into_result().map_err(|e| anyhow::anyhow!("{}", e))
}

/// Unmark a branch as frozen.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to unfreeze (defaults to current)
/// * `only` - Only unfreeze this branch, not downstack
///
/// # Gating
///
/// Uses `requirements::MUTATING_METADATA_ONLY` via `Command` trait.
pub fn unfreeze(ctx: &Context, branch: Option<&str>, only: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = UnfreezeCommand { ctx, branch, only };

    let output = run_command(&cmd, &git, ctx).map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })?;

    output.into_result().map_err(|e| anyhow::anyhow!("{}", e))
}
