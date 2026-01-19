//! cli::commands::sync
//!
//! Sync with remote (fetch, update trunk, detect merged PRs).
//!
//! # Design
//!
//! Per SPEC.md Section 8E.3, the sync command:
//! - Fetches from remote
//! - Fast-forwards trunk (or errors if diverged without --force)
//! - Detects merged/closed PRs and prompts to delete local branches
//! - Updates stack comments in PR descriptions
//! - Optionally restacks after syncing
//!
//! # Bare Repository Support
//!
//! Per SPEC.md Section 4.6.7, in bare repositories:
//! - `lattice sync` MUST refuse unless `--no-restack` is provided
//! - With `--no-restack`: may fetch, trunk FF, PR checks, branch deletion prompts
//! - MUST NOT attempt any rebase/restack
//!
//! # Example
//!
//! ```bash
//! # Sync with remote
//! lattice sync
//!
//! # Force reset trunk to remote
//! lattice sync --force
//!
//! # Restack after syncing
//! lattice sync --restack
//!
//! # Sync from bare repo (no restack)
//! lattice sync --no-restack
//! ```

use crate::engine::Context;
use anyhow::{bail, Result};

use super::stack_comment_ops::update_stack_comments_for_branches;

/// Run the sync command.
///
/// This is a synchronous wrapper that uses tokio to run the async implementation.
pub fn sync(ctx: &Context, force: bool, restack: bool) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(sync_async(ctx, force, restack))
}

/// Async implementation of sync.
async fn sync_async(ctx: &Context, force: bool, restack: bool) -> Result<()> {
    use crate::cli::commands::auth::get_github_token;
    use crate::core::metadata::schema::PrState;
    use crate::engine::scan::scan;
    use crate::forge::PrState as ForgePrState;
    use crate::git::Git;
    use std::process::Command;

    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;
    let snapshot = scan(&git)?;

    // Per SPEC.md §4.6.7: sync MUST refuse in bare repos if restack is requested
    let is_bare = git.info()?.work_dir.is_none();
    if is_bare && restack {
        bail!(
            "This is a bare repository. The `sync` command cannot restack without a working directory.\n\n\
             To sync without restacking (fetch, trunk FF, PR checks only), use:\n\n\
                 lattice sync --no-restack"
        );
    }

    // Get trunk
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Fetch from remote
    if !ctx.quiet {
        println!("Fetching from origin...");
    }

    let fetch_status = Command::new("git")
        .current_dir(&cwd)
        .args(["fetch", "origin"])
        .status()?;

    if !fetch_status.success() {
        bail!("git fetch failed");
    }

    // Check trunk state
    let local_trunk = format!("refs/heads/{}", trunk);
    let remote_trunk = format!("refs/remotes/origin/{}", trunk);

    let local_oid = git.resolve_ref(&local_trunk)?;
    let remote_oid = match git.resolve_ref(&remote_trunk) {
        Ok(oid) => oid,
        Err(_) => {
            if !ctx.quiet {
                println!("Remote trunk not found. Nothing to sync.");
            }
            return Ok(());
        }
    };

    if local_oid != remote_oid {
        // Check if we can fast-forward
        let is_ancestor = git.is_ancestor(&local_oid, &remote_oid)?;

        if is_ancestor {
            // Fast-forward
            if !ctx.quiet {
                println!("Fast-forwarding {} to origin/{}...", trunk, trunk);
            }

            let ff_status = Command::new("git")
                .current_dir(&cwd)
                .args(["checkout", trunk.as_str()])
                .status()?;

            if !ff_status.success() {
                bail!("git checkout failed");
            }

            let merge_status = Command::new("git")
                .current_dir(&cwd)
                .args(["merge", "--ff-only", &format!("origin/{}", trunk)])
                .status()?;

            if !merge_status.success() {
                bail!("git merge --ff-only failed");
            }
        } else if force {
            // Force reset
            if !ctx.quiet {
                println!(
                    "Force resetting {} to origin/{} (diverged)...",
                    trunk, trunk
                );
            }

            let checkout_status = Command::new("git")
                .current_dir(&cwd)
                .args(["checkout", trunk.as_str()])
                .status()?;

            if !checkout_status.success() {
                bail!("git checkout failed");
            }

            let reset_status = Command::new("git")
                .current_dir(&cwd)
                .args(["reset", "--hard", &format!("origin/{}", trunk)])
                .status()?;

            if !reset_status.success() {
                bail!("git reset --hard failed");
            }
        } else {
            bail!(
                "Trunk '{}' has diverged from origin. Use --force to reset.",
                trunk
            );
        }
    } else if !ctx.quiet {
        println!("Trunk '{}' is up to date.", trunk);
    }

    // Check PR states for tracked branches and update stack comments (requires auth)
    if let Ok(token) = get_github_token() {
        let remote_url = git.remote_url("origin")?;
        if let Some(url) = remote_url {
            if let Ok(forge) = crate::forge::create_forge(&url, &token, None) {
                let mut open_branches = Vec::new();

                for (branch, scanned) in &snapshot.metadata {
                    if let PrState::Linked { number, .. } = &scanned.metadata.pr {
                        match forge.get_pr(*number).await {
                            Ok(pr) => {
                                if pr.state == ForgePrState::Merged
                                    || pr.state == ForgePrState::Closed
                                {
                                    if !ctx.quiet {
                                        println!(
                                            "PR #{} for '{}' is {}.",
                                            number, branch, pr.state
                                        );
                                        // Would prompt to delete in interactive mode
                                    }
                                } else {
                                    // PR is still open, track for stack comment update
                                    open_branches.push(branch.clone());
                                }
                            }
                            Err(e) => {
                                if !ctx.quiet {
                                    eprintln!(
                                        "Warning: Could not check PR #{} for '{}': {}",
                                        number, branch, e
                                    );
                                }
                            }
                        }
                    }
                }

                // Update stack comments for all open PRs
                // This keeps PR descriptions in sync after merges/changes
                if !open_branches.is_empty() {
                    if !ctx.quiet {
                        println!("Updating stack comments...");
                    }

                    let updated = update_stack_comments_for_branches(
                        forge.as_ref(),
                        &snapshot,
                        &open_branches,
                        ctx.quiet,
                    )
                    .await?;

                    if updated > 0 && !ctx.quiet {
                        println!("  Updated {} PR description(s)", updated);
                    }
                }
            }
        }
    }

    // Restack if requested (per SPEC.md 8E.3)
    // "If --restack enabled: restack all restackable branches; skip those that conflict and report"
    if restack {
        if !ctx.quiet {
            println!("Restacking branches...");
        }

        // Restack from trunk to catch all branches that may need realignment
        // after trunk was updated. This reuses the full restack implementation:
        // - Lock acquisition
        // - Journal management for crash safety
        // - Conflict detection and pause/continue model
        // - Frozen branch skipping
        // - Topological ordering for correct rebase sequence
        super::restack::restack(ctx, Some(trunk.as_str()), false, false)?;
    }

    if !ctx.quiet {
        println!("Sync complete.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sync_command_compiles() {
        // Basic compilation test - verifies module structure
    }
}
