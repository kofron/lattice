# Milestone 0.5 Implementation Notes

## Summary

Successfully implemented multi-step journal continuation, enabling `lattice continue` to resume operations from where they paused rather than assuming completion after a single conflict resolution.

## Key Implementation Decisions

### D1: Storing Remaining Steps as JSON String

**Decision:** Store `remaining_steps` as `Option<String>` (JSON) rather than `Vec<PlanStep>`.

**Rationale:** Avoided circular module dependency. The `engine::plan` module imports from `core::ops::journal`, so importing `PlanStep` into `journal.rs` would create a cycle. JSON serialization provides a clean decoupling while maintaining full fidelity.

```rust
// In StepKind::ConflictPaused
remaining_steps_json: Option<String>,  // Serialized Vec<PlanStep>
```

### D2: Test-only Helper Method

Added `#[cfg(test)]` method `record_conflict_paused_with_remaining_steps()` for unit testing the new functionality without deprecation warnings. Production code uses `append_conflict_paused()` which persists immediately.

### D3: Clippy Compliance

Added `#[allow(clippy::too_many_arguments)]` to `pause_for_nested_conflict()` as the 8-parameter signature is justified by the operation's complexity (context, git, paths, state, journal, branch, git_state, remaining_steps).

## Files Modified

| File | Changes |
|------|---------|
| `src/core/ops/journal.rs` | Extended `ConflictPaused` with `remaining_steps_json`, added helper methods (`remaining_steps_json()`, `has_remaining_steps()`, `paused_branch()`, `remaining_branches()`), added test helper and 10 new unit tests |
| `src/engine/exec.rs` | Serialize remaining steps to JSON when pausing, pass to `append_conflict_paused()` |
| `src/cli/commands/recovery.rs` | Complete rewrite of `continue_op()` to handle multi-step resumption, added helper functions for step execution, nested conflict handling, and operation completion |

## New Test Coverage

Added 10 unit tests for journal continuation support:
- `record_conflict_paused_with_remaining_steps_json`
- `remaining_steps_json_returns_none_when_no_conflict`
- `remaining_steps_json_returns_json_from_conflict_paused`
- `has_remaining_steps_false_when_no_conflict`
- `has_remaining_steps_false_when_empty_json`
- `has_remaining_steps_true_when_steps_present`
- `paused_branch_returns_branch_when_conflict_paused`
- `paused_branch_returns_none_when_not_paused`
- `remaining_branches_returns_list_from_conflict_paused`
- `remaining_branches_returns_empty_when_not_paused`

## Verification

All acceptance gates pass:
- `cargo check` - Clean
- `cargo clippy -- -D warnings` - Clean
- `cargo test` - 833 tests pass (823 + 10 new)
- `cargo fmt --check` - Clean

## Architecture Alignment

Implementation follows ARCHITECTURE.md principles:
- **Functional core, imperative shell:** Journal helper methods are pure; recovery.rs handles I/O
- **Crash consistency:** Remaining steps persisted via fsync'd journal append
- **Worktree occupancy:** Re-validated before executing remaining steps
- **Lock acquisition:** Repository lock acquired before continuation per §6.2
