//! checkout command - Check out a branch

use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};
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
pub fn checkout(ctx: &Context, branch: Option<&str>, trunk_flag: bool, stack: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Determine target branch
    let target = if trunk_flag {
        // Check out trunk
        snapshot
            .trunk
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Trunk not configured"))?
            .clone()
    } else if let Some(name) = branch {
        // Direct branch name
        BranchName::new(name).context("Invalid branch name")?
    } else if ctx.interactive {
        // Interactive selection
        let candidates: Vec<_> = if stack {
            // Filter to current stack
            if let Some(ref current) = snapshot.current_branch {
                get_stack_branches(&snapshot, current)
            } else {
                snapshot.branches.keys().cloned().collect()
            }
        } else {
            snapshot.branches.keys().cloned().collect()
        };

        if candidates.is_empty() {
            bail!("No branches found");
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
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let idx = input
            .trim()
            .parse::<usize>()
            .context("Invalid selection")?
            .saturating_sub(1);

        if idx >= candidates.len() {
            bail!("Invalid selection");
        }

        candidates[idx].clone()
    } else {
        bail!("No branch specified. Use --trunk or run interactively.");
    };

    // Check if branch exists
    if !snapshot.branches.contains_key(&target) {
        bail!("Branch '{}' does not exist", target);
    }

    // Run git checkout
    let status = Command::new("git")
        .args(["checkout", target.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to run git checkout")?;

    if !status.success() {
        bail!("git checkout failed");
    }

    Ok(())
}

/// Get all branches in the same stack as the given branch.
pub fn get_stack_branches(
    snapshot: &crate::engine::scan::RepoSnapshot,
    branch: &BranchName,
) -> Vec<BranchName> {
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
