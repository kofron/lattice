//! continue and abort commands - Resume or cancel paused operations
//!
//! Per SPEC.md Section 4.2.2, these commands provide crash recovery:
//! - `continue`: Resume a paused operation after resolving conflicts
//! - `abort`: Cancel a paused operation and restore pre-operation state
//!
//! Per SPEC.md Section 4.6.5, these must run from the originating worktree
//! when the operation is paused due to Git conflicts.
//!
//! # Multi-step Continuation (Milestone 0.5)
//!
//! When an operation pauses due to a conflict (e.g., restack with 5 branches
//! pausing on branch 2), `continue` now:
//! 1. Completes the immediate git operation (finish the rebase)
//! 2. Loads remaining steps from the journal
//! 3. Executes remaining steps in sequence
//! 4. Handles nested conflicts (pause again if needed)
//! 5. Clears op-state only after all steps complete

use crate::core::ops::journal::{AwaitingReason, Journal, OpPhase, OpState, PLAN_SCHEMA_VERSION};
use crate::core::ops::lock::RepoLock;
use crate::core::paths::LatticePaths;
use crate::core::types::BranchName;
use crate::engine::gate::requirements;
use crate::engine::ledger::{Event, EventLedger};
use crate::engine::plan::PlanStep;
use crate::engine::rollback::{rollback_journal, RollbackResult};
use crate::engine::Context;
use crate::git::{Git, GitState};
use anyhow::{bail, Context as _, Result};
use std::path::Path;
use std::process::Command;

/// Continue a paused operation after resolving conflicts.
///
/// Per Milestone 0.5, this now properly resumes multi-step operations:
/// 1. Completes the immediate git operation
/// 2. Loads and executes remaining steps from the journal
/// 3. Handles nested conflicts
/// 4. Only clears op-state after all steps complete
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `all` - Stage all changes before continuing
pub fn continue_op(ctx: &Context, all: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Pre-flight gating check (RECOVERY is minimal - just RepoOpen)
    crate::engine::runner::check_requirements(&git, &requirements::RECOVERY)
        .map_err(|bundle| anyhow::anyhow!("Repository needs repair: {}", bundle))?;

    // Check for in-progress operation
    let op_state =
        OpState::read(&paths)?.ok_or_else(|| anyhow::anyhow!("No operation in progress"))?;

    if op_state.phase != OpPhase::Paused {
        bail!(
            "Operation '{}' is not paused (phase: {:?})",
            op_state.command,
            op_state.phase
        );
    }

    // Verify plan schema version compatibility (SPEC.md §4.6.5)
    if op_state.plan_schema_version != PLAN_SCHEMA_VERSION {
        bail!(
            "Operation created by plan schema v{}; this binary expects v{}.\n\
             Run 'lattice abort' to cancel, or use a matching binary version to continue.",
            op_state.plan_schema_version,
            PLAN_SCHEMA_VERSION
        );
    }

    // Stage all if requested
    if all {
        let status = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add")?;

        if !status.success() {
            bail!("git add failed");
        }
    }

    // Check git state and continue if needed
    let git_state = git.state();
    if git_state.is_in_progress() {
        // Continue the git operation
        let continue_args = match git_state {
            GitState::Rebase { .. } => vec!["rebase", "--continue"],
            GitState::Merge => vec!["merge", "--continue"],
            GitState::CherryPick => vec!["cherry-pick", "--continue"],
            GitState::Revert => vec!["revert", "--continue"],
            GitState::Bisect => bail!("Cannot continue a bisect operation with lattice"),
            GitState::ApplyMailbox => vec!["am", "--continue"],
            GitState::Clean => unreachable!(), // Already checked is_in_progress()
        };

        if !ctx.quiet {
            println!("Continuing git operation...");
        }

        let status = Command::new("git")
            .args(&continue_args)
            .current_dir(&cwd)
            .status()
            .context("Failed to continue git operation")?;

        if !status.success() {
            // Check if still in conflict
            let new_state = git.state();
            if new_state.is_in_progress() {
                println!();
                println!("Conflicts remain. Resolve them and run 'lattice continue' again.");
                return Ok(());
            }
            bail!("git {} failed", continue_args.join(" "));
        }
    }

    // Git operation completed - check for remaining steps (Milestone 0.5)
    let journal =
        Journal::read(&paths, &op_state.op_id).context("Failed to read operation journal")?;

    if journal.has_remaining_steps() {
        // Execute remaining steps
        execute_remaining_steps(ctx, &git, &paths, &op_state, &journal)?;
    } else {
        // No remaining steps - operation complete
        complete_operation(ctx, &git, &paths, &op_state)?;
    }

    Ok(())
}

/// Abort a paused operation and restore pre-operation state.
///
/// Per SPEC.md Section 4.2.2, abort must:
/// 1. Validate origin worktree (can only abort from where op started)
/// 2. Abort any in-progress Git operation
/// 3. Roll back ref changes using journal
/// 4. Record Aborted event in ledger
/// 5. Clear op-state marker
pub fn abort(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Pre-flight gating check (RECOVERY is minimal - just RepoOpen)
    crate::engine::runner::check_requirements(&git, &requirements::RECOVERY)
        .map_err(|bundle| anyhow::anyhow!("Repository needs repair: {}", bundle))?;

    // Check for in-progress operation
    let op_state =
        OpState::read(&paths)?.ok_or_else(|| anyhow::anyhow!("No operation in progress"))?;

    // Step 1: Validate origin worktree
    // Per SPEC.md §4.6.5, abort must run from the originating worktree
    if let Err(msg) = op_state.check_origin_worktree(&info.git_dir) {
        bail!("{}", msg);
    }

    if !ctx.quiet {
        println!("Aborting {}...", op_state.command);
    }

    // Step 2: Abort the git operation if any
    abort_git_operation(&git, &cwd)?;

    // Step 3: Roll back ref changes using journal
    let rollback_result = rollback_refs(&git, &paths, &op_state, ctx)?;

    // Step 4: Record Aborted event in ledger
    record_aborted_event(&git, &op_state, ctx);

    // Step 5: Clear op-state (only if rollback was complete)
    if rollback_result.complete {
        OpState::remove(&paths)?;
        if !ctx.quiet {
            println!("Operation '{}' aborted.", op_state.command);
        }
    } else {
        // Partial rollback - leave op-state but update phase
        let mut updated_state = op_state.clone();
        updated_state.phase = OpPhase::Paused;
        updated_state.write(&paths)?;

        eprintln!();
        eprintln!("Warning: Partial rollback - some refs could not be restored:");
        for (refname, error) in &rollback_result.failed {
            eprintln!("  {}: {}", refname, error);
        }
        eprintln!();
        eprintln!("The repository may be in an inconsistent state.");
        eprintln!("Run 'lattice doctor' for guidance on resolving this.");
    }

    Ok(())
}

/// Abort any in-progress Git operation.
fn abort_git_operation(git: &Git, cwd: &Path) -> Result<()> {
    let git_state = git.state();
    let abort_args: Option<Vec<&str>> = match git_state {
        GitState::Rebase { .. } => Some(vec!["rebase", "--abort"]),
        GitState::Merge => Some(vec!["merge", "--abort"]),
        GitState::CherryPick => Some(vec!["cherry-pick", "--abort"]),
        GitState::Revert => Some(vec!["revert", "--abort"]),
        GitState::Bisect => Some(vec!["bisect", "reset"]),
        GitState::ApplyMailbox => Some(vec!["am", "--abort"]),
        GitState::Clean => None,
    };

    if let Some(args) = abort_args {
        let status = Command::new("git")
            .args(&args)
            .current_dir(cwd)
            .status()
            .context("Failed to abort git operation")?;

        if !status.success() {
            eprintln!("Warning: git {} may have failed", args.join(" "));
        }
    }

    Ok(())
}

/// Roll back ref changes using journal.
fn rollback_refs(
    git: &Git,
    paths: &LatticePaths,
    op_state: &OpState,
    ctx: &Context,
) -> Result<RollbackResult> {
    // Load the journal
    let journal = match Journal::read(paths, &op_state.op_id) {
        Ok(j) => j,
        Err(e) => {
            if !ctx.quiet {
                eprintln!("Warning: Could not load journal: {}", e);
                eprintln!("Skipping ref rollback.");
            }
            // Return an empty successful result - no refs to roll back
            return Ok(RollbackResult::new());
        }
    };

    // Check if there are any ref updates to roll back
    let rollback_entries = journal.ref_updates_for_rollback();
    if rollback_entries.is_empty() {
        if ctx.debug {
            eprintln!("[debug] No ref updates to roll back");
        }
        return Ok(RollbackResult::new());
    }

    if ctx.debug {
        eprintln!(
            "[debug] Rolling back {} ref updates",
            rollback_entries.len()
        );
    }

    // Perform the rollback
    let result = rollback_journal(git, &journal);

    if ctx.debug {
        eprintln!(
            "[debug] Rollback result: {} succeeded, {} failed",
            result.rolled_back.len(),
            result.failed.len()
        );
    }

    Ok(result)
}

/// Record an Aborted event in the event ledger.
fn record_aborted_event(git: &Git, op_state: &OpState, ctx: &Context) {
    let ledger = EventLedger::new(git);

    let event = Event::aborted(op_state.op_id.as_str(), "user-initiated abort");

    if let Err(e) = ledger.append(event) {
        if ctx.debug {
            eprintln!("[debug] Warning: Could not record abort event: {}", e);
        }
    }
}

// =============================================================================
// Multi-step Continuation Support (Milestone 0.5)
// =============================================================================

/// Execute remaining steps from a paused operation.
///
/// This is the core of Milestone 0.5: after resolving a conflict and completing
/// the immediate git operation, we execute any remaining steps that were stored
/// in the journal when the operation paused.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `git` - Git interface
/// * `paths` - Repository paths
/// * `op_state` - Current operation state
/// * `journal` - The operation journal with remaining steps
fn execute_remaining_steps(
    ctx: &Context,
    git: &Git,
    paths: &LatticePaths,
    op_state: &OpState,
    journal: &Journal,
) -> Result<()> {
    // Deserialize remaining steps from JSON
    let remaining_json = journal
        .remaining_steps_json()
        .ok_or_else(|| anyhow::anyhow!("No remaining steps found in journal"))?;

    let remaining_steps: Vec<PlanStep> = serde_json::from_str(remaining_json)
        .context("Failed to deserialize remaining steps from journal")?;

    if remaining_steps.is_empty() {
        // No steps to execute - complete the operation
        return complete_operation(ctx, git, paths, op_state);
    }

    if !ctx.quiet {
        println!(
            "Resuming {} with {} remaining steps...",
            op_state.command,
            remaining_steps.len()
        );
    }

    // Acquire lock before continuing execution (per ARCHITECTURE.md §6.2)
    let _lock = RepoLock::acquire(paths).context("Failed to acquire repository lock")?;

    // Re-validate worktree occupancy (per ARCHITECTURE.md §6.2)
    // Occupancy may have changed since we paused
    validate_occupancy_for_steps(git, &remaining_steps)?;

    // Re-load journal for appending (we'll add new steps as we execute)
    let mut journal = Journal::read(paths, &op_state.op_id)?;

    // Execute remaining steps one by one
    for (i, step) in remaining_steps.iter().enumerate() {
        if ctx.debug {
            eprintln!(
                "[debug] Executing remaining step {}/{}: {:?}",
                i + 1,
                remaining_steps.len(),
                step.description()
            );
        }

        match execute_single_step(git, step, &mut journal, paths)? {
            ContinueStepResult::Continue => {
                // Step executed successfully, continue to next
            }
            ContinueStepResult::Pause { branch, git_state } => {
                // Nested conflict - need to pause again
                let new_remaining: Vec<PlanStep> = remaining_steps[i + 1..].to_vec();
                pause_for_nested_conflict(
                    ctx,
                    git,
                    paths,
                    op_state,
                    &mut journal,
                    &branch,
                    &git_state,
                    new_remaining,
                )?;
                return Ok(());
            }
            ContinueStepResult::Abort { error } => {
                // Step failed - abort the operation
                bail!("Step failed: {}", error);
            }
        }
    }

    // All remaining steps completed successfully
    complete_operation(ctx, git, paths, op_state)
}

/// Result of executing a single step during continuation.
enum ContinueStepResult {
    /// Step completed successfully.
    Continue,
    /// Step caused a conflict, need to pause again.
    Pause { branch: String, git_state: GitState },
    /// Step failed.
    Abort { error: String },
}

/// Execute a single plan step during continuation.
///
/// This replicates the logic from `Executor::execute_step` but adapted for
/// the continuation context where we don't have a full Executor instance.
fn execute_single_step(
    git: &Git,
    step: &PlanStep,
    journal: &mut Journal,
    paths: &LatticePaths,
) -> Result<ContinueStepResult> {
    use crate::core::metadata::store::MetadataStore;
    use crate::core::types::Oid;

    match step {
        PlanStep::UpdateRefCas {
            refname,
            old_oid,
            new_oid,
            reason,
        } => {
            let new = Oid::new(new_oid).context("Invalid new OID")?;
            let old = old_oid
                .as_ref()
                .map(Oid::new)
                .transpose()
                .context("Invalid old OID")?;

            git.update_ref_cas(refname, &new, old.as_ref(), reason)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "CAS failed for {}: expected {:?}, repository may have changed",
                        refname,
                        old_oid
                    )
                    .context(e)
                })?;

            journal.append_ref_update(paths, refname, old_oid.clone(), new_oid)?;
            Ok(ContinueStepResult::Continue)
        }

        PlanStep::DeleteRefCas {
            refname,
            old_oid,
            reason: _,
        } => {
            let old = Oid::new(old_oid).context("Invalid old OID")?;

            git.delete_ref_cas(refname, &old).map_err(|e| {
                anyhow::anyhow!(
                    "CAS failed for {}: expected {}, repository may have changed",
                    refname,
                    old_oid
                )
                .context(e)
            })?;

            journal.append_ref_update(paths, refname, Some(old_oid.clone()), "")?;
            Ok(ContinueStepResult::Continue)
        }

        PlanStep::WriteMetadataCas {
            branch,
            old_ref_oid,
            metadata,
        } => {
            let store = MetadataStore::new(git);
            let branch_name = BranchName::new(branch).context("Invalid branch name")?;

            let old = old_ref_oid
                .as_ref()
                .map(Oid::new)
                .transpose()
                .context("Invalid old metadata OID")?;

            let new_oid = store
                .write_cas(&branch_name, old.as_ref(), metadata)
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Metadata CAS failed for {}: expected {:?}, repository may have changed",
                        branch,
                        old_ref_oid
                    )
                    .context(e)
                })?;

            journal.append_metadata_write(
                paths,
                branch,
                old_ref_oid.clone(),
                new_oid.to_string(),
            )?;
            Ok(ContinueStepResult::Continue)
        }

        PlanStep::DeleteMetadataCas {
            branch,
            old_ref_oid,
        } => {
            let store = MetadataStore::new(git);
            let branch_name = BranchName::new(branch).context("Invalid branch name")?;
            let old = Oid::new(old_ref_oid).context("Invalid old metadata OID")?;

            store.delete_cas(&branch_name, &old).map_err(|e| {
                anyhow::anyhow!(
                    "Metadata CAS failed for {}: expected {}, repository may have changed",
                    branch,
                    old_ref_oid
                )
                .context(e)
            })?;

            journal.append_metadata_delete(paths, branch, old_ref_oid)?;
            Ok(ContinueStepResult::Continue)
        }

        PlanStep::RunGit {
            args,
            description,
            expected_effects,
        } => {
            // Record intent in journal before executing
            journal.append_git_process(paths, args.clone(), description)?;

            // Execute the git command
            let result = git.run_command(args)?;

            if !result.success {
                return Ok(ContinueStepResult::Abort {
                    error: format!(
                        "git {} failed (exit code {}): {}",
                        args.first().unwrap_or(&String::new()),
                        result.exit_code,
                        result.stderr.trim()
                    ),
                });
            }

            // Check for conflicts after git command
            let git_state = git.state();
            if git_state.is_in_progress() {
                // Conflict occurred - need to pause for user resolution
                let branch = expected_effects
                    .first()
                    .and_then(|r| r.strip_prefix("refs/heads/"))
                    .unwrap_or("unknown")
                    .to_string();
                return Ok(ContinueStepResult::Pause { branch, git_state });
            }

            // Verify expected effects
            for effect in expected_effects {
                if git.try_resolve_ref(effect)?.is_none() {
                    return Ok(ContinueStepResult::Abort {
                        error: format!(
                            "git command succeeded but expected ref '{}' was not created",
                            effect
                        ),
                    });
                }
            }

            Ok(ContinueStepResult::Continue)
        }

        PlanStep::Checkpoint { name } => {
            journal.append_checkpoint(paths, name)?;
            Ok(ContinueStepResult::Continue)
        }

        PlanStep::PotentialConflictPause { .. } => {
            // This is a marker, not an action
            Ok(ContinueStepResult::Continue)
        }

        PlanStep::CreateSnapshotBranch { .. } => {
            // This step type is complex and unlikely during continuation
            // For now, fail with a clear message
            Ok(ContinueStepResult::Abort {
                error: "CreateSnapshotBranch not supported during continuation".to_string(),
            })
        }

        PlanStep::Checkout { branch, reason } => {
            // Execute git checkout
            let args = vec!["checkout".to_string(), branch.clone()];

            // Record intent in journal before executing
            journal.append_git_process(paths, args.clone(), reason)?;

            // Execute the checkout
            let result = git.run_command(&args)?;

            if !result.success {
                return Ok(ContinueStepResult::Abort {
                    error: format!(
                        "git checkout '{}' failed (exit code {}): {}",
                        branch,
                        result.exit_code,
                        result.stderr.trim()
                    ),
                });
            }

            Ok(ContinueStepResult::Continue)
        }
    }
}

/// Validate worktree occupancy for remaining steps.
///
/// Per ARCHITECTURE.md §6.2, we must re-check occupancy after acquiring the lock
/// because it may have changed since the operation was paused.
fn validate_occupancy_for_steps(git: &Git, steps: &[PlanStep]) -> Result<()> {
    for step in steps {
        let refname = match step {
            PlanStep::UpdateRefCas { refname, .. } => Some(refname.as_str()),
            PlanStep::DeleteRefCas { refname, .. } => Some(refname.as_str()),
            _ => None,
        };

        if let Some(refname) = refname {
            if let Some(branch) = refname.strip_prefix("refs/heads/") {
                let branch_name = BranchName::new(branch)
                    .map_err(|e| anyhow::anyhow!("Invalid branch name '{}': {}", branch, e))?;

                if let Some(wt_path) = git
                    .branch_checked_out_elsewhere(&branch_name)
                    .map_err(|e| anyhow::anyhow!("Failed to check worktree occupancy: {}", e))?
                {
                    bail!(
                        "Branch '{}' is checked out in worktree at {}.\n\
                         Switch that worktree to a different branch first.",
                        branch,
                        wt_path.display()
                    );
                }
            }
        }
    }
    Ok(())
}

/// Pause for a nested conflict during continuation.
///
/// When we encounter a new conflict while executing remaining steps, we need to:
/// 1. Record the new conflict state in the journal
/// 2. Update op-state to paused
/// 3. Inform the user
#[allow(clippy::too_many_arguments)]
fn pause_for_nested_conflict(
    ctx: &Context,
    _git: &Git,
    paths: &LatticePaths,
    op_state: &OpState,
    journal: &mut Journal,
    branch: &str,
    git_state: &GitState,
    new_remaining: Vec<PlanStep>,
) -> Result<()> {
    let remaining_names: Vec<String> = new_remaining
        .iter()
        .filter_map(|s| {
            if let PlanStep::WriteMetadataCas { branch, .. } = s {
                Some(branch.clone())
            } else {
                None
            }
        })
        .collect();

    // Serialize new remaining steps
    let remaining_steps_json = if new_remaining.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&new_remaining).context("Failed to serialize remaining steps")?)
    };

    // Record new conflict in journal
    journal.append_conflict_paused(
        paths,
        branch,
        git_state.description(),
        remaining_names.clone(),
        remaining_steps_json,
    )?;
    journal.pause();
    journal.write(paths)?;

    // Update op-state
    let mut new_op_state = op_state.clone();
    new_op_state.pause_with_reason(AwaitingReason::RebaseConflict, paths)?;

    if !ctx.quiet {
        println!();
        println!(
            "Conflict on '{}'. Resolve it and run 'lattice continue' again.",
            branch
        );
        if !remaining_names.is_empty() {
            println!("Remaining branches: {}", remaining_names.join(", "));
        }
    }

    Ok(())
}

/// Complete the operation - update journal, clear op-state.
fn complete_operation(
    ctx: &Context,
    git: &Git,
    paths: &LatticePaths,
    op_state: &OpState,
) -> Result<()> {
    // Record completion event
    let ledger = EventLedger::new(git);
    let _ = ledger.append(Event::committed(
        op_state.op_id.as_str(),
        "continuation-complete",
    ));

    // Update journal to committed
    let mut journal = Journal::read(paths, &op_state.op_id).unwrap_or_else(|_| {
        // If journal can't be read, create a minimal one for cleanup
        Journal::new(&op_state.command)
    });
    journal.commit();
    let _ = journal.write(paths); // Best effort

    // Clear op-state
    OpState::remove(paths)?;

    if !ctx.quiet {
        println!("Operation '{}' completed.", op_state.command);
    }

    Ok(())
}
