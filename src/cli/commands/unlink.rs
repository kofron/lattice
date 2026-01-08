//! cli::commands::unlink
//!
//! Remove PR linkage from branch metadata.
//!
//! # Design
//!
//! Per SPEC.md Section 8E.7, the unlink command:
//! - Removes PR linkage from metadata
//! - Does NOT alter the PR on GitHub
//! - Is safe to run multiple times (idempotent)
//!
//! # Example
//!
//! ```bash
//! # Unlink current branch
//! lattice unlink
//!
//! # Unlink specific branch
//! lattice unlink feature-branch
//! ```

use crate::core::metadata::schema::PrState;
use crate::core::metadata::store::MetadataStore;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Run the unlink command.
///
/// # Arguments
///
/// * `ctx` - Engine context
/// * `branch` - Optional branch name (defaults to current)
pub fn unlink(ctx: &Context, branch: Option<&str>) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;
    let snapshot = scan(&git)?;

    // Resolve target branch
    let target = if let Some(b) = branch {
        crate::core::types::BranchName::new(b)?
    } else {
        snapshot
            .current_branch
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Not on a branch. Specify a branch name."))?
    };

    // Get metadata for the branch
    let scanned = snapshot
        .metadata
        .get(&target)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' is not tracked by Lattice.", target))?;

    // Check if already unlinked
    if matches!(scanned.metadata.pr, PrState::None) {
        if !ctx.quiet {
            println!("Branch '{}' has no PR linkage.", target);
        }
        return Ok(());
    }

    // Create updated metadata with PrState::None
    let mut metadata = scanned.metadata.clone();
    metadata.pr = PrState::None;
    metadata.touch(); // Update timestamp

    // Write via metadata store with CAS
    let store = MetadataStore::new(&git);
    store
        .write_cas(&target, Some(&scanned.ref_oid), &metadata)
        .context("Failed to update metadata")?;

    if !ctx.quiet {
        println!("Unlinked PR from branch '{}'.", target);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_state_none_is_unlinked() {
        let pr_state = PrState::None;
        assert!(matches!(pr_state, PrState::None));
    }
}
