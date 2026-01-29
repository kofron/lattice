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
//! # Gating
//!
//! Uses `requirements::MUTATING_METADATA_ONLY` - this command only modifies
//! metadata refs and does not require a working directory.
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
use crate::engine::gate::requirements;
use crate::engine::runner::{run_gated, RunError};
use crate::engine::Context;
use crate::git::Git;
use anyhow::Result;

/// Run the unlink command.
///
/// # Arguments
///
/// * `ctx` - Engine context
/// * `branch` - Optional branch name (defaults to current)
///
/// # Gating
///
/// Uses `requirements::MUTATING_METADATA_ONLY`.
pub fn unlink(ctx: &Context, branch: Option<&str>) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;

    run_gated(&git, ctx, &requirements::MUTATING_METADATA_ONLY, |ready| {
        let snapshot = &ready.snapshot;

        // Resolve target branch
        let target = if let Some(b) = branch {
            crate::core::types::BranchName::new(b).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Invalid branch name: {}",
                    e
                )))
            })?
        } else {
            snapshot.current_branch.clone().ok_or_else(|| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "Not on a branch. Specify a branch name.".to_string(),
                ))
            })?
        };

        // Get metadata for the branch
        let scanned = snapshot.metadata.get(&target).ok_or_else(|| {
            RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                "Branch '{}' is not tracked by Lattice.",
                target
            )))
        })?;

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
            .map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to update metadata: {}",
                    e
                )))
            })?;

        if !ctx.quiet {
            println!("Unlinked PR from branch '{}'.", target);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_state_none_is_unlinked() {
        let pr_state = PrState::None;
        assert!(matches!(pr_state, PrState::None));
    }
}
