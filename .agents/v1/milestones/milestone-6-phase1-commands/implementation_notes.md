# Milestone 6: Phase 1 Commands - Implementation Notes

## Summary

Successfully implemented all Phase 1 commands for the Lattice local stack engine. These commands follow the validated execution model (Scan → Gate → Plan → Execute → Verify) and use the infrastructure built in Milestones 0-5.

## Commands Implemented

### Phase A: Read-Only Commands
- **log** (`log_cmd.rs`) - Display tracked branches in stack layout
- **info** (`info.rs`) - Show tracking status, parent, freeze state, PR state
- **parent** (`relationships.rs`) - Show parent of a branch
- **children** (`relationships.rs`) - Show children of a branch
- **trunk** (`trunk.rs`) - Display or set configured trunk branch

### Phase B: Setup Commands
- **init** (`init.rs`) - Initialize Lattice in repository with trunk selection
- **config** (`config_cmd.rs`) - Get/set/list configuration values
- **completion** (`completion.rs`) - Generate shell completion scripts (bash, zsh, fish, powershell)
- **changelog** (`changelog.rs`) - Display version and recent changes

### Phase C: Tracking Commands
- **track** (`track.rs`) - Start tracking a branch with parent selection
- **untrack** (`untrack.rs`) - Stop tracking a branch (with descendant handling)
- **freeze** (`freeze.rs`) - Freeze a branch to prevent restacking
- **unfreeze** (`freeze.rs`) - Unfreeze a branch

### Phase D: Navigation Commands
- **checkout** (`checkout.rs`) - Check out a branch by name, trunk, or stack
- **up** (`navigation.rs`) - Move up the stack toward children
- **down** (`navigation.rs`) - Move down the stack toward parent
- **top** (`navigation.rs`) - Jump to the topmost branch in stack
- **bottom** (`navigation.rs`) - Jump to the trunk-child (per SPEC 8C.3)

### Phase E: Core Mutating Commands
- **restack** (`restack.rs`) - Rebase tracked branches to align with parent tips
  - Full engine lifecycle with journaling
  - Conflict detection and pause/resume capability
  - Topological ordering for correct rebase order
- **continue** (`recovery.rs`) - Resume paused operations after conflict resolution
- **abort** (`recovery.rs`) - Cancel paused operations and restore state
- **undo** (`undo.rs`) - Rollback the last committed operation using journal
- **create** (`create.rs`) - Create a new tracked branch with optional commit

## Key Implementation Details

### API Conventions Discovered
- `MetadataStore` uses CAS semantics: `write_cas(branch, expected_old_oid, metadata)` and `delete_cas(branch, expected_oid)`
- `Config` uses `Config::load(git_dir)` and `Config::write_repo(repo_root, config)` - note: write_repo takes repo root, not git_dir
- `Git::merge_base()` returns `Result<Option<Oid>, GitError>` 
- `GitState` is an enum with variants: `Clean`, `Rebase { current, total }`, `Merge`, `CherryPick`, `Revert`, `Bisect`, `ApplyMailbox`
- `Journal` has public fields (`op_id`, `command`, `phase`, `steps`) not getter methods
- Schema types: `ParentInfo`, `BaseInfo`, `BranchInfo`, `FreezeState`, `PrState` (not `ParentRef`, `BaseCommit`, etc.)
- `FreezeState::frozen(FreezeScope, Option<String>)` and `FreezeState::Unfrozen` variant

### Module Organization
```
src/cli/commands/
├── mod.rs           # Dispatch logic + re-exports
├── log_cmd.rs       # log command (avoid collision with log macro)
├── info.rs          # info command
├── relationships.rs # parent/children commands
├── trunk.rs         # trunk command
├── init.rs          # init command
├── config_cmd.rs    # config command (avoid collision with module)
├── completion.rs    # completion command
├── changelog.rs     # changelog command
├── track.rs         # track command
├── untrack.rs       # untrack command
├── freeze.rs        # freeze/unfreeze commands
├── checkout.rs      # checkout command
├── navigation.rs    # up/down/top/bottom commands
├── restack.rs       # restack command
├── recovery.rs      # continue/abort commands
├── undo.rs          # undo command
└── create.rs        # create command
```

### Dependencies Added
- `clap_complete = "4"` for shell completion generation

### Bug Fixes During Implementation
- `init.rs`: Fixed `Config::write_repo` call to pass repo root (`cwd`) instead of `git_dir`

## Integration Tests

Created `tests/commands_integration.rs` with 43 tests covering:

### Init Command (3 tests)
- `init_creates_config` - Verifies config file creation
- `init_with_custom_trunk` - Custom trunk branch selection
- `init_reset_clears_metadata` - Reset clears all metadata refs

### Track/Untrack Commands (6 tests)
- `track_creates_metadata` - Basic tracking
- `track_with_branch_parent` - Non-trunk parent
- `track_already_tracked_is_idempotent` - Idempotent behavior
- `track_as_frozen` - Track with frozen state
- `untrack_removes_metadata` - Basic untracking
- `untrack_with_descendants` - Cascade untrack

### Freeze/Unfreeze Commands (2 tests)
- `freeze_sets_frozen_state`
- `unfreeze_clears_frozen_state`

### Navigation Commands (7 tests)
- `checkout_switches_branch`
- `checkout_trunk`
- `down_navigates_to_parent`
- `up_navigates_to_child`
- `bottom_navigates_to_trunk_child`
- `top_navigates_to_leaf`
- Navigation edge cases (no-ops)

### Restack Commands (4 tests)
- `restack_updates_branch_base`
- `restack_skips_frozen_branches`
- `restack_already_aligned_is_noop`
- `restack_chain_updates_all`

### Info/Log Commands (6 tests)
- `info_shows_branch_details`
- `info_on_untracked_branch`
- `log_shows_stack`
- `parent_returns_parent_name`
- `children_returns_child_names`
- `trunk_returns_trunk_name`

### Create Command (2 tests)
- `create_makes_tracked_branch`
- `create_with_explicit_parent`

### Config/Completion/Changelog (4 tests)
- `config_get_trunk`
- `config_list`
- `completion_generates_scripts`
- `changelog_outputs_version`

### Error Cases (5 tests)
- `track_nonexistent_branch_fails`
- `track_trunk_fails`
- `checkout_nonexistent_branch_fails`
- `restack_untracked_branch_fails`
- `info_nonexistent_branch_fails`

### Graph/Stack Tests (3 tests)
- `graph_preserves_structure_after_operations`
- `multiple_independent_stacks`
- `metadata_cas_prevents_race`

## Acceptance Gate Status

- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes  
- [x] `cargo test --lib` passes (388 tests)
- [x] `cargo test --tests` passes (43 commands + 48 git + 29 persistence + 12 property = 132 tests)
- [x] `cargo doc --no-deps` succeeds
- [x] Phase 1 command integration tests (43 tests)
- [ ] Fault injection tests for executor step boundaries (future milestone)
- [x] Freeze enforcement blocks rewriting commands (tested)

## Known Limitations

1. **Restack cascading**: After restacking a branch, its children's base still points to the old parent tip. A second restack is needed to cascade updates. This is documented in the test.

2. **Pre-existing doctest failures**: Two doctest failures exist in `src/doctor/fixes.rs` due to private module access in documentation examples. These are pre-existing and unrelated to Milestone 6.

## SPEC Compliance Notes

- `bottom` command goes to trunk-child (first tracked branch), not trunk itself (per SPEC 8C.3)
- `up`/`down` with no valid target is a no-op, not an error
- Commands are re-exported from `cli::commands` for testing
