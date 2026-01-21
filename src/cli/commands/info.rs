//! info command - Show tracking status, parent, freeze state for a branch
//!
//! # Architecture
//!
//! This is a read-only command that implements `ReadOnlyCommand` and uses
//! `requirements::READ_ONLY`. It flows through `run_readonly_command` to
//! ensure proper gating.

use crate::core::types::BranchName;
use crate::engine::command::ReadOnlyCommand;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::PlanError;
use crate::engine::runner::{run_readonly_command, RunError};
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};
use std::path::PathBuf;

/// Command to show tracking status, parent, freeze state for a branch.
pub struct InfoCommand<'a> {
    cwd: PathBuf,
    branch: Option<&'a str>,
    diff: bool,
    stat: bool,
    patch: bool,
}

impl ReadOnlyCommand for InfoCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
    type Output = ();

    fn execute(&self, ready: &ReadyContext) -> Result<Self::Output, PlanError> {
        let snapshot = &ready.snapshot;

        // Resolve target branch
        let target = if let Some(name) = self.branch {
            BranchName::new(name)
                .map_err(|e| PlanError::InvalidState(format!("Invalid branch name: {}", e)))?
        } else if let Some(ref current) = snapshot.current_branch {
            current.clone()
        } else {
            return Err(PlanError::InvalidState(
                "Not on any branch and no branch specified".to_string(),
            ));
        };

        // Check if branch exists
        if !snapshot.branches.contains_key(&target) {
            return Err(PlanError::InvalidState(format!(
                "Branch '{}' does not exist",
                target
            )));
        }

        // Check if tracked
        let metadata = snapshot.metadata.get(&target);
        let is_tracked = metadata.is_some();

        println!("Branch: {}", target);
        println!("Tracked: {}", if is_tracked { "yes" } else { "no" });

        if let Some(m) = metadata {
            // Parent
            let parent_name = m.metadata.parent.name().to_string();
            let parent_type = if m.metadata.parent.is_trunk() {
                " (trunk)"
            } else {
                ""
            };
            println!("Parent: {}{}", parent_name, parent_type);

            // Base commit
            println!("Base: {}", &m.metadata.base.oid);

            // Freeze state
            match &m.metadata.freeze {
                crate::core::metadata::schema::FreezeState::Frozen { reason, .. } => {
                    println!("Frozen: yes");
                    if let Some(r) = reason {
                        println!("Freeze reason: {}", r);
                    }
                }
                crate::core::metadata::schema::FreezeState::Unfrozen => {
                    println!("Frozen: no");
                }
            }

            // PR linkage
            match &m.metadata.pr {
                crate::core::metadata::schema::PrState::Linked { number, url, .. } => {
                    println!("PR: linked");
                    println!("PR number: {}", number);
                    println!("PR URL: {}", url);
                }
                crate::core::metadata::schema::PrState::None => {
                    println!("PR: none");
                }
            }

            // Timestamps
            println!("Created: {}", m.metadata.timestamps.created_at);
            println!("Updated: {}", m.metadata.timestamps.updated_at);

            // Show diff/stat/patch if requested
            if self.diff || self.stat || self.patch {
                let base_oid = &m.metadata.base.oid;
                println!();

                if self.stat {
                    println!("--- Changes from base (stat) ---");
                    let output = std::process::Command::new("git")
                        .args(["diff", "--stat", base_oid.as_str(), "HEAD"])
                        .current_dir(&self.cwd)
                        .output()
                        .map_err(|e| {
                            PlanError::InvalidState(format!("Failed to run git diff --stat: {}", e))
                        })?;
                    print!("{}", String::from_utf8_lossy(&output.stdout));
                }

                if self.diff || self.patch {
                    println!("--- Changes from base ---");
                    let output = std::process::Command::new("git")
                        .args(["diff", base_oid.as_str(), "HEAD"])
                        .current_dir(&self.cwd)
                        .output()
                        .map_err(|e| {
                            PlanError::InvalidState(format!("Failed to run git diff: {}", e))
                        })?;
                    print!("{}", String::from_utf8_lossy(&output.stdout));
                }
            }
        } else {
            // Not tracked - show basic info
            if let Some(oid) = snapshot.branches.get(&target) {
                println!("Tip: {}", oid);
            }
        }

        Ok(())
    }
}

/// Show tracking status, parent, freeze state for a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to show info for (defaults to current)
/// * `diff` - Show diff from base
/// * `stat` - Show stat from base
/// * `patch` - Show full patch from base
///
/// # Gating
///
/// Uses `requirements::READ_ONLY` via `ReadOnlyCommand` trait.
pub fn info(
    ctx: &Context,
    branch: Option<&str>,
    diff: bool,
    stat: bool,
    patch: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = InfoCommand {
        cwd: cwd.clone(),
        branch,
        diff,
        stat,
        patch,
    };

    run_readonly_command(&cmd, &git, ctx).map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}
