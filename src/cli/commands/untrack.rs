//! untrack command - Stop tracking a branch
//!
//! # Gating
//!
//! Uses `requirements::MUTATING_METADATA_ONLY` - this command only modifies
//! metadata refs and does not require a working directory.

use crate::core::metadata::store::MetadataStore;
use crate::core::types::BranchName;
use crate::engine::gate::requirements;
use crate::engine::runner::{run_gated, RunError};
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};
use std::io::{self, Write};

/// Stop tracking a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to untrack (defaults to current)
/// * `force` - Also untrack all descendants without prompting
///
/// # Gating
///
/// Uses `requirements::MUTATING_METADATA_ONLY`.
pub fn untrack(ctx: &Context, branch: Option<&str>, force: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    run_gated(&git, ctx, &requirements::MUTATING_METADATA_ONLY, |ready| {
        let snapshot = &ready.snapshot;

        // Resolve target branch
        let target = if let Some(name) = branch {
            BranchName::new(name).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Invalid branch name: {}",
                    e
                )))
            })?
        } else if let Some(ref current) = snapshot.current_branch {
            current.clone()
        } else {
            return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                "Not on any branch and no branch specified".to_string(),
            )));
        };

        // Check if tracked
        if !snapshot.metadata.contains_key(&target) {
            if !ctx.quiet {
                println!("Branch '{}' is not tracked", target);
            }
            return Ok(());
        }

        // Check for descendants
        let descendants = get_descendants(&target, snapshot);

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
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Aborted.");
                    return Ok(());
                }
            } else {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    format!(
                        "Branch '{}' has {} descendant(s). Use --force to untrack all.",
                        target,
                        descendants.len()
                    ),
                )));
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
                    store.delete_cas(branch, &scanned.ref_oid).map_err(|e| {
                        RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                            "Failed to delete metadata for '{}': {}",
                            branch, e
                        )))
                    })?;
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
                    return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                        format!("Failed to read metadata for '{}': {}", branch, e),
                    )));
                }
            }
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

/// Get all descendants of a branch (recursive).
pub fn get_descendants(branch: &BranchName, snapshot: &RepoSnapshot) -> Vec<BranchName> {
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
