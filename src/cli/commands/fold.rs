//! fold command - Merge current branch into parent and delete
//!
//! Per SPEC.md 8D.8:
//!
//! - Merge current branch's changes into its parent, then delete current branch
//! - Re-parent children to parent
//! - --keep: keep the current branch name by renaming parent branch to current name after fold
//!
//! # Integrity Contract
//!
//! - Must never fold frozen branches
//! - Must re-parent children before deleting
//! - Metadata updated only after refs succeed

use std::process::Command;

use anyhow::{bail, Context as _, Result};

use crate::cli::commands::phase3_helpers::{check_freeze_affected_set, reparent_children};
use crate::core::metadata::schema::BranchInfo;
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Fold current branch into parent.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `keep` - Keep the current branch name by renaming parent
pub fn fold(ctx: &Context, keep: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let git_dir = git.git_dir();

    // Check for in-progress operation
    if let Some(op_state) = OpState::read(git_dir)? {
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
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    // Check if tracked
    if !snapshot.metadata.contains_key(&current) {
        bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            current
        );
    }

    // Get current metadata
    let current_meta = snapshot
        .metadata
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    // Get parent
    let parent_name = if current_meta.metadata.parent.is_trunk() {
        trunk.clone()
    } else {
        BranchName::new(current_meta.metadata.parent.name())
            .context("Invalid parent name in metadata")?
    };

    // Cannot fold into trunk (trunk is special)
    if &parent_name == trunk {
        bail!("Cannot fold into trunk. Use 'lattice merge' instead.");
    }

    // Build list of branches to check for freeze
    let mut affected = vec![current.clone(), parent_name.clone()];
    if let Some(children) = snapshot.graph.children(&current) {
        affected.extend(children.iter().cloned());
    }

    // Check freeze policy
    check_freeze_affected_set(&affected, &snapshot)?;

    // Get parent metadata
    let parent_meta = snapshot
        .metadata
        .get(&parent_name)
        .ok_or_else(|| anyhow::anyhow!("Parent '{}' is not tracked", parent_name))?;

    if !ctx.quiet {
        println!("Folding '{}' into '{}'...", current, parent_name);
    }

    // Acquire lock
    let _lock = RepoLock::acquire(git_dir).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("fold");

    // Write op-state
    let op_state = OpState::from_journal(&journal);
    op_state.write(git_dir)?;

    // Get current and parent tips
    let current_tip = snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?;

    let parent_tip = snapshot
        .branches
        .get(&parent_name)
        .ok_or_else(|| anyhow::anyhow!("Parent branch '{}' not found", parent_name))?;

    // Checkout parent
    let status = Command::new("git")
        .args(["checkout", parent_name.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to checkout parent")?;

    if !status.success() {
        OpState::remove(git_dir)?;
        bail!("git checkout failed");
    }

    // Merge current into parent (fast-forward if possible, otherwise create merge commit)
    let status = Command::new("git")
        .args(["merge", "--ff", current.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to merge")?;

    if !status.success() {
        // Try with merge commit
        let status = Command::new("git")
            .args([
                "merge",
                "--no-ff",
                "-m",
                &format!("Fold '{}' into '{}'", current, parent_name),
                current.as_str(),
            ])
            .current_dir(&cwd)
            .status()
            .context("Failed to merge")?;

        if !status.success() {
            // Restore state
            let _ = Command::new("git")
                .args(["checkout", current.as_str()])
                .current_dir(&cwd)
                .status();
            OpState::remove(git_dir)?;
            bail!("git merge failed");
        }
    }

    // Get new parent tip
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&cwd)
        .output()
        .context("Failed to get HEAD")?;

    let new_parent_tip = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Record parent ref update
    journal.record_ref_update(
        format!("refs/heads/{}", parent_name),
        Some(parent_tip.to_string()),
        new_parent_tip,
    );

    // Reparent children of current to parent
    let reparented = reparent_children(&current, &parent_name, &snapshot, &git, &mut journal)?;

    if !ctx.quiet && !reparented.is_empty() {
        println!(
            "  Reparented {} child(ren) to '{}'",
            reparented.len(),
            parent_name
        );
    }

    let store = MetadataStore::new(&git);

    // Delete current branch
    let status = Command::new("git")
        .args(["branch", "-D", current.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to delete current branch")?;

    if !status.success() {
        OpState::remove(git_dir)?;
        bail!("git branch -D failed");
    }

    journal.record_ref_update(
        format!("refs/heads/{}", current),
        Some(current_tip.to_string()),
        "0000000000000000000000000000000000000000".to_string(),
    );

    // Delete current metadata
    store
        .delete_cas(&current, &current_meta.ref_oid)
        .with_context(|| format!("Failed to delete metadata for '{}'", current))?;

    journal.record_metadata_delete(current.as_str(), current_meta.ref_oid.to_string());

    if !ctx.quiet {
        println!("  Deleted '{}'", current);
    }

    // Handle --keep: rename parent to current's name
    if keep {
        if !ctx.quiet {
            println!("  Renaming '{}' to '{}'...", parent_name, current);
        }

        // Rename the branch
        let status = Command::new("git")
            .args(["branch", "-m", parent_name.as_str(), current.as_str()])
            .current_dir(&cwd)
            .status()
            .context("Failed to rename branch")?;

        if !status.success() {
            OpState::remove(git_dir)?;
            bail!("git branch -m failed");
        }

        // Get current parent OID (after merge)
        let output = Command::new("git")
            .args(["rev-parse", current.as_str()])
            .current_dir(&cwd)
            .output()
            .context("Failed to get new branch OID")?;

        let renamed_oid = String::from_utf8_lossy(&output.stdout).trim().to_string();

        journal.record_ref_update(
            format!("refs/heads/{}", parent_name),
            Some(renamed_oid.clone()),
            "0000000000000000000000000000000000000000".to_string(),
        );
        journal.record_ref_update(format!("refs/heads/{}", current), None, renamed_oid);

        // Update parent metadata with new name
        let mut updated_parent_meta = parent_meta.metadata.clone();
        updated_parent_meta.branch = BranchInfo {
            name: current.to_string(),
        };
        updated_parent_meta.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

        // Write new metadata under new name
        let new_ref_oid = store
            .write_cas(&current, None, &updated_parent_meta)
            .with_context(|| format!("Failed to write metadata for '{}'", current))?;

        journal.record_metadata_write(current.as_str(), None, new_ref_oid);

        // Delete old parent metadata
        store
            .delete_cas(&parent_name, &parent_meta.ref_oid)
            .with_context(|| format!("Failed to delete metadata for '{}'", parent_name))?;

        journal.record_metadata_delete(parent_name.as_str(), parent_meta.ref_oid.to_string());

        // Update children that were just reparented to use new name
        // (They were reparented to parent_name, but now parent_name is renamed to current)
        // Actually, we need to update their parent refs again
        let fresh_snapshot = scan(&git)?;
        for child in &reparented {
            if let Some(child_meta) = fresh_snapshot.metadata.get(child) {
                let mut updated = child_meta.metadata.clone();
                updated.parent = crate::core::metadata::schema::ParentInfo::Branch {
                    name: current.to_string(),
                };
                updated.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

                let child_ref_oid = store.write_cas(child, Some(&child_meta.ref_oid), &updated)?;

                journal.record_metadata_write(
                    child.as_str(),
                    Some(child_meta.ref_oid.to_string()),
                    child_ref_oid,
                );
            }
        }
    }

    // Commit journal
    journal.commit();
    journal.write(git_dir)?;

    // Clear op-state
    OpState::remove(git_dir)?;

    if !ctx.quiet {
        println!("Fold complete.");
    }

    Ok(())
}
