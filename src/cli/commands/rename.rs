//! rename command - Rename current branch
//!
//! Per SPEC.md 8D.11:
//!
//! - Renames current branch
//! - Updates refs/heads/`<old>` -> `<new>`
//! - Updates metadata ref name
//! - Fixes parent references in other branches pointing to old name
//! - Journals ref renames (copy + delete pattern)
//!
//! # Integrity Contract
//!
//! - Must update all metadata parent references atomically
//! - Must never rename frozen branches
//! - Metadata updated only after refs succeed

use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::commands::phase3_helpers::check_freeze;
use crate::core::metadata::schema::{BranchInfo, ParentInfo};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::paths::LatticePaths;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Rename the current branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `new_name` - New name for the branch
pub fn rename(ctx: &Context, new_name: &str) -> Result<()> {
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

    // Get current branch
    let old_branch = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    // Check if tracked
    if !snapshot.metadata.contains_key(&old_branch) {
        bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            old_branch
        );
    }

    // Validate new name
    let new_branch = BranchName::new(new_name).context("Invalid new branch name")?;

    // Check if new name already exists
    if snapshot.branches.contains_key(&new_branch) {
        bail!("Branch '{}' already exists", new_branch);
    }

    // Check if same name
    if old_branch == new_branch {
        if !ctx.quiet {
            println!("Branch is already named '{}'", old_branch);
        }
        return Ok(());
    }

    // Cannot rename trunk
    if &old_branch == trunk {
        bail!("Cannot rename trunk branch");
    }

    // Check freeze policy
    check_freeze(&old_branch, &snapshot)?;

    if !ctx.quiet {
        println!("Renaming '{}' to '{}'...", old_branch, new_branch);
    }

    // Acquire lock
    let _lock = RepoLock::acquire(&paths).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("rename");

    // Write op-state
    let op_state = OpState::from_journal(&journal, &paths, info.work_dir.clone());
    op_state.write(&paths)?;

    // Get old branch OID
    let old_oid = snapshot
        .branches
        .get(&old_branch)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", old_branch))?;

    // Rename the git branch
    let status = Command::new("git")
        .args(["branch", "-m", old_branch.as_str(), new_branch.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to rename branch")?;

    if !status.success() {
        OpState::remove(&paths)?;
        bail!("git branch -m failed");
    }

    // Record ref changes in journal
    journal.record_ref_update(
        format!("refs/heads/{}", old_branch),
        Some(old_oid.to_string()),
        "0000000000000000000000000000000000000000".to_string(),
    );
    journal.record_ref_update(
        format!("refs/heads/{}", new_branch),
        None,
        old_oid.to_string(),
    );

    let store = MetadataStore::new(&git);

    // Get old metadata
    let old_meta = snapshot
        .metadata
        .get(&old_branch)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", old_branch))?;

    // Create new metadata with updated branch name
    let mut new_metadata = old_meta.metadata.clone();
    new_metadata.branch = BranchInfo {
        name: new_branch.to_string(),
    };
    new_metadata.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

    // Write new metadata
    let new_ref_oid = store
        .write_cas(&new_branch, None, &new_metadata)
        .with_context(|| format!("Failed to write metadata for '{}'", new_branch))?;

    journal.record_metadata_write(new_branch.as_str(), None, new_ref_oid);

    // Delete old metadata
    store
        .delete_cas(&old_branch, &old_meta.ref_oid)
        .with_context(|| format!("Failed to delete metadata for '{}'", old_branch))?;

    journal.record_metadata_delete(old_branch.as_str(), old_meta.ref_oid.to_string());

    // Update parent references in all branches that pointed to old name
    let mut updated_children = Vec::new();
    for (branch_name, scanned) in &snapshot.metadata {
        let parent_name = scanned.metadata.parent.name();
        if parent_name == old_branch.as_str() {
            // This branch's parent was the old name, update it
            let mut updated = scanned.metadata.clone();
            updated.parent = ParentInfo::Branch {
                name: new_branch.to_string(),
            };
            updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            let child_ref_oid = store
                .write_cas(branch_name, Some(&scanned.ref_oid), &updated)
                .with_context(|| format!("Failed to update parent in '{}'", branch_name))?;

            journal.record_metadata_write(
                branch_name.as_str(),
                Some(scanned.ref_oid.to_string()),
                child_ref_oid,
            );

            updated_children.push(branch_name.clone());
        }
    }

    if !ctx.quiet && !updated_children.is_empty() {
        println!(
            "  Updated parent references in {} branch(es)",
            updated_children.len()
        );
    }

    // Commit journal
    journal.commit();
    journal.write(&paths)?;

    // Clear op-state
    OpState::remove(&paths)?;

    if !ctx.quiet {
        println!("Rename complete.");
    }

    Ok(())
}
