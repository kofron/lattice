//! revert command - Create revert branch off trunk
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
//! 1. Checkpoint for recovery
//! 2. RunGit: checkout -b <branch> <trunk>
//! 3. RunGit: revert <sha>
//! 4. PotentialConflictPause marker
//! 5. WriteMetadataCas to track the new branch
//!
//! Per SPEC.md 8D.12:
//!
//! - Creates new branch off trunk and performs `git revert <sha>`
//! - Handles conflicts with pause/continue/abort
//!
//! # Integrity Contract
//!
//! - Validates sha exists and is a commit
//! - Tracks new branch with parent = trunk
//! - Metadata updated only after refs succeed

use std::process::Command as StdCommand;

use anyhow::{Context as _, Result};

use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeState, ParentInfo, PrState, Timestamps,
    METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::ops::journal::OpId;
use crate::core::types::{BranchName, UtcTimestamp};
use crate::engine::command::{Command, CommandOutput, SimpleCommand};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::run_command;
use crate::engine::Context;
use crate::git::Git;

/// Create a revert branch for a commit.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `sha` - Commit SHA to revert
pub fn revert(ctx: &Context, sha: &str) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // Validate sha exists and is a commit BEFORE creating the command.
    // This is a pre-plan validation that must happen before we enter the
    // command lifecycle, since plan() must be pure.
    let output = StdCommand::new("git")
        .args(["rev-parse", "--verify", &format!("{}^{{commit}}", sha)])
        .current_dir(&cwd)
        .output()
        .context("Failed to verify commit")?;

    if !output.status.success() {
        anyhow::bail!("'{}' is not a valid commit", sha);
    }

    let full_sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let short_sha = full_sha[..7.min(full_sha.len())].to_string();

    // Generate branch name
    let branch_name = BranchName::new(format!("revert-{}", short_sha))?;

    if !ctx.quiet {
        println!(
            "Creating revert branch '{}' for commit {}...",
            branch_name, short_sha
        );
    }

    let cmd = RevertCommand {
        full_sha,
        short_sha: short_sha.clone(),
        branch_name: branch_name.clone(),
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
                println!("Revert complete.");
                println!("  Created '{}' reverting commit {}", branch_name, short_sha);
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

/// Command struct for revert operation.
pub struct RevertCommand {
    /// Full SHA of the commit to revert.
    full_sha: String,
    /// Short SHA for display.
    short_sha: String,
    /// Name for the new branch.
    branch_name: BranchName,
    /// Whether to run git hooks (--verify vs --no-verify).
    verify: bool,
}

impl Command for RevertCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ();

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        // Get trunk from snapshot
        let trunk = ctx
            .snapshot
            .trunk()
            .ok_or_else(|| PlanError::MissingData("trunk not configured".to_string()))?
            .clone();

        // Check if branch already exists
        if ctx.snapshot.branches.contains_key(&self.branch_name) {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' already exists",
                self.branch_name
            )));
        }

        // Get trunk tip for the base
        let trunk_tip =
            ctx.snapshot.branches.get(&trunk).ok_or_else(|| {
                PlanError::MissingData(format!("Trunk branch '{}' not found", trunk))
            })?;

        // Build plan
        let mut plan = Plan::new(OpId::new(), "revert");

        // Checkpoint before operation
        plan = plan.with_step(PlanStep::Checkpoint {
            name: "before-revert".to_string(),
        });

        // Create branch off trunk
        plan = plan.with_step(PlanStep::RunGit {
            args: vec![
                "checkout".to_string(),
                "-b".to_string(),
                self.branch_name.to_string(),
                trunk.to_string(),
            ],
            description: format!("Create branch '{}' from '{}'", self.branch_name, trunk),
            expected_effects: vec![format!("refs/heads/{}", self.branch_name)],
        });

        // Run git revert
        let mut revert_args = vec!["revert".to_string()];
        if !self.verify {
            revert_args.push("--no-verify".to_string());
        }
        revert_args.extend(["--no-edit".to_string(), self.full_sha.clone()]);

        plan = plan.with_step(PlanStep::RunGit {
            args: revert_args,
            description: format!("Revert commit {}", self.short_sha),
            expected_effects: vec![format!("refs/heads/{}", self.branch_name)],
        });

        // Mark potential conflict point
        plan = plan.with_step(PlanStep::PotentialConflictPause {
            branch: self.branch_name.to_string(),
            git_operation: "revert".to_string(),
        });

        // Create metadata for the new branch
        let now = UtcTimestamp::now();
        let metadata = BranchMetadataV1 {
            kind: METADATA_KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            branch: BranchInfo {
                name: self.branch_name.to_string(),
            },
            parent: ParentInfo::Trunk {
                name: trunk.to_string(),
            },
            base: BaseInfo {
                oid: trunk_tip.to_string(),
            },
            freeze: FreezeState::Unfrozen,
            pr: PrState::None,
            timestamps: Timestamps {
                created_at: now.clone(),
                updated_at: now,
            },
        };

        plan = plan.with_step(PlanStep::WriteMetadataCas {
            branch: self.branch_name.to_string(),
            old_ref_oid: None, // Creating new metadata
            metadata: Box::new(metadata),
        });

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        self.simple_finish(result)
    }
}

impl SimpleCommand for RevertCommand {}
