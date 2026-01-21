//! fold command - Merge current branch into parent and delete
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
//! 1. RunGit: checkout parent
//! 2. RunGit: merge current into parent
//! 3. PotentialConflictPause
//! 4. For each child: WriteMetadataCas to reparent
//! 5. RunGit: git branch -D current
//! 6. DeleteMetadataCas: remove current's metadata
//! 7. (If --keep): Additional steps to rename parent to current
//!
//! Per SPEC.md 8D.8:
//!
//! - Merge current branch's changes into its parent, then delete current branch
//! - Re-parent children to parent
//! - --keep: keep the current branch name by renaming parent branch to current name after fold
//!
//! # Integrity Contract
//!
//! - Must never fold frozen branches
//! - Must re-parent children before deleting
//! - Metadata updated only after refs succeed

use anyhow::{Context as _, Result};

use crate::core::metadata::schema::{BaseInfo, BranchInfo, ParentInfo};
use crate::core::ops::journal::OpId;
use crate::core::types::BranchName;
use crate::engine::command::{Command, CommandOutput, SimpleCommand};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::run_command;
use crate::engine::Context;
use crate::git::Git;

/// Fold current branch into parent.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `keep` - Keep the current branch name by renaming parent
pub fn fold(ctx: &Context, keep: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = FoldCommand {
        keep,
        verify: ctx.verify,
    };

    let output = run_command(&cmd, &git, ctx).map_err(|e| match e {
        crate::engine::runner::RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })?;

    match output {
        CommandOutput::Success(()) => {
            if !ctx.quiet {
                println!("Fold complete.");
            }
            Ok(())
        }
        CommandOutput::Paused { message } => {
            println!();
            println!("{}", message);
            Ok(())
        }
        CommandOutput::Failed { error } => Err(anyhow::anyhow!("{}", error)),
    }
}

/// Command struct for fold operation.
pub struct FoldCommand {
    /// Keep the current branch name by renaming parent.
    keep: bool,
    /// Whether to run git hooks.
    verify: bool,
}

impl Command for FoldCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ();

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let snapshot = &ctx.snapshot;

        // Get trunk
        let trunk = snapshot
            .trunk()
            .ok_or_else(|| PlanError::MissingData("trunk not configured".to_string()))?;

        // Get current branch
        let current = snapshot
            .current_branch
            .as_ref()
            .ok_or_else(|| PlanError::InvalidState("Not on any branch".to_string()))?;

        // Check if tracked
        if !snapshot.metadata.contains_key(current) {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' is not tracked",
                current
            )));
        }

        // Get current metadata
        let current_meta = snapshot.metadata.get(current).ok_or_else(|| {
            PlanError::MissingData(format!("Metadata not found for '{}'", current))
        })?;

        // Get parent
        let parent_name = if current_meta.metadata.parent.is_trunk() {
            trunk.clone()
        } else {
            BranchName::new(current_meta.metadata.parent.name())
                .map_err(|e| PlanError::MissingData(format!("Invalid parent name: {}", e)))?
        };

        // Cannot fold into trunk
        if &parent_name == trunk {
            return Err(PlanError::InvalidState(
                "Cannot fold into trunk. Use 'lattice merge' instead.".to_string(),
            ));
        }

        // Check freeze policy on current
        if current_meta.metadata.freeze.is_frozen() {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' is frozen. Unfreeze it first.",
                current
            )));
        }

        // Get parent metadata
        let parent_meta = snapshot.metadata.get(&parent_name).ok_or_else(|| {
            PlanError::InvalidState(format!("Parent '{}' is not tracked", parent_name))
        })?;

        // Check freeze on parent
        if parent_meta.metadata.freeze.is_frozen() {
            return Err(PlanError::InvalidState(format!(
                "Parent branch '{}' is frozen. Unfreeze it first.",
                parent_name
            )));
        }

        // Check freeze on children
        if let Some(children) = snapshot.graph.children(current) {
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

        // Build plan
        let mut plan = Plan::new(OpId::new(), "fold");

        // Step 1: Checkout parent
        plan = plan.with_step(PlanStep::RunGit {
            args: vec!["checkout".to_string(), parent_name.to_string()],
            description: format!("Checkout parent '{}'", parent_name),
            expected_effects: vec![],
        });

        // Step 2: Merge current into parent (try fast-forward first)
        let mut merge_args = vec!["merge".to_string()];
        if !self.verify {
            merge_args.push("--no-verify".to_string());
        }
        merge_args.extend(["--ff".to_string(), current.to_string()]);

        plan = plan.with_step(PlanStep::RunGit {
            args: merge_args,
            description: format!("Merge '{}' into '{}'", current, parent_name),
            expected_effects: vec![format!("refs/heads/{}", parent_name)],
        });

        // Step 3: Potential conflict
        plan = plan.with_step(PlanStep::PotentialConflictPause {
            branch: parent_name.to_string(),
            git_operation: "merge".to_string(),
        });

        // Step 4: Reparent children of current to parent
        // After fold, the parent will have current's commits merged in, so the parent's
        // current tip should be valid as the children's new base (conservative approach).
        // The children's commits are still based on current's work which is now in parent.
        let parent_tip_oid = snapshot.branch_tip(&parent_name).ok_or_else(|| {
            PlanError::MissingData(format!("Branch tip not found for parent '{}'", parent_name))
        })?;

        if let Some(children) = snapshot.graph.children(current) {
            for child in children {
                if let Some(child_scanned) = snapshot.metadata.get(child) {
                    let mut updated = child_scanned.metadata.clone();
                    // For now, point to parent_name. If --keep, we'll update again.
                    let new_parent_name = if self.keep {
                        current.to_string() // Will be renamed to current
                    } else {
                        parent_name.to_string()
                    };
                    updated.parent = ParentInfo::Branch {
                        name: new_parent_name,
                    };
                    // Update base to the parent's tip (will be valid after merge)
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

        // Step 5: Delete current branch
        plan = plan.with_step(PlanStep::RunGit {
            args: vec!["branch".to_string(), "-D".to_string(), current.to_string()],
            description: format!("Delete branch '{}'", current),
            expected_effects: vec![], // Ref no longer exists after delete
        });

        // Step 6: Delete current's metadata
        plan = plan.with_step(PlanStep::DeleteMetadataCas {
            branch: current.to_string(),
            old_ref_oid: current_meta.ref_oid.to_string(),
        });

        // Step 7: Handle --keep (rename parent to current's name)
        if self.keep {
            // Rename parent branch to current name
            plan = plan.with_step(PlanStep::RunGit {
                args: vec![
                    "branch".to_string(),
                    "-m".to_string(),
                    parent_name.to_string(),
                    current.to_string(),
                ],
                description: format!("Rename '{}' to '{}'", parent_name, current),
                expected_effects: vec![format!("refs/heads/{}", current)], // Only new name exists after rename
            });

            // Create new metadata under current name (copy from parent with new name)
            let mut new_metadata = parent_meta.metadata.clone();
            new_metadata.branch = BranchInfo {
                name: current.to_string(),
            };
            new_metadata.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: current.to_string(),
                old_ref_oid: None, // Creating new
                metadata: Box::new(new_metadata),
            });

            // Delete old parent metadata
            plan = plan.with_step(PlanStep::DeleteMetadataCas {
                branch: parent_name.to_string(),
                old_ref_oid: parent_meta.ref_oid.to_string(),
            });
        }

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        self.simple_finish(result)
    }
}

impl SimpleCommand for FoldCommand {}
