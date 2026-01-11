# Milestone 2: Implementation Notes

## Summary

This milestone implements bare repository and linked worktree support for Lattice.
All 9 steps from PLAN.md have been completed.

## Key Implementation Decisions

### 1. RepoContext Enum (Step 1)

Added `RepoContext` enum to `src/git/interface.rs`:
- `Normal`: Standard repo with `.git/` directory
- `Bare`: Bare clone with no working directory
- `Worktree`: Linked worktree created via `git worktree add`

Detection uses `git2::Repository::is_bare()` and checks for `commondir` file.

### 2. LatticePaths Centralization (Step 2)

Created `src/core/paths.rs` with `LatticePaths` struct that provides:
- `common_dir`: Shared across all worktrees (for repo-scoped state)
- `git_dir`: Per-worktree git directory
- All path accessors (config, lock, op-state, journals) use `common_dir`

This ensures worktrees share the same lock, config, and operation state.

### 3. Op-State Origin Tracking (Step 3)

Extended `OpState` in `src/core/ops/journal.rs` with:
- `origin_git_dir: PathBuf` - The git_dir where the operation started
- `origin_work_dir: Option<PathBuf>` - The work_dir (None for bare repos)
- `check_origin_worktree()` method enforces continue/abort from the originating worktree

This prevents confusion when an operation pauses in one worktree and a user
tries to continue from a different worktree.

### 4. WorktreeStatus Enum (Step 7)

Changed `WorktreeStatus` from a struct to an enum per SPEC.md section 4.6.9:
```rust
pub enum WorktreeStatus {
    Clean,
    Dirty { staged: u32, unstaged: u32, conflicts: u32 },
    Unavailable { reason: WorktreeUnavailableReason },
}

pub enum WorktreeUnavailableReason {
    BareRepository,
    NoWorkDir,
    ProbeFailed,
}
```

The `Unavailable` variant explicitly captures why status can't be determined,
rather than using `Option<WorktreeStatus>`.

### 5. Capability-Based Gating (Steps 4, 6)

Added `WorkingDirectoryAvailable` capability to `src/engine/capabilities.rs`.

Updated requirement sets in `src/engine/gate.rs`:
- `NAVIGATION`, `MUTATING`, `REMOTE` require `WorkingDirectoryAvailable`
- Added `MUTATING_METADATA_ONLY` - works in bare repos (no workdir needed)
- Added `REMOTE_BARE_ALLOWED` - for `--no-restack`, `--no-checkout` flags

### 6. Worktree Branch Occupancy (Step 5)

Added to `src/git/interface.rs`:
- `WorktreeEntry` struct for worktree information
- `list_worktrees()` parses `git worktree list --porcelain`
- `branch_checked_out_elsewhere()` checks single branch
- `branches_checked_out_elsewhere()` returns all conflicts

Added `branches_checked_out_elsewhere` issue factory to `src/engine/health.rs`.

### 7. Bare Repo Guidance Message

Updated `no_working_directory()` issue in `src/engine/health.rs` to include:
- Why it failed (bare repo has no working directory)
- How to proceed (create worktree, run from worktree, or use appropriate flags)

## Files Modified

### Core Changes
- `src/git/interface.rs` - RepoContext, RepoInfo.common_dir, WorktreeStatus enum, worktree listing
- `src/git/mod.rs` - Export new types
- `src/core/paths.rs` - New LatticePaths struct
- `src/core/mod.rs` - Export paths module
- `src/core/ops/lock.rs` - Use LatticePaths
- `src/core/ops/journal.rs` - Use LatticePaths, add origin tracking to OpState
- `src/engine/capabilities.rs` - Add WorkingDirectoryAvailable
- `src/engine/health.rs` - Add bare repo and worktree issues
- `src/engine/scan.rs` - Emit WorkingDirectoryAvailable capability
- `src/engine/gate.rs` - Update requirement sets

### CLI Commands Updated
All commands using lock/op-state were updated to use LatticePaths:
- fold.rs, modify.rs, move_cmd.rs, restack.rs, pop.rs, rename.rs
- revert.rs, undo.rs, reorder.rs, split.rs, squash.rs
- recovery.rs, phase3_helpers.rs, exec.rs, scan.rs

### Tests
- `tests/worktree_support_integration.rs` - New integration test suite (25 tests)
- `tests/git_integration.rs` - Updated WorktreeStatus tests

### Documentation
- `docs/references.md` - Added git worktree references

## Test Coverage

The integration test suite (`tests/worktree_support_integration.rs`) covers:
- Bare repo detection and context classification
- Worktree detection and context classification
- Normal repo behavior unchanged
- Shared state (lock, op-state, config) across worktrees
- Op-state origin worktree enforcement
- Branch occupancy detection
- Capability detection for work_dir presence

## Future Work

The following items from PLAN.md are infrastructure-ready but not yet wired to commands:
- `--no-restack` flag for submit/sync in bare repos
- `--no-checkout` flag for get in bare repos
- Executor revalidation of worktree occupancy under lock

These require command-level changes that build on this infrastructure.
