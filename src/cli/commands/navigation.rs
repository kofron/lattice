//! navigation commands - up, down, top, bottom

use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};
use std::io::{self, Write};
use std::process::Command;

/// Move up to a child branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `steps` - Number of steps to move
pub fn up(ctx: &Context, steps: u32) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?;

    let mut target = current.clone();

    for _ in 0..steps {
        let children = snapshot.graph.children(&target);

        match children {
            None => {
                if !ctx.quiet {
                    println!("Already at top of stack ({})", target);
                }
                return Ok(());
            }
            Some(kids) if kids.is_empty() => {
                if !ctx.quiet {
                    println!("Already at top of stack ({})", target);
                }
                return Ok(());
            }
            Some(kids) if kids.len() == 1 => {
                target = kids.iter().next().unwrap().clone();
            }
            Some(kids) => {
                // Multiple children - need to select
                if ctx.interactive {
                    target = select_child(ctx, &kids.iter().cloned().collect::<Vec<_>>())?;
                } else {
                    bail!(
                        "Multiple children from '{}': {}. Run interactively to select.",
                        target,
                        kids.iter()
                            .map(|b| b.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
    }

    if &target == current {
        return Ok(());
    }

    // Checkout target
    checkout_branch(&cwd, &target)
}

/// Move down to the parent branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `steps` - Number of steps to move
pub fn down(ctx: &Context, steps: u32) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?;

    let mut target = current.clone();

    for _ in 0..steps {
        match snapshot.graph.parent(&target) {
            Some(parent) => {
                target = parent.clone();
            }
            None => {
                // Check if we're at a trunk-child or untracked
                if snapshot.metadata.contains_key(&target) {
                    // We're tracked, so parent is trunk
                    if let Some(trunk) = &snapshot.trunk {
                        target = trunk.clone();
                        break; // Can't go below trunk
                    }
                }
                if !ctx.quiet {
                    println!("Already at bottom of stack ({})", target);
                }
                return Ok(());
            }
        }
    }

    if &target == current {
        return Ok(());
    }

    // Checkout target
    checkout_branch(&cwd, &target)
}

/// Move to the top of the current stack (leaf).
pub fn top(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?;

    let mut target = current.clone();

    loop {
        let children = snapshot.graph.children(&target);

        match children {
            None => {
                break;
            }
            Some(kids) if kids.is_empty() => {
                break;
            }
            Some(kids) if kids.len() == 1 => {
                target = kids.iter().next().unwrap().clone();
            }
            Some(kids) => {
                // Multiple children - need to select
                if ctx.interactive {
                    target = select_child(ctx, &kids.iter().cloned().collect::<Vec<_>>())?;
                } else {
                    bail!(
                        "Multiple paths to top from '{}'. Run interactively to select.",
                        target
                    );
                }
            }
        }
    }

    if &target == current {
        if !ctx.quiet {
            println!("Already at top of stack ({})", target);
        }
        return Ok(());
    }

    checkout_branch(&cwd, &target)
}

/// Move to the bottom of the current stack (trunk-child).
pub fn bottom(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?;

    // If not tracked, can't navigate
    if !snapshot.metadata.contains_key(current) {
        bail!("Current branch '{}' is not tracked", current);
    }

    let mut target = current.clone();
    let mut prev = target.clone();

    // Walk down until we hit trunk or untracked
    while let Some(parent) = snapshot.graph.parent(&target) {
        prev = target.clone();
        target = parent.clone();
    }

    // target is now trunk (or the root of tracking), we want prev (the trunk-child)
    // But if current was already the trunk-child, prev == current
    let final_target = if snapshot.metadata.contains_key(&target) {
        // target is still tracked, so it's the bottom
        target
    } else {
        // target is trunk (untracked), so prev is the trunk-child
        prev
    };

    if &final_target == current {
        if !ctx.quiet {
            println!("Already at bottom of stack ({})", current);
        }
        return Ok(());
    }

    checkout_branch(&cwd, &final_target)
}

/// Interactively select a child branch.
pub fn select_child(_ctx: &Context, children: &[BranchName]) -> Result<BranchName> {
    println!("Multiple children, select one:");
    for (i, child) in children.iter().enumerate() {
        println!("  {}. {}", i + 1, child);
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

    if idx >= children.len() {
        bail!("Invalid selection");
    }

    Ok(children[idx].clone())
}

/// Checkout a branch using git.
pub fn checkout_branch(cwd: &std::path::Path, branch: &BranchName) -> Result<()> {
    let status = Command::new("git")
        .args(["checkout", branch.as_str()])
        .current_dir(cwd)
        .status()
        .context("Failed to run git checkout")?;

    if !status.success() {
        bail!("git checkout failed");
    }

    Ok(())
}
