//! track command - Start tracking a branch

use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeScope, FreezeState, ParentInfo, PrState,
    Timestamps, METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::metadata::store::MetadataStore;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};
use std::io::{self, Write};

/// Start tracking a branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `branch` - Branch to track (defaults to current)
/// * `parent` - Set parent branch explicitly
/// * `force` - Auto-select nearest tracked ancestor
/// * `as_frozen` - Track as frozen
pub fn track(
    ctx: &Context,
    branch: Option<&str>,
    parent: Option<&str>,
    force: bool,
    as_frozen: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Ensure trunk is configured
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

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

    // Check if already tracked
    if snapshot.metadata.contains_key(&target) {
        if !ctx.quiet {
            println!("Branch '{}' is already tracked", target);
        }
        return Ok(());
    }

    // Can't track trunk
    if &target == trunk {
        bail!("Cannot track trunk branch '{}'", trunk);
    }

    // Determine parent
    let parent_branch = if let Some(name) = parent {
        let p = BranchName::new(name).context("Invalid parent branch name")?;
        // Parent must be tracked or trunk
        if &p != trunk && !snapshot.metadata.contains_key(&p) {
            bail!(
                "Parent '{}' is not tracked. Track it first or use trunk.",
                p
            );
        }
        p
    } else if force {
        // Find nearest tracked ancestor via git merge-base
        find_nearest_tracked_ancestor(&git, &target, &snapshot)?
    } else if ctx.interactive {
        // Interactive selection
        let mut candidates: Vec<_> = snapshot.metadata.keys().collect();
        candidates.push(trunk);
        candidates.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        candidates.dedup();

        println!("Select parent branch for '{}':", target);
        for (i, b) in candidates.iter().enumerate() {
            let trunk_marker = if *b == trunk { " (trunk)" } else { "" };
            println!("  {}. {}{}", i + 1, b, trunk_marker);
        }
        print!("Enter number: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let idx = input
            .trim()
            .parse::<usize>()
            .context("Invalid selection")?
            .saturating_sub(1);

        if idx >= candidates.len() {
            bail!("Invalid selection");
        }

        candidates[idx].clone()
    } else {
        bail!("No parent specified. Use --parent, --force, or run interactively.");
    };

    // Get branch tip and parent tip
    let branch_oid = snapshot
        .branches
        .get(&target)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", target))?;

    let parent_oid = if &parent_branch == trunk {
        snapshot
            .branches
            .get(trunk)
            .ok_or_else(|| anyhow::anyhow!("Trunk branch '{}' not found", trunk))?
    } else {
        snapshot
            .branches
            .get(&parent_branch)
            .ok_or_else(|| anyhow::anyhow!("Parent branch '{}' not found", parent_branch))?
    };

    // Compute base via merge-base (the point where branch diverged from parent)
    // This is more correct than using parent tip directly, especially when
    // parent has advanced past the divergence point.
    let base_oid = git
        .merge_base(branch_oid, parent_oid)
        .context("Failed to compute merge-base")?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot track '{}': no common ancestor with parent '{}'. \
                 The branch may have been created independently or from a different history.",
                target,
                parent_branch
            )
        })?;

    // Create parent ref
    let parent_ref = if &parent_branch == trunk {
        ParentInfo::Trunk {
            name: parent_branch.to_string(),
        }
    } else {
        ParentInfo::Branch {
            name: parent_branch.to_string(),
        }
    };

    // Create metadata
    let freeze_state = if as_frozen {
        FreezeState::frozen(FreezeScope::Single, None)
    } else {
        FreezeState::Unfrozen
    };

    let now = crate::core::types::UtcTimestamp::now();
    let metadata = BranchMetadataV1 {
        kind: METADATA_KIND.to_string(),
        schema_version: SCHEMA_VERSION,
        branch: BranchInfo {
            name: target.to_string(),
        },
        parent: parent_ref,
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
    let store = MetadataStore::new(&git);
    store
        .write_cas(&target, None, &metadata)
        .context("Failed to write metadata")?;

    if !ctx.quiet {
        println!(
            "Tracking '{}' with parent '{}' (base: {})",
            target,
            parent_branch,
            &base_oid.as_str()[..7]
        );
        if as_frozen {
            println!("  (frozen)");
        }
    }

    Ok(())
}

/// Find the nearest tracked ancestor of a branch.
pub fn find_nearest_tracked_ancestor(
    git: &Git,
    branch: &BranchName,
    snapshot: &crate::engine::scan::RepoSnapshot,
) -> Result<BranchName> {
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured"))?;

    let branch_oid = snapshot
        .branches
        .get(branch)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch))?;

    // Check all tracked branches plus trunk
    let mut candidates: Vec<(&BranchName, &crate::core::types::Oid)> = snapshot
        .metadata
        .keys()
        .filter_map(|b| snapshot.branches.get(b).map(|oid| (b, oid)))
        .collect();

    // Add trunk
    if let Some(trunk_oid) = snapshot.branches.get(trunk) {
        candidates.push((trunk, trunk_oid));
    }

    // Find merge-base with each candidate and pick the closest
    let mut best: Option<(BranchName, i32)> = None;

    for (candidate, candidate_oid) in candidates {
        if let Ok(Some(merge_base)) = git.merge_base(branch_oid, candidate_oid) {
            // Count commits from merge_base to branch tip
            let distance = count_commits(git, &merge_base, branch_oid).unwrap_or(i32::MAX);

            if best.is_none() || distance < best.as_ref().unwrap().1 {
                best = Some((candidate.clone(), distance));
            }
        }
    }

    best.map(|(b, _)| b)
        .ok_or_else(|| anyhow::anyhow!("No tracked ancestor found"))
}

/// Count commits between two OIDs.
pub fn count_commits(
    git: &Git,
    from: &crate::core::types::Oid,
    to: &crate::core::types::Oid,
) -> Result<i32> {
    // Use rev-list to count
    let output = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("{}..{}", from, to)])
        .current_dir(git.git_dir().parent().unwrap_or(git.git_dir()))
        .output()
        .context("Failed to run git rev-list")?;

    let count_str = String::from_utf8_lossy(&output.stdout);
    count_str
        .trim()
        .parse()
        .context("Failed to parse commit count")
}
