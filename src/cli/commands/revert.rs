//! revert command - Create revert branch off trunk
//!
//! Per SPEC.md 8D.12:
//!
//! - Creates new branch off trunk and performs `git revert <sha>`
//! - Handles conflicts with pause/continue/abort
//!
//! # Integrity Contract
//!
//! - Validates sha exists and is a commit
//! - Tracks new branch with parent = trunk
//! - Metadata updated only after refs succeed

use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeState, ParentInfo, PrState, Timestamps,
    METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpPhase, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::paths::LatticePaths;
use crate::core::types::{BranchName, UtcTimestamp};
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::{Git, GitState};

/// Create a revert branch for a commit.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `sha` - Commit SHA to revert
pub fn revert(ctx: &Context, sha: &str) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Check for in-progress operation
    if let Some(op_state) = OpState::read(&paths)? {
        bail!(
            "Another operation is in progress: {} ({}). Use 'lattice continue' or 'lattice abort'.",
            op_state.command,
            op_state.op_id
        );
    }

    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Ensure trunk is configured
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Validate sha exists and is a commit
    let output = Command::new("git")
        .args(["rev-parse", "--verify", &format!("{}^{{commit}}", sha)])
        .current_dir(&cwd)
        .output()
        .context("Failed to verify commit")?;

    if !output.status.success() {
        bail!("'{}' is not a valid commit", sha);
    }

    let full_sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let short_sha = &full_sha[..7.min(full_sha.len())];

    // Generate branch name
    let branch_name = BranchName::new(format!("revert-{}", short_sha))?;

    // Check if branch already exists
    if snapshot.branches.contains_key(&branch_name) {
        bail!("Branch '{}' already exists", branch_name);
    }

    // Get trunk tip
    let trunk_tip = snapshot
        .branches
        .get(trunk)
        .ok_or_else(|| anyhow::anyhow!("Trunk branch '{}' not found", trunk))?;

    if !ctx.quiet {
        println!(
            "Creating revert branch '{}' for commit {}...",
            branch_name, short_sha
        );
    }

    // Acquire lock
    let _lock = RepoLock::acquire(&paths).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("revert");

    // Write op-state
    let op_state = OpState::from_journal(&journal, &paths, info.work_dir.clone());
    op_state.write(&paths)?;

    // Create new branch off trunk
    let status = Command::new("git")
        .args(["checkout", "-b", branch_name.as_str(), trunk.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to create branch")?;

    if !status.success() {
        OpState::remove(&paths)?;
        bail!("git checkout -b failed");
    }

    journal.record_ref_update(
        format!("refs/heads/{}", branch_name),
        None,
        trunk_tip.to_string(),
    );

    // Execute git revert
    let status = Command::new("git")
        .args(["revert", "--no-edit", &full_sha])
        .current_dir(&cwd)
        .status()
        .context("Failed to run git revert")?;

    if !status.success() {
        // Check if it's a conflict
        let git_state = git.state();
        if matches!(git_state, GitState::Revert) || matches!(git_state, GitState::CherryPick) {
            // Conflict - pause
            journal.record_conflict_paused(branch_name.as_str(), "revert", vec![]);
            journal.pause();
            journal.write(&paths)?;

            let mut op_state = OpState::from_journal(&journal, &paths, info.work_dir.clone());
            op_state.phase = OpPhase::Paused;
            op_state.write(&paths)?;

            println!();
            println!("Conflict while reverting commit {}.", short_sha);
            println!("Resolve conflicts, then run 'lattice continue'.");
            println!("To abort, run 'lattice abort'.");
            return Ok(());
        } else {
            // Some other error - clean up
            let _ = Command::new("git")
                .args(["checkout", trunk.as_str()])
                .current_dir(&cwd)
                .status();
            let _ = Command::new("git")
                .args(["branch", "-D", branch_name.as_str()])
                .current_dir(&cwd)
                .status();
            OpState::remove(&paths)?;
            bail!("git revert failed");
        }
    }

    // Get new tip
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&cwd)
        .output()
        .context("Failed to get HEAD")?;

    let new_tip = String::from_utf8_lossy(&output.stdout).trim().to_string();

    journal.record_ref_update(
        format!("refs/heads/{}", branch_name),
        Some(trunk_tip.to_string()),
        new_tip,
    );

    // Create metadata for new branch
    let store = MetadataStore::new(&git);

    let now = UtcTimestamp::now();
    let metadata = BranchMetadataV1 {
        kind: METADATA_KIND.to_string(),
        schema_version: SCHEMA_VERSION,
        branch: BranchInfo {
            name: branch_name.to_string(),
        },
        parent: ParentInfo::Trunk {
            name: trunk.to_string(),
        },
        base: BaseInfo {
            oid: trunk_tip.to_string(),
        },
        freeze: FreezeState::Unfrozen,
        pr: PrState::None,
        timestamps: Timestamps {
            created_at: now.clone(),
            updated_at: now,
        },
    };

    let new_ref_oid = store
        .write_cas(&branch_name, None, &metadata)
        .with_context(|| format!("Failed to write metadata for '{}'", branch_name))?;

    journal.record_metadata_write(branch_name.as_str(), None, new_ref_oid);

    // Commit journal
    journal.commit();
    journal.write(&paths)?;

    // Clear op-state
    OpState::remove(&paths)?;

    if !ctx.quiet {
        println!("Revert complete.");
        println!("  Created '{}' reverting commit {}", branch_name, short_sha);
    }

    Ok(())
}
