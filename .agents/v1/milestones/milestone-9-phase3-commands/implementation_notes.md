# Milestone 9 Implementation Notes

## Overview

Implemented 10 Phase 3 "sharp knife" commands for advanced branch manipulation. These commands enable sophisticated structural mutations on the branch DAG while maintaining transactional integrity.

## Commands Implemented

### modify
Amends the current commit or creates the first commit on an empty branch. Automatically restacks descendants after modification.

**Key decisions:**
- Uses `git commit --amend` for existing commits
- Uses `git commit` (without --amend) when `-c` flag creates first commit
- Integrates with restack logic for descendant updates

### move
Reparents a branch onto a different parent with cycle detection.

**Key decisions:**
- Uses `is_descendant_of()` helper to prevent cycles
- Performs `git rebase --onto` to move commits
- Updates metadata parent pointer and base OID

### rename
Renames the current branch and updates all references.

**Key decisions:**
- Creates new branch ref, creates new metadata, updates children's parent pointers, then deletes old refs
- Uses transaction pattern to ensure atomicity

### delete
Deletes a branch with optional `--upstack` or `--downstack` scope.

**Key decisions:**
- Default behavior reparents children to the deleted branch's parent
- `--upstack` deletes all descendants
- `--downstack` deletes all ancestors up to trunk
- Requires `--force` to confirm deletion

### squash
Collapses all commits on current branch into a single commit.

**Key decisions:**
- Uses `git reset --soft <base>` followed by `git commit`
- Preserves all changes in a single commit
- Restacks descendants after squash
- Single-commit branches are a no-op

### fold
Merges current branch into its parent and deletes.

**Key decisions:**
- Applies branch diff to parent as a new commit
- Reparents children to the parent branch
- `--keep` flag renames parent to current branch's name

### pop
Deletes current branch but keeps changes as uncommitted.

**Key decisions:**
- Requires clean working tree (no staged/unstaged changes)
- Extracts diff and applies to parent without committing
- Reparents children to parent

### reorder
Editor-driven reordering of branch stack.

**Key decisions:**
- Uses `$EDITOR` / `$VISUAL` / `vi` fallback
- Validates edited list (same branches, no duplicates)
- Requires at least 2 branches in stack to reorder

### split
Splits branch by commit or by file.

**Key decisions:**
- `--by-commit`: Creates chain of branches, one per commit
- `--by-file`: Extracts specified file changes into new branch
- `--by-hunk`: Returns "not implemented" per SPEC.md v2 deferral
- Detaches HEAD before force-updating current branch

### revert
Creates a revert branch off trunk.

**Key decisions:**
- Creates branch named `revert-<short-sha>`
- Uses `git revert` for the actual revert
- Tracks new branch with trunk as parent

## Shared Helper Module

Created `phase3_helpers.rs` with reusable functions:

- **`check_freeze_affected_set()`**: Validates freeze policy for a set of branches
- **`check_freeze()`**: Single-branch freeze check
- **`rebase_onto_with_journal()`**: Rebase with journal integration and conflict handling
- **`reparent_children()`**: Updates parent pointers for all children of a branch
- **`get_net_diff()`**: Gets diff between two commits
- **`get_commits_in_range()`**: Lists commits in range (oldest first)
- **`count_commits_in_range()`**: Counts commits in range
- **`is_descendant_of()`**: Checks if one branch descends from another
- **`is_working_tree_clean()`**: Checks for clean working tree

## Bug Fixes During Implementation

### squash: Missing git commit execution
**Problem**: When `-m <message>` was passed, the code built `commit_args` but never executed the git command.
**Fix**: Added explicit `Command::new("git").args(["commit", "-m", msg])` execution in the `if let Some(msg) = message` branch.

### split: Cannot force-update current branch
**Problem**: `git branch -f` fails when the branch is currently checked out.
**Fix**: Added `git checkout --detach` before the loop that creates branches, then checkout back to the final branch at the end.

### Tests: Interactive prompts in non-TTY
**Problem**: `lattice track` prompts for parent selection, failing in tests.
**Fix**: Changed tests to use `lattice track --force` to auto-select nearest tracked ancestor.

## Test Coverage

Created `tests/phase3_commands.rs` with 24 integration tests:

- **modify**: 3 tests (amend, create first commit, frozen fails)
- **move**: 2 tests (reparent, cycle prevention)
- **rename**: 3 tests (ref updates, parent fixes, existing name fails)
- **delete**: 3 tests (reparent children, upstack, frozen fails)
- **squash**: 2 tests (collapse commits, single no-op)
- **fold**: 2 tests (merge into parent, reparent children)
- **pop**: 2 tests (uncommitted changes, reparent children)
- **reorder**: 1 test (needs multiple branches)
- **split**: 2 tests (by-commit chain, requires mode)
- **revert**: 2 tests (creates branch, invalid sha fails)
- **Cross-cutting freeze**: 2 tests (move frozen, squash frozen)

## Patterns Used

All commands follow the established pattern from existing commands:

1. Check for in-progress operations (OpState)
2. Scan repository for current state
3. Validate inputs and check freeze policy
4. Acquire repository lock
5. Create journal for transactional integrity
6. Write OpState
7. Execute git operations with journal recording
8. Update metadata using CAS (write_cas/delete_cas)
9. Commit and write journal
10. Remove OpState

## Files Modified/Created

**New files (12):**
- `src/cli/commands/phase3_helpers.rs`
- `src/cli/commands/modify.rs`
- `src/cli/commands/move_cmd.rs`
- `src/cli/commands/rename.rs`
- `src/cli/commands/delete.rs`
- `src/cli/commands/squash.rs`
- `src/cli/commands/fold.rs`
- `src/cli/commands/pop.rs`
- `src/cli/commands/reorder.rs`
- `src/cli/commands/split.rs`
- `src/cli/commands/revert.rs`
- `tests/phase3_commands.rs`

**Modified files (2):**
- `src/cli/args.rs` - Added 10 Command enum variants
- `src/cli/commands/mod.rs` - Added module exports and dispatch cases
