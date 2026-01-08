//! parent and children commands - Simple relationship queries

use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Print parent branch name.
///
/// Outputs nothing (exit 0) if the branch has no parent (is trunk-child).
pub fn parent(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?;

    // Check if tracked
    let metadata = snapshot.metadata.get(current);
    if metadata.is_none() {
        if !ctx.quiet {
            eprintln!("Branch '{}' is not tracked", current);
        }
        return Ok(());
    }

    // Get parent from graph
    if let Some(parent) = snapshot.graph.parent(current) {
        println!("{}", parent);
    }
    // No output if no parent (trunk-child)

    Ok(())
}

/// Print child branch names.
///
/// Outputs nothing (exit 0) if the branch has no children.
pub fn children(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?;

    // Get children from graph
    if let Some(children) = snapshot.graph.children(current) {
        for child in children {
            println!("{}", child);
        }
    }
    // No output if no children

    Ok(())
}
