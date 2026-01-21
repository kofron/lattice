//! delete command - Delete a branch
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
//! 1. For each child to reparent: WriteMetadataCas to update parent
//! 2. (If on deleted branch) RunGit: checkout parent
//! 3. For each branch to delete (leaves first):
//!    - RunGit: git branch -D <branch>
//!    - DeleteMetadataCas: remove metadata
//!
//! Per SPEC.md 8D.10:
//!
//! - Deletes local branch and metadata
//! - Re-parents children to deleted branch's parent
//! - Does not close PRs or delete remote branches
//! - --upstack deletes descendants too
//! - --downstack deletes ancestors (never trunk)
//!
//! # Integrity Contract
//!
//! - Must never delete frozen branches
//! - Must re-parent children before deleting
//! - Metadata updated only after refs succeed

use std::io::{self, Write};

use anyhow::{Context as _, Result};

use crate::cli::commands::restack::get_ancestors_inclusive;
use crate::core::metadata::schema::{BaseInfo, ParentInfo};
use crate::core::ops::journal::OpId;
use crate::core::types::BranchName;
use crate::engine::command::{Command, CommandOutput, SimpleCommand};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::run_command;
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::Git;

/// Delete a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to delete (defaults to current)
/// * `upstack` - Also delete all descendants
/// * `downstack` - Also delete all ancestors (not trunk)
/// * `force` - Skip confirmation prompts
pub fn delete(
    ctx: &Context,
    branch: Option<&str>,
    upstack: bool,
    downstack: bool,
    force: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // Parse target branch name if provided
    let target_name = branch.map(BranchName::new).transpose()?;

    // We need to do a preliminary scan to determine what branches will be deleted
    // for the confirmation prompt. This happens BEFORE entering the command lifecycle.
    // The actual scan inside run_command will re-validate the state.
    let preliminary_snapshot =
        crate::engine::scan::scan(&git).context("Failed to scan repository")?;

    let trunk = preliminary_snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Resolve target branch
    let target = if let Some(ref name) = target_name {
        name.clone()
    } else if let Some(ref current) = preliminary_snapshot.current_branch {
        current.clone()
    } else {
        anyhow::bail!("Not on any branch and no branch specified");
    };

    // Cannot delete trunk
    if &target == trunk {
        anyhow::bail!("Cannot delete trunk branch");
    }

    // Check if tracked
    if !preliminary_snapshot.metadata.contains_key(&target) {
        anyhow::bail!(
            "Branch '{}' is not tracked. Use 'git branch -d' for untracked branches.",
            target
        );
    }

    // Determine branches to delete based on flags
    let to_delete =
        compute_branches_to_delete(&target, upstack, downstack, trunk, &preliminary_snapshot);

    // Check freeze policy on all branches to delete
    for branch_name in &to_delete {
        if let Some(meta) = preliminary_snapshot.metadata.get(branch_name) {
            if meta.metadata.freeze.is_frozen() {
                anyhow::bail!(
                    "Branch '{}' is frozen. Unfreeze it first with 'lattice unfreeze'.",
                    branch_name
                );
            }
        }
    }

    // Show what will be deleted
    if !ctx.quiet {
        println!("Will delete {} branch(es):", to_delete.len());
        for b in &to_delete {
            println!("  - {}", b);
        }
    }

    // Confirm unless --force (interactive confirmation BEFORE command lifecycle)
    if !force && ctx.interactive {
        print!("Continue? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let cmd = DeleteCommand {
        target: target.clone(),
        upstack,
        downstack,
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
                println!("Delete complete. Removed {} branch(es).", to_delete.len());
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

/// Compute which branches to delete based on flags.
fn compute_branches_to_delete(
    target: &BranchName,
    upstack: bool,
    downstack: bool,
    trunk: &BranchName,
    snapshot: &RepoSnapshot,
) -> Vec<BranchName> {
    let mut to_delete = vec![target.clone()];

    if upstack {
        // Add all descendants
        let mut stack = vec![target.clone()];
        while let Some(current) = stack.pop() {
            if let Some(children) = snapshot.graph.children(&current) {
                for child in children {
                    if !to_delete.contains(child) {
                        to_delete.push(child.clone());
                        stack.push(child.clone());
                    }
                }
            }
        }
    }

    if downstack {
        // Add all ancestors except trunk
        let ancestors = get_ancestors_inclusive(target, snapshot);
        for ancestor in ancestors {
            if &ancestor != trunk && !to_delete.contains(&ancestor) {
                to_delete.push(ancestor);
            }
        }
    }

    // Remove trunk from list if somehow added
    to_delete.retain(|b| b != trunk);
    to_delete
}

/// Count the number of descendants of a branch.
fn count_descendants(branch: &BranchName, snapshot: &RepoSnapshot) -> usize {
    let mut count = 0;
    let mut stack = vec![branch.clone()];

    while let Some(current) = stack.pop() {
        if let Some(children) = snapshot.graph.children(&current) {
            for child in children {
                count += 1;
                stack.push(child.clone());
            }
        }
    }

    count
}

/// Command struct for delete operation.
pub struct DeleteCommand {
    /// Target branch to delete.
    target: BranchName,
    /// Also delete all descendants.
    upstack: bool,
    /// Also delete all ancestors (not trunk).
    downstack: bool,
}

impl Command for DeleteCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ();

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let snapshot = &ctx.snapshot;

        // Get trunk
        let trunk = snapshot
            .trunk()
            .ok_or_else(|| PlanError::MissingData("trunk not configured".to_string()))?;

        // Cannot delete trunk
        if &self.target == trunk {
            return Err(PlanError::InvalidState(
                "Cannot delete trunk branch".to_string(),
            ));
        }

        // Check if tracked
        if !snapshot.metadata.contains_key(&self.target) {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' is not tracked",
                self.target
            )));
        }

        // Compute branches to delete
        let to_delete =
            compute_branches_to_delete(&self.target, self.upstack, self.downstack, trunk, snapshot);

        // Check freeze policy on all branches
        for branch_name in &to_delete {
            if let Some(meta) = snapshot.metadata.get(branch_name) {
                if meta.metadata.freeze.is_frozen() {
                    return Err(PlanError::InvalidState(format!(
                        "Branch '{}' is frozen. Unfreeze it first.",
                        branch_name
                    )));
                }
            }
        }

        // Get parent of target for potential checkout and reparenting
        let target_meta = snapshot.metadata.get(&self.target).ok_or_else(|| {
            PlanError::MissingData(format!("Metadata not found for '{}'", self.target))
        })?;

        let parent_name = if target_meta.metadata.parent.is_trunk() {
            trunk.clone()
        } else {
            BranchName::new(target_meta.metadata.parent.name())
                .map_err(|e| PlanError::MissingData(format!("Invalid parent name: {}", e)))?
        };

        // Build plan
        let mut plan = Plan::new(OpId::new(), "delete");

        // Step 1: Reparent children of deleted branches (if not upstack mode)
        if !self.upstack {
            for branch_to_delete in &to_delete {
                // Find the parent of this branch
                let meta = snapshot.metadata.get(branch_to_delete).ok_or_else(|| {
                    PlanError::MissingData(format!("Metadata not found for '{}'", branch_to_delete))
                })?;

                let branch_parent = if meta.metadata.parent.is_trunk() {
                    trunk.clone()
                } else {
                    BranchName::new(meta.metadata.parent.name()).map_err(|e| {
                        PlanError::MissingData(format!("Invalid parent name: {}", e))
                    })?
                };

                // Get the parent's tip OID for updating child's base
                let parent_tip_oid = snapshot.branch_tip(&branch_parent).ok_or_else(|| {
                    PlanError::MissingData(format!(
                        "Branch tip not found for parent '{}'",
                        branch_parent
                    ))
                })?;

                // Find and reparent children of this branch
                for (child_name, child_scanned) in &snapshot.metadata {
                    let child_parent_name = child_scanned.metadata.parent.name();
                    if child_parent_name == branch_to_delete.as_str()
                        && !to_delete.contains(child_name)
                    {
                        // This child's parent is being deleted, reparent it
                        let mut updated = child_scanned.metadata.clone();
                        if &branch_parent == trunk {
                            updated.parent = ParentInfo::Trunk {
                                name: branch_parent.to_string(),
                            };
                        } else {
                            updated.parent = ParentInfo::Branch {
                                name: branch_parent.to_string(),
                            };
                        }
                        // Update base to point to the new parent's tip
                        updated.base = BaseInfo {
                            oid: parent_tip_oid.to_string(),
                        };
                        updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

                        plan = plan.with_step(PlanStep::WriteMetadataCas {
                            branch: child_name.to_string(),
                            old_ref_oid: Some(child_scanned.ref_oid.to_string()),
                            metadata: Box::new(updated),
                        });
                    }
                }
            }
        }

        // Step 2: Checkout parent if we're on a branch being deleted
        let need_checkout = snapshot
            .current_branch
            .as_ref()
            .map(|c| to_delete.contains(c))
            .unwrap_or(false);

        if need_checkout {
            plan = plan.with_step(PlanStep::RunGit {
                args: vec!["checkout".to_string(), parent_name.to_string()],
                description: format!("Checkout '{}' before delete", parent_name),
                expected_effects: vec![], // HEAD change, not a ref update
            });
        }

        // Step 3: Delete branches in order (leaves first)
        let mut delete_order = to_delete.clone();
        delete_order.sort_by(|a, b| {
            let count_a = count_descendants(a, snapshot);
            let count_b = count_descendants(b, snapshot);
            count_a.cmp(&count_b)
        });

        for branch_name in &delete_order {
            // Delete git ref
            plan = plan.with_step(PlanStep::RunGit {
                args: vec![
                    "branch".to_string(),
                    "-D".to_string(),
                    branch_name.to_string(),
                ],
                description: format!("Delete branch '{}'", branch_name),
                expected_effects: vec![], // Ref no longer exists after delete
            });

            // Delete metadata
            if let Some(scanned) = snapshot.metadata.get(branch_name) {
                plan = plan.with_step(PlanStep::DeleteMetadataCas {
                    branch: branch_name.to_string(),
                    old_ref_oid: scanned.ref_oid.to_string(),
                });
            }
        }

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        self.simple_finish(result)
    }
}

impl SimpleCommand for DeleteCommand {}
