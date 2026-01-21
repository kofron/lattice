# Phase 5 Implementation Checklist

## Progress Tracker

### Task 5.1: `restack` (REFERENCE IMPLEMENTATION) - COMPLETE
- [x] Create `RestackCommand` struct
- [x] Implement `Command` trait
- [x] Update entry point to use `run_command_with_scope()`
- [x] Remove direct `scan()` calls
- [x] Remove manual `Journal` usage
- [x] Remove manual `RepoLock` usage
- [x] Generate plan with Checkpoint + RunGit + PotentialConflictPause + WriteMetadataCas
- [x] Implement `finish()` with proper conflict messaging
- [x] Verify all existing tests pass
- [x] Verify `cargo clippy` passes

**Notes:**
- Fixed executor's `RunGit` handler to check for conflict state BEFORE checking exit code
- Added scoped verification (`verify_branches()`) to only verify touched branches
- Maintained backward compatibility of `get_parent_tip()` signature (returns `Result<Oid>`)

### Task 5.2: `create`
- [ ] Create `CreateCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point to use `run_command()`
- [ ] Handle empty branch creation
- [ ] Handle `--insert` reparenting
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.3: `modify`
- [ ] Create `ModifyCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point to use `run_command_with_scope()`
- [ ] Handle amend vs create mode
- [ ] Include descendant restack in plan
- [ ] Handle deferred ref resolution (post-commit tip)
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.4: `delete`
- [ ] Create `DeleteCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point to use `run_command()`
- [ ] Handle `--upstack` scope
- [ ] Handle `--downstack` scope
- [ ] Handle orphan reparenting
- [ ] Handle checkout before delete
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.5: `rename`
- [ ] Create `RenameCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point to use `run_command()`
- [ ] Rename git branch ref
- [ ] Move metadata ref to new name
- [ ] Update all children's parent pointers
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.6: `squash`
- [ ] Create `SquashCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point
- [ ] Handle interactive rebase to single commit
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.7: `fold`
- [ ] Create `FoldCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point
- [ ] Merge changes into parent
- [ ] Delete folded branch
- [ ] Reparent children to parent
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.8: `move`
- [ ] Create `MoveCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point
- [ ] Change parent pointer
- [ ] Rebase onto new parent
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.9: `pop`
- [ ] Create `PopCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point
- [ ] Delete branch preserving changes
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.10: `reorder`
- [ ] Create `ReorderCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point
- [ ] Handle editor-based reordering
- [ ] Generate multi-rebase plan
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.11: `split`
- [ ] Create `SplitCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point
- [ ] Handle `--by-commit` mode
- [ ] Handle `--by-file` mode
- [ ] Create multiple branches with correct metadata
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

### Task 5.12: `revert`
- [ ] Create `RevertCommand` struct
- [ ] Implement `Command` trait
- [ ] Update entry point
- [ ] Create revert branch with revert commit
- [ ] Verify all existing tests pass
- [ ] Verify `cargo clippy` passes

---

## Final Verification

- [ ] All 12 commands migrated
- [ ] Zero direct `scan()` imports in command files
- [ ] Zero manual `Journal` usage in command files
- [ ] Zero manual `RepoLock` usage in command files
- [ ] All existing tests pass
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] Engine hooks fire (OOB drift harness validates)

---

## Implementation Notes

### 2026-01-21: Restack Migration Complete

**Infrastructure Changes Made:**

1. **Executor conflict detection fix** (`src/engine/exec.rs`):
   - `RunGit` handler now checks for git conflict state BEFORE checking exit code
   - This allows rebase conflicts to properly pause instead of abort

2. **Scoped verification** (`src/engine/verify.rs`):
   - Added `verify_branches()` function for verifying only specific branches
   - Executor now uses scoped verification to only check branches touched by the plan
   - Prevents false failures when untouched branches have stale base metadata

**Key Patterns Established:**

1. Command struct holds args, not context references
2. `plan()` uses `ctx.snapshot` from `ReadyContext` - no direct `scan()` calls
3. `run_command_with_scope()` for multi-branch operations
4. Backward compatibility maintained for helper functions used by other commands
5. Use `PlanError` for planning errors, convert from other error types with `map_err()`
