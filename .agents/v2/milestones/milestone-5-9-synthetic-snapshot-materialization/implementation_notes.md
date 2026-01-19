# Milestone 5.9: Synthetic Stack Snapshot Materialization - Implementation Notes

## Summary

This milestone implements the ability to materialize local snapshot branches from closed PRs that were merged into a synthetic stack head. When a user selects this fix option, Lattice creates frozen local branches (named `lattice/snap/pr-<number>`) pointing to the PR commits, providing historical context for the synthetic stack.

## Key Implementation Decisions

### 1. Reused Existing Types

Rather than creating new fix types, we extended the existing `FixOption`/`FixPreview` framework:
- `ClosedPrToMaterialize` struct in `fixes.rs` captures PR info needed for materialization
- `RefChange::Create` and `MetadataChange::Create` used for plan preview
- This follows the "Reuse" principle from CLAUDE.md

### 2. Snapshot Branch Naming

The `snapshot_branch_name` function in `generators.rs` generates unique branch names:
- Base pattern: `lattice/snap/pr-<number>`
- Collision avoidance: appends `-1`, `-2`, etc. if name exists
- Fallback: uses Unix timestamp for extreme edge cases

### 3. CreateSnapshotBranch Plan Step

Added a new `PlanStep::CreateSnapshotBranch` variant that:
- Stores branch name, PR number, head branch, and head OID
- Is marked as a mutation (`is_mutation() = true`)
- Touches the target ref (`refs/heads/{branch_name}`)

### 4. Two-Phase Execution

The execution in `exec.rs` follows a safe two-phase approach:
1. **Fetch phase**: Injected `FetchRef` step fetches `refs/pull/{number}/head` before snapshot creation
2. **Create phase**: `CreateSnapshotBranch` reads FETCH_HEAD, validates ancestry, creates branch with CAS

### 5. Ancestry Validation

Before creating a snapshot branch, we validate that the PR commit is an ancestor of the synthetic head:
- Uses `git.is_ancestor(fetch_head, head_oid)`
- Aborts with clear error if validation fails
- Prevents invalid snapshot branches from divergent commits

### 6. Freeze Semantics

Snapshot branches are created with `FreezeState::Frozen` and reason `remote_synthetic_snapshot`:
- Prevents accidental modification of historical snapshots
- Clearly identifies the branch's purpose in metadata
- Uses the new `FREEZE_REASON_SYNTHETIC_SNAPSHOT` constant

### 7. Planner Integration

The `SnapshotContext` struct passes head branch info through the planning phase:
- Built from fix option metadata in `add_fix_steps`
- Passed to `ref_change_to_step` to construct proper `CreateSnapshotBranch` steps
- Enables injection of `FetchRef` steps before snapshot creation

## Files Modified

| File | Changes |
|------|---------|
| `src/doctor/fixes.rs` | Added `ClosedPrToMaterialize` struct |
| `src/core/metadata/schema.rs` | Added `FREEZE_REASON_SYNTHETIC_SNAPSHOT` constant |
| `src/doctor/generators.rs` | Added snapshot name generator, fix generator, and 11 unit tests |
| `src/engine/plan.rs` | Added `CreateSnapshotBranch` variant with trait implementations |
| `src/doctor/planner.rs` | Added `SnapshotContext`, snapshot step conversion, fetch injection |
| `src/engine/exec.rs` | Added `CreateSnapshotBranch` execution with ancestry validation |

## Test Coverage

Added comprehensive unit tests in `generators.rs`:
- `snapshot_branch_name_no_collision` - basic naming
- `snapshot_branch_name_with_collision` - single collision handling
- `snapshot_branch_name_multiple_collisions` - cascade collision handling
- `generate_materialize_snapshot_fixes_with_evidence` - full fix generation
- `generate_materialize_snapshot_fixes_no_evidence` - missing evidence case
- `generate_materialize_snapshot_fixes_empty_closed_prs` - empty list case
- `generate_materialize_snapshot_dispatch_routes_correctly` - dispatcher routing
- `materialize_snapshot_fix_has_correct_preconditions` - precondition verification
- `extract_synthetic_head_branch_from_message` - message parsing
- `extract_synthetic_head_branch_from_evidence` - evidence extraction
- `snapshot_prefix_constant` - constant verification

Added planner test:
- `ref_change_to_step_snapshot_branch` - snapshot branch step conversion

## Dependencies

This milestone builds on:
- **Milestone 5.5**: `is_ancestor`, `fetch_ref`, Git interface methods
- **Milestone 5.8**: `PotentialSyntheticStackHead` issue, `SyntheticStackChildren` evidence

## Verification

All verification commands pass:
```
cargo check        ✓
cargo clippy       ✓  
cargo test         ✓ (724 tests)
cargo fmt --check  ✓
```
