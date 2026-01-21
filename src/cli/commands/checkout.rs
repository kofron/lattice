//! checkout command - Check out a branch
//!
//! # Gating
//!
//! Uses `requirements::NAVIGATION` - this command reads stack structure
//! and checkouts out branches, requiring a working directory.

use crate::core::types::BranchName;
use crate::engine::gate::requirements;
use crate::engine::runner::{run_gated, RunError};
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};
use std::io::{self, Write};
use std::process::Command;

/// Check out a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to check out
/// * `trunk_flag` - Check out trunk
/// * `stack` - Filter selector to current stack
///
/// # Gating
///
/// Uses `requirements::NAVIGATION`.
pub fn checkout(ctx: &Context, branch: Option<&str>, trunk_flag: bool, stack: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    run_gated(&git, ctx, &requirements::NAVIGATION, |ready| {
        let snapshot = &ready.snapshot;

        // Determine target branch
        let target = if trunk_flag {
            // Check out trunk
            snapshot
                .trunk
                .as_ref()
                .ok_or_else(|| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(
                        "Trunk not configured".to_string(),
                    ))
                })?
                .clone()
        } else if let Some(name) = branch {
            // Direct branch name
            BranchName::new(name).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Invalid branch name: {}",
                    e
                )))
            })?
        } else if ctx.interactive {
            // Interactive selection
            let candidates: Vec<_> = if stack {
                // Filter to current stack
                if let Some(ref current) = snapshot.current_branch {
                    get_stack_branches(snapshot, current)
                } else {
                    snapshot.branches.keys().cloned().collect()
                }
            } else {
                snapshot.branches.keys().cloned().collect()
            };

            if candidates.is_empty() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "No branches found".to_string(),
                )));
            }

            println!("Select branch to check out:");
            for (i, b) in candidates.iter().enumerate() {
                let current_marker = if snapshot.current_branch.as_ref() == Some(b) {
                    " (current)"
                } else {
                    ""
                };
                let trunk_marker = if snapshot.trunk.as_ref() == Some(b) {
                    " (trunk)"
                } else {
                    ""
                };
                println!("  {}. {}{}{}", i + 1, b, current_marker, trunk_marker);
            }
            print!("Enter number: ");
            io::stdout().flush().map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to flush stdout: {}",
                    e
                )))
            })?;

            let mut input = String::new();
            io::stdin().read_line(&mut input).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to read input: {}",
                    e
                )))
            })?;
            let idx = input
                .trim()
                .parse::<usize>()
                .map_err(|_| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(
                        "Invalid selection".to_string(),
                    ))
                })?
                .saturating_sub(1);

            if idx >= candidates.len() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "Invalid selection".to_string(),
                )));
            }

            candidates[idx].clone()
        } else {
            return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                "No branch specified. Use --trunk or run interactively.".to_string(),
            )));
        };

        // Check if branch exists
        if !snapshot.branches.contains_key(&target) {
            return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                format!("Branch '{}' does not exist", target),
            )));
        }

        // Run git checkout
        let status = Command::new("git")
            .args(["checkout", target.as_str()])
            .current_dir(&cwd)
            .status()
            .map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to run git checkout: {}",
                    e
                )))
            })?;

        if !status.success() {
            return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                "git checkout failed".to_string(),
            )));
        }

        Ok(())
    })
    .map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}

/// Get all branches in the same stack as the given branch.
pub fn get_stack_branches(snapshot: &RepoSnapshot, branch: &BranchName) -> Vec<BranchName> {
    let mut result = vec![branch.clone()];

    // Walk up to root
    let mut current = branch.clone();
    while let Some(parent) = snapshot.graph.parent(&current) {
        result.push(parent.clone());
        current = parent.clone();
    }

    // Walk down to leaves (all descendants)
    let mut stack = vec![branch.clone()];
    while let Some(current) = stack.pop() {
        if let Some(children) = snapshot.graph.children(&current) {
            for child in children {
                if !result.contains(child) {
                    result.push(child.clone());
                    stack.push(child.clone());
                }
            }
        }
    }

    result
}
