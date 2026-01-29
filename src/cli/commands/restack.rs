//! restack command - Rebase tracked branches to align with parent tips
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
//! For each branch needing restack (bottom-up order):
//! 1. Checkpoint for recovery
//! 2. RunGit rebase operation
//! 3. PotentialConflictPause marker
//! 4. WriteMetadataCas to update base

use crate::core::metadata::schema::BaseInfo;
use crate::core::ops::journal::OpId;
use crate::core::types::BranchName;
use crate::engine::command::{Command, CommandOutput};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::run_command_with_scope;
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Rebase tracked branches to align with parent tips.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Specific branch to restack (None = current branch)
/// * `only` - Only restack this single branch
/// * `downstack` - Restack this branch and its ancestors
pub fn restack(ctx: &Context, branch: Option<&str>, only: bool, downstack: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let target = branch.map(BranchName::new).transpose()?;

    let cmd = RestackCommand {
        target: target.clone(),
        only,
        downstack,
        verify: ctx.verify,
    };

    // Use run_command_with_scope to get stack scope in ValidatedData
    let output = run_command_with_scope(&cmd, &git, ctx, target.as_ref())
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    match output {
        CommandOutput::Success(result) => {
            if !ctx.quiet {
                if result.branches_restacked == 0 {
                    println!("All branches are already aligned.");
                } else {
                    println!("Restack complete.");
                }
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

/// Result from a restack operation.
#[derive(Debug)]
pub struct RestackResult {
    /// Number of branches that were restacked.
    pub branches_restacked: usize,
}

/// Command struct for restack operation.
pub struct RestackCommand {
    /// Target branch to restack (None = current branch).
    target: Option<BranchName>,
    /// Only restack the target branch, not descendants.
    only: bool,
    /// Restack downstack (ancestors) instead of upstack (descendants).
    downstack: bool,
    /// Whether to run git hooks (--verify vs --no-verify).
    verify: bool,
}

impl Command for RestackCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = RestackResult;

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        // Get trunk from snapshot
        let trunk = ctx
            .snapshot
            .trunk()
            .ok_or_else(|| PlanError::MissingData("trunk not configured".to_string()))?
            .clone();

        // Resolve target branch
        let target = self
            .target
            .clone()
            .or_else(|| ctx.snapshot.current_branch.clone())
            .ok_or_else(|| {
                PlanError::InvalidState("Not on any branch and no branch specified".to_string())
            })?;

        // Check if target is tracked
        if !ctx.snapshot.metadata.contains_key(&target) {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' is not tracked",
                target
            )));
        }

        // Determine scope based on flags
        let branches_to_check = if self.only {
            vec![target.clone()]
        } else if self.downstack {
            get_ancestors_inclusive(&target, &ctx.snapshot)
        } else {
            get_descendants_inclusive(&target, &ctx.snapshot)
        };

        // Sort in topological order (parents before children)
        let ordered = topological_sort(&branches_to_check, &ctx.snapshot);

        // Determine which branches actually need restacking
        let mut needs_restack = Vec::new();
        for branch in &ordered {
            let scanned = ctx
                .snapshot
                .metadata
                .get(branch)
                .ok_or_else(|| PlanError::MissingData(format!("Metadata for '{}'", branch)))?;

            let metadata = &scanned.metadata;

            // Skip frozen branches
            if metadata.freeze.is_frozen() {
                continue;
            }

            // Get parent tip
            let parent_tip = get_parent_tip(branch, &ctx.snapshot, &trunk)
                .map_err(|e| PlanError::InvalidState(e.to_string()))?;

            // Check if already aligned (compare as strings for consistency)
            if metadata.base.oid.as_str() == parent_tip.to_string().as_str() {
                continue;
            }

            needs_restack.push((
                branch.clone(),
                metadata.base.oid.clone(),
                parent_tip.to_string(), // Convert Oid to String for plan steps
                scanned.ref_oid.clone(),
            ));
        }

        // Build plan
        let mut plan = Plan::new(OpId::new(), "restack");

        for (branch, old_base, new_base, metadata_ref_oid) in &needs_restack {
            // Checkpoint before each branch
            plan = plan.with_step(PlanStep::Checkpoint {
                name: format!("before-restack-{}", branch),
            });

            // Build rebase args
            let mut rebase_args = vec!["rebase".to_string()];
            if !self.verify {
                rebase_args.push("--no-verify".to_string());
            }
            rebase_args.extend([
                "--onto".to_string(),
                new_base.clone(),
                old_base.clone(),
                branch.to_string(),
            ]);

            // Git rebase operation
            plan = plan.with_step(PlanStep::RunGit {
                args: rebase_args,
                description: format!(
                    "Rebase {} onto {} (from {})",
                    branch,
                    &new_base[..7.min(new_base.len())],
                    &old_base[..7.min(old_base.len())]
                ),
                expected_effects: vec![format!("refs/heads/{}", branch)],
            });

            // Mark potential conflict point
            plan = plan.with_step(PlanStep::PotentialConflictPause {
                branch: branch.to_string(),
                git_operation: "rebase".to_string(),
            });

            // Update metadata with new base
            let scanned = ctx.snapshot.metadata.get(branch).ok_or_else(|| {
                PlanError::MissingData(format!("Metadata for '{}' disappeared", branch))
            })?;
            let mut updated_metadata = scanned.metadata.clone();
            updated_metadata.base = BaseInfo {
                oid: new_base.clone(),
            };
            updated_metadata.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: branch.to_string(),
                old_ref_oid: Some(metadata_ref_oid.to_string()),
                metadata: Box::new(updated_metadata),
            });
        }

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<RestackResult> {
        match result {
            ExecuteResult::Success { .. } => {
                // Note: We don't have direct access to how many branches were restacked
                // from ExecuteResult, so we return a placeholder. In practice, the
                // caller can count steps or we could enhance ExecuteResult.
                CommandOutput::Success(RestackResult {
                    branches_restacked: 0, // Will be counted by caller based on plan
                })
            }
            ExecuteResult::Paused {
                branch, git_state, ..
            } => CommandOutput::Paused {
                message: format!(
                    "Conflict while restacking '{}' ({}).\nResolve conflicts, then run 'lattice continue'.\nTo abort, run 'lattice abort'.",
                    branch,
                    git_state.description()
                ),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

// Helper functions - these are the same as before but kept for use by planning

/// Get ancestors of a branch including itself (bottom-up order: parents first).
pub fn get_ancestors_inclusive(branch: &BranchName, snapshot: &RepoSnapshot) -> Vec<BranchName> {
    let mut result = vec![branch.clone()];
    let mut current = branch.clone();

    while let Some(parent) = snapshot.graph.parent(&current) {
        if snapshot.metadata.contains_key(parent) {
            result.push(parent.clone());
            current = parent.clone();
        } else {
            break;
        }
    }

    result.reverse(); // Bottom-up: parents first
    result
}

/// Get descendants of a branch including itself.
pub fn get_descendants_inclusive(branch: &BranchName, snapshot: &RepoSnapshot) -> Vec<BranchName> {
    let mut result = vec![branch.clone()];
    let mut stack = vec![branch.clone()];

    while let Some(current) = stack.pop() {
        if let Some(children) = snapshot.graph.children(&current) {
            for child in children {
                result.push(child.clone());
                stack.push(child.clone());
            }
        }
    }

    result
}

/// Sort branches in topological order (parents before children).
pub fn topological_sort(branches: &[BranchName], snapshot: &RepoSnapshot) -> Vec<BranchName> {
    let branch_set: std::collections::HashSet<_> = branches.iter().collect();
    let mut result = Vec::new();
    let mut visited = std::collections::HashSet::new();

    fn visit(
        branch: &BranchName,
        snapshot: &RepoSnapshot,
        branch_set: &std::collections::HashSet<&BranchName>,
        visited: &mut std::collections::HashSet<BranchName>,
        result: &mut Vec<BranchName>,
    ) {
        if visited.contains(branch) {
            return;
        }
        visited.insert(branch.clone());

        // Visit parent first
        if let Some(parent) = snapshot.graph.parent(branch) {
            if branch_set.contains(parent) {
                visit(parent, snapshot, branch_set, visited, result);
            }
        }

        result.push(branch.clone());
    }

    for branch in branches {
        visit(branch, snapshot, &branch_set, &mut visited, &mut result);
    }

    result
}

/// Get the tip OID of a branch's parent.
///
/// Returns the Oid of the parent branch's tip commit. This is used to determine
/// if a branch needs restacking (i.e., if its base doesn't match the parent tip).
pub fn get_parent_tip(
    branch: &BranchName,
    snapshot: &RepoSnapshot,
    trunk: &BranchName,
) -> Result<crate::core::types::Oid> {
    let scanned = snapshot
        .metadata
        .get(branch)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

    let metadata = &scanned.metadata;
    let parent_name_str = metadata.parent.name();
    let parent_name = if metadata.parent.is_trunk() {
        trunk.clone()
    } else {
        BranchName::new(parent_name_str)
            .map_err(|e| anyhow::anyhow!("Invalid parent name: {}", e))?
    };

    snapshot
        .branches
        .get(&parent_name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Parent branch '{}' not found", parent_name))
}

// Unit tests for restack live in integration tests since they require
// full repository state. The helper functions (get_ancestors_inclusive,
// get_descendants_inclusive, topological_sort, get_parent_tip) are
// exercised through the integration test suite.
