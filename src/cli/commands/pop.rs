//! pop command - Delete branch but keep changes uncommitted
//!
//! This command implements the `Command` trait for the unified lifecycle.
//!
//! # Gating
//!
//! Uses `requirements::MUTATING` - requires working directory, trunk known,
//! no ops in progress, frozen policy satisfied.
//!
//! # Plan Generation
//!
//! 1. For each child: WriteMetadataCas to reparent
//! 2. RunGit: checkout parent
//! 3. RunGit: git branch -D <current>
//! 4. DeleteMetadataCas: remove metadata
//!
//! Note: The diff application happens POST-PLAN as it's not a transactional
//! operation - it leaves changes uncommitted in the working tree.
//!
//! Per SPEC.md 8D.9:
//!
//! - Delete current branch but keep its net changes applied to parent as uncommitted changes
//! - Requires clean working tree at start
//! - Must remove metadata and re-parent children
//!
//! # Integrity Contract
//!
//! - Must never pop frozen branches
//! - Must require clean working tree
//! - Metadata updated only after refs succeed

use std::io::Write as IoWrite;
use std::process::{Command as StdCommand, Stdio};

use anyhow::{Context as _, Result};

use crate::cli::commands::phase3_helpers::{get_net_diff, is_working_tree_clean};
use crate::core::metadata::schema::{BaseInfo, ParentInfo};
use crate::core::ops::journal::OpId;
use crate::core::types::{BranchName, Oid};
use crate::engine::command::{Command, CommandOutput};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::run_command;
use crate::engine::Context;
use crate::git::Git;

/// Pop current branch, keeping changes uncommitted.
///
/// # Arguments
///
/// * `ctx` - Execution context
pub fn pop(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // Require clean working tree BEFORE entering command lifecycle
    if !is_working_tree_clean(&cwd)? {
        anyhow::bail!("Working tree is not clean. Commit or stash your changes first.");
    }

    // Do preliminary scan to compute the diff BEFORE the command runs
    // This is because we need the diff computed at the current state,
    // and it's not a transactional operation.
    let preliminary_snapshot =
        crate::engine::scan::scan(&git).context("Failed to scan repository")?;

    let trunk = preliminary_snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    let current = preliminary_snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    // Check if tracked
    if !preliminary_snapshot.metadata.contains_key(&current) {
        anyhow::bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            current
        );
    }

    let current_meta = preliminary_snapshot
        .metadata
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    // Get parent name
    let parent_name = if current_meta.metadata.parent.is_trunk() {
        trunk.clone()
    } else {
        BranchName::new(current_meta.metadata.parent.name())
            .context("Invalid parent name in metadata")?
    };

    // Get base and tip for diff
    let base_oid = Oid::new(&current_meta.metadata.base.oid).context("Invalid base OID")?;
    let current_tip = preliminary_snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?;

    // Compute the diff now, before the branch is deleted
    let diff = get_net_diff(&cwd, &base_oid, current_tip)?;

    if !ctx.quiet {
        println!(
            "Popping '{}' (changes will be uncommitted on '{}')...",
            current, parent_name
        );
        if diff.is_empty() {
            println!("  No changes in branch.");
        }
    }

    let cmd = PopCommand {
        branch: current.clone(),
    };

    let output = run_command(&cmd, &git, ctx).map_err(|e| match e {
        crate::engine::runner::RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })?;

    match output {
        CommandOutput::Success(result) => {
            // POST-PLAN: Apply the diff as uncommitted changes
            if !diff.is_empty() {
                apply_diff(&cwd, &diff, ctx.quiet)?;
            }

            if !ctx.quiet {
                println!(
                    "Pop complete. Changes are staged on '{}'.",
                    result.parent_name
                );
            }
            Ok(())
        }
        CommandOutput::Paused { message } => {
            println!("{}", message);
            Ok(())
        }
        CommandOutput::Failed { error } => Err(anyhow::anyhow!("{}", error)),
    }
}

/// Apply a diff as uncommitted changes.
fn apply_diff(cwd: &std::path::Path, diff: &str, quiet: bool) -> Result<()> {
    // Try with --3way first
    let mut child = StdCommand::new("git")
        .args(["apply", "--3way"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn git apply")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(diff.as_bytes())
            .context("Failed to write diff to git apply")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for git apply")?;

    if !output.status.success() {
        // Try without --3way
        let mut child = StdCommand::new("git")
            .args(["apply"])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn git apply")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(diff.as_bytes())
                .context("Failed to write diff to git apply")?;
        }

        let output = child.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Warning: Could not apply changes cleanly: {}", stderr);
            eprintln!("The branch has been deleted but changes may be incomplete.");
        }
    }

    if !quiet {
        println!("  Applied changes as uncommitted files.");
    }

    Ok(())
}

/// Result from a pop operation.
#[derive(Debug)]
pub struct PopResult {
    /// Name of the parent branch (where we are now).
    pub parent_name: String,
}

/// Command struct for pop operation.
pub struct PopCommand {
    /// Branch to pop (must be current branch).
    branch: BranchName,
}

impl Command for PopCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = PopResult;

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let snapshot = &ctx.snapshot;

        // Get trunk
        let trunk = snapshot
            .trunk()
            .ok_or_else(|| PlanError::MissingData("trunk not configured".to_string()))?;

        // Verify we're on the target branch
        let current = snapshot
            .current_branch
            .as_ref()
            .ok_or_else(|| PlanError::InvalidState("Not on any branch".to_string()))?;

        if current != &self.branch {
            return Err(PlanError::InvalidState(format!(
                "Expected to be on '{}' but on '{}'",
                self.branch, current
            )));
        }

        // Check if tracked
        if !snapshot.metadata.contains_key(&self.branch) {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' is not tracked",
                self.branch
            )));
        }

        // Get metadata
        let current_meta = snapshot.metadata.get(&self.branch).ok_or_else(|| {
            PlanError::MissingData(format!("Metadata not found for '{}'", self.branch))
        })?;

        // Check freeze policy
        if current_meta.metadata.freeze.is_frozen() {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' is frozen. Unfreeze it first.",
                self.branch
            )));
        }

        // Check freeze on children too
        if let Some(children) = snapshot.graph.children(&self.branch) {
            for child in children {
                if let Some(child_meta) = snapshot.metadata.get(child) {
                    if child_meta.metadata.freeze.is_frozen() {
                        return Err(PlanError::InvalidState(format!(
                            "Child branch '{}' is frozen. Unfreeze it first.",
                            child
                        )));
                    }
                }
            }
        }

        // Get parent
        let parent_name = if current_meta.metadata.parent.is_trunk() {
            trunk.clone()
        } else {
            BranchName::new(current_meta.metadata.parent.name())
                .map_err(|e| PlanError::MissingData(format!("Invalid parent name: {}", e)))?
        };

        // Build plan
        let mut plan = Plan::new(OpId::new(), "pop");

        // Get parent's tip for updating children's base
        let parent_tip_oid = snapshot.branch_tip(&parent_name).ok_or_else(|| {
            PlanError::MissingData(format!("Branch tip not found for parent '{}'", parent_name))
        })?;

        // Step 1: Reparent children
        if let Some(children) = snapshot.graph.children(&self.branch) {
            for child in children {
                if let Some(child_scanned) = snapshot.metadata.get(child) {
                    let mut updated = child_scanned.metadata.clone();
                    if &parent_name == trunk {
                        updated.parent = ParentInfo::Trunk {
                            name: parent_name.to_string(),
                        };
                    } else {
                        updated.parent = ParentInfo::Branch {
                            name: parent_name.to_string(),
                        };
                    }
                    // Update base to the new parent's tip
                    updated.base = BaseInfo {
                        oid: parent_tip_oid.to_string(),
                    };
                    updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

                    plan = plan.with_step(PlanStep::WriteMetadataCas {
                        branch: child.to_string(),
                        old_ref_oid: Some(child_scanned.ref_oid.to_string()),
                        metadata: Box::new(updated),
                    });
                }
            }
        }

        // Step 2: Checkout parent
        plan = plan.with_step(PlanStep::RunGit {
            args: vec!["checkout".to_string(), parent_name.to_string()],
            description: format!("Checkout parent '{}'", parent_name),
            expected_effects: vec![],
        });

        // Step 3: Delete branch
        plan = plan.with_step(PlanStep::RunGit {
            args: vec![
                "branch".to_string(),
                "-D".to_string(),
                self.branch.to_string(),
            ],
            description: format!("Delete branch '{}'", self.branch),
            expected_effects: vec![], // Ref no longer exists after delete
        });

        // Step 4: Delete metadata
        plan = plan.with_step(PlanStep::DeleteMetadataCas {
            branch: self.branch.to_string(),
            old_ref_oid: current_meta.ref_oid.to_string(),
        });

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        match result {
            ExecuteResult::Success { .. } => {
                // We don't have direct access to parent name from ExecuteResult
                CommandOutput::Success(PopResult {
                    parent_name: "parent".to_string(), // Placeholder, caller knows the actual name
                })
            }
            ExecuteResult::Paused {
                branch, git_state, ..
            } => CommandOutput::Paused {
                message: format!(
                    "Paused for {} on '{}'. Resolve and run 'lattice continue', or 'lattice abort'.",
                    git_state.description(),
                    branch
                ),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}
