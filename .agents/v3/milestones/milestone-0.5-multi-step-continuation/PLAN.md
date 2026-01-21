# Milestone 0.5: Multi-step Journal Continuation

## Status: COMPLETE

---

## Overview

**Goal:** Enable `lattice continue` to resume multi-step operations from where they paused, executing remaining steps rather than assuming the operation is complete.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs, Tests are everything.

**Priority:** HIGH - Continue doesn't resume remaining steps

**Spec References:**
- SPEC.md Section 4.2.1 "Journal structure"
- SPEC.md Section 4.2.2 "Crash consistency contract"
- ARCHITECTURE.md Section 6.2 "The Executor contract"
- ARCHITECTURE.md Section 12 "Command lifecycle"

---

## Problem Statement

ROADMAP.md identifies this critical gap:

> The `continue_op()` function admits:
> ```rust
> // ❌ CRITICAL GAP: Multi-step journal not resumed
> // For now, we assume the operation is complete
> ```

When an operation pauses (e.g., restack conflict on branch 2 of 5), `lattice continue` currently:
1. Completes the immediate git operation (finish the rebase)
2. Clears the op-state
3. **Does NOT execute remaining steps** (branches 3, 4, 5 never get restacked)

This violates the crash consistency contract: users expect that after resolving a conflict and running `continue`, the entire operation completes.

### Current State Evidence

From `src/cli/commands/recovery.rs`:

```rust
pub fn continue_op(ctx: &Context, all: bool) -> Result<()> {
    // ... validation and git continue ...

    // Git operation completed - check if there are more steps
    // For now, we assume the operation is complete
    // A full implementation would read the journal and continue remaining steps

    // Clear op-state
    OpState::remove(&paths)?;

    if !ctx.quiet {
        println!("Operation '{}' completed.", op_state.command);
    }

    Ok(())
}
```

### Impact

1. **Partial restacks:** User restacks 5 branches, conflict on branch 2, resolves and continues - only 2 branches get restacked
2. **Silent data loss:** User thinks operation completed but 3 branches were never updated
3. **Broken invariant:** Stack is left in inconsistent state (some branches restacked, others not)

---

## Design Analysis

### What "Remaining Steps" Means

The journal records steps as they are **executed**, not as they are **planned**. When we pause:

1. **Executed steps:** Recorded in journal via `append_*` methods
2. **Planned but unexecuted steps:** Stored in `ExecuteResult::Paused { remaining_steps }`

The challenge: `remaining_steps` is an in-memory value at pause time, but after `continue`, we've lost that context.

### Solution: Store Remaining Steps in Journal

Per SPEC.md §4.2.1, the journal's `ConflictPaused` step already stores `remaining_branches`. However, this is inadequate because:

1. It only stores branch names, not full `PlanStep` objects
2. It can't represent arbitrary plan steps (RunGit, UpdateRefCas, etc.)

**Design Decision D1:** Store serialized remaining steps in the journal when pausing.

### Reconciliation Challenge

Per ROADMAP.md:

> **Q2: How to reconcile "plan from the past" with "repo reality now"?**
> 
> **Answer:** Re-scan before continuing, but don't re-plan. Validate:
> - Preconditions for remaining steps still hold
> - CAS expected values match current reality
> 
> If not, abort with "repository changed; cannot continue safely."

This is critical: between pause and continue, the user might:
- Run other git commands
- Push/pull
- Modify refs manually

We must detect and refuse if CAS preconditions no longer match.

---

## Design Decisions

### D1: Where to store remaining steps?

**Options:**

a) **Embed in ConflictPaused step:** Extend `StepKind::ConflictPaused` to include `Vec<PlanStep>`
b) **New StepKind:** Add `StepKind::RemainingPlan { steps: Vec<PlanStep> }`
c) **Separate file:** Store remaining plan in `<common_dir>/lattice/ops/<op_id>-remaining.json`
d) **Embed in OpState:** Add `remaining_steps: Vec<PlanStep>` to OpState

**Decision:** Option (a) - Extend `StepKind::ConflictPaused` to include serialized remaining steps.

**Rationale:**
- Keeps all recovery data in one place (the journal)
- Journal is already fsynced at step boundaries
- No new files to manage
- ConflictPaused is already the "pause marker" - natural extension

### D2: How to validate CAS preconditions on continue?

**Options:**

a) **Re-validate touched refs:** Check each ref in remaining steps against current reality
b) **Use plan digest:** Compare stored digest with re-computed digest (won't work - plan changed)
c) **Per-step validation:** Validate each step's CAS condition before executing

**Decision:** Option (c) - Per-step CAS validation during execution.

**Rationale:**
- Executor already does CAS validation per step
- No need for separate pre-validation pass
- Simpler implementation
- If a step fails CAS, executor will abort properly

### D3: What if remaining steps reference outdated OIDs?

This happens if user modified refs between pause and continue.

**Decision:** Detect via CAS failure, abort with clear message.

**Rationale:**
- Per ARCHITECTURE.md §6.2, CAS failures abort execution and record `Aborted` event
- The message should explain: "Repository changed since operation paused. Run `lattice abort` then retry."
- Do NOT attempt automatic repair or re-planning

### D4: How to handle nested conflicts?

User continues → new conflict on next branch → need to pause again.

**Decision:** Transition back to `awaiting_user` with updated journal.

**Rationale:**
- This is exactly what the executor does for initial conflicts
- Journal gets new `ConflictPaused` step with updated remaining steps
- OpState phase returns to `Paused`
- User runs `continue` again

### D5: Should continue re-acquire lock?

**Decision:** Yes, continue must acquire the repo lock before executing remaining steps.

**Rationale:**
- Per ARCHITECTURE.md §6.2, all mutations require the lock
- Between pause and continue, another worktree might have modified the repo
- Lock ensures single-writer semantics

---

## Implementation Phases

### Phase 1: Extend Journal Schema

#### Step 1.1: Update ConflictPaused to Store Remaining Steps

**File:** `src/core/ops/journal.rs`

Extend `StepKind::ConflictPaused`:

```rust
/// Conflict detected during operation.
///
/// Records the state when the operation was paused for user intervention.
/// Includes remaining steps so `continue` can resume execution.
ConflictPaused {
    /// The branch where the conflict occurred.
    branch: String,
    /// Type of git operation that conflicted (rebase, merge, etc.).
    git_state: String,
    /// Branches remaining to process after conflict resolution (for display).
    remaining_branches: Vec<String>,
    /// Serialized remaining plan steps for continuation.
    /// These will be executed after conflict resolution.
    remaining_steps: Vec<PlanStep>,
},
```

Note: This requires `PlanStep` to be importable in `journal.rs`. Check for circular dependency.

#### Step 1.2: Handle Circular Dependency (if needed)

If `journal.rs` can't import `PlanStep` due to module hierarchy:

**Option A:** Move `PlanStep` to a shared types module
**Option B:** Store steps as serialized JSON string: `remaining_steps_json: String`

Prefer Option A if feasible, as it's cleaner.

#### Step 1.3: Update append_conflict_paused

```rust
pub fn append_conflict_paused(
    &mut self,
    paths: &LatticePaths,
    branch: impl Into<String>,
    git_state: impl Into<String>,
    remaining_branches: Vec<String>,
    remaining_steps: Vec<PlanStep>,  // NEW
) -> Result<(), JournalError> {
    self.steps.push(JournalStep {
        kind: StepKind::ConflictPaused {
            branch: branch.into(),
            git_state: git_state.into(),
            remaining_branches,
            remaining_steps,
        },
        timestamp: UtcTimestamp::now(),
    });
    self.write(paths)
}
```

---

### Phase 2: Update Executor Pause Logic

#### Step 2.1: Update Executor to Store Remaining Steps

**File:** `src/engine/exec.rs`

When pausing, the executor already has `remaining: Vec<PlanStep>`. Update to pass these to the journal:

```rust
StepResult::Pause { branch, git_state } => {
    // Record conflict in journal
    let remaining: Vec<PlanStep> = step_iter.map(|(_, s)| s.clone()).collect();
    let remaining_names: Vec<String> = remaining
        .iter()
        .filter_map(|s| {
            if let PlanStep::WriteMetadataCas { branch, .. } = s {
                Some(branch.clone())
            } else {
                None
            }
        })
        .collect();

    // Use append_* method with remaining steps
    journal.append_conflict_paused(
        &paths,
        &branch,
        git_state.description(),
        remaining_names,
        remaining.clone(),  // NEW: pass actual steps
    )?;
    journal.pause();
    journal.write(&paths)?;

    // ... rest unchanged ...
}
```

---

### Phase 3: Implement Continue Resumption

#### Step 3.1: Add Journal Method to Get Remaining Steps

**File:** `src/core/ops/journal.rs`

```rust
impl Journal {
    /// Get remaining steps from the last ConflictPaused step.
    ///
    /// Returns `None` if:
    /// - Journal has no steps
    /// - Last step is not ConflictPaused
    /// - ConflictPaused has no remaining steps
    ///
    /// This is used by `continue` to resume multi-step operations.
    pub fn remaining_steps(&self) -> Option<&[PlanStep]> {
        self.steps.last().and_then(|step| {
            if let StepKind::ConflictPaused { remaining_steps, .. } = &step.kind {
                if remaining_steps.is_empty() {
                    None
                } else {
                    Some(remaining_steps.as_slice())
                }
            } else {
                None
            }
        })
    }

    /// Check if the journal has remaining steps to execute.
    pub fn has_remaining_steps(&self) -> bool {
        self.remaining_steps().map_or(false, |s| !s.is_empty())
    }
}
```

#### Step 3.2: Update continue_op to Execute Remaining Steps

**File:** `src/cli/commands/recovery.rs`

This is the main implementation change. After completing the git operation:

```rust
pub fn continue_op(ctx: &Context, all: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Pre-flight gating check
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

    // Verify plan schema version compatibility
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

    // Git operation completed - check for remaining steps
    let journal = Journal::read(&paths, &op_state.op_id)
        .context("Failed to read operation journal")?;

    if journal.has_remaining_steps() {
        // Execute remaining steps
        execute_remaining_steps(ctx, &git, &paths, &op_state, &journal)?;
    } else {
        // No remaining steps - operation complete
        complete_operation(ctx, &paths, &op_state)?;
    }

    Ok(())
}

/// Execute remaining steps from a paused operation.
fn execute_remaining_steps(
    ctx: &Context,
    git: &Git,
    paths: &LatticePaths,
    op_state: &OpState,
    journal: &Journal,
) -> Result<()> {
    let remaining_steps = journal.remaining_steps()
        .ok_or_else(|| anyhow::anyhow!("No remaining steps found"))?;

    if !ctx.quiet {
        println!("Resuming {} remaining steps...", remaining_steps.len());
    }

    // Acquire lock before continuing execution
    let _lock = RepoLock::acquire(paths)
        .context("Failed to acquire repository lock")?;

    // Re-validate worktree occupancy (per ARCHITECTURE.md §6.2)
    // Occupancy may have changed since we paused
    validate_occupancy_for_steps(git, remaining_steps)?;

    // Create a continuation plan with remaining steps
    let continuation_plan = Plan::new(op_state.op_id.clone(), &op_state.command)
        .with_steps(remaining_steps.iter().cloned());

    // Re-load journal for appending (we'll add new steps as we execute)
    let mut journal = Journal::read(paths, &op_state.op_id)?;

    // Execute remaining steps via the executor's step execution logic
    let executor = Executor::new(git);
    
    for (i, step) in remaining_steps.iter().enumerate() {
        if ctx.debug {
            eprintln!("[debug] Executing remaining step {}: {:?}", i + 1, step.description());
        }

        match executor.execute_step(step, &mut journal, paths)? {
            StepResult::Continue => {
                // Step executed successfully
            }
            StepResult::Pause { branch, git_state } => {
                // Nested conflict - need to pause again
                let new_remaining: Vec<PlanStep> = remaining_steps[i+1..].to_vec();
                let new_remaining_names: Vec<String> = new_remaining
                    .iter()
                    .filter_map(|s| {
                        if let PlanStep::WriteMetadataCas { branch, .. } = s {
                            Some(branch.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                journal.append_conflict_paused(
                    paths,
                    &branch,
                    git_state.description(),
                    new_remaining_names,
                    new_remaining,
                )?;
                journal.pause();
                journal.write(paths)?;

                // Update op-state
                let mut new_op_state = op_state.clone();
                new_op_state.pause_with_reason(AwaitingReason::RebaseConflict, paths)?;

                if !ctx.quiet {
                    println!();
                    println!("Conflict on '{}'. Resolve it and run 'lattice continue' again.", branch);
                }

                return Ok(());
            }
            StepResult::Abort { error } => {
                // Step failed - need to abort
                bail!("Step failed: {}", error);
            }
        }
    }

    // All remaining steps completed successfully
    complete_operation(ctx, paths, op_state)?;

    Ok(())
}

/// Validate worktree occupancy for a set of steps.
fn validate_occupancy_for_steps(git: &Git, steps: &[PlanStep]) -> Result<()> {
    for step in steps {
        if let PlanStep::UpdateRefCas { refname, .. } | PlanStep::DeleteRefCas { refname, .. } = step {
            if let Some(branch) = refname.strip_prefix("refs/heads/") {
                let branch_name = BranchName::new(branch)
                    .map_err(|e| anyhow::anyhow!("Invalid branch name: {}", e))?;
                if let Some(wt_path) = git.branch_checked_out_elsewhere(&branch_name)
                    .map_err(|e| anyhow::anyhow!("Failed to check worktree occupancy: {}", e))? 
                {
                    bail!(
                        "Branch '{}' is checked out in worktree at {}. \
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

/// Complete the operation - update journal, clear op-state.
fn complete_operation(ctx: &Context, paths: &LatticePaths, op_state: &OpState) -> Result<()> {
    // Record completion event
    let git = Git::open(&std::env::current_dir()?)?;
    let ledger = EventLedger::new(&git);
    // Compute fingerprint for committed event
    // (simplified - actual implementation may need full fingerprint computation)
    let _ = ledger.append(Event::committed(op_state.op_id.as_str(), "continuation-complete"));

    // Update journal to committed
    let mut journal = Journal::read(paths, &op_state.op_id)?;
    journal.commit();
    journal.write(paths)?;

    // Clear op-state
    OpState::remove(paths)?;

    if !ctx.quiet {
        println!("Operation '{}' completed.", op_state.command);
    }

    Ok(())
}
```

#### Step 3.3: Expose execute_step from Executor

The above code calls `executor.execute_step()` which is currently private. We need to make it accessible:

**File:** `src/engine/exec.rs`

```rust
impl<'a> Executor<'a> {
    /// Execute a single step.
    ///
    /// This is used by `continue` to resume execution of remaining steps.
    /// 
    /// # Note
    /// This method should not be called directly for new operations.
    /// Use `execute()` instead, which handles the full lifecycle.
    pub fn execute_step_for_continuation(
        &self,
        step: &PlanStep,
        journal: &mut Journal,
        paths: &LatticePaths,
    ) -> Result<StepResult, ExecuteError> {
        self.execute_step(step, journal, paths)
    }
}
```

Alternatively, refactor to share the step execution logic via a helper.

---

### Phase 4: Update Tests

#### Step 4.1: Unit Tests for Journal Changes

**File:** `src/core/ops/journal.rs` (test module)

```rust
#[test]
fn remaining_steps_empty_when_no_conflict() {
    let journal = Journal::new("test");
    assert!(journal.remaining_steps().is_none());
}

#[test]
fn remaining_steps_extracted_from_conflict_paused() {
    let temp = create_test_dir();
    let paths = create_test_paths(&temp);
    
    let mut journal = Journal::new("restack");
    
    let remaining = vec![
        PlanStep::UpdateRefCas {
            refname: "refs/heads/branch-c".to_string(),
            old_oid: Some("abc".to_string()),
            new_oid: "def".to_string(),
            reason: "rebase".to_string(),
        },
        PlanStep::Checkpoint { name: "done".to_string() },
    ];
    
    journal.append_conflict_paused(
        &paths,
        "branch-b",
        "rebase",
        vec!["branch-c".to_string()],
        remaining.clone(),
    ).unwrap();
    
    let extracted = journal.remaining_steps().unwrap();
    assert_eq!(extracted.len(), 2);
    // Verify steps match
}

#[test]
fn has_remaining_steps_true_when_present() {
    // Similar to above, verify has_remaining_steps() returns true
}

#[test]
fn has_remaining_steps_false_when_empty_remaining() {
    let temp = create_test_dir();
    let paths = create_test_paths(&temp);
    
    let mut journal = Journal::new("restack");
    journal.append_conflict_paused(
        &paths,
        "branch-b",
        "rebase",
        vec![],
        vec![],  // Empty remaining steps
    ).unwrap();
    
    assert!(!journal.has_remaining_steps());
}
```

#### Step 4.2: Integration Tests for Continue Resumption

**File:** `tests/continue_resumption.rs` (NEW)

```rust
//! Integration tests for multi-step operation continuation.
//!
//! Per SPEC.md §4.2.2, `continue` must resume remaining steps.

use std::process::Command;
use tempfile::TempDir;

/// Helper to create a test repo with a stack.
fn setup_stack_repo() -> (TempDir, String) {
    // Create repo with main + feature-a + feature-b + feature-c
    // Each branch has one commit
    unimplemented!("Setup test repo")
}

#[test]
fn continue_executes_remaining_steps_after_conflict() {
    // 1. Setup: Create stack with 3 branches on main
    // 2. Modify main to create need for restack
    // 3. Start restack (should conflict on branch 2)
    // 4. Resolve conflict
    // 5. Run `lattice continue`
    // 6. Assert: All 3 branches were restacked
}

#[test]
fn continue_handles_nested_conflict() {
    // 1. Setup: Create stack with 3 branches
    // 2. Create conditions for conflicts on branch 2 AND 3
    // 3. Start restack (conflicts on branch 2)
    // 4. Resolve, continue (conflicts on branch 3)
    // 5. Resolve, continue again
    // 6. Assert: All branches restacked
}

#[test]
fn continue_aborts_on_cas_mismatch() {
    // 1. Setup: Create stack, start restack, pause on conflict
    // 2. Manually change a ref that remaining steps expect
    // 3. Resolve conflict, run continue
    // 4. Assert: Fails with "repository changed" error
}

#[test]
fn continue_from_wrong_worktree_fails() {
    // 1. Setup: Create repo with worktree
    // 2. Start operation from main repo, pause
    // 3. Attempt continue from worktree
    // 4. Assert: Fails with guidance to use original worktree
}
```

#### Step 4.3: Fault Injection Tests

**File:** `tests/journal_crash_recovery.rs` (extend existing)

```rust
#[test]
fn crash_during_continuation_recovers() {
    // 1. Setup: Create stack, start restack, pause
    // 2. Set fault injection to crash during continuation
    // 3. Resolve conflict, run continue (crashes mid-way)
    // 4. Re-run continue
    // 5. Assert: Remaining steps from crash point are executed
}
```

---

### Phase 5: Module Reorganization (if needed)

If circular dependency issues arise from importing `PlanStep` in `journal.rs`:

#### Option A: Create Shared Types Module

**File:** `src/engine/types.rs` (NEW)

Move `PlanStep` definition here, import from both `plan.rs` and `journal.rs`.

#### Option B: Store as Serialized JSON

If Option A is too invasive:

```rust
ConflictPaused {
    branch: String,
    git_state: String,
    remaining_branches: Vec<String>,
    /// JSON-serialized remaining steps (Vec<PlanStep>)
    remaining_steps_json: String,
}

impl Journal {
    pub fn remaining_steps(&self) -> Option<Vec<PlanStep>> {
        self.steps.last().and_then(|step| {
            if let StepKind::ConflictPaused { remaining_steps_json, .. } = &step.kind {
                serde_json::from_str(remaining_steps_json).ok()
            } else {
                None
            }
        })
    }
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/core/ops/journal.rs` | MODIFY | Extend ConflictPaused, add remaining_steps() |
| `src/engine/exec.rs` | MODIFY | Store remaining steps on pause, expose execute_step |
| `src/cli/commands/recovery.rs` | MODIFY | Execute remaining steps on continue |
| `tests/continue_resumption.rs` | NEW | Integration tests for continuation |
| `tests/journal_crash_recovery.rs` | EXTEND | Fault injection tests for continuation |

---

## Acceptance Gates

Per ROADMAP.md:

- [x] `continue` resumes from last checkpoint
- [x] Remaining branches processed after conflict resolution
- [x] Nested conflicts pause again correctly (transition back to `awaiting_user`)
- [x] Op-state cleared only after all steps complete
- [x] Precondition validation on continue (CAS checks)
- [x] Worktree occupancy re-validated before continuation
- [x] `cargo test` passes (833 tests)
- [x] `cargo clippy` passes
- [x] `cargo fmt --check` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `remaining_steps_empty_when_no_conflict` | journal.rs | No remaining steps for non-paused journal |
| `remaining_steps_extracted_from_conflict_paused` | journal.rs | Steps extracted correctly |
| `has_remaining_steps_true/false` | journal.rs | Helper method works |
| `append_conflict_paused_with_steps` | journal.rs | Steps serialized correctly |

### Integration Tests

| Test | File | Description |
|------|------|-------------|
| `continue_executes_remaining_steps` | continue_resumption.rs | Main happy path |
| `continue_handles_nested_conflict` | continue_resumption.rs | Multiple pauses |
| `continue_aborts_on_cas_mismatch` | continue_resumption.rs | CAS validation |
| `continue_from_wrong_worktree_fails` | continue_resumption.rs | Worktree origin check |

### Fault Injection Tests

| Test | File | Description |
|------|------|-------------|
| `crash_during_continuation_recovers` | journal_crash_recovery.rs | Crash mid-continuation |

---

## Error Messages

### CAS Mismatch on Continue

```
Error: Repository changed since operation paused.

The ref 'refs/heads/feature-c' was expected to be 'abc123' but is now 'def456'.
Someone or something modified the repository while the operation was paused.

Options:
1. Run 'lattice abort' to cancel the operation
2. Manually restore the expected state and try again
```

### Wrong Worktree on Continue

```
Error: This operation was started in a different worktree.

The operation was started from: /path/to/original/worktree
You are running from: /path/to/different/worktree

Please run 'lattice continue' from the original worktree.
```

### Nested Conflict

```
Conflict on 'feature-c'. Resolve it and run 'lattice continue' again.

Remaining branches: feature-d, feature-e
```

---

## Performance Considerations

**Concern:** Re-acquiring lock and re-validating occupancy adds latency to continue.

**Analysis:** This is necessary for correctness. The latency is minimal (milliseconds) compared to the git operations. No optimization needed.

---

## Dependencies

**Depends on:**
- Milestone 0.3 (Journal Rollback) - COMPLETE - Uses rollback on failure
- Milestone 0.9 (Journal Fsync) - COMPLETE - Uses append_* methods

**Blocked by this:**
- No direct dependencies identified

---

## Complexity Assessment

**Estimated complexity:** HIGH

This milestone requires:
1. Schema changes to journal (ConflictPaused with remaining_steps)
2. Significant changes to recovery.rs (execute_remaining_steps)
3. Potential module reorganization for PlanStep imports
4. Comprehensive integration tests with conflict scenarios

The changes are correctness-critical and require careful testing.

---

## Verification Commands

After implementation, run:

```bash
# Type checks
cargo check

# Lint
cargo clippy -- -D warnings

# All tests
cargo test

# Specific tests
cargo test journal
cargo test continue
cargo test resumption

# Format check
cargo fmt --check
```

---

## Open Questions

### Q1: What if the user modifies the working tree between resolve and continue?

**Current thinking:** Not our problem. If they have uncommitted changes, git operations may fail. We don't need to validate working tree state beyond what git requires.

### Q2: Should we re-scan metadata before continuing?

**Current thinking:** No. The CAS preconditions in remaining steps are the source of truth. If metadata changed, CAS will fail. No need for separate metadata validation.

### Q3: What about plan digest verification?

**Current thinking:** Plan digest verification was part of Milestone 0.4's design for continue. However, since we're storing and executing the actual remaining steps (not re-planning), digest verification is less relevant. The remaining steps ARE the plan - there's nothing to compare against.

---

## Next Steps (After Completion)

Per ROADMAP.md execution order:
1. ~~Milestone 0.4: OpState Full Payload~~ - COMPLETE
2. ~~Milestone 0.1: Gating Integration + Scope Walking~~ - COMPLETE  
3. ~~Milestone 0.2 + 0.6: Occupancy + Post-Verify~~ - COMPLETE
4. ~~Milestone 0.3: Journal Rollback~~ - COMPLETE
5. ~~Milestone 0.9: Journal Fsync Step Boundary~~ - COMPLETE
6. **Milestone 0.5: Multi-step Journal Continuation** (this)
7. Milestone 0.8: Bare Repo Compliance
8. Milestone 0.7: TokenProvider Integration
