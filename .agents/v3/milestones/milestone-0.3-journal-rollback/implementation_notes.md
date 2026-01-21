# Implementation Notes: Milestone 0.3 - Journal Rollback

**Completed:** 2026-01-20

---

## Summary

Implemented the full journal rollback functionality for the `abort()` command, completing the crash safety contract from SPEC.md Section 4.2.2. The implementation extracts shared rollback logic into a dedicated module used by both the Executor and the abort command.

---

## Implementation Choices

### 1. Shared Rollback Module Architecture

**Choice:** Created `src/engine/rollback.rs` as a standalone module rather than keeping rollback logic inline in both the Executor and abort command.

**Rationale:** 
- **Reuse principle** - Both `Executor::attempt_rollback()` and `abort()` need identical rollback logic
- **Purity principle** - The rollback logic is pure: given a Git interface and journal, it performs CAS operations in reverse order
- **Simplicity** - Single source of truth for rollback behavior

**Result:** The Executor's `attempt_rollback()` went from ~100 lines to ~20 lines, and the abort command uses the same battle-tested logic.

### 2. RollbackResult Structure

**Choice:** Created `RollbackResult` with explicit success/failure tracking instead of a simple Result<(), Error>.

**Rationale:**
- Partial rollbacks are valid (some refs succeed, others fail CAS)
- Need to track which refs were rolled back for reporting
- Need to track which refs failed and why for doctor issue generation
- The `complete` flag provides a simple check for full success

### 3. Known Limitations Handling

**Choice:** Documented metadata update/delete limitations explicitly in both code comments and error messages.

**Rationale:**
- Per SPEC.md, the journal doesn't store old content for metadata
- Full rollback support would require extending journal format (out of scope)
- Clear error messages help users understand why rollback is partial
- The `CannotRestore` error variant explicitly names the limitation

### 4. Worktree Origin Check Position

**Choice:** Placed worktree origin check at the very beginning of `abort()`, before any Git operations.

**Rationale:**
- Fail fast if running from wrong worktree
- Per SPEC.md §4.6.5, this is a hard requirement
- Don't want to abort a Git operation only to find we can't complete the abort

### 5. Event Ledger Recording

**Choice:** Record `Aborted` event even if journal loading fails.

**Rationale:**
- The abort happened, it should be recorded
- Ledger errors are non-fatal (logged but don't fail the operation)
- Provides audit trail even for edge cases

---

## Files Changed

| File | Lines Changed | Description |
|------|---------------|-------------|
| `src/engine/rollback.rs` | +280 (new) | Shared rollback logic with RollbackError, RollbackResult, and rollback_journal() |
| `src/engine/mod.rs` | +2 | Export rollback module and re-exports |
| `src/engine/exec.rs` | -80, +20 | Refactored attempt_rollback() to use shared module |
| `src/cli/commands/recovery.rs` | +100 | Full abort implementation with worktree check, rollback, ledger recording |
| `src/core/ops/journal.rs` | +80 | Added can_fully_rollback(), rollback_summary(), RollbackSummary |
| `src/engine/health.rs` | +30 | Added partial_rollback_failure() issue function |

---

## Test Coverage

### Unit Tests Added

1. **rollback.rs tests:**
   - `RollbackResult::new()` defaults
   - `record_success()` keeps complete flag
   - `record_failure()` clears complete flag
   - `has_failures()` check
   - `summary()` string generation
   - `RollbackError` display formatting for all variants

2. **journal.rs tests:**
   - `can_fully_rollback()` with ref-only operations
   - `can_fully_rollback()` with metadata creates
   - `can_fully_rollback()` with metadata updates (returns false)
   - `can_fully_rollback()` with metadata deletes (returns false)
   - `rollback_summary()` categorization
   - `RollbackSummary::is_complete()` check

### Existing Tests

All 802 existing tests continue to pass, including:
- 19 rollback-specific tests
- Executor tests that use rollback
- Journal tests for step recording

---

## Verification Results

```
cargo check        ✓
cargo clippy       ✓ (no warnings)
cargo fmt --check  ✓
cargo test         ✓ (802 tests passed)
cargo test rollback ✓ (19 tests passed)
```

---

## Integration with Existing Code

### Executor Integration

The `Executor::attempt_rollback()` method now delegates to the shared module:

```rust
fn attempt_rollback(&self, journal: &Journal) -> Result<(), ExecuteError> {
    use super::rollback::rollback_journal;
    let result = rollback_journal(self.git, journal);
    if result.complete {
        Ok(())
    } else {
        let failures: Vec<String> = result.failed.iter()
            .map(|(refname, err)| format!("{}: {}", refname, err))
            .collect();
        Err(ExecuteError::Internal(format!(
            "partial rollback - succeeded: [{}], failed: [{}]",
            result.rolled_back.join(", "), failures.join("; ")
        )))
    }
}
```

### Abort Integration

The `abort()` function now follows the full SPEC.md contract:

1. Validates worktree origin
2. Aborts Git operation
3. Loads journal and calls `rollback_journal()`
4. Records `Aborted` event in ledger
5. Handles partial rollback with doctor guidance

---

## Design Decisions Deferred

The following were intentionally not implemented in this milestone:

1. **Full metadata rollback** - Would require storing old content in journal
2. **Integration tests for abort** - Require worktree test infrastructure
3. **Doctor fix for partial rollback** - Issue is surfaced; fix plan deferred

---

## Next Milestone

Per ROADMAP.md execution order, the next milestone is **0.9: Journal Fsync Step Boundary**.
