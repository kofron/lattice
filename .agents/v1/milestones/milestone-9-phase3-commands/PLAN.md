# Milestone 9: Phase 3 Advanced Rewriting and Structural Mutation Commands

## Summary

Implement 10 Phase 3 commands that perform sophisticated structural mutations on the branch DAG. These are the "sharp knives" of Lattice - advanced rewriting commands that depend on the solid foundation built in Milestones 0-8.

**Commands:** `modify`, `move`, `rename`, `delete`, `squash`, `fold`, `pop`, `reorder`, `split`, `revert`

**Core principles:** All commands must enforce freeze policy validation, transactional integrity via journal/lock, and CAS ref updates per ARCHITECTURE.md.

---

## Cross-Cutting Requirements

All Phase 3 commands **MUST** enforce:

1. **Freeze Policy Validation**: Check current branch, descendants being rebased, ancestors impacted by fold/delete. Block if any affected branch is frozen.

2. **Transactional Integrity**: Journal before/after OIDs for all ref changes. Update metadata only after branch refs succeed. Pause safely on conflicts.

3. **State Management**: Use executor's plan/journal/lock system. CAS ref updates for branch + metadata refs.

---

## Implementation Steps

### Step 1: Create Shared Helper Module (`src/cli/commands/phase3_helpers.rs`)

Reusable functions for Phase 3 commands:

- `check_freeze_affected_set()` - Check freeze policy for affected branches
- `check_freeze()` - Check freeze for single branch
- `rebase_onto_with_journal()` - Git rebase with journal integration and conflict handling
- `reparent_children()` - Update parent pointers for children of deleted/folded branch
- `get_net_diff()` - Get net diff between two commits
- `get_commits_in_range()` - List commits in range
- `count_commits_in_range()` - Count commits in range
- `is_descendant_of()` - Check if one branch is descendant of another
- `is_working_tree_clean()` - Check for clean working tree

### Step 2-11: Implement Commands

Each command follows the standard pattern:
1. Check for in-progress operations
2. Scan repository
3. Validate inputs and freeze policy
4. Acquire lock
5. Create journal
6. Execute git operations
7. Update metadata with CAS
8. Commit journal
9. Clear op-state

### Step 12-13: CLI Integration

- Add 10 Command variants to `src/cli/args.rs`
- Update dispatch in `src/cli/commands/mod.rs`

### Step 14: Integration Tests

24 tests covering:
- Happy path for each command
- Freeze policy enforcement
- Edge cases (cycles, existing names, etc.)

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/cli/args.rs` | Modify | Add 10 Command variants |
| `src/cli/commands/mod.rs` | Modify | Export modules, update dispatch |
| `src/cli/commands/phase3_helpers.rs` | NEW | Shared helpers |
| `src/cli/commands/modify.rs` | NEW | modify command |
| `src/cli/commands/move_cmd.rs` | NEW | move command |
| `src/cli/commands/rename.rs` | NEW | rename command |
| `src/cli/commands/delete.rs` | NEW | delete command |
| `src/cli/commands/squash.rs` | NEW | squash command |
| `src/cli/commands/fold.rs` | NEW | fold command |
| `src/cli/commands/pop.rs` | NEW | pop command |
| `src/cli/commands/reorder.rs` | NEW | reorder command |
| `src/cli/commands/split.rs` | NEW | split command |
| `src/cli/commands/revert.rs` | NEW | revert command |
| `tests/phase3_commands.rs` | NEW | Integration tests |

---

## Test Requirements

| Command | Happy Path | Freeze | Other |
|---------|------------|--------|-------|
| modify | amend, create | Yes | - |
| move | reparent | Yes | cycle prevention |
| rename | ref updates | Yes | parent fixes |
| delete | reparent children | Yes | upstack/downstack |
| squash | collapse | Yes | single no-op |
| fold | merge, reparent | - | --keep |
| pop | uncommitted | - | reparent |
| reorder | needs multiple | - | - |
| split | by-commit | Yes | requires mode |
| revert | branch creation | - | invalid sha |

---

## Acceptance Gates

- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes
- [x] `cargo doc --no-deps` succeeds
- [x] All 10 Phase 3 commands implemented
- [x] Each command has happy path + freeze test (where applicable)
- [x] No mutations to frozen branches
- [x] Journal/OpState integration works
- [x] Milestone documentation complete
