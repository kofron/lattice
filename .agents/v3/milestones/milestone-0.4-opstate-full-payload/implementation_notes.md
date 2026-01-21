# Implementation Notes: Milestone 0.4 - OpState Full Payload

## Date: 2026-01-20

## Summary

Milestone 0.4 implements the missing `OpState` fields required by SPEC.md §4.6.5. This was the **first milestone in execution order** but had been skipped in earlier work.

---

## Key Implementation Decisions

### 1. Avoiding Circular Dependencies

The `Plan` type is in `engine::plan`, and `OpState` is in `core::ops::journal`. Since `core` cannot depend on `engine`, we couldn't pass a `Plan` directly to `OpState::from_journal()`.

**Solution:** The constructor accepts the extracted values (`plan_digest: String`, `touched_refs: Vec<TouchedRef>`) instead of the `Plan` itself. The caller (in `engine`) calls `plan.digest()` and `plan.touched_refs_with_oids()` to extract these values.

### 2. Legacy Command Support

Many CLI commands use an ad-hoc execution approach rather than the executor pattern. These commands don't have a `Plan` object, so they can't provide plan info.

**Solution:** Added `OpState::from_journal_legacy()` as a deprecated transitional constructor that uses empty defaults for plan fields. This allows migration to happen incrementally.

### 3. TouchedRef Design

The `TouchedRef` struct stores:
- `refname: String` - Full ref path
- `expected_old: Option<String>` - OID before operation (None for creates)

**Why String instead of Oid?** For serialization simplicity with serde. The journal already uses strings for OIDs.

### 4. Plan::touched_refs_with_oids()

Added a new method to `Plan` that extracts CAS preconditions from plan steps:
- Iterates through steps collecting `old_oid` values
- Deduplicates by refname (first occurrence wins)
- Handles both branch refs and metadata refs

### 5. Version Verification Only (Not Digest)

The plan initially called for verifying `plan_digest` on continue. However:
- Digest verification requires loading the plan from somewhere
- The plan is not currently stored persistently (only op-state is)
- Full digest verification is better suited for Milestone 0.5 (Multi-step Continuation)

**Decision:** Implemented schema version verification only. Digest field is populated and stored, ready for 0.5.

---

## Test Coverage

### New Tests in journal.rs
- `awaiting_reason::*` - 3 tests for serialization
- `touched_ref::*` - 4 tests for construction and serialization
- `plan_schema_version::version_is_one`
- `op_state::pause_with_reason_sets_fields`

### New Tests in plan.rs
- `touched_refs_with_oids_extracts_cas_preconditions`
- `touched_refs_with_oids_deduplicates`
- `touched_refs_with_oids_includes_metadata_refs`
- `touched_refs_with_oids_handles_new_refs`

### Updated Tests
- All `op_state::*` tests in journal.rs updated for new signature
- Integration tests in `persistence_integration.rs` use legacy constructor
- Integration tests in `worktree_support_integration.rs` use legacy constructor

---

## Files Changed

| File | Lines Changed | Type |
|------|--------------|------|
| `src/core/ops/journal.rs` | +200 | New types, extended struct, tests |
| `src/engine/plan.rs` | +100 | New method, tests |
| `src/engine/exec.rs` | +20 | Use new signature |
| `src/cli/commands/recovery.rs` | +12 | Version verification |
| `src/cli/commands/*.rs` | +2 each | 12 files use legacy constructor |
| `tests/persistence_integration.rs` | +5 | Use legacy constructor |
| `tests/worktree_support_integration.rs` | +4 | Use legacy constructor |

---

## Known Limitations

1. **Legacy commands don't populate plan fields** - This is acceptable; they use empty defaults until migrated to executor pattern.

2. **Digest verification deferred** - Full digest verification requires plan persistence, which is part of Milestone 0.5.

3. **Migration path for old op-state files** - Old files will fail to deserialize. Since op-state is transient, this is acceptable. Users should abort old operations before upgrading.

---

## Verification Results

```
cargo check        ✓
cargo clippy       ✓ (no warnings)
cargo fmt --check  ✓
cargo test         ✓ (815+ tests pass)
```

---

## Follow-up Items for Future Milestones

1. **Milestone 0.5**: Implement digest verification in `continue_op()` once plan persistence is added
2. **Future**: Migrate CLI commands from legacy constructor to executor pattern
3. **Future**: Consider storing full `Plan` in op-state for better recovery
