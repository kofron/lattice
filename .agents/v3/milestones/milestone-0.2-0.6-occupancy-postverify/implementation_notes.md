# Milestone 0.2 + 0.6: Implementation Notes

## Status: COMPLETE

---

## Summary

This milestone wired worktree occupancy checking and post-verification into the executor contract, making them self-enforcing. Commands can no longer mutate branches checked out in other worktrees, and post-execution verification cannot be bypassed.

---

## Changes Made

### Phase 1: Plan Enhancement

**File:** `src/engine/plan.rs`

Added two new methods to the `Plan` struct:

1. `touched_branches() -> Vec<BranchName>` - Extracts branch names from `refs/heads/` refs in the plan's touched refs. Used for occupancy checking.

2. `touches_branch_refs() -> bool` - Returns true if the plan touches any branch refs. Used to skip occupancy checks for metadata-only operations (track, freeze, etc.) per SPEC.md §4.6.8.

Also added import for `BranchName` type.

### Phase 2: Executor Enhancement

**File:** `src/engine/exec.rs`

1. **New error variants:**
   - `ExecuteError::OccupancyViolation { branch, worktree_path }` - Returned when a touched branch is checked out elsewhere
   - `ExecuteError::VerificationFailed { message, rollback_succeeded }` - Returned when post-verification fails

2. **New methods:**
   - `revalidate_occupancy(&self, plan: &Plan)` - Checks worktree occupancy under lock before mutations
   - `post_verify(&self)` - Re-scans and runs fast_verify after execution (self-enforcing)
   - `attempt_rollback(&self, journal: &Journal)` - Attempts to restore refs using journal on verify failure

3. **Wiring in `execute()`:**
   - Added occupancy revalidation immediately after acquiring lock, before any mutations
   - Added post-verification after all steps complete but before recording `Committed` event
   - On verify failure: attempts rollback, records `Aborted` event if rollback succeeds, transitions to `awaiting_user` if rollback fails

### Phase 3: Runner Integration

**File:** `src/engine/runner.rs`

1. Added import for `Plan`
2. Removed import for `fast_verify` (now in executor)
3. Added `check_occupancy_for_plan()` helper function - provides nice UX with actionable guidance before acquiring lock
4. Integrated pre-execution occupancy check after planning but before execution (Step 3.5)
5. Removed redundant verification from runner (Step 5) - now self-enforcing in executor

### Phase 4: Verify Module

**File:** `src/engine/verify.rs`

Added `VerifyError::ScanFailed(String)` variant for when re-scanning fails during executor's post-verification.

---

## Key Design Decisions

### Two-Phase Occupancy Checking

Per the plan, occupancy is checked in two places:

1. **Pre-lock (runner):** Nice UX check with actionable guidance. Returns `NeedsRepair` with `BranchCheckedOutElsewhere` issue.

2. **Under-lock (executor):** Hard correctness check. Returns `OccupancyViolation` error if occupancy changed between scan and execution.

### Verification Inside Executor

Post-verification is now inside the executor's `execute()` method, making it self-enforcing per ARCHITECTURE.md §6.2. The runner no longer needs to call `fast_verify()` after execution.

### Rollback on Verify Failure

When post-verification fails, the executor:
1. Attempts rollback using journal's recorded ref updates
2. If rollback succeeds: records `Aborted` event, clears op-state, returns error
3. If rollback fails: transitions to `awaiting_user` phase, returns error with `rollback_succeeded: false`

### Metadata-Only Exclusion

Plans that only touch metadata refs (e.g., track, freeze) skip occupancy checks entirely per SPEC.md §4.6.8. This is implemented via `plan.touches_branch_refs()`.

---

## Testing

All verification passed:

```
cargo check       ✓
cargo clippy      ✓
cargo fmt --check ✓
cargo test        ✓ (786 tests)
```

Specific tests added:
- `touched_branches_extracts_branch_names`
- `touched_branches_ignores_non_branch_refs`
- `touches_branch_refs_true_when_has_branch`
- `touches_branch_refs_false_for_checkpoint_only`
- `touches_branch_refs_false_for_empty_plan`
- `display_occupancy_violation`
- `display_verification_failed`
- `display_verification_failed_rollback_info`

Existing occupancy tests still pass:
- `branch_checked_out_elsewhere_detected`
- `branches_checked_out_elsewhere_multiple`
- `current_branch_not_elsewhere`
- `list_worktrees_includes_main_and_linked`

---

## Acceptance Criteria Status

### Milestone 0.2 (Occupancy)
- [x] `Plan::touched_branches()` returns all branches affected by plan
- [x] `Plan::touches_branch_refs()` correctly identifies branch-mutating plans
- [x] Post-plan occupancy check in Engine (before lock)
- [x] Under-lock revalidation in Executor (after lock, before mutations)
- [x] `BranchCheckedOutElsewhere` issue includes worktree path and guidance
- [x] Metadata-only plans are NOT blocked by occupancy

### Milestone 0.6 (Post-Verification)
- [x] Executor calls `post_verify()` after successful execution
- [x] Executor revalidates occupancy under lock
- [x] Verification failure attempts rollback first
- [x] Successful rollback on verify failure records `Aborted` event
- [x] Failed rollback on verify failure transitions to `awaiting_user`
- [x] Never claims success when verification fails

### General
- [x] `cargo test` passes
- [x] `cargo clippy` passes
- [x] `cargo fmt --check` passes

---

## Limitations

1. **Metadata rollback:** When rolling back `MetadataWrite` steps where metadata existed before, we can't restore the old content because the journal doesn't store it. This is noted with TODO comments.

2. **Metadata deletion rollback:** Similarly, `MetadataDelete` steps can't be rolled back without the original content.

These limitations are acceptable for now per ROADMAP.md - full journal content storage would be addressed in Milestone 0.4 (OpState Full Payload).

---

## Next Steps

Per ROADMAP.md execution order:
1. ✅ Milestone 0.1: Gating Integration + Scope Walking
2. ✅ Milestone 0.2 + 0.6: Occupancy + Post-Verify (this)
3. → Milestone 0.3: Journal Rollback
4. → Milestone 0.9: Journal Fsync
