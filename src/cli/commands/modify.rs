//! modify command - Amend commits or create first commit, auto-restack descendants
//!
//! This is the simplest Phase 3 rewriting command and establishes patterns
//! for the others. Per SPEC.md 8D.2:
//!
//! - Default: amend HEAD commit on current branch with staged changes
//! - If branch is empty (no commits unique beyond base), creates first commit
//! - After mutation, automatically restack descendants unless prevented by freeze
//! - Conflicts during descendant restack pause the operation
//!
//! # Integrity Contract
//!
//! - Must never rewrite frozen branches
//! - Metadata must be updated only after branch refs have moved successfully
//!
//! # Gating
//!
//! Uses `requirements::MUTATING` - requires working directory, trunk known,
//! no ops in progress, frozen policy satisfied.

use std::process::Command as ProcessCommand;

use anyhow::{Context as _, Result};

use crate::cli::commands::phase3_helpers::count_commits_in_range;
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

/// Result of modify command
#[derive(Debug)]
pub struct ModifyResult {
    /// The branch that was modified
    pub branch: BranchName,
    /// Whether this was a new commit (vs amend)
    pub was_create: bool,
    /// Branches that were restacked
    pub restacked: Vec<BranchName>,
    /// Branches that were skipped (frozen)
    pub skipped_frozen: Vec<BranchName>,
}

/// Pre-computed data for modify command (gathered before plan phase)
pub struct ModifyPrecomputed {
    /// The branch being modified
    pub branch: BranchName,
    /// Whether this is an empty branch (no unique commits)
    pub is_empty_branch: bool,
    /// Whether there are staged changes
    pub has_staged: bool,
    /// Descendants that need restacking with their metadata
    pub descendants_to_restack: Vec<DescendantRestackInfo>,
    /// Frozen branches that will be skipped
    pub frozen_to_skip: Vec<BranchName>,
}

/// Info needed to plan a descendant restack
pub struct DescendantRestackInfo {
    /// Branch name
    pub branch: BranchName,
    /// Current base OID (what it's based on now)
    pub old_base: String,
    /// New base (parent's new tip - as branch name for resolution)
    pub parent_branch: BranchName,
    /// Current metadata ref OID for CAS
    pub metadata_ref_oid: Oid,
    /// Current metadata for cloning/updating
    pub metadata: crate::core::metadata::schema::BranchMetadataV1,
}

/// Modify command implementing Command trait
pub struct ModifyCommand {
    /// Precomputed data from preliminary scan
    precomputed: ModifyPrecomputed,
    /// Force create new commit instead of amend
    create: bool,
    /// Commit message
    message: Option<String>,
    /// Open editor for commit message
    edit: bool,
    /// Whether to run git hooks
    verify: bool,
}

impl Command for ModifyCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ModifyResult;

    fn plan(&self, _ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let pre = &self.precomputed;
        let branch = &pre.branch;

        let mut plan = Plan::new(OpId::new(), "modify");

        // Step 1: Checkpoint before commit
        plan = plan.with_step(PlanStep::Checkpoint {
            name: format!("before-modify-{}", branch),
        });

        // Step 2: Build and run commit command
        let mut commit_args = vec!["commit".to_string()];

        if !self.verify {
            commit_args.push("--no-verify".to_string());
        }

        if pre.is_empty_branch || self.create {
            // Create new commit - no special args needed
        } else {
            // Amend existing commit
            commit_args.push("--amend".to_string());

            // Allow empty if we're just changing the message
            if !pre.has_staged {
                commit_args.push("--allow-empty".to_string());
            }
        }

        // Add message handling
        if let Some(ref msg) = self.message {
            commit_args.push("-m".to_string());
            commit_args.push(msg.clone());
        } else if self.edit || pre.is_empty_branch || self.create {
            // Open editor for message (git commit without -m opens editor by default)
        } else {
            // Amend without changing message
            commit_args.push("--no-edit".to_string());
        }

        let description = if pre.is_empty_branch || self.create {
            format!("Create commit on {}", branch)
        } else {
            format!("Amend commit on {}", branch)
        };

        plan = plan.with_step(PlanStep::RunGit {
            args: commit_args,
            description,
            expected_effects: vec![format!("refs/heads/{}", branch)],
        });

        // Step 3: Restack descendants
        // After our commit, descendants need to rebase onto our new tip
        for desc_info in &pre.descendants_to_restack {
            // Checkpoint before each descendant restack
            plan = plan.with_step(PlanStep::Checkpoint {
                name: format!("before-restack-{}", desc_info.branch),
            });

            // Build rebase args
            // Use parent branch name so git resolves it at runtime to current tip
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

            // Potential conflict pause
            plan = plan.with_step(PlanStep::PotentialConflictPause {
                branch: desc_info.branch.to_string(),
                git_operation: "rebase".to_string(),
            });

            // Update metadata for descendant - new base is its parent's tip
            let mut updated_metadata = desc_info.metadata.clone();
            updated_metadata.base = BaseInfo {
                oid: desc_info.parent_branch.to_string(), // Will resolve to actual OID at write time
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
            ExecuteResult::Success { .. } => CommandOutput::Success(ModifyResult {
                branch: self.precomputed.branch.clone(),
                was_create: self.precomputed.is_empty_branch || self.create,
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
                    "Conflict while restacking '{}' ({}).\n\
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

/// Amend commits or create first commit on current branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `create` - Force create new commit instead of amend
/// * `all` - Stage all changes (git add -A)
/// * `update` - Stage modified tracked files (git add -u)
/// * `patch` - Interactive patch staging (git add -p)
/// * `message` - Commit message
/// * `edit` - Open editor for commit message
pub fn modify(
    ctx: &Context,
    create: bool,
    all: bool,
    update: bool,
    patch: bool,
    message: Option<&str>,
    edit: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // =========================================================================
    // PRE-PLAN: Interactive staging (must happen before unified lifecycle)
    // =========================================================================

    if all {
        let status = ProcessCommand::new("git")
            .args(["add", "-A"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add -A")?;

        if !status.success() {
            anyhow::bail!("git add -A failed");
        }
    } else if update {
        let status = ProcessCommand::new("git")
            .args(["add", "-u"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add -u")?;

        if !status.success() {
            anyhow::bail!("git add -u failed");
        }
    } else if patch {
        let status = ProcessCommand::new("git")
            .args(["add", "-p"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add -p")?;

        if !status.success() {
            anyhow::bail!("git add -p failed");
        }
    }

    // =========================================================================
    // PRE-PLAN: Scan and compute state needed for planning
    // =========================================================================

    let snapshot = scan(&git).context("Failed to scan repository")?;

    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?
        .clone();

    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    if !snapshot.metadata.contains_key(&current) {
        anyhow::bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            current
        );
    }

    let scanned = snapshot
        .metadata
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    // Check freeze on current branch
    if scanned.metadata.freeze.is_frozen() {
        anyhow::bail!(
            "Cannot modify frozen branch '{}'. Use 'lattice unfreeze' first.",
            current
        );
    }

    let current_tip = snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?
        .clone();

    let base_oid = &scanned.metadata.base.oid;
    let base_oid_parsed = Oid::new(base_oid).context("Invalid base OID in metadata")?;
    let commit_count = count_commits_in_range(&cwd, &base_oid_parsed, &current_tip)?;
    let is_empty_branch = commit_count == 0;

    // Check for staged changes
    let has_staged = ProcessCommand::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(&cwd)
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    // Validate staging requirements
    if (is_empty_branch || create) && !has_staged {
        anyhow::bail!("No staged changes to commit. Use -a to stage all changes.");
    }

    // Get descendants and check freeze policy
    let descendants = get_descendants_inclusive(&current, &snapshot);

    let mut descendants_to_restack = Vec::new();
    let mut frozen_to_skip = Vec::new();

    // Process descendants (excluding current branch)
    let descendants_only: Vec<_> = descendants
        .iter()
        .filter(|b| *b != &current)
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

        // Get parent branch name
        let parent_branch = match &branch_meta.metadata.parent {
            ParentInfo::Trunk { name } => BranchName::new(name)?,
            ParentInfo::Branch { name } => BranchName::new(name)?,
        };

        // Get parent's current tip
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

    let precomputed = ModifyPrecomputed {
        branch: current.clone(),
        is_empty_branch,
        has_staged,
        descendants_to_restack,
        frozen_to_skip: frozen_to_skip.clone(),
    };

    // =========================================================================
    // EXECUTE: Run command through unified lifecycle
    // =========================================================================

    let cmd = ModifyCommand {
        precomputed,
        create,
        message: message.map(String::from),
        edit,
        verify: ctx.verify,
    };

    let output = run_command(&cmd, &git, ctx)?;

    // =========================================================================
    // POST-EXECUTE: Display results
    // =========================================================================

    match output {
        CommandOutput::Success(result) => {
            if !ctx.quiet {
                if result.was_create {
                    println!("Created commit on '{}'", result.branch);
                } else {
                    println!("Amended commit on '{}'", result.branch);
                }

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

                println!("Modify complete.");
            }
        }
        CommandOutput::Paused { message } => {
            println!();
            println!("{}", message);
        }
        CommandOutput::Failed { error } => {
            anyhow::bail!("Modify failed: {}", error);
        }
    }

    Ok(())
}
