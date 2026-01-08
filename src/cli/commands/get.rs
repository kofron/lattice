//! cli::commands::get
//!
//! Fetch a branch or PR from remote.
//!
//! # Design
//!
//! Per SPEC.md Section 8E.4, the get command:
//! - Accepts branch name or PR number
//! - Fetches from remote
//! - Determines parent from PR base or trunk
//! - Tracks fetched branch (frozen by default)
//! - Optionally restacks after fetching
//!
//! # Example
//!
//! ```bash
//! # Fetch by branch name
//! lattice get feature-branch
//!
//! # Fetch by PR number
//! lattice get 42
//!
//! # Fetch unfrozen (editable)
//! lattice get feature-branch --unfrozen
//! ```

use crate::engine::Context;
use anyhow::{bail, Result};

/// Run the get command.
///
/// This is a synchronous wrapper that uses tokio to run the async implementation.
pub fn get(
    ctx: &Context,
    target: &str,
    downstack: bool,
    force: bool,
    restack: bool,
    unfrozen: bool,
) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(get_async(ctx, target, downstack, force, restack, unfrozen))
}

/// Async implementation of get.
async fn get_async(
    ctx: &Context,
    target: &str,
    _downstack: bool,
    force: bool,
    _restack: bool,
    unfrozen: bool,
) -> Result<()> {
    use crate::cli::commands::auth::get_github_token;
    use crate::git::Git;
    use std::process::Command;

    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;

    // Determine if target is a PR number or branch name
    let (branch_name, _pr_info) = if let Ok(pr_number) = target.parse::<u64>() {
        // It's a PR number - fetch details from API
        let token = get_github_token()?;
        let remote_url = git
            .remote_url("origin")?
            .ok_or_else(|| anyhow::anyhow!("No 'origin' remote configured."))?;

        let forge = crate::forge::create_forge(&remote_url, &token, None)?;

        if !ctx.quiet {
            println!("Fetching PR #{}...", pr_number);
        }

        let pr = forge.get_pr(pr_number).await?;
        (pr.head.clone(), Some(pr))
    } else {
        // It's a branch name
        (target.to_string(), None)
    };

    // Check if branch already exists locally
    let local_ref = format!("refs/heads/{}", branch_name);
    let exists_locally = git.resolve_ref(&local_ref).is_ok();

    if exists_locally && !force {
        bail!(
            "Branch '{}' already exists locally. Use --force to overwrite.",
            branch_name
        );
    }

    // Fetch the branch from remote
    if !ctx.quiet {
        println!("Fetching branch '{}'...", branch_name);
    }

    let fetch_status = Command::new("git")
        .current_dir(&cwd)
        .args([
            "fetch",
            "origin",
            &format!("{}:{}", branch_name, branch_name),
        ])
        .status()?;

    if !fetch_status.success() {
        // Try fetching without creating local branch, then create it
        let fetch_ref_status = Command::new("git")
            .current_dir(&cwd)
            .args(["fetch", "origin", &branch_name])
            .status()?;

        if !fetch_ref_status.success() {
            bail!("Failed to fetch branch '{}' from origin.", branch_name);
        }

        // Create local branch tracking remote
        let branch_status = Command::new("git")
            .current_dir(&cwd)
            .args([
                "branch",
                if force { "-f" } else { "" },
                &branch_name,
                &format!("origin/{}", branch_name),
            ])
            .status()?;

        if !branch_status.success() {
            bail!("Failed to create local branch '{}'.", branch_name);
        }
    }

    // Note about tracking
    let freeze_note = if unfrozen { "unfrozen" } else { "frozen" };
    if !ctx.quiet {
        println!(
            "Fetched '{}'. Run 'lattice track {}' to track it ({} by default).",
            branch_name,
            if unfrozen { "--force" } else { "" },
            freeze_note
        );
    }

    // Would track the branch and set up metadata here
    // For now, just inform the user to track manually

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_pr_number() {
        assert!("42".parse::<u64>().is_ok());
        assert!("feature-branch".parse::<u64>().is_err());
    }
}
