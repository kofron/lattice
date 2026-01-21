//! undo command - Undo the last completed operation
//!
//! Per SPEC.md §8F.3, undo reverses the most recent committed Lattice operation:
//! - Ref moves are restored to their previous values
//! - Metadata changes are reverted
//!
//! # Limitations
//!
//! - Cannot undo remote operations (push, PR creation)
//! - Metadata updates where the old content wasn't stored may not be fully restorable
//! - Cannot undo while another operation is in progress (use `abort` first)
//!
//! # Remote Operation Warnings
//!
//! When undoing an operation that included remote changes (pushes, fetches),
//! the undo command displays a warning listing the remote operations. Local refs
//! are restored, but remote branches remain as-is and may require manual cleanup
//! (e.g., force-push to revert remote state).
//!
//! # Implementation Notes
//!
//! Undo uses force ref updates (`Git::update_ref_force`) rather than CAS updates
//! because the repository state may have changed since the operation was recorded.
//! The goal is unconditional restoration to the known good state from the journal.
//!
//! # Ledger Integration (Phase 7)
//!
//! After a successful undo, an `UndoApplied` event is recorded in the event ledger
//! for audit purposes. This includes the operation ID that was undone and the number
//! of refs that were restored.

use crate::core::ops::journal::{Journal, OpPhase, OpState, StepKind};
use crate::core::paths::LatticePaths;
use crate::core::types::Oid;
use crate::engine::gate::requirements;
use crate::engine::ledger::{Event, EventLedger};
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Undo the last completed operation.
///
/// Per SPEC.md §8F.3:
/// - Undoes the most recent committed Lattice operation
/// - Cannot undo remote PR creation or pushes
/// - Uses stored journal snapshots for ref restoration
///
/// # Phase 7 Improvements
///
/// - Records `UndoApplied` event in ledger
/// - Warns about remote operations that cannot be undone
/// - Uses Git interface instead of raw commands
pub fn undo(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    if ctx.debug {
        eprintln!("[debug] undo: opening repository at {:?}", cwd);
    }

    // Pre-flight gating check (RECOVERY is minimal - just RepoOpen)
    crate::engine::runner::check_requirements(&git, &requirements::RECOVERY)
        .map_err(|bundle| anyhow::anyhow!("Repository needs repair: {}", bundle))?;

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

    // Warn about remote operations that cannot be undone (Phase 7)
    if journal.has_remote_operations() {
        let remote_ops = journal.remote_operation_descriptions();
        if !remote_ops.is_empty() {
            eprintln!();
            eprintln!("Warning: This operation included remote changes that cannot be undone:");
            for desc in &remote_ops {
                eprintln!("  - {}", desc);
            }
            eprintln!();
            eprintln!("Local refs will be restored, but remote branches remain as-is.");
            eprintln!("You may need to force-push or manually revert changes on the remote.");
            eprintln!();
        }
    }

    // Get rollback operations from journal
    let rollbacks = journal.ref_updates_for_rollback();

    if rollbacks.is_empty() {
        if !ctx.quiet {
            println!("No ref changes to undo.");
        }
        return Ok(());
    }

    if ctx.debug {
        eprintln!("[debug] undo: {} ref changes to restore", rollbacks.len());
    }

    // Track successful restorations for ledger event
    let mut refs_restored: usize = 0;

    // Apply rollbacks using Git interface (Phase 7 improvement)
    for step in &rollbacks {
        match step {
            StepKind::RefUpdate {
                refname,
                old_oid,
                new_oid: _,
            } => {
                if let Some(old) = old_oid {
                    // Restore old value using Git interface
                    let old_oid_obj = Oid::new(old).context("Invalid old OID in journal")?;

                    // For undo, we use force update since the repo may have changed
                    // and we want to restore to the known good state unconditionally.
                    git.update_ref_force(
                        refname,
                        &old_oid_obj,
                        &format!("undo: restore {}", refname),
                    )
                    .with_context(|| format!("Failed to restore ref {}", refname))?;

                    if ctx.debug {
                        eprintln!("[debug] Restored {} to {}", refname, old);
                    }
                    refs_restored += 1;
                } else {
                    // Delete ref (was created by the operation)
                    // Use force delete since CAS may fail if ref changed
                    git.delete_ref_force(refname)
                        .with_context(|| format!("Failed to delete ref {}", refname))?;

                    if ctx.debug {
                        eprintln!("[debug] Deleted {}", refname);
                    }
                    refs_restored += 1;
                }
            }
            StepKind::MetadataWrite {
                branch,
                old_ref_oid,
                new_ref_oid: _,
            } => {
                // For metadata, restore the old ref or delete if it was created
                let refname = format!("refs/branch-metadata/{}", branch);
                if let Some(old) = old_ref_oid {
                    let old_oid_obj = Oid::new(old).context("Invalid old metadata OID")?;

                    git.update_ref_force(
                        &refname,
                        &old_oid_obj,
                        &format!("undo: restore metadata for {}", branch),
                    )
                    .with_context(|| format!("Failed to restore metadata ref {}", refname))?;

                    if ctx.debug {
                        eprintln!("[debug] Restored metadata for {}", branch);
                    }
                    refs_restored += 1;
                } else {
                    // Delete metadata ref (was created by the operation)
                    git.delete_ref_force(&refname)
                        .with_context(|| format!("Failed to delete metadata ref {}", refname))?;

                    if ctx.debug {
                        eprintln!("[debug] Deleted metadata for {}", branch);
                    }
                    refs_restored += 1;
                }
            }
            StepKind::MetadataDelete {
                branch,
                old_ref_oid,
            } => {
                // Restore the deleted metadata ref
                let refname = format!("refs/branch-metadata/{}", branch);
                let old_oid_obj = Oid::new(old_ref_oid).context("Invalid old metadata OID")?;

                git.update_ref_force(
                    &refname,
                    &old_oid_obj,
                    &format!("undo: restore deleted metadata for {}", branch),
                )
                .with_context(|| format!("Failed to restore metadata ref {}", refname))?;

                if ctx.debug {
                    eprintln!("[debug] Restored deleted metadata for {}", branch);
                }
                refs_restored += 1;
            }
            _ => {
                // Skip other step kinds (checkpoints, conflict paused, etc.)
            }
        }
    }

    // Record UndoApplied event in ledger (Phase 7)
    let ledger = EventLedger::new(&git);
    let event = Event::undo_applied(journal.op_id.as_str(), refs_restored);
    if let Err(e) = ledger.append(event) {
        if ctx.debug {
            eprintln!(
                "[debug] Warning: Could not record undo event in ledger: {}",
                e
            );
        }
        // Don't fail the undo just because ledger append failed
    }

    if !ctx.quiet {
        println!("Undo complete. {} ref(s) restored.", refs_restored);
    }

    Ok(())
}
