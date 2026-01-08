//! untrack command - Stop tracking a branch

use crate::core::metadata::store::MetadataStore;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};
use std::io::{self, Write};

/// Stop tracking a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to untrack (defaults to current)
/// * `force` - Also untrack all descendants without prompting
pub fn untrack(ctx: &Context, branch: Option<&str>, force: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Resolve target branch
    let target = if let Some(name) = branch {
        BranchName::new(name).context("Invalid branch name")?
    } else if let Some(ref current) = snapshot.current_branch {
        current.clone()
    } else {
        bail!("Not on any branch and no branch specified");
    };

    // Check if tracked
    if !snapshot.metadata.contains_key(&target) {
        if !ctx.quiet {
            println!("Branch '{}' is not tracked", target);
        }
        return Ok(());
    }

    // Check for descendants
    let descendants = get_descendants(&target, &snapshot);

    if !descendants.is_empty() && !force {
        if ctx.interactive {
            println!(
                "Branch '{}' has {} descendant(s) that will also be untracked:",
                target,
                descendants.len()
            );
            for d in &descendants {
                println!("  - {}", d);
            }
            print!("Continue? [y/N] ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return Ok(());
            }
        } else {
            bail!(
                "Branch '{}' has {} descendant(s). Use --force to untrack all.",
                target,
                descendants.len()
            );
        }
    }

    // Delete metadata for target and all descendants
    let store = MetadataStore::new(&git);
    let mut to_untrack = vec![target.clone()];
    to_untrack.extend(descendants);

    for branch in &to_untrack {
        // Read metadata to get ref_oid for CAS delete
        match store.read(branch) {
            Ok(Some(scanned)) => {
                store
                    .delete_cas(branch, &scanned.ref_oid)
                    .with_context(|| format!("Failed to delete metadata for '{}'", branch))?;
                if !ctx.quiet {
                    println!("Untracked '{}'", branch);
                }
            }
            Ok(None) => {
                // Already deleted
                if !ctx.quiet {
                    println!("Branch '{}' was already untracked", branch);
                }
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Failed to read metadata for '{}'", branch));
            }
        }
    }

    Ok(())
}

/// Get all descendants of a branch (recursive).
pub fn get_descendants(
    branch: &BranchName,
    snapshot: &crate::engine::scan::RepoSnapshot,
) -> Vec<BranchName> {
    let mut result = Vec::new();
    let mut stack = vec![branch.clone()];

    while let Some(current) = stack.pop() {
        if let Some(children) = snapshot.graph.children(&current) {
            for child in children {
                result.push(child.clone());
                stack.push(child.clone());
            }
        }
    }

    result
}
