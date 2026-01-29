# Milestone 0.10: Direct .git File Access Fix

## Status: COMPLETE

---

## Overview

**Goal:** Eliminate direct `.git` file reads that violate the single Git interface principle, replacing them with proper git2 API calls.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Reuse, Purity, No stubs, Tests are everything.

**Priority:** MEDIUM - Violates single-interface principle

**Spec Reference:** 
- ARCHITECTURE.md Section 10.1 "Single Git interface"

---

## Problem Statement

Per ARCHITECTURE.md Section 10.1:

> "All Git interactions are performed through a single Git interface component... Direct parsing of `.git` internal files outside this interface is prohibited."

Two methods in `src/git/interface.rs` violate this principle by reading `.git` internal files directly:

### 1. `read_rebase_progress()` (lines 738-767)

```rust
fn read_rebase_progress(&self) -> (Option<usize>, Option<usize>) {
    let git_dir = self.repo.path();

    // Try rebase-merge first (interactive rebase)
    let rebase_merge = git_dir.join("rebase-merge");
    if rebase_merge.exists() {
        let current = std::fs::read_to_string(rebase_merge.join("msgnum"))...
        let total = std::fs::read_to_string(rebase_merge.join("end"))...
        return (current, total);
    }

    // Try rebase-apply (non-interactive rebase)
    let rebase_apply = git_dir.join("rebase-apply");
    if rebase_apply.exists() {
        let current = std::fs::read_to_string(rebase_apply.join("next"))...
        let total = std::fs::read_to_string(rebase_apply.join("last"))...
        return (current, total);
    }

    (None, None)
}
```

**Problems:**
- Directly reads `.git/rebase-merge/msgnum`, `.git/rebase-merge/end`
- Directly reads `.git/rebase-apply/next`, `.git/rebase-apply/last`
- May break in linked worktrees where these paths differ
- Bypasses git2's type-safe abstraction layer

### 2. `read_fetch_head()` (lines 1949-1970)

```rust
fn read_fetch_head(&self) -> Result<Oid, GitError> {
    let fetch_head_path = self.repo.path().join("FETCH_HEAD");
    let content = std::fs::read_to_string(&fetch_head_path)...
    // Parse manually...
}
```

**Problems:**
- Directly reads `.git/FETCH_HEAD` file
- FETCH_HEAD location differs in linked worktrees (per-worktree file)
- Manual parsing of Git's internal file format

---

## Design Decisions

### Q1: How to replace `read_rebase_progress()`?

**Decision:** Use git2's `Rebase` API

**Rationale:**
- git2 exposes `Repository::open_rebase()` to open an existing rebase
- `Rebase::operation_current()` returns the current operation index (0-based)
- `Rebase::len()` returns total number of operations
- This is type-safe, context-aware, and works across all repo types

**Implementation:**
```rust
fn read_rebase_progress(&self) -> (Option<usize>, Option<usize>) {
    // Only attempt if we're in a rebase state
    let state = self.repo.state();
    if !matches!(state, 
        git2::RepositoryState::Rebase | 
        git2::RepositoryState::RebaseInteractive | 
        git2::RepositoryState::RebaseMerge
    ) {
        return (None, None);
    }
    
    // Open the existing rebase to query progress
    match self.repo.open_rebase(None) {
        Ok(rebase) => {
            let total = rebase.len();
            // operation_current returns Option<usize>, 0-indexed
            // Convert to 1-indexed for display (matches git behavior)
            let current = rebase.operation_current().map(|i| i + 1);
            (current, Some(total))
        }
        Err(_) => (None, None),
    }
}
```

### Q2: How to replace `read_fetch_head()`?

**Decision:** Use git2's `fetchhead_foreach()` API

**Rationale:**
- git2 provides `Repository::fetchhead_foreach()` for iterating FETCH_HEAD entries
- Type-safe callback receives: ref name, remote URL, target OID, merge flag
- No manual file parsing needed
- Handles all repository contexts correctly

**Implementation:**
```rust
fn read_fetch_head(&self) -> Result<Oid, GitError> {
    let mut result_oid: Option<Oid> = None;
    
    self.repo.fetchhead_foreach(|_refname, _remote_url, oid, is_merge| {
        // Take the first entry marked for merge, or first entry overall
        if result_oid.is_none() || is_merge {
            result_oid = Oid::new(oid.to_string()).ok();
        }
        // Return true to continue iteration (or false to stop)
        result_oid.is_none() || !is_merge
    }).map_err(|e| GitError::Internal {
        message: format!("failed to read FETCH_HEAD: {}", e.message()),
    })?;
    
    result_oid.ok_or_else(|| GitError::RefNotFound {
        refname: "FETCH_HEAD".to_string(),
    })
}
```

### Q3: Why git2 over CLI?

**Decision:** Use git2 exclusively (no CLI fallback)

**Rationale:**
1. **Consistency:** The codebase already uses git2 as the primary Git interface
2. **Performance:** No subprocess spawning overhead
3. **Type safety:** git2 provides structured data, not string parsing
4. **Context handling:** git2 methods are worktree-aware internally
5. **Simplicity:** "Everything should be made as simple as possible, but no simpler"

### Q4: Do these changes work in linked worktrees?

**Decision:** Yes, git2 APIs handle worktrees correctly.

**Rationale:**
- git2 `Repository` object is context-aware
- `fetchhead_foreach()` reads from the correct per-worktree FETCH_HEAD
- `open_rebase()` accesses the correct per-worktree rebase state

### Q5: Do these changes work in bare repositories?

**Decision:** Yes with appropriate behavior.

**Rationale:**
- `FETCH_HEAD` exists in bare repos after fetch operations; `fetchhead_foreach()` works
- Rebase is not possible in bare repos; `state()` returns `Clean`, `read_rebase_progress()` returns `(None, None)` correctly

---

## Implementation Plan

### Phase 1: Replace `read_rebase_progress()` with git2 Rebase API

**File:** `src/git/interface.rs`

1. **Modify `read_rebase_progress()` to use git2:**
   ```rust
   /// Read rebase progress using git2's Rebase API.
   ///
   /// Returns (current_step, total_steps) if a rebase is in progress.
   /// Uses git2's type-safe API instead of reading .git internal files.
   fn read_rebase_progress(&self) -> (Option<usize>, Option<usize>) {
       // Only attempt if we're in a rebase state
       let state = self.repo.state();
       if !matches!(state, 
           git2::RepositoryState::Rebase | 
           git2::RepositoryState::RebaseInteractive | 
           git2::RepositoryState::RebaseMerge
       ) {
           return (None, None);
       }
       
       // Open the existing rebase to query progress
       match self.repo.open_rebase(None) {
           Ok(rebase) => {
               let total = rebase.len();
               // operation_current() returns 0-indexed; convert to 1-indexed for display
               let current = rebase.operation_current().map(|i| i + 1);
               (current, Some(total))
           }
           Err(_) => (None, None),
       }
   }
   ```

2. **Verify existing `state()` method still works correctly**

### Phase 2: Replace `read_fetch_head()` with git2 fetchhead_foreach()

**File:** `src/git/interface.rs`

1. **Modify `read_fetch_head()` to use git2:**
   ```rust
   /// Read the OID from FETCH_HEAD after a fetch operation.
   ///
   /// Uses git2's `fetchhead_foreach()` API for type-safe access
   /// that works correctly in all repository contexts including
   /// linked worktrees.
   fn read_fetch_head(&self) -> Result<Oid, GitError> {
       let mut result_oid: Option<Oid> = None;
       
       self.repo.fetchhead_foreach(|_refname, _remote_url, oid, is_merge| {
           // Prefer entries marked for merge; otherwise take first entry
           if result_oid.is_none() || is_merge {
               result_oid = Oid::new(oid.to_string()).ok();
           }
           // Continue iteration until we find a merge entry
           result_oid.is_none() || !is_merge
       }).map_err(|e| GitError::Internal {
           message: format!("failed to read FETCH_HEAD: {}", e.message()),
       })?;
       
       result_oid.ok_or_else(|| GitError::RefNotFound {
           refname: "FETCH_HEAD".to_string(),
       })
   }
   ```

### Phase 3: Add/Update Tests

**Files:** `src/git/interface.rs` (unit tests), `tests/worktree_integration.rs`

1. **Unit tests for new implementations:**
   - `read_fetch_head_returns_oid_after_fetch`
   - `read_fetch_head_errors_when_not_exists`
   - `read_rebase_progress_returns_none_when_clean`
   - `read_rebase_progress_returns_values_during_rebase`

2. **Integration tests for worktree scenarios:**
   - `fetch_head_readable_in_linked_worktree`
   - `rebase_state_detected_in_linked_worktree`

### Phase 4: Verification

1. Run full test suite
2. Verify no direct `.git` file reads remain (grep audit)
3. Manual testing in worktree scenario

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/git/interface.rs` | MODIFY | Replace direct file reads with git2 API calls |
| `tests/worktree_integration.rs` | ADD/MODIFY | Add worktree-specific tests |

---

## Acceptance Gates

From ROADMAP.md:

- [ ] No direct `.git` file reads (grep audit passes)
- [ ] `read_fetch_head()` uses `git2::Repository::fetchhead_foreach()`
- [ ] `read_rebase_progress()` uses `git2::Repository::open_rebase()` + `Rebase` API
- [ ] Works in linked worktrees (integration test)
- [ ] Works in bare repositories (for applicable operations)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Verification Commands

```bash
# Type checks
cargo check

# Lint
cargo clippy -- -D warnings

# All tests
cargo test

# Audit for remaining direct .git file access
grep -rn "\.join(\"rebase\|\.join(\"FETCH\|\.join(\"HEAD\"\|\.join(\"MERGE" src/

# Specific tests
cargo test interface
cargo test worktree

# Format check
cargo fmt --check
```

---

## Dependencies

**Depends on:**
- None - this milestone is independent

**Blocks:**
- None - this is a correctness improvement

---

## Estimated Effort

| Task | Effort |
|------|--------|
| Phase 1: Replace read_rebase_progress | 45 minutes |
| Phase 2: Replace read_fetch_head | 45 minutes |
| Phase 3: Add/update tests | 1 hour |
| Phase 4: Verification | 30 minutes |
| **Total** | **~3 hours** |

---

## Risk Assessment

**Low Risk:**
- git2 APIs are stable and well-documented
- Changes are localized to two methods
- Existing tests will catch regressions
- git2 is already the primary interface in the codebase

**Potential Issues:**
- `open_rebase(None)` may require specific options in edge cases
- Mitigation: Test thoroughly; fall back to `(None, None)` on any error (current behavior)

---

## Test Strategy

### Unit Tests

1. **`read_fetch_head_returns_oid_after_fetch`**
   - Setup: Create repo, add remote, fetch
   - Assert: `read_fetch_head()` returns valid OID

2. **`read_fetch_head_errors_when_not_exists`**
   - Setup: Fresh repo, no fetch performed
   - Assert: Returns `RefNotFound` error

3. **`read_rebase_progress_returns_none_when_clean`**
   - Setup: Clean repo
   - Assert: Returns `(None, None)`

4. **`read_rebase_progress_returns_values_during_rebase`**
   - Setup: Start rebase with multiple commits
   - Assert: Returns `(Some(current), Some(total))` with valid values

5. **`state_detects_rebase_in_progress`**
   - Setup: Start rebase with conflicts
   - Assert: `state()` returns `GitState::Rebase { current: Some(_), total: Some(_) }`

### Integration Tests (Worktree)

6. **`fetch_head_readable_in_linked_worktree`**
   - Create main repo with remote
   - Create linked worktree
   - Fetch from worktree
   - Assert: FETCH_HEAD readable from worktree Git instance

7. **`rebase_state_detected_in_linked_worktree`**
   - Create main repo with commits
   - Create linked worktree
   - Start conflicting rebase in worktree
   - Assert: `state()` returns Rebase with progress from worktree Git instance

---

## Conclusion

This milestone eliminates direct `.git` file access by using git2's proper APIs:

1. **`read_fetch_head()`**: Use `git2::Repository::fetchhead_foreach()` 
2. **`read_rebase_progress()`**: Use `git2::Repository::open_rebase()` with `Rebase::len()` and `Rebase::operation_current()`

Both solutions are:
- **Pure git2** - no CLI subprocess spawning
- **Type-safe** - structured data instead of string parsing
- **Context-aware** - correctly handle linked worktrees and bare repos
- **Compliant** - follow ARCHITECTURE.md Section 10.1's single interface principle
