//! reorder command - Editor-driven branch reordering
//!
//! Per SPEC.md 8D.5:
//!
//! - Opens editor with list of branches between trunk and current (exclusive of trunk)
//! - User reorders lines
//! - Validates same set of branches, no duplicates, no missing entries
//! - Computes required rebase sequence to realize new ordering
//! - Journals each rebase step
//! - Conflicts pause
//!
//! # Integrity Contract
//!
//! - Must never reorder frozen branches
//! - Must validate edit result
//! - Metadata updated only after refs succeed

use std::fs;
use std::io::{self, Write};
use std::process::Command as ProcessCommand;

use anyhow::{Context as _, Result};

use crate::cli::commands::restack::get_ancestors_inclusive;
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

/// Result of reorder command
#[derive(Debug)]
pub struct ReorderResult {
    /// Number of branches reordered
    pub branches_reordered: usize,
}

/// Info for reordering a single branch
pub struct BranchReorderInfo {
    /// Branch being reordered
    pub branch: BranchName,
    /// New parent branch
    pub new_parent: BranchName,
    /// Whether new parent is trunk
    pub parent_is_trunk: bool,
    /// Current base OID
    pub old_base: String,
    /// Current metadata ref OID for CAS
    pub metadata_ref_oid: Oid,
    /// Current metadata
    pub metadata: crate::core::metadata::schema::BranchMetadataV1,
}

/// Pre-computed data for reorder command
pub struct ReorderPrecomputed {
    /// Branches to reorder with their rebase info
    pub branches_to_reorder: Vec<BranchReorderInfo>,
}

/// Reorder command implementing Command trait
pub struct ReorderCommand {
    /// Precomputed data
    precomputed: ReorderPrecomputed,
    /// Whether to run git hooks
    verify: bool,
}

impl Command for ReorderCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ReorderResult;

    fn plan(&self, _ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let mut plan = Plan::new(OpId::new(), "reorder");

        for (i, info) in self.precomputed.branches_to_reorder.iter().enumerate() {
            // Checkpoint before each branch
            plan = plan.with_step(PlanStep::Checkpoint {
                name: format!("before-reorder-{}", info.branch),
            });

            // Rebase onto new parent
            let mut rebase_args = vec!["rebase".to_string()];
            if !self.verify {
                rebase_args.push("--no-verify".to_string());
            }

            // Use new_parent branch name (not tip OID) so git resolves at runtime
            // This handles cascading rebases correctly
            rebase_args.extend([
                "--onto".to_string(),
                info.new_parent.to_string(),
                info.old_base.clone(),
                info.branch.to_string(),
            ]);

            plan = plan.with_step(PlanStep::RunGit {
                args: rebase_args,
                description: format!("Rebase {} onto {}", info.branch, info.new_parent),
                expected_effects: vec![format!("refs/heads/{}", info.branch)],
            });

            // Potential conflict pause
            plan = plan.with_step(PlanStep::PotentialConflictPause {
                branch: info.branch.to_string(),
                git_operation: "rebase".to_string(),
            });

            // Update metadata with new parent and base
            let new_parent_ref = if info.parent_is_trunk {
                ParentInfo::Trunk {
                    name: info.new_parent.to_string(),
                }
            } else {
                ParentInfo::Branch {
                    name: info.new_parent.to_string(),
                }
            };

            let mut updated_metadata = info.metadata.clone();
            updated_metadata.parent = new_parent_ref;
            updated_metadata.base = BaseInfo {
                oid: info.new_parent.to_string(), // Use branch name, resolved at write time
            };
            updated_metadata.timestamps.updated_at = UtcTimestamp::now();

            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: info.branch.to_string(),
                old_ref_oid: Some(info.metadata_ref_oid.to_string()),
                metadata: Box::new(updated_metadata),
            });

            // For debugging: show remaining branches
            if i < self.precomputed.branches_to_reorder.len() - 1 {
                let _ = i; // Silence unused warning
            }
        }

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        match result {
            ExecuteResult::Success { .. } => CommandOutput::Success(ReorderResult {
                branches_reordered: self.precomputed.branches_to_reorder.len(),
            }),
            ExecuteResult::Paused {
                branch, git_state, ..
            } => CommandOutput::Paused {
                message: format!(
                    "Conflict while reordering '{}' ({}).\n\
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

/// Reorder branches in current stack using editor.
///
/// # Arguments
///
/// * `ctx` - Execution context
pub fn reorder(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = crate::core::paths::LatticePaths::from_repo_info(&info);

    // =========================================================================
    // PRE-PLAN: Editor interaction (must happen before unified lifecycle)
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

    // Get stack from trunk to current (ancestors including current, excluding trunk)
    let ancestors = get_ancestors_inclusive(&current, &snapshot);
    let stack: Vec<_> = ancestors.into_iter().filter(|b| b != &trunk).collect();

    if stack.len() < 2 {
        if !ctx.quiet {
            println!(
                "Need at least 2 branches to reorder. Stack has {} tracked branch(es).",
                stack.len()
            );
        }
        return Ok(());
    }

    // Check freeze policy on all branches
    for branch in &stack {
        let meta = snapshot
            .metadata
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;
        if meta.metadata.freeze.is_frozen() {
            anyhow::bail!(
                "Cannot reorder: branch '{}' is frozen. Use 'lattice unfreeze' first.",
                branch
            );
        }
    }

    // Create temp file with branch list
    let temp_file = paths.git_dir.join("REORDER_BRANCHES");
    let content = format!(
        "# Reorder branches by moving lines. Lines starting with # are ignored.\n\
         # Do not add or remove branches.\n\
         # Original order (bottom to top, trunk-child first):\n\
         \n\
         {}\n",
        stack
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );

    fs::write(&temp_file, &content).context("Failed to write reorder file")?;

    // Get editor
    let editor = std::env::var("LATTICE_TEST_EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    if !ctx.quiet {
        println!("Opening editor to reorder {} branches...", stack.len());
    }

    // Open editor
    let status = ProcessCommand::new(&editor)
        .arg(&temp_file)
        .status()
        .with_context(|| format!("Failed to open editor '{}'", editor))?;

    if !status.success() {
        fs::remove_file(&temp_file).ok();
        anyhow::bail!("Editor exited with error");
    }

    // Read edited file
    let edited = fs::read_to_string(&temp_file).context("Failed to read edited file")?;
    fs::remove_file(&temp_file).ok();

    // Parse edited order
    let new_order: Vec<BranchName> = edited
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| BranchName::new(line.trim()))
        .collect::<Result<Vec<_>, _>>()
        .context("Invalid branch name in edited file")?;

    // Validate: same set, no duplicates
    if new_order.len() != stack.len() {
        anyhow::bail!(
            "Invalid edit: expected {} branches, got {}. Do not add or remove branches.",
            stack.len(),
            new_order.len()
        );
    }

    let new_set: std::collections::HashSet<_> = new_order.iter().collect();
    if new_set.len() != new_order.len() {
        anyhow::bail!("Invalid edit: duplicate branch names detected");
    }

    let old_set: std::collections::HashSet<_> = stack.iter().collect();
    if new_set != old_set {
        let missing: Vec<_> = old_set.difference(&new_set).collect();
        let added: Vec<_> = new_set.difference(&old_set).collect();

        if !missing.is_empty() {
            anyhow::bail!("Invalid edit: missing branches: {:?}", missing);
        }
        if !added.is_empty() {
            anyhow::bail!("Invalid edit: unknown branches: {:?}", added);
        }
    }

    // Check if order actually changed
    if new_order == stack {
        if !ctx.quiet {
            println!("No changes to branch order.");
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!("New order:");
        for (i, branch) in new_order.iter().enumerate() {
            let parent = if i == 0 {
                trunk.to_string()
            } else {
                new_order[i - 1].to_string()
            };
            println!("  {} (parent: {})", branch, parent);
        }
        println!();
    }

    // Confirm
    if ctx.interactive {
        print!("Apply this reorder? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // =========================================================================
    // PRE-PLAN: Compute rebase sequence
    // =========================================================================

    let mut branches_to_reorder = Vec::new();

    for (i, branch) in new_order.iter().enumerate() {
        let new_parent = if i == 0 {
            trunk.clone()
        } else {
            new_order[i - 1].clone()
        };

        let branch_meta = snapshot
            .metadata
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

        let new_parent_tip = snapshot
            .branches
            .get(&new_parent)
            .ok_or_else(|| anyhow::anyhow!("Parent '{}' not found", new_parent))?;

        // Check if already in correct position
        let current_parent_name = branch_meta.metadata.parent.name();
        if current_parent_name == new_parent.as_str()
            && branch_meta.metadata.base.oid == new_parent_tip.as_str()
        {
            // Skip - already in position
            continue;
        }

        let parent_is_trunk = new_parent == trunk;

        branches_to_reorder.push(BranchReorderInfo {
            branch: branch.clone(),
            new_parent: new_parent.clone(),
            parent_is_trunk,
            old_base: branch_meta.metadata.base.oid.clone(),
            metadata_ref_oid: branch_meta.ref_oid.clone(),
            metadata: branch_meta.metadata.clone(),
        });
    }

    if branches_to_reorder.is_empty() {
        if !ctx.quiet {
            println!("All branches already in correct position.");
        }
        return Ok(());
    }

    let precomputed = ReorderPrecomputed {
        branches_to_reorder,
    };

    // =========================================================================
    // EXECUTE: Run command through unified lifecycle
    // =========================================================================

    let cmd = ReorderCommand {
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
                println!(
                    "Reordered {} branch(es). Reorder complete.",
                    result.branches_reordered
                );
            }
        }
        CommandOutput::Paused { message } => {
            println!();
            println!("{}", message);
        }
        CommandOutput::Failed { error } => {
            anyhow::bail!("Reorder failed: {}", error);
        }
    }

    Ok(())
}
