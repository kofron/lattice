//! trunk command - Display or set the trunk branch

use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Display or set the trunk branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `set` - If provided, set trunk to this branch
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

    // Display current trunk
    let snapshot = scan(&git).context("Failed to scan repository")?;

    if let Some(ref trunk) = snapshot.trunk {
        println!("{}", trunk);
    } else {
        bail!("Trunk not configured. Run 'lattice init --trunk <branch>' to configure.");
    }

    Ok(())
}
