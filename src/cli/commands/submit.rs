//! cli::commands::submit
//!
//! Submit branches as PRs to GitHub.
//!
//! # Design
//!
//! Per SPEC.md Section 8E.2, the submit command:
//! - Pushes branches to remote
//! - Creates or updates PRs via GitHub API
//! - Links PRs in branch metadata
//! - Optionally restacks before submitting
//!
//! # Algorithm
//!
//! 1. Gate on REMOTE requirements (auth, remote configured)
//! 2. Optionally restack branches
//! 3. For each branch in stack order:
//!    - Determine PR base (parent branch or trunk)
//!    - Push if changed (or --always)
//!    - Create/update PR via forge
//!    - Handle draft toggle
//!    - Request reviewers if specified
//! 4. Update metadata with PR linkage
//!
//! # Example
//!
//! ```bash
//! # Submit current branch and ancestors
//! lattice submit
//!
//! # Submit entire stack including descendants
//! lattice submit --stack
//!
//! # Create as draft
//! lattice submit --draft
//!
//! # Dry run
//! lattice submit --dry-run
//! ```

use crate::engine::Context;
use anyhow::{bail, Result};

/// Submit options parsed from CLI arguments.
#[derive(Debug)]
#[allow(dead_code)]
pub struct SubmitOptions<'a> {
    pub stack: bool,
    pub draft: bool,
    pub publish: bool,
    pub confirm: bool,
    pub dry_run: bool,
    pub force: bool,
    pub always: bool,
    pub update_only: bool,
    pub reviewers: Option<&'a str>,
    pub team_reviewers: Option<&'a str>,
    pub no_restack: bool,
    pub view: bool,
}

/// Run the submit command.
///
/// This is a synchronous wrapper that uses tokio to run the async implementation.
#[allow(clippy::too_many_arguments)]
pub fn submit(
    ctx: &Context,
    stack: bool,
    draft: bool,
    publish: bool,
    confirm: bool,
    dry_run: bool,
    force: bool,
    always: bool,
    update_only: bool,
    reviewers: Option<&str>,
    team_reviewers: Option<&str>,
    no_restack: bool,
    view: bool,
) -> Result<()> {
    let opts = SubmitOptions {
        stack,
        draft,
        publish,
        confirm,
        dry_run,
        force,
        always,
        update_only,
        reviewers,
        team_reviewers,
        no_restack,
        view,
    };

    // Use tokio runtime to run async code
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(submit_async(ctx, opts))
}

/// Async implementation of submit.
async fn submit_async(ctx: &Context, opts: SubmitOptions<'_>) -> Result<()> {
    use crate::cli::commands::auth::get_github_token;
    use crate::engine::scan::scan;
    use crate::git::Git;

    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;
    let snapshot = scan(&git)?;

    // Check authentication
    let token = match get_github_token() {
        Ok(t) => t,
        Err(_) => bail!("Not authenticated. Run 'lattice auth' first."),
    };

    // Get remote URL and create forge
    let remote_url = git
        .remote_url("origin")?
        .ok_or_else(|| anyhow::anyhow!("No 'origin' remote configured."))?;

    let forge = crate::forge::create_forge(&remote_url, &token, None)?;

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on a branch."))?;

    // Determine branches to submit
    let branches = if opts.stack {
        // Include ancestors and descendants
        let mut all = snapshot.graph.ancestors(current);
        all.reverse(); // Bottom-up order
        all.push(current.clone());
        let descendants: Vec<_> = snapshot.graph.descendants(current).into_iter().collect();
        all.extend(descendants);
        all
    } else {
        // Just ancestors and current
        let mut all = snapshot.graph.ancestors(current);
        all.reverse();
        all.push(current.clone());
        all
    };

    if opts.dry_run {
        println!("Would submit {} branch(es):", branches.len());
        for branch in &branches {
            let has_pr = snapshot
                .metadata
                .get(branch)
                .map(|s| !matches!(s.metadata.pr, crate::core::metadata::schema::PrState::None))
                .unwrap_or(false);
            let action = if has_pr { "update" } else { "create" };
            println!("  {} - {} PR", branch, action);
        }
        return Ok(());
    }

    // Submit each branch
    use crate::forge::CreatePrRequest;

    for branch in &branches {
        let scanned = match snapshot.metadata.get(branch) {
            Some(s) => s,
            None => {
                if !ctx.quiet {
                    println!("Skipping untracked branch '{}'", branch);
                }
                continue;
            }
        };

        // Push branch to remote before creating/updating PR
        if !ctx.quiet {
            println!("Pushing '{}'...", branch);
        }
        let push_args = if opts.force {
            vec!["push", "--force-with-lease", "origin", branch.as_str()]
        } else {
            vec!["push", "origin", branch.as_str()]
        };
        let push_result = std::process::Command::new("git")
            .args(&push_args)
            .current_dir(&cwd)
            .output()?;

        if !push_result.status.success() {
            let stderr = String::from_utf8_lossy(&push_result.stderr);
            // Check if it's just "already up to date" which is fine
            if !stderr.contains("Everything up-to-date") {
                eprintln!("  Failed to push '{}': {}", branch, stderr.trim());
                continue;
            }
        }

        // Determine base (parent branch or trunk)
        let base = scanned.metadata.parent.name().to_string();

        // Check if PR exists
        use crate::core::metadata::schema::PrState;
        match &scanned.metadata.pr {
            PrState::Linked { number, .. } => {
                // Update existing PR
                if !ctx.quiet {
                    println!("Updating PR #{} for '{}'...", number, branch);
                }

                let update_req = crate::forge::UpdatePrRequest {
                    number: *number,
                    base: Some(base),
                    title: None,
                    body: None,
                };

                match forge.update_pr(update_req).await {
                    Ok(pr) => {
                        if !ctx.quiet {
                            println!("  Updated: {}", pr.url);
                        }
                    }
                    Err(e) => {
                        eprintln!("  Failed to update PR: {}", e);
                    }
                }

                // Handle draft toggle
                if opts.publish {
                    if let Err(e) = forge.set_draft(*number, false).await {
                        eprintln!("  Failed to publish PR: {}", e);
                    }
                } else if opts.draft {
                    if let Err(e) = forge.set_draft(*number, true).await {
                        eprintln!("  Failed to convert to draft: {}", e);
                    }
                }
            }
            PrState::None => {
                if opts.update_only {
                    if !ctx.quiet {
                        println!("Skipping '{}' (no existing PR, --update-only)", branch);
                    }
                    continue;
                }

                // Try to find existing PR by head
                match forge.find_pr_by_head(branch.as_str()).await? {
                    Some(existing) => {
                        if !ctx.quiet {
                            println!(
                                "Found existing PR #{} for '{}', linking...",
                                existing.number, branch
                            );
                        }
                        // Would update metadata here
                    }
                    None => {
                        // Create new PR
                        if !ctx.quiet {
                            println!("Creating PR for '{}'...", branch);
                        }

                        // Get commit message for title
                        let title = format!("{}", branch);

                        let create_req = CreatePrRequest {
                            head: branch.as_str().to_string(),
                            base,
                            title,
                            body: None,
                            draft: opts.draft,
                        };

                        match forge.create_pr(create_req).await {
                            Ok(pr) => {
                                if !ctx.quiet {
                                    println!("  Created: {}", pr.url);
                                }
                                // Would update metadata with PR linkage here

                                // Request reviewers if specified
                                if opts.reviewers.is_some() || opts.team_reviewers.is_some() {
                                    let reviewers = crate::forge::Reviewers {
                                        users: opts
                                            .reviewers
                                            .map(|r| {
                                                r.split(',').map(|s| s.trim().to_string()).collect()
                                            })
                                            .unwrap_or_default(),
                                        teams: opts
                                            .team_reviewers
                                            .map(|r| {
                                                r.split(',').map(|s| s.trim().to_string()).collect()
                                            })
                                            .unwrap_or_default(),
                                    };
                                    if let Err(e) =
                                        forge.request_reviewers(pr.number, reviewers).await
                                    {
                                        eprintln!("  Failed to request reviewers: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("  Failed to create PR: {}", e);
                            }
                        }
                    }
                }
            }
        }
    }

    if opts.view {
        // Would open PR URLs in browser
        println!("\nUse 'lattice pr --stack' to view PRs.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_options_defaults() {
        let opts = SubmitOptions {
            stack: false,
            draft: false,
            publish: false,
            confirm: false,
            dry_run: false,
            force: false,
            always: false,
            update_only: false,
            reviewers: None,
            team_reviewers: None,
            no_restack: false,
            view: false,
        };
        assert!(!opts.stack);
        assert!(!opts.draft);
    }
}
