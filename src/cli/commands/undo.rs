//! undo command - Undo the last completed operation

use crate::core::ops::journal::{Journal, OpPhase, OpState, StepKind};
use crate::core::paths::LatticePaths;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Undo the last completed operation.
pub fn undo(ctx: &Context) -> Result<()> {
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
            "Cannot undo while operation '{}' is in progress. Use 'lattice abort' first.",
            op_state.command
        );
    }

    // Find most recent committed journal
    let ops_dir = paths.repo_ops_dir();
    if !ops_dir.exists() {
        bail!("No operations to undo");
    }

    let mut journals: Vec<_> = std::fs::read_dir(&ops_dir)
        .context("Failed to read ops directory")?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .collect();

    if journals.is_empty() {
        bail!("No operations to undo");
    }

    // Sort by modification time (most recent first)
    journals.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).ok();
        let b_time = b.metadata().and_then(|m| m.modified()).ok();
        b_time.cmp(&a_time)
    });

    // Find most recent committed journal
    let mut last_committed: Option<Journal> = None;
    for entry in journals {
        let path = entry.path();
        let content = std::fs::read_to_string(&path).context("Failed to read journal")?;
        let journal: Journal = serde_json::from_str(&content).context("Failed to parse journal")?;

        if journal.phase == OpPhase::Committed {
            last_committed = Some(journal);
            break;
        }
    }

    let journal =
        last_committed.ok_or_else(|| anyhow::anyhow!("No committed operations to undo"))?;

    if !ctx.quiet {
        println!("Undoing: {} ({})", journal.command, journal.op_id);
    }

    // Check if operation is undoable (local refs only)
    // For now, we assume all operations are undoable
    // A full implementation would check journal.has_remote_operations()

    // Get rollback operations from journal
    let rollbacks = journal.ref_updates_for_rollback();

    if rollbacks.is_empty() {
        if !ctx.quiet {
            println!("No ref changes to undo.");
        }
        return Ok(());
    }

    // Apply rollbacks
    for step in &rollbacks {
        match step {
            StepKind::RefUpdate {
                refname,
                old_oid,
                new_oid: _,
            } => {
                if let Some(old) = old_oid {
                    // Restore old value
                    let status = std::process::Command::new("git")
                        .args(["update-ref", refname, old])
                        .current_dir(&cwd)
                        .status()
                        .with_context(|| format!("Failed to restore ref {}", refname))?;

                    if !status.success() {
                        bail!("Failed to restore ref {}", refname);
                    }

                    if ctx.debug {
                        eprintln!("[debug] Restored {} to {}", refname, old);
                    }
                } else {
                    // Delete ref (was created by the operation)
                    let status = std::process::Command::new("git")
                        .args(["update-ref", "-d", refname])
                        .current_dir(&cwd)
                        .status()
                        .with_context(|| format!("Failed to delete ref {}", refname))?;

                    if !status.success() {
                        eprintln!("Warning: Failed to delete ref {}", refname);
                    }

                    if ctx.debug {
                        eprintln!("[debug] Deleted {}", refname);
                    }
                }
            }
            StepKind::MetadataWrite {
                branch,
                old_ref_oid,
                new_ref_oid: _,
            } => {
                // For metadata, we need to restore the old ref or delete if it was created
                let refname = format!("refs/lattice/metadata/{}", branch);
                if let Some(old) = old_ref_oid {
                    let status = std::process::Command::new("git")
                        .args(["update-ref", &refname, old])
                        .current_dir(&cwd)
                        .status()
                        .with_context(|| format!("Failed to restore metadata ref {}", refname))?;

                    if !status.success() {
                        eprintln!("Warning: Failed to restore metadata ref {}", refname);
                    }
                } else {
                    let status = std::process::Command::new("git")
                        .args(["update-ref", "-d", &refname])
                        .current_dir(&cwd)
                        .status()
                        .with_context(|| format!("Failed to delete metadata ref {}", refname))?;

                    if !status.success() {
                        eprintln!("Warning: Failed to delete metadata ref {}", refname);
                    }
                }
            }
            StepKind::MetadataDelete {
                branch,
                old_ref_oid,
            } => {
                // Restore the deleted metadata ref
                let refname = format!("refs/lattice/metadata/{}", branch);
                let status = std::process::Command::new("git")
                    .args(["update-ref", &refname, old_ref_oid])
                    .current_dir(&cwd)
                    .status()
                    .with_context(|| format!("Failed to restore metadata ref {}", refname))?;

                if !status.success() {
                    eprintln!("Warning: Failed to restore metadata ref {}", refname);
                }
            }
            _ => {
                // Skip other step kinds (checkpoints, conflict paused, etc.)
            }
        }
    }

    if !ctx.quiet {
        println!("Undo complete. {} ref(s) restored.", rollbacks.len());
    }

    Ok(())
}
