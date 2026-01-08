//! log command - Display tracked branches in stack layout
//!
//! Shows the stack graph with branch names, commit counts, and PR status.

use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Display tracked branches in stack layout.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `short` - Short format (branch names only)
/// * `long` - Long format with full details
/// * `stack` - Filter to current branch's stack
/// * `all` - Show all tracked branches
/// * `reverse` - Reverse display order
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
    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Get branches to display
    let mut branches: Vec<_> = if stack {
        // Filter to current branch's stack
        if let Some(ref current) = snapshot.current_branch {
            get_stack_branches(&snapshot, current)
        } else {
            vec![]
        }
    } else if all {
        snapshot.graph.branches().cloned().collect()
    } else {
        // Default: show current stack or all if no current branch
        if let Some(ref current) = snapshot.current_branch {
            get_stack_branches(&snapshot, current)
        } else {
            snapshot.graph.branches().cloned().collect()
        }
    };

    if reverse {
        branches.reverse();
    }

    if branches.is_empty() {
        if !ctx.quiet {
            println!("No tracked branches.");
        }
        return Ok(());
    }

    // Display branches
    for branch in &branches {
        let is_current = snapshot
            .current_branch
            .as_ref()
            .map(|c| c == branch)
            .unwrap_or(false);
        let prefix = if is_current { "* " } else { "  " };

        if short {
            println!("{}{}", prefix, branch);
        } else if long {
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

    Ok(())
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
