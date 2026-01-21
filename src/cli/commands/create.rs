//! create command - Create a new tracked branch
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
//! 1. RunGit: checkout -b <branch>
//! 2. (If staged + message) RunGit: commit -m <message>
//! 3. WriteMetadataCas: Create metadata for new branch
//! 4. (If insert) WriteMetadataCas: Update child's parent reference
//!
//! Note: Interactive prompts and staging happen BEFORE the plan phase.

use std::io::{self, Write as IoWrite};
use std::process::Command as StdCommand;

use anyhow::{Context as _, Result};

use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeState, ParentInfo, PrState, Timestamps,
    METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::ops::journal::OpId;
use crate::core::types::BranchName;
use crate::engine::command::{Command, CommandOutput, SimpleCommand};
use crate::engine::exec::ExecuteResult;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::{Plan, PlanError, PlanStep};
use crate::engine::runner::run_command;
use crate::engine::Context;
use crate::git::Git;

/// Create a new tracked branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `name` - Name for the new branch
/// * `message` - Commit message (creates a commit with staged changes)
/// * `all` - Stage all changes before committing
/// * `update` - Stage modified tracked files before committing
/// * `patch` - Interactive patch staging
/// * `insert` - Insert between current branch and its child
pub fn create(
    ctx: &Context,
    name: Option<&str>,
    message: Option<&str>,
    all: bool,
    update: bool,
    patch: bool,
    insert: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // Do preliminary scan for interactive prompts and pre-command validation
    let preliminary_snapshot =
        crate::engine::scan::scan(&git).context("Failed to scan repository")?;

    let trunk = preliminary_snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Get current branch (will be the parent)
    let parent = preliminary_snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    // Determine branch name (interactive prompt if needed - BEFORE plan)
    let branch_name = if let Some(n) = name {
        BranchName::new(n)?
    } else if let Some(msg) = message {
        // Derive from message
        let slug = slugify(msg);
        BranchName::new(&slug)?
    } else if ctx.interactive {
        // Prompt for name
        print!("Branch name: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();
        if input.is_empty() {
            anyhow::bail!("Branch name required");
        }
        BranchName::new(input)?
    } else {
        anyhow::bail!("Branch name required. Use --message to derive from commit message.");
    };

    // Check if branch already exists
    if preliminary_snapshot.branches.contains_key(&branch_name) {
        anyhow::bail!("Branch '{}' already exists", branch_name);
    }

    // Handle insert mode - determine child to reparent (interactive if needed - BEFORE plan)
    let child_to_reparent = if insert {
        let children = preliminary_snapshot.graph.children(&parent);
        match children {
            None => anyhow::bail!("No child branch to insert before"),
            Some(kids) if kids.is_empty() => anyhow::bail!("No child branch to insert before"),
            Some(kids) if kids.len() == 1 => Some(kids.iter().next().unwrap().clone()),
            Some(kids) if ctx.interactive => {
                // Prompt for selection
                println!("Select child to insert before:");
                let kids_vec: Vec<_> = kids.iter().collect();
                for (i, child) in kids_vec.iter().enumerate() {
                    println!("  {}. {}", i + 1, child);
                }
                print!("Enter number: ");
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                let idx: usize = input
                    .trim()
                    .parse()
                    .map_err(|_| anyhow::anyhow!("Invalid selection"))?;
                let idx = idx.saturating_sub(1);

                if idx >= kids_vec.len() {
                    anyhow::bail!("Invalid selection");
                }

                Some(kids_vec[idx].clone())
            }
            Some(kids) => {
                anyhow::bail!(
                    "Multiple children. Run interactively to select: {}",
                    kids.iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    } else {
        None
    };

    // Stage changes if requested (BEFORE plan - not transactional)
    if all {
        let status = StdCommand::new("git")
            .args(["add", "-A"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add")?;

        if !status.success() {
            anyhow::bail!("git add failed");
        }
    } else if update {
        let status = StdCommand::new("git")
            .args(["add", "-u"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add")?;

        if !status.success() {
            anyhow::bail!("git add failed");
        }
    } else if patch {
        let status = StdCommand::new("git")
            .args(["add", "-p"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add")?;

        if !status.success() {
            anyhow::bail!("git add failed");
        }
    }

    // Check for staged changes
    let has_staged = StdCommand::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(&cwd)
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);

    // Determine if we should create a commit
    let should_commit = has_staged && message.is_some();

    // If staged but no message in non-interactive mode, warn
    if has_staged && message.is_none() && !ctx.interactive && !ctx.quiet {
        println!("Staged changes exist. Use --message or run interactively to commit.");
    }

    // If interactive with staged changes but no message, we'll do interactive commit
    let interactive_commit = has_staged && message.is_none() && ctx.interactive;

    let cmd = CreateCommand {
        branch_name: branch_name.clone(),
        parent: parent.clone(),
        parent_is_trunk: &parent == trunk,
        message: message.map(String::from),
        should_commit,
        interactive_commit,
        child_to_reparent: child_to_reparent.clone(),
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
                // Get parent tip for display
                let parent_tip = preliminary_snapshot
                    .branches
                    .get(&parent)
                    .map(|o| &o.as_str()[..7])
                    .unwrap_or("unknown");
                println!(
                    "Created '{}' with parent '{}' (base: {})",
                    branch_name, parent, parent_tip
                );
                if let Some(child) = child_to_reparent {
                    println!("Reparented '{}' under '{}'", child, branch_name);
                }
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

/// Convert a string to a branch-name-safe slug.
pub fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(5) // Limit to first 5 words
        .collect::<Vec<_>>()
        .join("-")
}

/// Command struct for create operation.
pub struct CreateCommand {
    /// Name for the new branch.
    branch_name: BranchName,
    /// Parent branch name.
    parent: BranchName,
    /// Whether parent is trunk.
    parent_is_trunk: bool,
    /// Commit message (if creating a commit).
    message: Option<String>,
    /// Whether to create a commit with message.
    should_commit: bool,
    /// Whether to do interactive commit (editor).
    interactive_commit: bool,
    /// Child to reparent (for insert mode).
    child_to_reparent: Option<BranchName>,
    /// Whether to run git hooks.
    verify: bool,
}

impl Command for CreateCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ();

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let snapshot = &ctx.snapshot;

        // Verify parent still exists
        let parent_oid = snapshot.branches.get(&self.parent).ok_or_else(|| {
            PlanError::MissingData(format!("Parent branch '{}' not found", self.parent))
        })?;

        // Verify branch doesn't exist
        if snapshot.branches.contains_key(&self.branch_name) {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' already exists",
                self.branch_name
            )));
        }

        // Build plan
        let mut plan = Plan::new(OpId::new(), "create");

        // Step 1: Create branch
        plan = plan.with_step(PlanStep::RunGit {
            args: vec![
                "checkout".to_string(),
                "-b".to_string(),
                self.branch_name.to_string(),
            ],
            description: format!("Create branch '{}'", self.branch_name),
            expected_effects: vec![format!("refs/heads/{}", self.branch_name)],
        });

        // Step 2: Create commit if needed
        if self.should_commit {
            if let Some(ref msg) = self.message {
                let mut commit_args = vec!["commit".to_string()];
                if !self.verify {
                    commit_args.push("--no-verify".to_string());
                }
                commit_args.extend(["-m".to_string(), msg.clone()]);

                plan = plan.with_step(PlanStep::RunGit {
                    args: commit_args,
                    description: "Create initial commit".to_string(),
                    expected_effects: vec![format!("refs/heads/{}", self.branch_name)],
                });
            }
        } else if self.interactive_commit {
            // Interactive commit with editor
            let mut commit_args = vec!["commit".to_string()];
            if !self.verify {
                commit_args.push("--no-verify".to_string());
            }

            plan = plan.with_step(PlanStep::RunGit {
                args: commit_args,
                description: "Create initial commit (interactive)".to_string(),
                expected_effects: vec![format!("refs/heads/{}", self.branch_name)],
            });
        }

        // Step 3: Create metadata
        let parent_ref = if self.parent_is_trunk {
            ParentInfo::Trunk {
                name: self.parent.to_string(),
            }
        } else {
            ParentInfo::Branch {
                name: self.parent.to_string(),
            }
        };

        let now = crate::core::types::UtcTimestamp::now();
        let metadata = BranchMetadataV1 {
            kind: METADATA_KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            branch: BranchInfo {
                name: self.branch_name.to_string(),
            },
            parent: parent_ref,
            base: BaseInfo {
                oid: parent_oid.to_string(),
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
            old_ref_oid: None, // Creating new
            metadata: Box::new(metadata),
        });

        // Step 4: Reparent child if in insert mode
        if let Some(ref child) = self.child_to_reparent {
            let child_scanned = snapshot.metadata.get(child).ok_or_else(|| {
                PlanError::MissingData(format!("Child '{}' metadata not found", child))
            })?;

            let mut updated_child = child_scanned.metadata.clone();
            updated_child.parent = ParentInfo::Branch {
                name: self.branch_name.to_string(),
            };
            // Note: base stays the same for now - child will need restack
            updated_child.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: child.to_string(),
                old_ref_oid: Some(child_scanned.ref_oid.to_string()),
                metadata: Box::new(updated_child),
            });
        }

        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        self.simple_finish(result)
    }
}

impl SimpleCommand for CreateCommand {}
