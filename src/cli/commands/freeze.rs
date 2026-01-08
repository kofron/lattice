//! freeze and unfreeze commands - Mark branches as frozen/unfrozen

use crate::core::metadata::schema::FreezeState;
use crate::core::metadata::store::MetadataStore;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Mark a branch as frozen.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to freeze (defaults to current)
/// * `only` - Only freeze this branch, not downstack
pub fn freeze(ctx: &Context, branch: Option<&str>, only: bool) -> Result<()> {
    set_freeze_state(ctx, branch, only, true)
}

/// Unmark a branch as frozen.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to unfreeze (defaults to current)
/// * `only` - Only unfreeze this branch, not downstack
pub fn unfreeze(ctx: &Context, branch: Option<&str>, only: bool) -> Result<()> {
    set_freeze_state(ctx, branch, only, false)
}

/// Set freeze state for a branch (and optionally its ancestors).
pub fn set_freeze_state(
    ctx: &Context,
    branch: Option<&str>,
    only: bool,
    frozen: bool,
) -> Result<()> {
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
        bail!("Branch '{}' is not tracked", target);
    }

    // Get branches to update
    let branches_to_update = if only {
        vec![target.clone()]
    } else {
        // Include all ancestors (downstack)
        let mut branches = vec![target.clone()];
        let mut current = target.clone();
        while let Some(parent) = snapshot.graph.parent(&current) {
            // Stop at trunk (which isn't tracked)
            if !snapshot.metadata.contains_key(parent) {
                break;
            }
            branches.push(parent.clone());
            current = parent.clone();
        }
        branches
    };

    let store = MetadataStore::new(&git);
    let action = if frozen { "Froze" } else { "Unfroze" };

    for branch in &branches_to_update {
        // Get current metadata
        let scanned = snapshot
            .metadata
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", branch))?;

        // Skip if already in desired state
        if scanned.metadata.freeze.is_frozen() == frozen {
            if !ctx.quiet {
                let state = if frozen { "frozen" } else { "unfrozen" };
                println!("'{}' is already {}", branch, state);
            }
            continue;
        }

        // Update freeze state
        let mut updated = scanned.metadata.clone();
        updated.freeze = if frozen {
            FreezeState::frozen(crate::core::metadata::schema::FreezeScope::Single, None)
        } else {
            FreezeState::Unfrozen
        };
        updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

        // Write with CAS
        store
            .write_cas(branch, Some(&scanned.ref_oid), &updated)
            .with_context(|| format!("Failed to update metadata for '{}'", branch))?;

        if !ctx.quiet {
            println!("{} '{}'", action, branch);
        }
    }

    Ok(())
}
