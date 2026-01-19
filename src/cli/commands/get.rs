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
//! # Bare Repository Support
//!
//! Per SPEC.md Section 4.6.7, in bare repositories:
//! - `lattice get` MUST refuse unless `--no-checkout` is provided
//! - With `--no-checkout`: fetch, track branch with parent inference,
//!   compute base, default frozen, print worktree guidance
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
//!
//! # Fetch in bare repo (no checkout)
//! lattice get feature-branch --no-checkout
//! ```

use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeScope, FreezeState, ParentInfo, PrState,
    Timestamps, METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::metadata::store::MetadataStore;
use crate::core::types::{BranchName, UtcTimestamp};
use crate::engine::Context;
use crate::forge::PullRequest;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};
use std::process::Command;

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
    no_checkout: bool,
) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(get_async(
        ctx,
        target,
        downstack,
        force,
        restack,
        unfrozen,
        no_checkout,
    ))
}

/// Async implementation of get.
async fn get_async(
    ctx: &Context,
    target: &str,
    _downstack: bool,
    force: bool,
    _restack: bool,
    unfrozen: bool,
    no_checkout: bool,
) -> Result<()> {
    use crate::cli::commands::auth::get_github_token;

    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;

    // Per SPEC.md §4.6.7: get MUST refuse in bare repos unless --no-checkout
    let is_bare = git.info()?.work_dir.is_none();
    if is_bare && !no_checkout {
        bail!(
            "This is a bare repository. The `get` command requires a working directory.\n\n\
             To fetch and track the branch without checkout, use:\n\n\
                 lattice get --no-checkout {}\n\n\
             After tracking, you can create a worktree to work on it:\n\n\
                 git worktree add <path> {}",
            target,
            target
        );
    }

    // Determine if target is a PR number or branch name
    let (branch_name, pr_info) = if let Ok(pr_number) = target.parse::<u64>() {
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
        let origin_ref = format!("origin/{}", branch_name);
        let mut branch_args = vec!["branch"];
        if force {
            branch_args.push("-f");
        }
        branch_args.push(&branch_name);
        branch_args.push(&origin_ref);

        let branch_status = Command::new("git")
            .current_dir(&cwd)
            .args(&branch_args)
            .status()?;

        if !branch_status.success() {
            bail!("Failed to create local branch '{}'.", branch_name);
        }
    }

    // Handle no-checkout mode (required for bare repos)
    if no_checkout {
        return handle_no_checkout_mode(ctx, &git, &branch_name, pr_info.as_ref(), unfrozen).await;
    }

    // Normal mode: Note about tracking
    let freeze_note = if unfrozen { "unfrozen" } else { "frozen" };
    if !ctx.quiet {
        println!(
            "Fetched '{}'. Run 'lattice track {}' to track it ({} by default).",
            branch_name,
            if unfrozen { "--force" } else { "" },
            freeze_note
        );
    }

    Ok(())
}

/// Handle no-checkout mode for bare repositories.
///
/// Per SPEC.md §4.6.7, with --no-checkout:
/// - Fetch the branch ref from remote (already done)
/// - Create/update the local branch ref (already done)
/// - Track the branch with parent inference
/// - Compute base as merge-base(branch_tip, parent_tip)
/// - Default to frozen unless --unfrozen
/// - Print worktree creation guidance
async fn handle_no_checkout_mode(
    ctx: &Context,
    git: &Git,
    branch_name: &str,
    pr_info: Option<&PullRequest>,
    unfrozen: bool,
) -> Result<()> {
    use crate::engine::scan::scan;

    let snapshot = scan(git).context("Failed to scan repository")?;

    // Get trunk for fallback parent
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    let branch = BranchName::new(branch_name).context("Invalid branch name")?;

    // Check if already tracked
    if snapshot.metadata.contains_key(&branch) {
        if !ctx.quiet {
            println!("Branch '{}' is already tracked.", branch_name);
        }
        return Ok(());
    }

    // Get branch tip
    let branch_tip = snapshot
        .branches
        .get(&branch)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found after fetch", branch_name))?
        .clone();

    // Determine parent from PR base or trunk
    let parent_name = determine_parent(pr_info, trunk);
    let parent_branch =
        BranchName::new(&parent_name).context("Invalid parent branch name from PR")?;

    // Get parent tip (fallback to trunk if PR base branch doesn't exist locally)
    let parent_tip = snapshot
        .branches
        .get(&parent_branch)
        .or_else(|| snapshot.branches.get(trunk))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Neither parent '{}' nor trunk '{}' found",
                parent_name,
                trunk
            )
        })?
        .clone();

    // Compute base as merge-base(branch_tip, parent_tip)
    let base_oid = git.merge_base(&branch_tip, &parent_tip)?.ok_or_else(|| {
        anyhow::anyhow!(
            "No common ancestor between '{}' and parent '{}'",
            branch_name,
            parent_name
        )
    })?;

    // Determine if parent is trunk
    let parent_is_trunk = &parent_branch == trunk;
    let parent_info = if parent_is_trunk {
        ParentInfo::Trunk {
            name: parent_branch.to_string(),
        }
    } else {
        ParentInfo::Branch {
            name: parent_branch.to_string(),
        }
    };

    // Default to frozen unless --unfrozen (per SPEC.md §4.6.7)
    let freeze_state = if unfrozen {
        FreezeState::Unfrozen
    } else {
        FreezeState::Frozen {
            scope: FreezeScope::Single,
            reason: Some("fetched in no-checkout mode".to_string()),
            frozen_at: UtcTimestamp::now(),
        }
    };

    // Create metadata
    let now = UtcTimestamp::now();
    let metadata = BranchMetadataV1 {
        kind: METADATA_KIND.to_string(),
        schema_version: SCHEMA_VERSION,
        branch: BranchInfo {
            name: branch.to_string(),
        },
        parent: parent_info,
        base: BaseInfo {
            oid: base_oid.to_string(),
        },
        freeze: freeze_state,
        pr: PrState::None,
        timestamps: Timestamps {
            created_at: now.clone(),
            updated_at: now,
        },
    };

    // Write metadata (new branch, no expected old value)
    let store = MetadataStore::new(git);
    store
        .write_cas(&branch, None, &metadata)
        .context("Failed to write metadata")?;

    // Print success and worktree guidance
    let freeze_status = if unfrozen { "unfrozen" } else { "frozen" };
    if !ctx.quiet {
        println!(
            "Tracked branch '{}' with parent '{}' (base: {})",
            branch_name,
            parent_branch,
            &base_oid.as_str()[..7.min(base_oid.as_str().len())]
        );
        println!("Branch is {} by default.", freeze_status);
        println!();
        println!("To work on this branch, create a worktree:");
        println!("    git worktree add <path> {}", branch_name);
    }

    Ok(())
}

/// Determine parent branch from PR info or fall back to trunk.
///
/// Per SPEC.md §4.6.7: use PR base branch or fall back to trunk.
fn determine_parent(pr_info: Option<&PullRequest>, trunk: &BranchName) -> String {
    if let Some(pr) = pr_info {
        // Use PR base branch as parent
        pr.base.clone()
    } else {
        // Fall back to trunk
        trunk.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pr_number() {
        assert!("42".parse::<u64>().is_ok());
        assert!("feature-branch".parse::<u64>().is_err());
    }

    #[test]
    fn determine_parent_with_pr() {
        use crate::forge::PrState as ForgePrState;

        let pr = PullRequest {
            number: 42,
            title: "Test PR".to_string(),
            head: "feature-branch".to_string(),
            base: "develop".to_string(),
            url: "https://github.com/test/repo/pull/42".to_string(),
            is_draft: false,
            state: ForgePrState::Open,
            body: None,
            node_id: None,
        };
        let trunk = BranchName::new("main").unwrap();

        let parent = determine_parent(Some(&pr), &trunk);
        assert_eq!(parent, "develop");
    }

    #[test]
    fn determine_parent_without_pr() {
        let trunk = BranchName::new("main").unwrap();

        let parent = determine_parent(None, &trunk);
        assert_eq!(parent, "main");
    }
}
