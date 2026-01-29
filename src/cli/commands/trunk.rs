//! trunk command - Display or set the trunk branch
//!
//! # Gating
//!
//! This is a read-only command that uses `requirements::READ_ONLY`.

use crate::engine::gate::requirements;
use crate::engine::runner::{run_gated, RunError};
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Display or set the trunk branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `set` - If provided, set trunk to this branch
///
/// # Gating
///
/// Uses `requirements::READ_ONLY` for display mode.
pub fn trunk(ctx: &Context, set: Option<&str>) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    if let Some(new_trunk) = set {
        // Setting trunk - this requires init or config set
        // For now, redirect to init or config
        bail!(
            "Use 'lattice init --trunk {}' or 'lattice config set trunk.branch {}' to set trunk",
            new_trunk,
            new_trunk
        );
    }

    // Display current trunk - run through gating
    run_gated(&git, ctx, &requirements::READ_ONLY, |ready| {
        let snapshot = &ready.snapshot;

        if let Some(ref trunk) = snapshot.trunk {
            println!("{}", trunk);
            Ok(())
        } else {
            Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                "Trunk not configured. Run 'lattice init --trunk <branch>' to configure."
                    .to_string(),
            )))
        }
    })
    .map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}
