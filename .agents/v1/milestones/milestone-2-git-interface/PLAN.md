# Milestone 2: Single Git Interface

## Status: COMPLETE

**Completed:** 2026-01-07

---

## Overview

**Goal:** Establish the Git interface as the single doorway to all repository operations. Per ARCHITECTURE.md Section 10.1, all Git interactions must flow through a single centralized interface that provides structured results and normalizes errors into typed failure categories.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs.

**Dependencies:** Milestone 0 (complete), Milestone 1 (complete)

---

## Acceptance Gates - ALL PASSED

### Functional Gates
- [x] `Git::open()` discovers repositories correctly (including from subdirectories)
- [x] `Git::resolve_ref()` returns typed `Oid` (not raw String)
- [x] `Git::update_ref_cas()` implements compare-and-swap semantics
- [x] `Git::delete_ref_cas()` implements compare-and-swap delete
- [x] `Git::merge_base()` finds common ancestor
- [x] `Git::is_ancestor()` correctly checks ancestry
- [x] `Git::list_refs_by_prefix()` enumerates refs matching pattern
- [x] `Git::read_blob()` / `Git::write_blob()` work with metadata content
- [x] `Git::state()` detects all in-progress operation types
- [x] `Git::is_worktree_clean()` accurately reports working tree status

### Quality Gates
- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes (165 tests total: 105 unit + 12 property + 48 integration)
- [x] `cargo doc --no-deps` succeeds
- [x] Integration tests use real git repositories (via tempfile)
- [x] Public functions have doctests (with `ignore` for those requiring repos)

### Architectural Gates
- [x] No other module imports `git2` directly (enforced by module structure)
- [x] All ref operations use CAS semantics where mutation occurs
- [x] Error types cover all categories from ROADMAP.md Section 2.1

---

## Implementation Summary

### GitError Enhanced (Step 1)

Full typed error categories implemented:
- `NotARepo { path }` - Repository discovery failed
- `BareRepo` - Bare repositories not supported
- `RefNotFound { refname }` - Ref doesn't exist
- `CasFailed { refname, expected, actual }` - CAS precondition failed
- `OperationInProgress { operation }` - Rebase/merge/etc in progress
- `DirtyWorktree { details }` - Working tree has changes
- `ObjectNotFound { oid }` - Git object not found
- `InvalidOid { oid }` - Invalid OID format
- `InvalidRefName { message }` - Invalid ref name
- `InvalidUtf8 { oid }` - Blob not valid UTF-8
- `AccessError { message }` - Permission/filesystem error
- `Internal { message }` - Internal git2 error

### Strong Types (Step 2)

All operations now return/accept domain types:
- `resolve_ref()` returns `Oid`
- `try_resolve_ref()` returns `Option<Oid>`
- `head_oid()` returns `Oid`
- `list_branches()` returns `Vec<BranchName>`
- `current_branch()` returns `Option<BranchName>`
- `write_blob()` returns `Oid`

### CAS Operations (Step 3)

Compare-and-swap semantics for all ref mutations:
- `update_ref_cas(refname, new_oid, expected_old, message)` - Create or update
- `delete_ref_cas(refname, expected_old)` - Delete with verification

### Ref Enumeration (Step 4)

- `RefEntry` struct with `name: RefName` and `oid: Oid`
- `list_refs_by_prefix(prefix)` - List refs matching pattern
- `list_branches()` - List local branches as `BranchName`
- `list_metadata_refs()` - List metadata refs as `(BranchName, Oid)` pairs

### Ancestry Queries (Step 5)

- `merge_base(oid1, oid2)` - Find common ancestor
- `is_ancestor(ancestor, descendant)` - Check ancestry (including self)
- `commit_count(base, tip)` - Count commits between two OIDs

### Blob Operations (Step 6)

- `write_blob(content)` returns `Oid`
- `read_blob(oid)` returns `Vec<u8>`
- `read_blob_as_string(oid)` returns `String` with UTF-8 validation

### State Detection (Step 7)

Enhanced `GitState` enum:
- `Clean` - No operation in progress
- `Rebase { current, total }` - With progress tracking
- `Merge`, `CherryPick`, `Revert`, `Bisect`, `ApplyMailbox`

Methods:
- `is_in_progress()` - Check if operation in progress
- `description()` - Human-readable state name
- `has_conflicts()` - Check for unresolved conflicts

### Working Tree Status (Step 8)

`WorktreeStatus` struct:
- `staged` - Count of staged changes
- `unstaged` - Count of unstaged changes
- `untracked` - Count of untracked files
- `has_conflicts` - Whether conflicts exist

Methods:
- `worktree_status(include_untracked)` - Get full status
- `is_worktree_clean()` - Quick check for clean state

### Remote Operations (Step 9)

- `remote_url(name)` - Get URL for remote
- `default_remote()` - Get default remote (prefers "origin")
- `parse_github_remote(url)` - Extract owner/repo from GitHub URLs

### Commit Information (Step 10)

`CommitInfo` struct:
- `oid`, `summary`, `message`
- `author_name`, `author_email`, `author_time`

Methods:
- `commit_info(oid)` - Get commit details
- `commit_parents(oid)` - Get parent OIDs

### Integration Tests (Step 11)

Created `tests/git_integration.rs` with 48 tests covering:
- Repository opening (from subdirectories, non-repo detection)
- Ref resolution (HEAD, branches, non-existent)
- CAS operations (create, update, delete with preconditions)
- Ref enumeration (prefix matching, metadata refs)
- Ancestry queries (is_ancestor, merge_base, commit_count)
- Blob operations (write/read roundtrip, UTF-8)
- State detection (clean state, conflicts)
- Working tree status (staged, unstaged, untracked)
- Remote operations (URL lookup, origin preference)
- Commit information (info, parents)

---

## Files Changed

| File | Action | Description |
|------|--------|-------------|
| `src/git/interface.rs` | Major modification | Full implementation (~1200 lines) |
| `src/git/mod.rs` | Minor modification | Updated exports and documentation |
| `tests/git_integration.rs` | Created | 48 integration tests (~600 lines) |

---

## Test Counts

| Category | Count |
|----------|-------|
| Unit tests (lib) | 105 |
| Property tests | 12 |
| Integration tests | 48 |
| Doc tests | 20 (15 ignored - require repos) |
| **Total** | **165 passing** |

---

## Implementation Notes

### CAS Semantics

The compare-and-swap pattern is critical for correctness:
1. Read current ref value via `try_resolve_ref_raw()`
2. Compare against expected value
3. Only perform update if precondition satisfied
4. Return `CasFailed` error on mismatch

This prevents race conditions and ensures the executor never applies changes to a repository that has changed since planning.

### Strong Types at the Boundary

All public methods return domain types (`Oid`, `BranchName`, `RefName`) rather than raw strings. This:
- Catches invalid values at the Git boundary
- Makes function signatures self-documenting
- Enables compile-time prevention of type confusion

### Module Isolation

The `git2` crate is only imported in `src/git/interface.rs`. All other modules interact with Git through our typed interface. If another module needs git2 types directly, that indicates missing functionality in the Git interface.

### macOS Path Handling

Integration tests use `canonicalize()` to handle macOS `/var` -> `/private/var` symlink resolution when comparing paths.

---

## Next Steps (Milestone 3)

Per ROADMAP.md, proceed to **Milestone 3: Persistence Layer**:
- Metadata store with CAS operations (using Git interface)
- Secret store implementations (FileSecretStore, KeychainSecretStore)
- Repo lock mechanism (.git/lattice/lock)
