//! info command - Show tracking status, parent, freeze state for a branch

use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Show tracking status, parent, freeze state for a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to show info for (defaults to current)
/// * `diff` - Show diff from base
/// * `stat` - Show stat from base
/// * `patch` - Show full patch from base
pub fn info(
    ctx: &Context,
    branch: Option<&str>,
    diff: bool,
    stat: bool,
    patch: bool,
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

    // Check if branch exists
    if !snapshot.branches.contains_key(&target) {
        bail!("Branch '{}' does not exist", target);
    }

    // Check if tracked
    let metadata = snapshot.metadata.get(&target);
    let is_tracked = metadata.is_some();

    println!("Branch: {}", target);
    println!("Tracked: {}", if is_tracked { "yes" } else { "no" });

    if let Some(m) = metadata {
        // Parent
        let parent_name = m.metadata.parent.name().to_string();
        let parent_type = if m.metadata.parent.is_trunk() {
            " (trunk)"
        } else {
            ""
        };
        println!("Parent: {}{}", parent_name, parent_type);

        // Base commit
        println!("Base: {}", &m.metadata.base.oid);

        // Freeze state
        match &m.metadata.freeze {
            crate::core::metadata::schema::FreezeState::Frozen { reason, .. } => {
                println!("Frozen: yes");
                if let Some(r) = reason {
                    println!("Freeze reason: {}", r);
                }
            }
            crate::core::metadata::schema::FreezeState::Unfrozen => {
                println!("Frozen: no");
            }
        }

        // PR linkage
        match &m.metadata.pr {
            crate::core::metadata::schema::PrState::Linked { number, url, .. } => {
                println!("PR: linked");
                println!("PR number: {}", number);
                println!("PR URL: {}", url);
            }
            crate::core::metadata::schema::PrState::None => {
                println!("PR: none");
            }
        }

        // Timestamps
        println!("Created: {}", m.metadata.timestamps.created_at);
        println!("Updated: {}", m.metadata.timestamps.updated_at);

        // Show diff/stat/patch if requested
        if diff || stat || patch {
            let base_oid = &m.metadata.base.oid;
            println!();

            if stat {
                println!("--- Changes from base (stat) ---");
                let output = std::process::Command::new("git")
                    .args(["diff", "--stat", base_oid.as_str(), "HEAD"])
                    .current_dir(&cwd)
                    .output()
                    .context("Failed to run git diff --stat")?;
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }

            if diff || patch {
                println!("--- Changes from base ---");
                let output = std::process::Command::new("git")
                    .args(["diff", base_oid.as_str(), "HEAD"])
                    .current_dir(&cwd)
                    .output()
                    .context("Failed to run git diff")?;
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
        }
    } else {
        // Not tracked - show basic info
        if let Some(oid) = snapshot.branches.get(&target) {
            println!("Tip: {}", oid);
        }
    }

    Ok(())
}
