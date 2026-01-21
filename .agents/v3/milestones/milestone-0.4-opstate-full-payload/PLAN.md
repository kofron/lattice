# Milestone 0.4: OpState Full Payload

## Status: COMPLETE

---

## Overview

**Goal:** Implement the missing fields in `OpState` as required by SPEC.md §4.6.5 and ROADMAP.md. This was supposed to be **Order 1** in the execution sequence but was initially skipped.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs, Tests are everything.

**Priority:** HIGH - Foundation for rollback + continuation

---

## Problem Statement

The `OpState` struct was missing fields required by SPEC.md and needed by rollback/continuation:

- `plan_digest` (required by spec for integrity checking)
- `plan_schema_version` (for cross-version compatibility)
- `touched_refs` with expected old OIDs (needed for CAS rollback)
- `awaiting_reason` (for user feedback on why operation is paused)

---

## Solution Implemented

### Phase 1: New Types Added to journal.rs

1. **`PLAN_SCHEMA_VERSION` constant** - Currently set to 1
2. **`AwaitingReason` enum** with variants:
   - `RebaseConflict`
   - `RollbackIncomplete { failed_refs: Vec<String> }`
   - `VerificationFailed { evidence: String }`
3. **`TouchedRef` struct** with:
   - `refname: String`
   - `expected_old: Option<String>`

### Phase 2: OpState Extended

Added four new fields to `OpState`:
- `plan_digest: String` - SHA-256 hash of plan JSON
- `plan_schema_version: u32` - Version for compatibility checking
- `touched_refs: Vec<TouchedRef>` - Refs with expected old OIDs
- `awaiting_reason: Option<AwaitingReason>` - Why operation is paused

### Phase 3: Constructor Updates

1. **`OpState::from_journal()`** now requires:
   - `plan_digest: String`
   - `touched_refs: Vec<TouchedRef>`

2. **`OpState::from_journal_legacy()`** added for CLI commands that haven't migrated to executor pattern yet (marked `#[deprecated]`)

3. **`OpState::pause_with_reason()`** added to set both phase and reason

### Phase 4: Plan.rs Enhancement

Added **`Plan::touched_refs_with_oids()`** method that extracts `TouchedRef` entries from plan steps with their CAS preconditions.

### Phase 5: Version Verification

Added schema version check to `continue_op()` in recovery.rs:
- If `op_state.plan_schema_version != PLAN_SCHEMA_VERSION`, bail with actionable error message

### Phase 6: All Callers Updated

- **src/engine/exec.rs** - Uses new signature with plan info
- **src/cli/commands/*.rs** - Use `from_journal_legacy()` (14 files)
- **Tests in journal.rs** - Updated with new signature

---

## Files Modified

| File | Changes |
|------|---------|
| `src/core/ops/journal.rs` | Added types, extended OpState, new methods, tests |
| `src/engine/plan.rs` | Added `touched_refs_with_oids()`, tests |
| `src/engine/exec.rs` | Updated to use new OpState signature |
| `src/cli/commands/recovery.rs` | Added version verification |
| `src/cli/commands/split.rs` | Use legacy constructor |
| `src/cli/commands/revert.rs` | Use legacy constructor |
| `src/cli/commands/modify.rs` | Use legacy constructor |
| `src/cli/commands/reorder.rs` | Use legacy constructor |
| `src/cli/commands/phase3_helpers.rs` | Use legacy constructor |
| `src/cli/commands/fold.rs` | Use legacy constructor |
| `src/cli/commands/pop.rs` | Use legacy constructor |
| `src/cli/commands/delete.rs` | Use legacy constructor |
| `src/cli/commands/rename.rs` | Use legacy constructor |
| `src/cli/commands/move_cmd.rs` | Use legacy constructor |
| `src/cli/commands/restack.rs` | Use legacy constructor |
| `src/cli/commands/squash.rs` | Use legacy constructor |

---

## Acceptance Gates

- [x] `OpState` includes `plan_digest`, `plan_schema_version`, `touched_refs`
- [x] Digest computed from stable JSON serialization (via Plan::digest())
- [x] Continue verifies schema version matches
- [x] Version mismatch produces actionable error
- [x] `touched_refs` available for rollback CAS
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

## Tests Added

### In journal.rs
- `awaiting_reason::rebase_conflict_serializes`
- `awaiting_reason::rollback_incomplete_serializes`
- `awaiting_reason::verification_failed_serializes`
- `touched_ref::new_creates_with_values`
- `touched_ref::new_with_none`
- `touched_ref::serializes_roundtrip`
- `touched_ref::serializes_with_none`
- `plan_schema_version::version_is_one`
- `op_state::from_journal` (updated with new fields)
- `op_state::pause_with_reason_sets_fields`

### In plan.rs
- `touched_refs_with_oids_extracts_cas_preconditions`
- `touched_refs_with_oids_deduplicates`
- `touched_refs_with_oids_includes_metadata_refs`
- `touched_refs_with_oids_handles_new_refs`

---

## Migration Notes

- Existing `op-state.json` files will fail to deserialize (missing fields)
- This is acceptable: op-state is transient, exists only during in-flight ops
- Users with old op-state should `abort` with old binary first

---

## Complexity Assessment

**LOW** - Primarily:
- Adding struct fields and types
- Updating constructors
- Adding verification logic
- Updating tests

No architectural changes required.
