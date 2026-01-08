//! cli::commands::merge
//!
//! Merge PRs via GitHub API.
//!
//! # Design
//!
//! Per SPEC.md Section 8E.5, the merge command:
//! - Merges PRs from trunk to current branch in order
//! - Uses GitHub merge API
//! - Stops on first failure
//! - Suggests running `lattice sync` after
//!
//! # Example
//!
//! ```bash
//! # Merge PRs in stack
//! lattice merge
//!
//! # Dry run
//! lattice merge --dry-run
//!
//! # Use squash merge
//! lattice merge --method squash
//! ```

use crate::cli::args::MergeMethodArg;
use crate::engine::Context;
use anyhow::{bail, Result};

/// Run the merge command.
///
/// This is a synchronous wrapper that uses tokio to run the async implementation.
pub fn merge(
    ctx: &Context,
    confirm: bool,
    dry_run: bool,
    method: Option<MergeMethodArg>,
) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(merge_async(ctx, confirm, dry_run, method))
}

/// Async implementation of merge.
async fn merge_async(
    ctx: &Context,
    _confirm: bool,
    dry_run: bool,
    method: Option<MergeMethodArg>,
) -> Result<()> {
    use crate::cli::commands::auth::get_github_token;
    use crate::core::metadata::schema::PrState;
    use crate::engine::scan::scan;
    use crate::forge::MergeMethod;
    use crate::git::Git;

    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;
    let snapshot = scan(&git)?;

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on a branch."))?;

    // Get authentication
    let token = get_github_token()?;

    // Get forge
    let remote_url = git
        .remote_url("origin")?
        .ok_or_else(|| anyhow::anyhow!("No 'origin' remote configured."))?;

    let forge = crate::forge::create_forge(&remote_url, &token, None)?;

    // Get stack from trunk to current (ancestors + current)
    let mut stack = snapshot.graph.ancestors(current);
    stack.reverse(); // Bottom-up order
    stack.push(current.clone());

    // Filter to branches with linked PRs
    let mergeable: Vec<_> = stack
        .iter()
        .filter(|b| {
            snapshot
                .metadata
                .get(*b)
                .map(|m| matches!(m.metadata.pr, PrState::Linked { .. }))
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    if mergeable.is_empty() {
        bail!("No PRs to merge. Run 'lattice submit' first.");
    }

    // Convert merge method
    let merge_method = match method {
        Some(MergeMethodArg::Merge) => MergeMethod::Merge,
        Some(MergeMethodArg::Squash) => MergeMethod::Squash,
        Some(MergeMethodArg::Rebase) => MergeMethod::Rebase,
        None => MergeMethod::Squash, // Default
    };

    if dry_run {
        println!(
            "Would merge {} PR(s) using {} method:",
            mergeable.len(),
            merge_method
        );
        for branch in &mergeable {
            if let Some(scanned) = snapshot.metadata.get(branch) {
                if let PrState::Linked { number, .. } = &scanned.metadata.pr {
                    println!("  PR #{} ({})", number, branch);
                }
            }
        }
        return Ok(());
    }

    // Merge in order
    for branch in &mergeable {
        if let Some(scanned) = snapshot.metadata.get(branch) {
            if let PrState::Linked { number, .. } = &scanned.metadata.pr {
                if !ctx.quiet {
                    println!("Merging PR #{} ({})...", number, branch);
                }

                match forge.merge_pr(*number, merge_method).await {
                    Ok(()) => {
                        if !ctx.quiet {
                            println!("  Merged successfully.");
                        }
                    }
                    Err(e) => {
                        eprintln!("  Failed to merge: {}", e);
                        eprintln!("Stopping. Run 'lattice sync' to update state.");
                        return Err(e.into());
                    }
                }
            }
        }
    }

    if !ctx.quiet {
        println!("\nAll PRs merged. Run 'lattice sync' to update local state.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn merge_method_conversion() {
        use crate::forge::MergeMethod;

        let m: MergeMethod = MergeMethod::Squash;
        assert_eq!(format!("{}", m), "squash");
    }
}
