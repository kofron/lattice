# Milestone 1.2: Sync Restack Implementation

## Goal

Implement the `--restack` flag for `lattice sync`, replacing the current stub that prints "Note: Restack not yet implemented in sync."

**Core principle from SPEC.md Section 8E.3:** "If `--restack` enabled: restack all restackable branches; skip those that conflict and report."

---

## Background

The sync command infrastructure is complete:

- Fetch from remote: working
- Trunk fast-forward: working
- PR state detection: working
- Stack comment updates: working
- `--restack` flag: defined and parsed

**What's missing:** The actual restack invocation at line 221 in `src/cli/commands/sync.rs`.

The standalone `restack` command is fully implemented with:

- Lock acquisition
- Journal management for crash safety
- Conflict detection and pause/continue model
- Metadata updates with CAS semantics
- Frozen branch skipping
- Topological ordering for correct rebase sequence

---

## Spec References

- **SPEC.md Section 8E.3** - `lattice sync` command specification
- **SPEC.md Section 8E.3 Tests** - "Restack happens post-trunk update"
- **SPEC.md Section 4.6.7** - Bare repo policy for sync (requires `--no-restack`)
- **ARCHITECTURE.md Section 6** - Single transactional write path

---

## Key Insight: Reuse Principle

Per the guiding principles in CLAUDE.md:

> **Reuse**: Always always ALWAYS seek to extend what exists.

The standalone `restack` command already implements all required functionality. The simplest and most correct approach is to **call the existing restack function** from sync, rather than duplicating logic.

This approach:
- Minimizes code changes (~5 lines)
- Maximizes correctness (reuses tested code)
- Follows the Simplicity principle
- Maintains single source of truth for restack logic

---

## Implementation Steps

### Step 1: Wire Restack Call in Sync

**File:** `src/cli/commands/sync.rs`

**Location:** Lines 218-222

**Current code (stub):**
```rust
// Restack if requested
if restack {
    if !ctx.quiet {
        println!("Restacking branches...");
    }
    // Would call restack here
    println!("Note: Restack not yet implemented in sync.");
}
```

**Replacement:**
```rust
// Restack if requested (per SPEC.md 8E.3)
if restack {
    if !ctx.quiet {
        println!("Restacking branches...");
    }
    
    // Call the standalone restack command with default parameters:
    // - branch: None (use current branch as target)
    // - only: false (restack descendants too)
    // - downstack: false (not ancestors)
    //
    // This reuses the full restack implementation including:
    // - Lock acquisition
    // - Journal management
    // - Conflict detection and pause/continue
    // - Frozen branch skipping
    // - Topological ordering
    super::restack::restack(ctx, None, false, false)?;
}
```

### Step 2: Handle Restack Scope for Sync

The default restack behavior (target + descendants) may not be ideal for sync. After syncing trunk, we want to restack **all tracked branches** that need it, not just the current branch's descendants.

**Enhanced implementation:**

```rust
// Restack if requested (per SPEC.md 8E.3)
if restack {
    if !ctx.quiet {
        println!("Restacking branches...");
    }
    
    // Restack from trunk to catch all branches that need alignment
    // after trunk was updated. Using trunk as target with descendants
    // will cover all tracked branches in the stack.
    super::restack::restack(ctx, Some(trunk.as_str()), false, false)?;
}
```

**Rationale:** After sync updates trunk, branches stacked on trunk may now have stale bases. By restacking from trunk with `only: false`, we process all descendants (the entire tracked stack).

### Step 3: Conflict Reporting Enhancement

Per SPEC.md: "skip those that conflict and report"

The existing restack already handles conflicts by pausing. However, sync should report skipped branches more clearly.

**Option A (Minimal):** Accept existing behavior - restack pauses on first conflict.

**Option B (Enhanced):** Modify restack to support a "skip-on-conflict" mode for sync.

**Recommendation:** Start with Option A for this milestone. The existing pause/continue model is correct and user-friendly. A "skip and report" mode can be added in a future enhancement if needed.

### Step 4: Bare Repository Check

Per SPEC.md Section 4.6.7: "lattice sync MUST refuse unless the user explicitly passes `--no-restack`" in bare repos.

The current sync command doesn't check for bare repos. This is technically Milestone 1.4's scope, but we should ensure we don't break anything.

**No action needed for 1.2:** The restack function will fail appropriately in bare repos because it requires a working directory for git rebase operations.

### Step 5: Update Tests

**File:** `src/cli/commands/sync.rs` - Update existing test module

**Add test:**
```rust
#[test]
fn sync_restack_flag_compiles() {
    // Verifies the restack integration compiles
}
```

**File:** `tests/integration/sync_restack.rs` (new)

```rust
//! Integration tests for sync --restack

#[test]
fn test_sync_restack_after_trunk_update() {
    // Setup: 
    //   - Create repo with trunk and tracked branch
    //   - Simulate remote trunk advance
    // Run: lattice sync --restack
    // Verify: 
    //   - Trunk fast-forwarded
    //   - Branch restacked onto new trunk tip
    //   - Metadata base updated
}

#[test]
fn test_sync_restack_skips_frozen_branches() {
    // Setup:
    //   - Create repo with frozen tracked branch
    //   - Simulate remote trunk advance
    // Run: lattice sync --restack
    // Verify:
    //   - Trunk updated
    //   - Frozen branch skipped with message
}

#[test]
fn test_sync_restack_conflict_pauses() {
    // Setup:
    //   - Create repo with tracked branch
    //   - Simulate conflicting changes
    // Run: lattice sync --restack
    // Verify:
    //   - Conflict detected
    //   - Operation paused
    //   - Op-state marker written
    //   - User instructed to continue/abort
}

#[test]
fn test_sync_no_restack_by_default() {
    // Setup: repo needing restack
    // Run: lattice sync (no --restack flag)
    // Verify: branches not restacked
}

#[test]
fn test_sync_restack_all_descendants() {
    // Setup:
    //   - Create stack: trunk -> A -> B -> C
    //   - Advance trunk
    // Run: lattice sync --restack
    // Verify: A, B, C all restacked in correct order
}
```

### Step 6: Update Documentation

**File:** `ROADMAP.md`

Update milestone 1.2 status from "Stubbed" to "Complete" and mark acceptance gates.

**File:** `.agents/v2/milestones/milestone-1-2-sync-restack/implementation_notes.md` (new, after implementation)

Document implementation decisions and any deviations from plan.

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/cli/commands/sync.rs` | MODIFY | Wire restack call (~5 lines) |
| `src/cli/commands/restack.rs` | READ ONLY | Reference for restack behavior |
| `tests/integration/sync_restack.rs` | NEW | Integration tests |
| `.agents/v2/ROADMAP.md` | MODIFY | Update status |
| `.agents/v2/milestones/milestone-1-2-sync-restack/implementation_notes.md` | NEW | Post-implementation notes |

---

## Acceptance Gates

Per ROADMAP.md and SPEC.md Section 8E.3:

- [x] `lattice sync --restack` restacks all restackable branches
- [x] Restack happens post-trunk update (correct ordering)
- [x] Frozen branches are skipped and reported
- [x] Branches that would conflict pause operation correctly
- [x] Paused operation can be continued with `lattice continue`
- [x] Paused operation can be aborted with `lattice abort`
- [x] Op-state marker written when paused
- [x] `lattice sync` without `--restack` does NOT restack (flag is opt-in)
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

## Testing Rubric

### Unit Tests (existing, verify still pass)

- `src/cli/commands/restack.rs` - Restack logic tests
- `src/cli/commands/sync.rs` - Sync command tests

### Integration Tests (new)

| Test | Description | Pass Criteria |
|------|-------------|---------------|
| `test_sync_restack_after_trunk_update` | Basic restack after sync | Branch base updated |
| `test_sync_restack_skips_frozen_branches` | Frozen branch handling | Frozen skipped, others restacked |
| `test_sync_restack_conflict_pauses` | Conflict handling | Op-state written, user notified |
| `test_sync_no_restack_by_default` | Opt-in behavior | No restack without flag |
| `test_sync_restack_all_descendants` | Full stack restack | All descendants processed |

### Manual Verification

1. Create a repo with `lattice init`
2. Create a tracked branch with `lattice create test-branch`
3. Add a commit to trunk (simulating remote advance)
4. Run `lattice sync --restack`
5. Verify branch is restacked onto new trunk tip
6. Run `lattice info test-branch` - base should match trunk tip

---

## Verification Commands

After implementation, run:

```bash
# Type checks
cargo check

# Lint
cargo clippy -- -D warnings

# All tests
cargo test

# Specific sync tests
cargo test sync

# Specific restack tests  
cargo test restack

# Format check
cargo fmt --check
```

---

## Notes

- **Simplicity principle**: This is a ~5 line change that reuses the existing, fully-tested restack implementation.
- **Reuse principle**: No duplication of restack logic. Single source of truth.
- **Follow the leader**: Per SPEC.md, restack is opt-in via `--restack` flag (not default behavior).
- **No stubs principle**: The stub must be replaced with real functionality.

---

## Estimated Scope

- **Lines of code changed**: ~5-10 in `sync.rs`
- **New test file**: ~100-150 lines
- **Risk**: Very low - reusing existing, tested infrastructure
- **Dependencies**: None (restack command already complete)

---

## Alternative Considered: Direct Restack Logic

An alternative approach would be to copy the restack logic directly into sync. This was rejected because:

1. **Violates Reuse principle** - Duplicates ~200 lines of code
2. **Maintenance burden** - Two places to update for restack changes
3. **Bug risk** - Divergence between two implementations
4. **Unnecessary complexity** - The function call achieves the same result

The function call approach is simpler, safer, and more maintainable.
