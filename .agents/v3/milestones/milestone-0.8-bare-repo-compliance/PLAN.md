# Milestone 0.8: Bare Repo Mode Compliance

## Status: VERIFICATION NEEDED (Implementation Appears Complete)

---

## Overview

**Goal:** Ensure `lattice submit`, `sync`, and `get` commands comply with SPEC.md §4.6.7 bare repository policy.

**Discovery:** Code exploration reveals that **all deliverables are already implemented**. This milestone should focus on verification, testing, and documentation rather than new implementation.

**Priority:** MEDIUM

**Spec Reference:** SPEC.md Section 4.6.7 "Bare repo policy for submit/sync/get"

---

## Problem Statement (from ROADMAP.md)

The SPEC has **very explicit bare-repo policy** that must be enforced:

- `submit`: refuse unless `--no-restack`, then enforce ancestry alignment + metadata normalization
- `sync`: refuse unless `--no-restack`
- `get`: refuse unless `--no-checkout`, then fetch + track + compute base + print worktree guidance

---

## Current Implementation Status

### 1. `submit --no-restack` in Bare Repos ✅ IMPLEMENTED

**File:** `src/cli/commands/submit.rs`

| Requirement | Implementation | Lines |
|-------------|----------------|-------|
| Refuse without `--no-restack` | `is_bare && !opts.no_restack` check | 267-276 |
| Check ancestry alignment | `check_submit_alignment()` | 565-620 |
| Normalize base metadata | `normalize_base_metadata()` | 627-652 |
| Orchestration | `check_and_normalize_alignment()` | 662-697 |
| Error message | "Restack required. Run from a worktree." | 689-697 |
| Success message | "Updated base metadata for N branches" | 678-682 |

**Gating:** Uses `requirements::REMOTE_BARE_ALLOWED` when `--no-restack` (line 243)

### 2. `sync --no-restack` in Bare Repos ✅ IMPLEMENTED

**File:** `src/cli/commands/sync.rs`

| Requirement | Implementation | Lines |
|-------------|----------------|-------|
| Refuse with restack in bare repo | `is_bare && restack` check | 84-91 |
| Use bare-allowed requirements | `REMOTE_BARE_ALLOWED` when not restacking | 52-56 |
| Fetch from remote | `git fetch origin` | 99-106 |
| Trunk fast-forward | Ancestry check + merge | 110-155 |
| PR state checks | `forge.get_pr()` loop | 160-200 |

**Gating:** Uses `requirements::REMOTE_BARE_ALLOWED` when not restacking (line 55)

### 3. `get --no-checkout` in Bare Repos ✅ IMPLEMENTED

**File:** `src/cli/commands/get.rs`

| Requirement | Implementation | Lines |
|-------------|----------------|-------|
| Refuse without `--no-checkout` | `is_bare && !no_checkout` check | 95-108 |
| Fetch branch ref | Already done before mode check | 150-170 |
| Track with parent inference | `determine_parent()` from PR or trunk | 344-355 |
| Compute base via merge-base | `git.merge_base()` | 278-283 |
| Default frozen | `FreezeState::Frozen` unless `--unfrozen` | 293-301 |
| Worktree guidance | Print message with `git worktree add` example | 319-324 |

**Gating:** Uses `requirements::REMOTE_BARE_ALLOWED` when `--no-checkout` (line 68)

---

## Existing Infrastructure

### Mode Types (from Milestone 0.1)

**File:** `src/engine/modes.rs`

```rust
pub enum SubmitMode { WithRestack, NoRestack }
pub enum SyncMode { WithRestack, NoRestack }
pub enum GetMode { WithCheckout, NoCheckout }
```

Each mode type has a `resolve()` method that returns `ModeError::BareRepoRequiresFlag` when bare repo detected without required flag.

### Requirement Sets

**File:** `src/engine/gate.rs`

| Set | WorkingDirectoryAvailable? | Used By |
|-----|---------------------------|---------|
| `REMOTE` | Yes (required) | submit, sync, get (default) |
| `REMOTE_BARE_ALLOWED` | No (not required) | submit --no-restack, sync --no-restack, get --no-checkout |

### WorkingDirectoryAvailable Capability

**File:** `src/engine/scan.rs` (lines 240-248)

```rust
if info.work_dir.is_some() {
    health.add_capability(Capability::WorkingDirectoryAvailable);
} else {
    health.add_issue(issues::no_working_directory());
}
```

---

## Verification Plan

Since implementation appears complete, this milestone focuses on **verification and testing**.

### Phase 1: Code Review Verification

Review each implementation against SPEC.md §4.6.7 requirements:

1. **submit.rs:**
   - [ ] Verify `check_submit_alignment()` correctly checks `is_ancestor(parent_tip, branch_tip)`
   - [ ] Verify `normalize_base_metadata()` updates `base.oid` to `parent_tip`
   - [ ] Verify error messages match SPEC requirements
   - [ ] Verify CAS semantics used for metadata writes

2. **sync.rs:**
   - [ ] Verify bare repo check is at correct location (before any mutation)
   - [ ] Verify no rebase/restack code paths can be reached in bare mode
   - [ ] Verify fetch, trunk FF, and PR checks work without working directory

3. **get.rs:**
   - [ ] Verify `handle_no_checkout_mode()` matches SPEC requirements
   - [ ] Verify merge-base computation is correct
   - [ ] Verify default frozen state with correct reason
   - [ ] Verify worktree guidance message is helpful

### Phase 2: Add Missing Integration Tests

**File:** `tests/bare_repo_mode_compliance.rs` (NEW)

Current `tests/worktree_support_integration.rs` has basic bare repo tests but lacks command-specific tests.

#### Required Tests

```rust
// submit tests
#[test]
fn submit_refuses_in_bare_repo_without_no_restack() { ... }

#[test]
fn submit_no_restack_fails_if_not_aligned() { ... }

#[test]
fn submit_no_restack_normalizes_stale_base_metadata() { ... }

#[test]
fn submit_no_restack_succeeds_when_aligned() { ... }

// sync tests
#[test]
fn sync_refuses_in_bare_repo_with_restack() { ... }

#[test]
fn sync_no_restack_performs_fetch_only() { ... }

// get tests
#[test]
fn get_refuses_in_bare_repo_without_no_checkout() { ... }

#[test]
fn get_no_checkout_tracks_with_correct_base() { ... }

#[test]
fn get_no_checkout_defaults_to_frozen() { ... }

#[test]
fn get_no_checkout_prints_worktree_guidance() { ... }
```

### Phase 3: Documentation Verification

1. Verify command documentation in `docs/commands/`:
   - [ ] `submit.md` documents `--no-restack` and bare repo behavior
   - [ ] `sync.md` documents `--no-restack` bare repo behavior  
   - [ ] `get.md` documents `--no-checkout` and bare repo behavior

2. Verify docstrings in command modules match implementation

### Phase 4: Run Full Verification Suite

```bash
cargo check
cargo clippy -- -D warnings
cargo test
cargo test bare_repo
cargo test worktree
cargo fmt --check
```

---

## Critical Files

| File | Purpose |
|------|---------|
| `src/cli/commands/submit.rs` | Submit implementation with alignment checks |
| `src/cli/commands/sync.rs` | Sync implementation with bare repo check |
| `src/cli/commands/get.rs` | Get implementation with no-checkout mode |
| `src/engine/gate.rs` | Requirement sets (REMOTE vs REMOTE_BARE_ALLOWED) |
| `src/engine/modes.rs` | Mode types with resolve() methods |
| `tests/bare_repo_mode_compliance.rs` | NEW: Integration tests |

---

## Acceptance Gates

From ROADMAP.md:

- [x] `submit` refuses in bare repo without `--no-restack` (implemented at submit.rs:267-276)
- [x] `submit --no-restack` enforces ancestry alignment (implemented at submit.rs:565-620)
- [x] `submit --no-restack` normalizes base metadata if aligned (implemented at submit.rs:627-652)
- [x] `sync` refuses in bare repo without `--no-restack` (implemented at sync.rs:84-91)
- [x] `get` refuses in bare repo without `--no-checkout` (implemented at get.rs:95-108)
- [x] `get --no-checkout` tracks branch with correct base (implemented at get.rs:278-283)
- [ ] Integration tests for all bare repo scenarios
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Estimated Work

| Task | Effort |
|------|--------|
| Code review verification | 1 hour |
| Add integration tests | 2-3 hours |
| Documentation verification | 30 min |
| Run verification suite | 30 min |
| Update ROADMAP status | 15 min |
| **Total** | **4-5 hours** |

---

## Dependencies

**Depends on (all COMPLETE):**
- Milestone 0.1 (Gating Integration) - provides mode types and requirement sets
- Milestone 0.4 (OpState Full Payload) - provides operation state tracking

---

## Conclusion

**The core implementation is complete.** The remaining work is:

1. Verify existing code matches SPEC exactly
2. Add comprehensive integration tests for bare repo scenarios
3. Update documentation if needed
4. Mark milestone as COMPLETE in ROADMAP

This is a **verification and testing milestone**, not an implementation milestone.
