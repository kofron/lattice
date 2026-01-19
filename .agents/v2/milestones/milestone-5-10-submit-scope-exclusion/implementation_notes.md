# Milestone 5.10 Implementation Notes

## Summary

Implemented submit scope exclusion for synthetic snapshot branches created by Milestone 5.9. This ensures that historical snapshot branches (frozen with reason `remote_synthetic_snapshot`) are automatically excluded from `lattice submit` operations.

## Files Modified

| File | Changes |
|------|---------|
| `src/cli/commands/submit.rs` | Added 4 helper functions, integrated filtering into `submit_async`, added 8 unit tests |
| `src/cli/args.rs` | Updated help text for submit command to document snapshot exclusion |

## Implementation Details

### Helper Functions Added (submit.rs:68-156)

1. **`is_synthetic_snapshot(branch, snapshot) -> bool`**
   - Checks if a branch is a synthetic snapshot by examining its freeze reason
   - Returns `true` only if freeze reason equals `FREEZE_REASON_SYNTHETIC_SNAPSHOT`
   - Safe for untracked branches (returns `false`)

2. **`filter_snapshot_branches(branches, snapshot) -> (filtered, excluded)`**
   - Filters a list of branches, separating snapshot branches from submittable ones
   - Returns tuple of (branches to submit, excluded snapshot branches)
   - Preserves order of non-snapshot branches

3. **`report_excluded_snapshots(excluded, quiet)`**
   - Prints user feedback when snapshot branches are excluded
   - Respects quiet mode
   - Message format: "Excluding N snapshot branch(es) from submit scope:"

4. **`check_current_branch_not_snapshot(current, snapshot) -> Result<()>`**
   - Early guard to refuse submission from snapshot branches
   - Returns error with helpful message suggesting `git checkout -b` workflow
   - Called before any scope computation

### Integration Point (submit.rs:373-389)

In `submit_async`, after computing the initial scope but before any operations:

```rust
// Check current branch is not a snapshot (refuse early)
check_current_branch_not_snapshot(current, &snapshot)?;

// ... scope computation ...

// Filter out snapshot branches (Milestone 5.10)
let (branches, excluded) = filter_snapshot_branches(branches, &snapshot);
report_excluded_snapshots(&excluded, ctx.quiet);

// Check we have branches to submit after filtering
if branches.is_empty() {
    bail!("No branches to submit after filtering...");
}
```

### Unit Tests (submit.rs:711-884)

Added `snapshot_exclusion` submodule with 8 tests:

| Test | Purpose |
|------|---------|
| `is_synthetic_snapshot_returns_true_for_snapshot` | Validates detection of snapshot branches |
| `is_synthetic_snapshot_returns_false_for_normal_branch` | Ensures normal branches pass through |
| `is_synthetic_snapshot_returns_false_for_other_frozen_branch` | Other freeze reasons are not excluded |
| `is_synthetic_snapshot_returns_false_for_untracked` | Untracked branches are not excluded |
| `filter_snapshot_branches_excludes_snapshots` | Filtering correctly separates snapshots |
| `filter_snapshot_branches_preserves_order` | Order of non-snapshot branches maintained |
| `check_current_branch_not_snapshot_passes_for_normal` | Normal branches allowed |
| `check_current_branch_not_snapshot_fails_for_snapshot` | Snapshot branches rejected with message |

### Help Text Update (args.rs:275-278)

Added note to submit command's `long_about`:

```
NOTE: Synthetic snapshot branches (created by `lattice doctor` from closed PRs)
are automatically excluded from the submit scope.
```

## Design Decisions

### Freeze Reason Over Name Pattern

Used freeze reason (`FREEZE_REASON_SYNTHETIC_SNAPSHOT`) rather than branch name pattern (`lattice/snap/pr-*`) because:

1. **Semantic clarity:** The freeze reason explicitly captures the intent
2. **Robustness:** Name collisions or manual renames don't cause issues
3. **Consistency:** Uses existing metadata infrastructure from Milestone 5.9
4. **Extensibility:** Other freeze reasons can have different exclusion rules

### Early Refusal for Current Branch

When the current branch is a snapshot, we refuse the entire operation rather than filtering it out because:

1. Clearer UX - user knows immediately their branch can't be submitted
2. Avoids confusion about what would happen with `--stack`
3. Provides helpful guidance on how to work with the code

### Silent vs Verbose Feedback

Chose verbose feedback (printing excluded branches) rather than silent exclusion because:

1. Users should understand why branches weren't submitted
2. Prevents confusion if user expects certain branches in the scope
3. Respects `--quiet` flag for scripting use cases

## Verification

All checks pass:

- `cargo check` - No type errors
- `cargo clippy -- -D warnings` - No lint warnings
- `cargo test` - All 60 doctests + all unit tests pass
- `cargo test snapshot_exclusion` - All 8 new tests pass
- `cargo fmt --check` - Code properly formatted

## Acceptance Criteria Status

Per ROADMAP.md Milestone 5.10:

- [x] Snapshot branches excluded from `lt submit` default scope
- [x] Snapshot branches excluded from `lt submit --stack` scope
- [x] Help text documents exclusion behavior
- [x] Clear error when attempting to submit from a snapshot branch
- [x] Clear message when branches are excluded from scope
- [x] Normal branches and other frozen branches are not affected
- [x] `cargo test` passes
- [x] `cargo clippy` passes
