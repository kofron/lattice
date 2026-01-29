//! move command - Reparent branch onto another branch
//!
//! Per SPEC.md 8D.4:
//!
//! - Changes parent of source to onto
//! - Prevents cycles: cannot move onto a descendant
//! - Rebases source onto onto.tip using source.base as the from point
//! - Descendants remain descendants of source
//!
//! # Integrity Contract
//!
//! - Must prevent cycles (cannot move onto descendant)
//! - Must never rewrite frozen branches
//! - Metadata updated only after refs succeed

use anyhow::{Context as _, Result};

use crate::cli::commands::phase3_helpers::is_descendant_of;
use crate::cli::commands::restack::{get_descendants_inclusive, get_parent_tip, topological_sort};
use crate::core::metadata::schema::{BaseInfo, ParentInfo};
use crate::core::ops::journal::OpId;
use crate::core::types::{BranchName, Oid, UtcTimestamp};
use crate::engine::command::{Command, CommandOutput};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::run_command;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Result of move command
#[derive(Debug)]
pub struct MoveResult {
    /// The branch that was moved
    pub source: BranchName,
    /// The new parent branch
    pub onto: BranchName,
    /// Branches that were restacked
    pub restacked: Vec<BranchName>,
    /// Branches that were skipped (frozen)
    pub skipped_frozen: Vec<BranchName>,
}

/// Pre-computed data for move command
pub struct MovePrecomputed {
    /// Source branch being moved
    pub source: BranchName,
    /// Target parent branch
    pub onto: BranchName,
    /// Whether onto is the trunk
    pub onto_is_trunk: bool,
    /// Source's current base OID
    pub old_base: String,
    /// Onto's current tip OID
    pub onto_tip: String,
    /// Source metadata ref OID for CAS
    pub source_metadata_ref_oid: Oid,
    /// Source's current metadata
    pub source_metadata: crate::core::metadata::schema::BranchMetadataV1,
    /// Descendants that need restacking
    pub descendants_to_restack: Vec<DescendantRestackInfo>,
    /// Frozen branches that will be skipped
    pub frozen_to_skip: Vec<BranchName>,
}

/// Info needed to plan a descendant restack
pub struct DescendantRestackInfo {
    /// Branch name
    pub branch: BranchName,
    /// Current base OID
    pub old_base: String,
    /// Parent branch name
    pub parent_branch: BranchName,
    /// Current metadata ref OID for CAS
    pub metadata_ref_oid: Oid,
    /// Current metadata
    pub metadata: crate::core::metadata::schema::BranchMetadataV1,
}

/// Move command implementing Command trait
pub struct MoveCommand {
    /// Precomputed data
    precomputed: MovePrecomputed,
    /// Whether to run git hooks
    verify: bool,
}

impl Command for MoveCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = MoveResult;

    fn plan(&self, _ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let pre = &self.precomputed;

        let mut plan = Plan::new(OpId::new(), "move");

        // Step 1: Checkpoint before move
        plan = plan.with_step(PlanStep::Checkpoint {
            name: format!("before-move-{}", pre.source),
        });

        // Step 2: Rebase source onto new parent
        let mut rebase_args = vec!["rebase".to_string()];
        if !self.verify {
            rebase_args.push("--no-verify".to_string());
        }
        rebase_args.extend([
            "--onto".to_string(),
            pre.onto_tip.clone(),
            pre.old_base.clone(),
            pre.source.to_string(),
        ]);

        plan = plan.with_step(PlanStep::RunGit {
            args: rebase_args,
            description: format!("Rebase {} onto {}", pre.source, pre.onto),
            expected_effects: vec![format!("refs/heads/{}", pre.source)],
        });

        // Step 3: Potential conflict pause
        plan = plan.with_step(PlanStep::PotentialConflictPause {
            branch: pre.source.to_string(),
            git_operation: "rebase".to_string(),
        });

        // Step 4: Update source metadata with new parent and base
        let new_parent = if pre.onto_is_trunk {
            ParentInfo::Trunk {
                name: pre.onto.to_string(),
            }
        } else {
            ParentInfo::Branch {
                name: pre.onto.to_string(),
            }
        };

        let mut updated_metadata = pre.source_metadata.clone();
        updated_metadata.parent = new_parent;
        updated_metadata.base = BaseInfo {
            oid: pre.onto_tip.clone(),
        };
        updated_metadata.timestamps.updated_at = UtcTimestamp::now();

        plan = plan.with_step(PlanStep::WriteMetadataCas {
            branch: pre.source.to_string(),
            old_ref_oid: Some(pre.source_metadata_ref_oid.to_string()),
            metadata: Box::new(updated_metadata),
        });

        // Step 5: Restack descendants
        for desc_info in &pre.descendants_to_restack {
            plan = plan.with_step(PlanStep::Checkpoint {
                name: format!("before-restack-{}", desc_info.branch),
            });

            let mut rebase_args = vec!["rebase".to_string()];
            if !self.verify {
                rebase_args.push("--no-verify".to_string());
            }
            rebase_args.extend([
                "--onto".to_string(),
                desc_info.parent_branch.to_string(),
                desc_info.old_base.clone(),
                desc_info.branch.to_string(),
            ]);

            plan = plan.with_step(PlanStep::RunGit {
                args: rebase_args,
                description: format!(
                    "Rebase {} onto {}",
                    desc_info.branch, desc_info.parent_branch
                ),
                expected_effects: vec![format!("refs/heads/{}", desc_info.branch)],
            });

            plan = plan.with_step(PlanStep::PotentialConflictPause {
                branch: desc_info.branch.to_string(),
                git_operation: "rebase".to_string(),
            });

            let mut updated_metadata = desc_info.metadata.clone();
            updated_metadata.base = BaseInfo {
                oid: desc_info.parent_branch.to_string(),
            };
            updated_metadata.timestamps.updated_at = UtcTimestamp::now();

            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: desc_info.branch.to_string(),
                old_ref_oid: Some(desc_info.metadata_ref_oid.to_string()),
                metadata: Box::new(updated_metadata),
            });
        }

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        match result {
            ExecuteResult::Success { .. } => CommandOutput::Success(MoveResult {
                source: self.precomputed.source.clone(),
                onto: self.precomputed.onto.clone(),
                restacked: self
                    .precomputed
                    .descendants_to_restack
                    .iter()
                    .map(|d| d.branch.clone())
                    .collect(),
                skipped_frozen: self.precomputed.frozen_to_skip.clone(),
            }),
            ExecuteResult::Paused {
                branch, git_state, ..
            } => CommandOutput::Paused {
                message: format!(
                    "Conflict while moving/restacking '{}' ({}).\n\
                     Resolve conflicts, then run 'lattice continue'.\n\
                     To abort, run 'lattice abort'.",
                    branch,
                    git_state.description()
                ),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

/// Move (reparent) a branch onto another branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `onto` - Target parent branch
/// * `source` - Branch to move (defaults to current)
pub fn move_branch(ctx: &Context, onto: &str, source: Option<&str>) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // =========================================================================
    // PRE-PLAN: Scan and compute state needed for planning
    // =========================================================================

    let snapshot = scan(&git).context("Failed to scan repository")?;

    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?
        .clone();

    // Resolve source branch
    let source_branch = if let Some(name) = source {
        BranchName::new(name).context("Invalid source branch name")?
    } else if let Some(ref current) = snapshot.current_branch {
        current.clone()
    } else {
        anyhow::bail!("Not on any branch and no source specified");
    };

    if !snapshot.metadata.contains_key(&source_branch) {
        anyhow::bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            source_branch
        );
    }

    // Resolve onto branch
    let onto_branch = BranchName::new(onto).context("Invalid onto branch name")?;

    if !snapshot.branches.contains_key(&onto_branch) {
        anyhow::bail!("Target branch '{}' does not exist", onto_branch);
    }

    // Prevent self-move
    if source_branch == onto_branch {
        anyhow::bail!("Cannot move a branch onto itself");
    }

    // Cycle detection
    if is_descendant_of(&onto_branch, &source_branch, &snapshot) {
        anyhow::bail!(
            "Cannot move '{}' onto '{}': would create a cycle (target is a descendant)",
            source_branch,
            onto_branch
        );
    }

    // Get source metadata
    let source_meta = snapshot
        .metadata
        .get(&source_branch)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", source_branch))?;

    // Check freeze on source
    if source_meta.metadata.freeze.is_frozen() {
        anyhow::bail!(
            "Cannot move frozen branch '{}'. Use 'lattice unfreeze' first.",
            source_branch
        );
    }

    let onto_tip = snapshot
        .branches
        .get(&onto_branch)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", onto_branch))?;

    // Check if already a child and aligned
    let current_parent_name = source_meta.metadata.parent.name();
    if current_parent_name == onto_branch.as_str()
        && source_meta.metadata.base.oid == onto_tip.as_str()
    {
        if !ctx.quiet {
            println!(
                "'{}' is already a child of '{}' and aligned.",
                source_branch, onto_branch
            );
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!(
            "Moving '{}' onto '{}' (was child of '{}')...",
            source_branch, onto_branch, current_parent_name
        );
    }

    // Get descendants and check freeze
    let descendants = get_descendants_inclusive(&source_branch, &snapshot);

    let mut descendants_to_restack = Vec::new();
    let mut frozen_to_skip = Vec::new();

    let descendants_only: Vec<_> = descendants
        .iter()
        .filter(|b| *b != &source_branch)
        .cloned()
        .collect();

    let ordered = topological_sort(&descendants_only, &snapshot);

    for branch in &ordered {
        let branch_meta = snapshot
            .metadata
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

        if branch_meta.metadata.freeze.is_frozen() {
            frozen_to_skip.push(branch.clone());
            continue;
        }

        let parent_branch = match &branch_meta.metadata.parent {
            ParentInfo::Trunk { name } => BranchName::new(name)?,
            ParentInfo::Branch { name } => BranchName::new(name)?,
        };

        let parent_tip = get_parent_tip(branch, &snapshot, &trunk)?;

        // Check if already aligned
        if branch_meta.metadata.base.oid.as_str() == parent_tip.to_string().as_str() {
            continue;
        }

        descendants_to_restack.push(DescendantRestackInfo {
            branch: branch.clone(),
            old_base: branch_meta.metadata.base.oid.clone(),
            parent_branch,
            metadata_ref_oid: branch_meta.ref_oid.clone(),
            metadata: branch_meta.metadata.clone(),
        });
    }

    let onto_is_trunk = onto_branch == trunk;

    let precomputed = MovePrecomputed {
        source: source_branch.clone(),
        onto: onto_branch.clone(),
        onto_is_trunk,
        old_base: source_meta.metadata.base.oid.clone(),
        onto_tip: onto_tip.to_string(),
        source_metadata_ref_oid: source_meta.ref_oid.clone(),
        source_metadata: source_meta.metadata.clone(),
        descendants_to_restack,
        frozen_to_skip: frozen_to_skip.clone(),
    };

    // =========================================================================
    // EXECUTE: Run command through unified lifecycle
    // =========================================================================

    let cmd = MoveCommand {
        precomputed,
        verify: ctx.verify,
    };

    let output = run_command(&cmd, &git, ctx)?;

    // =========================================================================
    // POST-EXECUTE: Display results
    // =========================================================================

    match output {
        CommandOutput::Success(result) => {
            if !ctx.quiet {
                println!("Moved '{}' onto '{}'", result.source, result.onto);

                if !result.restacked.is_empty() {
                    println!(
                        "Restacked {} descendant(s): {}",
                        result.restacked.len(),
                        result
                            .restacked
                            .iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }

                if !result.skipped_frozen.is_empty() {
                    println!(
                        "Skipped {} frozen branch(es): {}",
                        result.skipped_frozen.len(),
                        result
                            .skipped_frozen
                            .iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }

                println!("Move complete.");
            }
        }
        CommandOutput::Paused { message } => {
            println!();
            println!("{}", message);
        }
        CommandOutput::Failed { error } => {
            anyhow::bail!("Move failed: {}", error);
        }
    }

    Ok(())
}
