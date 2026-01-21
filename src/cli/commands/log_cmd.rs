//! log command - Display tracked branches in stack layout
//!
//! Shows the stack graph with branch names, commit counts, and PR status.
//! Supports degraded mode for repositories without tracked branches.
//!
//! # Architecture
//!
//! This is a read-only command that implements `ReadOnlyCommand` and uses
//! `requirements::READ_ONLY`. It flows through `run_readonly_command` to
//! ensure proper gating. Supports degraded mode when metadata is unavailable.

use crate::engine::command::ReadOnlyCommand;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::PlanError;
use crate::engine::runner::{run_readonly_command, RunError};
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Check if log should display in degraded mode.
///
/// Degraded mode is when no branches are tracked yet. This indicates
/// a bootstrap opportunity where we should show helpful guidance.
///
/// This is distinct from corruption (metadata parse errors), which is
/// a Doctor repair case.
fn is_degraded_mode(snapshot: &RepoSnapshot) -> bool {
    // No branches are tracked
    let no_tracked = snapshot.metadata.is_empty();

    // Trunk not configured means we're in early setup
    let trunk_not_configured = snapshot.trunk.is_none();

    // Degraded if nothing tracked OR trunk not configured
    no_tracked || trunk_not_configured
}

/// Print the degraded mode banner with guidance.
fn print_degraded_banner(snapshot: &RepoSnapshot) {
    eprintln!("---------------------------------------------------------------");
    eprintln!("  Degraded view - no branches are tracked yet");
    eprintln!("---------------------------------------------------------------");
    eprintln!();

    // Show trunk status
    if let Some(trunk) = &snapshot.trunk {
        eprintln!("  trunk: {}", trunk);
    } else {
        eprintln!("  trunk: (not configured - run 'lattice init')");
    }
    eprintln!();

    // Show call to action
    eprintln!("  To start tracking branches, run:");
    eprintln!("    lattice track <branch>     - track a single branch");
    eprintln!("    lattice doctor             - discover bootstrap opportunities");
    eprintln!();
    eprintln!("---------------------------------------------------------------");
    eprintln!();
}

/// Print untracked branches in degraded mode.
fn print_untracked_branches(snapshot: &RepoSnapshot) {
    // Get all local branches that are not tracked and not trunk
    let trunk_name = snapshot.trunk.as_ref().map(|t| t.as_str());

    let mut untracked: Vec<_> = snapshot
        .branches
        .keys()
        .filter(|b| !snapshot.metadata.contains_key(*b))
        .filter(|b| Some(b.as_str()) != trunk_name)
        .collect();

    untracked.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    if untracked.is_empty() {
        println!("No local branches found (besides trunk).");
        return;
    }

    println!("Untracked local branches:");
    println!();

    for branch in &untracked {
        let is_current = snapshot.current_branch.as_ref() == Some(*branch);
        let prefix = if is_current { "* " } else { "  " };
        println!("{}{}", prefix, branch);
    }

    println!();
    println!("({} branch(es) not tracked by Lattice)", untracked.len());
}

/// Command to display tracked branches in stack layout.
pub struct LogCommand<'a> {
    ctx: &'a Context,
    short: bool,
    long: bool,
    stack: bool,
    all: bool,
    reverse: bool,
}

impl ReadOnlyCommand for LogCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
    type Output = ();

    fn execute(&self, ready: &ReadyContext) -> Result<Self::Output, PlanError> {
        let snapshot = &ready.snapshot;

        // Check for degraded mode FIRST (no tracked branches)
        if is_degraded_mode(snapshot) {
            if !self.ctx.quiet {
                print_degraded_banner(snapshot);
                print_untracked_branches(snapshot);
            }
            return Ok(());
        }

        // Normal mode: show tracked branches
        let mut branches: Vec<_> = if self.stack {
            // Filter to current branch's stack
            if let Some(ref current) = snapshot.current_branch {
                get_stack_branches(snapshot, current)
            } else {
                vec![]
            }
        } else if self.all {
            snapshot.graph.branches().cloned().collect()
        } else {
            // Default: show current stack or all if no current branch
            if let Some(ref current) = snapshot.current_branch {
                get_stack_branches(snapshot, current)
            } else {
                snapshot.graph.branches().cloned().collect()
            }
        };

        if self.reverse {
            branches.reverse();
        }

        if branches.is_empty() {
            if !self.ctx.quiet {
                println!("No tracked branches.");
            }
            return Ok(());
        }

        // Display tracked branches
        for branch in &branches {
            let is_current = snapshot
                .current_branch
                .as_ref()
                .map(|c| c == branch)
                .unwrap_or(false);
            let prefix = if is_current { "* " } else { "  " };

            if self.short {
                println!("{}{}", prefix, branch);
            } else if self.long {
                // Long format with details
                let parent = snapshot.graph.parent(branch);
                let metadata = snapshot.metadata.get(branch);

                println!("{}{}", prefix, branch);
                if let Some(p) = parent {
                    println!("    parent: {}", p);
                }
                if let Some(m) = metadata {
                    println!("    base: {}", m.metadata.base.oid);
                    if m.metadata.freeze.is_frozen() {
                        println!("    frozen: yes");
                    }
                    if m.metadata.pr.is_linked() {
                        println!("    pr: linked");
                    }
                }
            } else {
                // Default format
                let parent_str = snapshot
                    .graph
                    .parent(branch)
                    .map(|p| format!(" (on {})", p))
                    .unwrap_or_default();
                let frozen = snapshot
                    .metadata
                    .get(branch)
                    .map(|m| {
                        if m.metadata.freeze.is_frozen() {
                            " [frozen]"
                        } else {
                            ""
                        }
                    })
                    .unwrap_or("");
                println!("{}{}{}{}", prefix, branch, parent_str, frozen);
            }
        }

        // In --all mode, also show untracked branches (mixed mode)
        if self.all {
            let trunk_name = snapshot.trunk.as_ref().map(|t| t.as_str());
            let mut untracked: Vec<_> = snapshot
                .branches
                .keys()
                .filter(|b| !snapshot.metadata.contains_key(*b))
                .filter(|b| Some(b.as_str()) != trunk_name)
                .collect();

            untracked.sort_by(|a, b| a.as_str().cmp(b.as_str()));

            if !untracked.is_empty() {
                println!();
                println!("Untracked branches:");
                for branch in untracked {
                    let is_current = snapshot.current_branch.as_ref() == Some(branch);
                    let prefix = if is_current { "* " } else { "  " };
                    println!("{}{}  (untracked)", prefix, branch);
                }
            }
        }

        Ok(())
    }
}

/// Display tracked branches in stack layout.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `short` - Short format (branch names only)
/// * `long` - Long format with full details
/// * `stack` - Filter to current branch's stack
/// * `all` - Show all tracked branches (includes untracked in mixed mode)
/// * `reverse` - Reverse display order
///
/// # Gating
///
/// Uses `requirements::READ_ONLY` via `ReadOnlyCommand` trait. Supports
/// degraded mode when metadata is unavailable.
pub fn log(
    ctx: &Context,
    short: bool,
    long: bool,
    stack: bool,
    all: bool,
    reverse: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = LogCommand {
        ctx,
        short,
        long,
        stack,
        all,
        reverse,
    };

    run_readonly_command(&cmd, &git, ctx).map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}

/// Get all branches in the same stack as the given branch.
pub fn get_stack_branches(
    snapshot: &crate::engine::scan::RepoSnapshot,
    branch: &crate::core::types::BranchName,
) -> Vec<crate::core::types::BranchName> {
    let mut result = vec![branch.clone()];

    // Walk up to root
    let mut current = branch.clone();
    while let Some(parent) = snapshot.graph.parent(&current) {
        result.push(parent.clone());
        current = parent.clone();
    }

    // Walk down to leaves (simplified - just immediate children)
    if let Some(children) = snapshot.graph.children(branch) {
        for child in children {
            result.push(child.clone());
        }
    }

    result
}
