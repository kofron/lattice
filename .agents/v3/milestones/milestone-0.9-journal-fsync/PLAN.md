# Milestone 0.9: Journal Fsync Step Boundary

## Status: COMPLETE

---

## Overview

**Goal:** Enforce fsync at each journal step boundary to satisfy the crash consistency contract from SPEC.md.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs, Tests are everything.

**Priority:** MEDIUM - SPEC requirement for crash contract

**Spec Reference:** SPEC.md Section 4.2.2 "Crash consistency contract"

---

## Problem Statement

SPEC.md Section 4.2.2 requires:

> "Journals must be written with fsync at each appended step boundary."

The current journal API has a structural issue: it separates step recording from persistence:

```rust
// Current pattern (problematic):
journal.record_ref_update(refname, old_oid, new_oid);  // Just adds to in-memory list
// ... caller must remember to call write() ...
if step.is_mutation() {
    journal.write(&paths)?;  // Caller responsible for persisting
}
```

This design has two problems:

1. **Easy to forget:** A caller can record a step without persisting it, violating the crash contract
2. **Batching risk:** Multiple steps can accumulate before a write, meaning a crash loses multiple steps
3. **Unclear API:** The contract between "record" and "persist" is implicit

**Impact:** If the process crashes after recording a step but before calling `write()`, the step is lost. This breaks the "recoverable from any crash point" guarantee.

---

## Current State Analysis

### Journal Structure (src/core/ops/journal.rs)

**Existing API:**

```rust
impl Journal {
    /// Add a step to the journal (in-memory only).
    pub fn add_step(&mut self, kind: StepKind) {
        self.steps.push(JournalStep {
            kind,
            timestamp: UtcTimestamp::now(),
        });
    }

    /// Convenience methods that call add_step()
    pub fn record_ref_update(&mut self, ...) { self.add_step(...) }
    pub fn record_metadata_write(&mut self, ...) { self.add_step(...) }
    pub fn record_metadata_delete(&mut self, ...) { self.add_step(...) }
    pub fn record_checkpoint(&mut self, ...) { self.add_step(...) }
    pub fn record_git_process(&mut self, ...) { self.add_step(...) }
    pub fn record_conflict_paused(&mut self, ...) { self.add_step(...) }

    /// Write to disk with fsync (separate call).
    pub fn write(&self, paths: &LatticePaths) -> Result<(), JournalError> {
        // ... writes and calls file.sync_all() ...
    }
}
```

### Current Executor Usage (src/engine/exec.rs)

The executor currently does call `write()` after each mutation:

```rust
match self.execute_step(step, &mut journal)? {
    StepResult::Continue => {
        applied_steps.push(step.clone());
        // Write journal after each mutation
        if step.is_mutation() {
            journal.write(&paths)?;
        }
    }
    // ...
}
```

And within `execute_step()`:

```rust
PlanStep::UpdateRefCas { ... } => {
    // ... perform git update ...
    journal.record_ref_update(refname, old_oid.clone(), new_oid);
    Ok(StepResult::Continue)
}
```

The problem: `record_ref_update()` happens inside `execute_step()`, but `write()` happens outside in the caller. If we crashed between these, the step would be lost.

### What the SPEC Intends

Per SPEC.md Section 4.2.2:

> "Journals must be written with fsync at each appended step boundary."

This means: **immediately after adding any step, the journal must be persisted to disk with fsync.** The recovery guarantee is that we can always reconstruct what was done.

---

## Design Decisions

### D1: How to enforce "write on every step"?

**Options:**

a) **Stateful Journal with LatticePaths:** Make Journal own paths and auto-write on add
b) **Rename `record_*` to take paths:** Each `record_*` method takes `paths` and writes immediately
c) **New `append_step()` method:** Add a new method that both adds and writes, deprecate the old pattern
d) **Builder pattern:** `journal.step(...).persist(&paths)?`

**Decision:** Option (b) - Rename `record_*` methods to `append_*` and have them take `paths` parameter and write immediately. This:
- Makes fsync mandatory (can't forget)
- Is explicit about the I/O happening
- Matches ROADMAP.md's suggested API
- Doesn't require restructuring Journal ownership

### D2: What about batch operations for performance?

**Consideration:** Writing after every single step could be slow for operations with many steps.

**Decision:** Accept the performance cost for correctness. Per the "Simplicity" principle, correctness trumps performance. The SPEC explicitly requires per-step fsync. If this becomes a bottleneck later, we can:
1. Measure actual impact
2. Propose a SPEC amendment for "checkpoint-based" batching
3. Implement with explicit opt-in

For v3, strict compliance with SPEC.

### D3: Should `add_step()` still exist?

**Decision:** Keep `add_step()` but make it `pub(crate)` or private. It's useful for internal construction (e.g., tests, journal loading) but should not be called directly by executor code.

### D4: How to handle the transition?

**Decision:** 
1. Rename existing `record_*` → `append_*` with new signature
2. Update all callers to pass `paths`
3. Remove the separate `write()` calls after mutations in executor
4. Keep `write()` for phase transitions (commit, pause, rollback) which update the phase field

---

## Implementation Steps

### Phase 1: Update Journal API

#### Step 1.1: Add New `append_*` Methods

**File:** `src/core/ops/journal.rs`

Add new methods that take `LatticePaths` and persist immediately:

```rust
/// Append a ref update step and persist to disk.
///
/// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
pub fn append_ref_update(
    &mut self,
    paths: &LatticePaths,
    refname: impl Into<String>,
    old_oid: Option<String>,
    new_oid: impl Into<String>,
) -> Result<(), JournalError> {
    self.steps.push(JournalStep {
        kind: StepKind::RefUpdate {
            refname: refname.into(),
            old_oid,
            new_oid: new_oid.into(),
        },
        timestamp: UtcTimestamp::now(),
    });
    self.write(paths)
}

/// Append a metadata write step and persist to disk.
///
/// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
pub fn append_metadata_write(
    &mut self,
    paths: &LatticePaths,
    branch: impl Into<String>,
    old_ref_oid: Option<String>,
    new_ref_oid: impl Into<String>,
) -> Result<(), JournalError> {
    self.steps.push(JournalStep {
        kind: StepKind::MetadataWrite {
            branch: branch.into(),
            old_ref_oid,
            new_ref_oid: new_ref_oid.into(),
        },
        timestamp: UtcTimestamp::now(),
    });
    self.write(paths)
}

/// Append a metadata delete step and persist to disk.
///
/// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
pub fn append_metadata_delete(
    &mut self,
    paths: &LatticePaths,
    branch: impl Into<String>,
    old_ref_oid: impl Into<String>,
) -> Result<(), JournalError> {
    self.steps.push(JournalStep {
        kind: StepKind::MetadataDelete {
            branch: branch.into(),
            old_ref_oid: old_ref_oid.into(),
        },
        timestamp: UtcTimestamp::now(),
    });
    self.write(paths)
}

/// Append a checkpoint step and persist to disk.
///
/// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
pub fn append_checkpoint(
    &mut self,
    paths: &LatticePaths,
    name: impl Into<String>,
) -> Result<(), JournalError> {
    self.steps.push(JournalStep {
        kind: StepKind::Checkpoint { name: name.into() },
        timestamp: UtcTimestamp::now(),
    });
    self.write(paths)
}

/// Append a git process step and persist to disk.
///
/// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
pub fn append_git_process(
    &mut self,
    paths: &LatticePaths,
    args: Vec<String>,
    description: impl Into<String>,
) -> Result<(), JournalError> {
    self.steps.push(JournalStep {
        kind: StepKind::GitProcess {
            args,
            description: description.into(),
        },
        timestamp: UtcTimestamp::now(),
    });
    self.write(paths)
}

/// Append a conflict paused step and persist to disk.
///
/// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
pub fn append_conflict_paused(
    &mut self,
    paths: &LatticePaths,
    branch: impl Into<String>,
    git_state: impl Into<String>,
    remaining_branches: Vec<String>,
) -> Result<(), JournalError> {
    self.steps.push(JournalStep {
        kind: StepKind::ConflictPaused {
            branch: branch.into(),
            git_state: git_state.into(),
            remaining_branches,
        },
        timestamp: UtcTimestamp::now(),
    });
    self.write(paths)
}
```

#### Step 1.2: Deprecate Old `record_*` Methods

Mark the old methods as deprecated with clear migration guidance:

```rust
/// Record a ref update step (in-memory only).
///
/// # Deprecated
/// Use `append_ref_update()` instead, which persists immediately per SPEC.md §4.2.2.
#[deprecated(since = "0.9.0", note = "Use append_ref_update() which persists immediately")]
pub fn record_ref_update(
    &mut self,
    refname: impl Into<String>,
    old_oid: Option<String>,
    new_oid: impl Into<String>,
) {
    self.add_step(StepKind::RefUpdate {
        refname: refname.into(),
        old_oid,
        new_oid: new_oid.into(),
    });
}

// ... similar for other record_* methods ...
```

#### Step 1.3: Make `add_step()` Non-Public

Change visibility to prevent external misuse:

```rust
/// Add a step to the journal (in-memory only).
///
/// This is internal - external code should use `append_*` methods
/// which persist immediately per SPEC.md §4.2.2.
pub(crate) fn add_step(&mut self, kind: StepKind) {
    self.steps.push(JournalStep {
        kind,
        timestamp: UtcTimestamp::now(),
    });
}
```

---

### Phase 2: Update Executor

#### Step 2.1: Update `execute_step()` to Use New API

**File:** `src/engine/exec.rs`

The executor's `execute_step()` method needs to:
1. Take `paths` as a parameter
2. Use `append_*` methods instead of `record_*`
3. Remove the separate `write()` calls after mutations

**Before:**

```rust
fn execute_step(
    &self,
    step: &PlanStep,
    journal: &mut Journal,
) -> Result<StepResult, ExecuteError> {
    match step {
        PlanStep::UpdateRefCas { ... } => {
            // ... perform git update ...
            journal.record_ref_update(refname, old_oid.clone(), new_oid);
            Ok(StepResult::Continue)
        }
        // ...
    }
}
```

**After:**

```rust
fn execute_step(
    &self,
    step: &PlanStep,
    journal: &mut Journal,
    paths: &LatticePaths,
) -> Result<StepResult, ExecuteError> {
    match step {
        PlanStep::UpdateRefCas { ... } => {
            // ... perform git update ...
            journal.append_ref_update(paths, refname, old_oid.clone(), new_oid)?;
            Ok(StepResult::Continue)
        }
        // ...
    }
}
```

#### Step 2.2: Update Main Execution Loop

**File:** `src/engine/exec.rs`

Remove the manual `write()` call after mutations:

**Before:**

```rust
while let Some((i, step)) = step_iter.next() {
    match self.execute_step(step, &mut journal)? {
        StepResult::Continue => {
            applied_steps.push(step.clone());
            // Write journal after each mutation
            if step.is_mutation() {
                journal.write(&paths)?;
            }
        }
        // ...
    }
}
```

**After:**

```rust
while let Some((i, step)) = step_iter.next() {
    match self.execute_step(step, &mut journal, &paths)? {
        StepResult::Continue => {
            applied_steps.push(step.clone());
            // Journal already persisted by append_* methods
        }
        // ...
    }
}
```

#### Step 2.3: Update Conflict Handling

The conflict handling code also needs to use the new API:

**Before:**

```rust
journal.record_conflict_paused(
    &branch,
    git_state.description(),
    remaining_names,
);
journal.pause();
journal.write(&paths)?;
```

**After:**

```rust
journal.append_conflict_paused(
    &paths,
    &branch,
    git_state.description(),
    remaining_names,
)?;
journal.pause();
journal.write(&paths)?;  // Still need write() for phase change
```

Note: We still call `write()` after `pause()` because the phase change needs to be persisted. The `append_*` persisted the step, `write()` persists the updated phase.

---

### Phase 3: Update Other Callers

#### Step 3.1: Audit All Journal Callers

Search for all uses of `record_*` methods and update them:

```bash
grep -r "journal\.record_" src/
grep -r "journal\.add_step" src/
```

Expected callers:
- `src/engine/exec.rs` - Main executor (updated in Phase 2)
- Tests in `src/core/ops/journal.rs` - May need updates or can use internal API

#### Step 3.2: Update Any Other Callers

If there are other callers outside the executor, update them to:
1. Have access to `LatticePaths`
2. Use `append_*` methods
3. Handle the `Result` from the append

---

### Phase 4: Fault Injection Testing

#### Step 4.1: Add Fault Injection Infrastructure

**File:** `src/core/ops/journal.rs` (or new test module)

Add a way to simulate crashes for testing:

```rust
#[cfg(any(test, feature = "fault_injection"))]
pub mod fault_injection {
    use std::sync::atomic::{AtomicUsize, Ordering};
    
    /// Counter for fault injection - crash after N writes.
    static CRASH_AFTER_WRITES: AtomicUsize = AtomicUsize::new(0);
    static WRITE_COUNT: AtomicUsize = AtomicUsize::new(0);
    
    /// Set the write count after which to simulate a crash.
    /// 0 means no crash simulation.
    pub fn set_crash_after(n: usize) {
        CRASH_AFTER_WRITES.store(n, Ordering::SeqCst);
        WRITE_COUNT.store(0, Ordering::SeqCst);
    }
    
    /// Check if we should simulate a crash.
    /// Returns true if the crash threshold has been reached.
    pub fn should_crash() -> bool {
        let threshold = CRASH_AFTER_WRITES.load(Ordering::SeqCst);
        if threshold == 0 {
            return false;
        }
        let count = WRITE_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        count >= threshold
    }
    
    /// Reset fault injection state.
    pub fn reset() {
        CRASH_AFTER_WRITES.store(0, Ordering::SeqCst);
        WRITE_COUNT.store(0, Ordering::SeqCst);
    }
}
```

#### Step 4.2: Integrate Fault Injection into Write Path

**File:** `src/core/ops/journal.rs`

```rust
impl Journal {
    pub fn write(&self, paths: &LatticePaths) -> Result<(), JournalError> {
        #[cfg(any(test, feature = "fault_injection"))]
        if fault_injection::should_crash() {
            return Err(JournalError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "simulated crash for fault injection",
            )));
        }
        
        // ... existing write logic ...
    }
}
```

#### Step 4.3: Write Fault Injection Tests

**File:** `tests/journal_crash_recovery.rs` (NEW)

```rust
//! Fault injection tests for journal crash recovery.
//!
//! Per SPEC.md §4.2.2, journals must be recoverable from any crash point.

use lattice::core::ops::journal::{fault_injection, Journal, StepKind};
use lattice::core::paths::LatticePaths;
use tempfile::TempDir;

fn setup_test_paths() -> (TempDir, LatticePaths) {
    let temp = TempDir::new().unwrap();
    // Create minimal paths structure
    let paths = LatticePaths::new(
        temp.path().join(".git"),
        temp.path().join(".git"),
    );
    std::fs::create_dir_all(paths.repo_ops_dir()).unwrap();
    (temp, paths)
}

#[test]
fn crash_after_first_step_recovers_empty() {
    let (_temp, paths) = setup_test_paths();
    fault_injection::reset();
    
    // Set up to crash on first write
    fault_injection::set_crash_after(1);
    
    let mut journal = Journal::new("test-op");
    
    // This should "crash"
    let result = journal.append_ref_update(
        &paths,
        "refs/heads/feature",
        None,
        "abc123",
    );
    assert!(result.is_err());
    
    // Journal file should not exist (crash before write completed)
    // In real crash, file might be partially written - we handle that
    
    fault_injection::reset();
}

#[test]
fn crash_after_second_step_recovers_first() {
    let (_temp, paths) = setup_test_paths();
    fault_injection::reset();
    
    let mut journal = Journal::new("test-op");
    
    // First step succeeds
    journal.append_ref_update(
        &paths,
        "refs/heads/feature-a",
        None,
        "abc123",
    ).unwrap();
    
    // Set up to crash on next write
    fault_injection::set_crash_after(1);
    
    // Second step "crashes"
    let result = journal.append_ref_update(
        &paths,
        "refs/heads/feature-b",
        None,
        "def456",
    );
    assert!(result.is_err());
    
    // Recovery: reload journal
    fault_injection::reset();
    let recovered = Journal::read(&paths, &journal.op_id).unwrap();
    
    // Should have exactly one step
    assert_eq!(recovered.steps.len(), 1);
    match &recovered.steps[0].kind {
        StepKind::RefUpdate { refname, .. } => {
            assert_eq!(refname, "refs/heads/feature-a");
        }
        _ => panic!("Expected RefUpdate step"),
    }
}

#[test]
fn all_steps_persisted_on_success() {
    let (_temp, paths) = setup_test_paths();
    fault_injection::reset();
    
    let mut journal = Journal::new("test-op");
    
    // Add multiple steps
    journal.append_ref_update(&paths, "refs/heads/a", None, "111").unwrap();
    journal.append_ref_update(&paths, "refs/heads/b", None, "222").unwrap();
    journal.append_ref_update(&paths, "refs/heads/c", None, "333").unwrap();
    journal.append_checkpoint(&paths, "done").unwrap();
    
    // Reload and verify
    let recovered = Journal::read(&paths, &journal.op_id).unwrap();
    assert_eq!(recovered.steps.len(), 4);
}

#[test]
fn partial_write_detected() {
    // This tests that if a write is interrupted mid-way (corrupt JSON),
    // we can detect and handle it.
    let (_temp, paths) = setup_test_paths();
    
    let mut journal = Journal::new("test-op");
    journal.append_ref_update(&paths, "refs/heads/a", None, "111").unwrap();
    
    // Manually corrupt the journal file
    let journal_path = paths.repo_op_journal_path(journal.op_id.as_str());
    let content = std::fs::read_to_string(&journal_path).unwrap();
    // Truncate to simulate partial write
    std::fs::write(&journal_path, &content[..content.len()/2]).unwrap();
    
    // Read should fail with parse error (not silent corruption)
    let result = Journal::read(&paths, &journal.op_id);
    assert!(result.is_err());
}
```

---

### Phase 5: Documentation and Cleanup

#### Step 5.1: Update Module Documentation

**File:** `src/core/ops/journal.rs`

Update the module doc to explain the crash safety contract:

```rust
//! Operation journaling for crash safety.
//!
//! This module implements the operation journal per SPEC.md §4.2.2:
//!
//! > "Journals must be written with fsync at each appended step boundary."
//!
//! # Crash Safety Contract
//!
//! The journal provides the following guarantees:
//!
//! 1. **Per-step persistence:** Every `append_*` method writes to disk with fsync
//!    before returning. A crash at any point leaves the journal in a consistent state.
//!
//! 2. **Recoverability:** After a crash, `Journal::read()` returns the journal as
//!    it was after the last successful `append_*` call.
//!
//! 3. **Rollback support:** The journal records enough information to reverse all
//!    ref updates via `ref_updates_for_rollback()`.
//!
//! # Usage
//!
//! ```rust,ignore
//! let mut journal = Journal::new("my-operation");
//!
//! // Each append_* persists immediately
//! journal.append_ref_update(&paths, "refs/heads/feature", None, "abc123")?;
//! journal.append_metadata_write(&paths, "feature", None, "meta-oid")?;
//!
//! // Phase transitions also persist
//! journal.commit();
//! journal.write(&paths)?;
//! ```
//!
//! # Migration from `record_*` Methods
//!
//! The old `record_*` methods are deprecated. They only modified in-memory state
//! and required a separate `write()` call, which could be forgotten. Use the
//! corresponding `append_*` methods instead:
//!
//! | Deprecated | Replacement |
//! |------------|-------------|
//! | `record_ref_update()` | `append_ref_update()` |
//! | `record_metadata_write()` | `append_metadata_write()` |
//! | `record_metadata_delete()` | `append_metadata_delete()` |
//! | `record_checkpoint()` | `append_checkpoint()` |
//! | `record_git_process()` | `append_git_process()` |
//! | `record_conflict_paused()` | `append_conflict_paused()` |
```

#### Step 5.2: Remove Deprecated Methods (Future)

After the deprecation period (next major version), remove the old `record_*` methods entirely. For now, keep them with `#[deprecated]` to avoid breaking any external users.

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/core/ops/journal.rs` | MODIFY | Add `append_*` methods, deprecate `record_*`, update docs |
| `src/engine/exec.rs` | MODIFY | Use `append_*` methods, remove manual `write()` calls |
| `tests/journal_crash_recovery.rs` | NEW | Fault injection tests |

---

## Acceptance Gates

Per ROADMAP.md:

- [ ] Journal `append_*` methods fsync at each step boundary
- [ ] Old `record_*` methods deprecated with clear migration guidance
- [ ] Executor uses new `append_*` methods
- [ ] Manual `write()` calls after mutations removed from executor
- [ ] Fault-injection tests verify crash recoverability
- [ ] Partial write corruption is detected (not silently accepted)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes
- [ ] `cargo fmt --check` passes

---

## Testing Rubric

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `append_ref_update_persists` | journal.rs | Verify step is on disk immediately |
| `append_metadata_write_persists` | journal.rs | Verify step is on disk immediately |
| `append_checkpoint_persists` | journal.rs | Verify step is on disk immediately |

### Fault Injection Tests

| Test | File | Description |
|------|------|-------------|
| `crash_after_first_step_recovers_empty` | journal_crash_recovery.rs | First step crash |
| `crash_after_second_step_recovers_first` | journal_crash_recovery.rs | Second step crash |
| `all_steps_persisted_on_success` | journal_crash_recovery.rs | Happy path |
| `partial_write_detected` | journal_crash_recovery.rs | Corrupt file detection |

### Integration Tests

| Test | Description |
|------|-------------|
| Verify restack with simulated crash mid-way recovers correctly |
| Verify abort uses journal to rollback even after simulated crash |

---

## Performance Considerations

**Concern:** Fsync after every step could be slow.

**Analysis:** Modern SSDs have fast fsync (typically <1ms). For a restack of 10 branches with ~20 steps total, this adds ~20ms. This is acceptable for correctness.

**Mitigation (if needed later):**
1. Measure actual impact with benchmarks
2. Consider batched checkpoints with explicit "batch start/end" markers
3. Propose SPEC amendment if batching is needed

For Milestone 0.9, strict SPEC compliance is the goal.

---

## Dependencies

**Depends on:**
- Milestone 0.3 (Journal Rollback) - COMPLETE - Uses journal structure

**Blocked by this:**
- Milestone 0.5 (Multi-step Journal Continuation) - Uses journal API

---

## Verification Commands

After implementation, run:

```bash
# Type checks
cargo check

# Lint (should show deprecation warnings for record_* if any remain)
cargo clippy -- -D warnings

# All tests
cargo test

# Specific tests
cargo test journal
cargo test crash_recovery

# Format check
cargo fmt --check
```

---

## Complexity Assessment

**Estimated complexity:** LOW

This milestone is primarily:
1. Adding new methods with similar logic to existing ones
2. Updating callers to use new API
3. Adding tests

The changes are mechanical and localized. No architectural changes required.

---

## Next Steps (After Completion)

Per ROADMAP.md execution order:
1. ~~Milestone 0.4: OpState Full Payload~~ - COMPLETE
2. ~~Milestone 0.1: Gating Integration + Scope Walking~~ - COMPLETE  
3. ~~Milestone 0.2 + 0.6: Occupancy + Post-Verify~~ - COMPLETE
4. ~~Milestone 0.3: Journal Rollback~~ - COMPLETE
5. **Milestone 0.9: Journal Fsync Step Boundary** (this)
6. Milestone 0.5: Multi-step Journal Continuation
7. Milestone 0.8: Bare Repo Compliance
