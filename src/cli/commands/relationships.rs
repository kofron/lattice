//! parent and children commands - Simple relationship queries
//!
//! # Architecture
//!
//! These are read-only commands that implement `ReadOnlyCommand` and use
//! `requirements::READ_ONLY`. They flow through `run_readonly_command` to
//! ensure proper gating.

use crate::engine::command::ReadOnlyCommand;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::PlanError;
use crate::engine::runner::{run_readonly_command, RunError};
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Command to print parent branch name.
pub struct ParentCommand<'a> {
    ctx: &'a Context,
}

impl ReadOnlyCommand for ParentCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
    type Output = ();

    fn execute(&self, ready: &ReadyContext) -> Result<Self::Output, PlanError> {
        let snapshot = &ready.snapshot;

        // Get current branch
        let current = snapshot
            .current_branch
            .as_ref()
            .ok_or_else(|| PlanError::InvalidState("Not on any branch".to_string()))?;

        // Check if tracked
        let metadata = snapshot.metadata.get(current);
        if metadata.is_none() {
            if !self.ctx.quiet {
                eprintln!("Branch '{}' is not tracked", current);
            }
            return Ok(());
        }

        // Get parent from graph
        if let Some(parent) = snapshot.graph.parent(current) {
            println!("{}", parent);
        }
        // No output if no parent (trunk-child)

        Ok(())
    }
}

/// Print parent branch name.
///
/// Outputs nothing (exit 0) if the branch has no parent (is trunk-child).
///
/// # Gating
///
/// Uses `requirements::READ_ONLY` via `ReadOnlyCommand` trait.
pub fn parent(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = ParentCommand { ctx };
    run_readonly_command(&cmd, &git, ctx).map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}

/// Command to print child branch names.
pub struct ChildrenCommand;

impl ReadOnlyCommand for ChildrenCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
    type Output = ();

    fn execute(&self, ready: &ReadyContext) -> Result<Self::Output, PlanError> {
        let snapshot = &ready.snapshot;

        // Get current branch
        let current = snapshot
            .current_branch
            .as_ref()
            .ok_or_else(|| PlanError::InvalidState("Not on any branch".to_string()))?;

        // Get children from graph
        if let Some(children) = snapshot.graph.children(current) {
            for child in children {
                println!("{}", child);
            }
        }
        // No output if no children

        Ok(())
    }
}

/// Print child branch names.
///
/// Outputs nothing (exit 0) if the branch has no children.
///
/// # Gating
///
/// Uses `requirements::READ_ONLY` via `ReadOnlyCommand` trait.
pub fn children(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = ChildrenCommand;
    run_readonly_command(&cmd, &git, ctx).map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}
