// Legacy journal API - these commands will be migrated to executor pattern
#![allow(deprecated)]

//! pop command - Delete branch but keep changes uncommitted
//!
//! Per SPEC.md 8D.9:
//!
//! - Delete current branch but keep its net changes applied to parent as uncommitted changes
//! - Requires clean working tree at start
//! - Must remove metadata and re-parent children
//!
//! # Integrity Contract
//!
//! - Must never pop frozen branches
//! - Must require clean working tree
//! - Metadata updated only after refs succeed

use std::process::Command;

use anyhow::{Context as _, Result};

use crate::cli::commands::phase3_helpers::{
    check_freeze_affected_set, get_net_diff, is_working_tree_clean, reparent_children,
};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::paths::LatticePaths;
use crate::core::types::{BranchName, Oid};
use crate::engine::gate::requirements;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Pop current branch, keeping changes uncommitted.
///
/// # Arguments
///
/// * `ctx` - Execution context
pub fn pop(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Check for in-progress operation
    if let Some(op_state) = OpState::read(&paths)? {
        anyhow::bail!(
            "Another operation is in progress: {} ({}). Use 'lattice continue' or 'lattice abort'.",
            op_state.command,
            op_state.op_id
        );
    }

    // Require clean working tree
    if !is_working_tree_clean(&cwd)? {
        anyhow::bail!("Working tree is not clean. Commit or stash your changes first.");
    }

    // Pre-flight gating check
    crate::engine::runner::check_requirements(&git, &requirements::MUTATING)
        .map_err(|bundle| anyhow::anyhow!("Repository needs repair: {}", bundle))?;

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
        anyhow::bail!(
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

    // Build list of branches to check for freeze
    let mut affected = vec![current.clone()];
    if let Some(children) = snapshot.graph.children(&current) {
        affected.extend(children.iter().cloned());
    }

    // Check freeze policy
    check_freeze_affected_set(&affected, &snapshot)?;

    // Get base and tip for diff
    let base_oid = Oid::new(&current_meta.metadata.base.oid).context("Invalid base OID")?;
    let current_tip = snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?;

    if !ctx.quiet {
        println!(
            "Popping '{}' (changes will be uncommitted on '{}')...",
            current, parent_name
        );
    }

    // Get the net diff
    let diff = get_net_diff(&cwd, &base_oid, current_tip)?;

    if diff.is_empty() && !ctx.quiet {
        println!("  No changes in branch.");
    }

    // Acquire lock
    let _lock = RepoLock::acquire(&paths).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("pop");

    // Write op-state
    #[allow(deprecated)]
    let op_state = OpState::from_journal_legacy(&journal, &paths, info.work_dir.clone());
    op_state.write(&paths)?;

    // Reparent children first (before we delete)
    let reparented = reparent_children(&current, &parent_name, &snapshot, &git, &mut journal)?;

    if !ctx.quiet && !reparented.is_empty() {
        println!(
            "  Reparented {} child(ren) to '{}'",
            reparented.len(),
            parent_name
        );
    }

    // Checkout parent
    let status = Command::new("git")
        .args(["checkout", parent_name.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to checkout parent")?;

    if !status.success() {
        OpState::remove(&paths)?;
        anyhow::bail!("git checkout failed");
    }

    // Delete current branch
    let status = Command::new("git")
        .args(["branch", "-D", current.as_str()])
        .current_dir(&cwd)
        .status()
        .context("Failed to delete branch")?;

    if !status.success() {
        OpState::remove(&paths)?;
        anyhow::bail!("git branch -D failed");
    }

    journal.record_ref_update(
        format!("refs/heads/{}", current),
        Some(current_tip.to_string()),
        "0000000000000000000000000000000000000000".to_string(),
    );

    // Delete metadata
    let store = MetadataStore::new(&git);
    store
        .delete_cas(&current, &current_meta.ref_oid)
        .with_context(|| format!("Failed to delete metadata for '{}'", current))?;

    journal.record_metadata_delete(current.as_str(), current_meta.ref_oid.to_string());

    // Apply the diff as uncommitted changes
    if !diff.is_empty() {
        use std::io::Write;
        use std::process::Stdio;

        let mut child = Command::new("git")
            .args(["apply", "--3way"])
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn git apply")?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(diff.as_bytes())
                .context("Failed to write diff to git apply")?;
        }

        let output = child
            .wait_with_output()
            .context("Failed to wait for git apply")?;

        if !output.status.success() {
            // Try without --3way
            let mut child = Command::new("git")
                .args(["apply"])
                .current_dir(&cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("Failed to spawn git apply")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(diff.as_bytes())
                    .context("Failed to write diff to git apply")?;
            }

            let output = child.wait_with_output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("Warning: Could not apply changes cleanly: {}", stderr);
                eprintln!("The branch has been deleted but changes may be incomplete.");
            }
        }

        if !ctx.quiet {
            println!("  Applied changes as uncommitted files.");
        }
    }

    // Commit journal
    journal.commit();
    journal.write(&paths)?;

    // Clear op-state
    OpState::remove(&paths)?;

    if !ctx.quiet {
        println!("Pop complete. Changes are staged on '{}'.", parent_name);
    }

    Ok(())
}
