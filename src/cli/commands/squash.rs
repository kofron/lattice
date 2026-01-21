//! squash command - Squash all commits in current branch into one
//!
//! Per SPEC.md 8D.7:
//!
//! - Squash all commits unique to current branch into one
//! - Preserve parent relation
//! - Restack descendants
//! - Respect freeze
//!
//! # Integrity Contract
//!
//! - Must never rewrite frozen branches
//! - Metadata updated only after refs succeed

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

/// Result of squash command
#[derive(Debug)]
pub struct SquashResult {
    /// The branch that was squashed
    pub branch: BranchName,
    /// Number of commits squashed
    pub commits_squashed: usize,
    /// Branches that were restacked
    pub restacked: Vec<BranchName>,
    /// Branches that were skipped (frozen)
    pub skipped_frozen: Vec<BranchName>,
}

/// Pre-computed data for squash command
pub struct SquashPrecomputed {
    /// The branch being squashed
    pub branch: BranchName,
    /// Base OID to reset to
    pub base_oid: String,
    /// Number of commits being squashed
    pub commit_count: usize,
    /// The message for the squashed commit
    pub squash_message: String,
    /// Descendants that need restacking
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
    /// Parent branch name for resolution
    pub parent_branch: BranchName,
    /// Current metadata ref OID for CAS
    pub metadata_ref_oid: Oid,
    /// Current metadata for cloning/updating
    pub metadata: crate::core::metadata::schema::BranchMetadataV1,
}

/// Squash command implementing Command trait
pub struct SquashCommand {
    /// Precomputed data from preliminary scan
    precomputed: SquashPrecomputed,
    /// Custom commit message (overrides collected)
    custom_message: Option<String>,
    /// Open editor for commit message
    edit: bool,
    /// Whether to run git hooks
    verify: bool,
}

impl Command for SquashCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = SquashResult;

    fn plan(&self, _ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let pre = &self.precomputed;
        let branch = &pre.branch;

        let mut plan = Plan::new(OpId::new(), "squash");

        // Step 1: Checkpoint before squash
        plan = plan.with_step(PlanStep::Checkpoint {
            name: format!("before-squash-{}", branch),
        });

        // Step 2: Soft reset to base
        plan = plan.with_step(PlanStep::RunGit {
            args: vec![
                "reset".to_string(),
                "--soft".to_string(),
                pre.base_oid.clone(),
            ],
            description: format!("Soft reset {} to base", branch),
            expected_effects: vec![format!("refs/heads/{}", branch)],
        });

        // Step 3: Create squashed commit
        let mut commit_args = vec!["commit".to_string()];
        if !self.verify {
            commit_args.push("--no-verify".to_string());
        }

        // Determine message handling
        if let Some(ref msg) = self.custom_message {
            commit_args.push("-m".to_string());
            commit_args.push(msg.clone());
        } else if self.edit {
            // Use combined messages and open editor
            // For executor pattern, we write to SQUASH_MSG and use -F -e
            // But we can't write files from plan() - it must be pure.
            // So we use the collected message directly.
            commit_args.push("-m".to_string());
            commit_args.push(pre.squash_message.clone());
            // Note: -e would require interactive editing which breaks purity.
            // The caller should handle --edit by running interactively before plan phase.
        } else {
            // Use first message as default
            let first_msg = pre
                .squash_message
                .split("---")
                .next()
                .unwrap_or("Squashed commits")
                .trim();
            commit_args.push("-m".to_string());
            commit_args.push(first_msg.to_string());
        }

        plan = plan.with_step(PlanStep::RunGit {
            args: commit_args,
            description: format!("Create squashed commit on {}", branch),
            expected_effects: vec![format!("refs/heads/{}", branch)],
        });

        // Step 4: Restack descendants
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
            ExecuteResult::Success { .. } => CommandOutput::Success(SquashResult {
                branch: self.precomputed.branch.clone(),
                commits_squashed: self.precomputed.commit_count,
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

/// Squash all commits in current branch into one.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `message` - Commit message for squashed commit
/// * `edit` - Open editor for commit message
pub fn squash(ctx: &Context, message: Option<&str>, edit: bool) -> Result<()> {
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
            "Cannot squash frozen branch '{}'. Use 'lattice unfreeze' first.",
            current
        );
    }

    let base_oid = Oid::new(&scanned.metadata.base.oid).context("Invalid base OID")?;

    let current_tip = snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?;

    let commit_count = count_commits_in_range(&cwd, &base_oid, current_tip)?;

    if commit_count <= 1 {
        if !ctx.quiet {
            println!(
                "Branch '{}' has {} commit(s). Nothing to squash.",
                current, commit_count
            );
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!("Squashing {} commits on '{}'...", commit_count, current);
    }

    // Get commit messages for squash message
    let log_output = ProcessCommand::new("git")
        .args([
            "log",
            "--reverse",
            "--format=%B%n---",
            &format!("{}..{}", base_oid.as_str(), current_tip.as_str()),
        ])
        .current_dir(&cwd)
        .output()
        .context("Failed to get commit messages")?;

    let combined_messages = String::from_utf8_lossy(&log_output.stdout).to_string();

    // Handle --edit interactively before entering unified lifecycle
    let final_message = if edit && message.is_none() {
        // Write combined messages to temp file for editor
        let info = git.info()?;
        let paths = crate::core::paths::LatticePaths::from_repo_info(&info);
        let temp_msg_file = paths.git_dir.join("SQUASH_MSG");
        std::fs::write(&temp_msg_file, &combined_messages)
            .context("Failed to write squash message template")?;

        // Open editor
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let status = ProcessCommand::new(&editor)
            .arg(&temp_msg_file)
            .status()
            .context("Failed to open editor")?;

        if !status.success() {
            anyhow::bail!("Editor exited with error");
        }

        // Read edited message
        let edited =
            std::fs::read_to_string(&temp_msg_file).context("Failed to read edited message")?;

        if edited.trim().is_empty() {
            anyhow::bail!("Aborting squash due to empty commit message");
        }

        Some(edited)
    } else {
        None
    };

    // Get descendants and check freeze
    let descendants = get_descendants_inclusive(&current, &snapshot);

    let mut descendants_to_restack = Vec::new();
    let mut frozen_to_skip = Vec::new();

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

        let parent_branch = match &branch_meta.metadata.parent {
            ParentInfo::Trunk { name } => BranchName::new(name)?,
            ParentInfo::Branch { name } => BranchName::new(name)?,
        };

        let parent_tip = get_parent_tip(branch, &snapshot, &trunk)?;

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

    let precomputed = SquashPrecomputed {
        branch: current.clone(),
        base_oid: base_oid.to_string(),
        commit_count,
        squash_message: combined_messages,
        descendants_to_restack,
        frozen_to_skip: frozen_to_skip.clone(),
    };

    // =========================================================================
    // EXECUTE: Run command through unified lifecycle
    // =========================================================================

    let cmd = SquashCommand {
        precomputed,
        custom_message: message.map(String::from).or(final_message),
        edit: false, // Already handled interactively above
        verify: ctx.verify,
    };

    let output = run_command(&cmd, &git, ctx)?;

    // =========================================================================
    // POST-EXECUTE: Display results
    // =========================================================================

    match output {
        CommandOutput::Success(result) => {
            if !ctx.quiet {
                println!(
                    "Squashed {} commits on '{}'",
                    result.commits_squashed, result.branch
                );

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

                println!("Squash complete.");
            }
        }
        CommandOutput::Paused { message } => {
            println!();
            println!("{}", message);
        }
        CommandOutput::Failed { error } => {
            anyhow::bail!("Squash failed: {}", error);
        }
    }

    Ok(())
}
