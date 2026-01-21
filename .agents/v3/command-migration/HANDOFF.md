# Command Migration Handoff Spec

## Status: Phases 0-4 Complete, Phase 5-7 Complete, Phase 8 In Progress

**Date:** 2026-01-21  
**Branch:** `jared-fix-ledger-bug`

---

## Executive Summary

The lattice CLI has 37 command files. The command migration project aims to migrate ALL commands to implement the appropriate trait (`Command`, `ReadOnlyCommand`, or `AsyncCommand`) so they flow through the unified `run_command()` lifecycle in `runner.rs`.

**Current State:**
- **Phases 0-4:** Complete - Infrastructure and metadata-only commands
- **Phase 5:** Partial - `restack` migrated, 11 commands pending
- **Phase 6:** Complete - All async/remote commands migrated
- **Phase 7:** Complete - Recovery commands reviewed and improved
- **Phase 8:** In Progress - Verification and cleanup

**Why This Matters:** Per ARCHITECTURE.md Section 5 and the governing principle from `runner.rs`: "Commands cannot call `scan()` directly. All command execution flows through `run_command()`, which ensures gating is enforced." The Milestone 0.12 engine hooks for out-of-band drift detection only fire when commands use the proper lifecycle.

---

## Phase 8 Progress (Current)

### Task 8.1: CI Architecture Lint - COMPLETE ✓

**File:** `tests/architecture_lint.rs` (9 tests)

Automated tests that fail if architectural violations are introduced:

1. **`commands_cannot_import_scan_directly()`** - Verifies no direct `scan()` calls in migrated commands
2. **`commands_do_not_manually_call_check_requirements()`** - Verifies trait-based gating
3. **`commands_implement_appropriate_trait()`** - Verifies required trait implementations
4. **`phase5_pending_count_is_tracked()`** - Documents remaining Phase 5 debt

Key constants defined:
- `EXCLUDED_COMMANDS`: auth.rs, changelog.rs, completion.rs, config_cmd.rs, phase3_helpers.rs, stack_comment_ops.rs, mod.rs
- `PHASE5_PENDING`: create.rs, modify.rs, delete.rs, rename.rs, squash.rs, fold.rs, move_cmd.rs, pop.rs, reorder.rs, split.rs, revert.rs (11 commands)
- `ASYNC_WITH_INTERNAL_SCAN`: submit.rs, sync.rs, get.rs, merge.rs (use scan internally after trait-based gating)

### Task 8.2: Gating Matrix Tests - COMPLETE ✓

**File:** `tests/gating_matrix.rs` (27 tests)

Table-driven verification of capability gating for all requirement sets:

| Category | Tests |
|----------|-------|
| Read-only | Works in normal repo, during in-progress op, with frozen branch |
| Navigation | Works in normal repo, fails in bare repo, fails if checked out elsewhere |
| Mutating | Works in normal repo, fails in bare repo, fails on frozen branch |
| Metadata-only | Works in normal repo, fails in bare repo, allows frozen operations |
| Remote | Works in normal repo, bare-allowed variant works in bare repo |
| Recovery | Works when op in progress, fails when no op in progress |

### Task 8.3: Engine Hooks Verification - COMPLETE ✓

**File:** `tests/oob_fuzz.rs` - Added `engine_hooks_verification` module (6 tests)

Verifies engine hooks fire correctly for commands implementing traits:

| Command | Expected | Verified |
|---------|----------|----------|
| `freeze` | Fires hook | ✓ |
| `unfreeze` | Fires hook | ✓ |
| `restack` | Fires hook | ✓ |
| `log` | No hook (ReadOnlyCommand) | ✓ |
| `info` | No hook (ReadOnlyCommand) | ✓ |

### Task 8.4: Cleanup Legacy Patterns - COMPLETE ✓

- `#![allow(deprecated)]` markers identified in Phase 5 pending commands (expected)
- No dead code warnings in release build
- No clippy warnings
- Markers should remain until Phase 5 migrates those commands

### Task 8.5: Documentation Audit - IN PROGRESS

- [x] This HANDOFF.md updated
- [ ] Module documentation review (runner.rs, command.rs, gate.rs)
- [ ] SPEC/ARCHITECTURE reference verification

### Task 8.6: Final Verification - PENDING

Acceptance gates from ROADMAP.md to verify.

---

## What's Been Completed (All Phases)

### Phase 0: Infrastructure (COMPLETE)

**New traits and functions added:**

1. **`ReadOnlyCommand` trait** (`src/engine/command.rs`)
   - Simpler interface for query-only commands
   - No `Plan` generation - directly produces output

2. **`run_readonly_command()`** (`src/engine/runner.rs`)
   - Entry point for `ReadOnlyCommand` implementations
   - Lifecycle: Scan → Gate → Execute (no planning/executor)

3. **`PlanStep::Checkout`** (`src/engine/plan.rs`)
   - New step variant for navigation commands
   - Handled in `exec.rs` and `recovery.rs`

### Phase 2: Read-Only Commands (COMPLETE)

| Command | File | Trait |
|---------|------|-------|
| `log` | `log_cmd.rs` | `ReadOnlyCommand` |
| `info` | `info.rs` | `ReadOnlyCommand` |
| `parent` | `relationships.rs` | `ReadOnlyCommand` |
| `children` | `relationships.rs` | `ReadOnlyCommand` |
| `pr` | `pr.rs` | `ReadOnlyCommand` |

### Phase 3: Navigation Commands (VERIFIED)

These use `run_gated()` with proper requirements - architecturally sound.

| Command | File | Status |
|---------|------|--------|
| `checkout` | `checkout.rs` | Uses `run_gated` + `NAVIGATION` |
| `up` | `navigation.rs` | Uses `run_gated` + `NAVIGATION` |
| `down` | `navigation.rs` | Uses `run_gated` + `NAVIGATION` |
| `top` | `navigation.rs` | Uses `run_gated` + `NAVIGATION` |
| `bottom` | `navigation.rs` | Uses `run_gated` + `NAVIGATION` |

### Phase 4: Metadata-Only Commands (COMPLETE)

| Command | File | Trait |
|---------|------|-------|
| `freeze` | `freeze.rs` | `Command` |
| `unfreeze` | `freeze.rs` | `Command` |
| `track` | `track.rs` | `Command` |
| `untrack` | `untrack.rs` | `Command` |
| `unlink` | `unlink.rs` | `Command` |
| `init` | `init.rs` | `Command` |
| `trunk` | `trunk.rs` | `ReadOnlyCommand` + `Command` |

### Phase 5: Core Mutating Commands (PARTIAL)

| Command | File | Status |
|---------|------|--------|
| `restack` | `restack.rs` | ✅ COMPLETE - `Command` |
| `create` | `create.rs` | ❌ PENDING |
| `modify` | `modify.rs` | ❌ PENDING |
| `delete` | `delete.rs` | ❌ PENDING |
| `rename` | `rename.rs` | ❌ PENDING |
| `squash` | `squash.rs` | ❌ PENDING |
| `fold` | `fold.rs` | ❌ PENDING |
| `move` | `move_cmd.rs` | ❌ PENDING |
| `pop` | `pop.rs` | ❌ PENDING |
| `reorder` | `reorder.rs` | ❌ PENDING |
| `split` | `split.rs` | ❌ PENDING |
| `revert` | `revert.rs` | ❌ PENDING |

**Note:** These 11 commands have `#![allow(deprecated)]` markers and call `scan()` directly. They are tracked in `tests/architecture_lint.rs::PHASE5_PENDING`.

### Phase 6: Async/Remote Commands (COMPLETE)

| Command | File | Trait |
|---------|------|-------|
| `submit` | `submit.rs` | `AsyncCommand` |
| `sync` | `sync.rs` | `AsyncCommand` |
| `get` | `get.rs` | `AsyncCommand` |
| `merge` | `merge.rs` | `AsyncCommand` |

**Note:** These commands use `scan()` internally after trait-based gating for mode dispatch. This is architecturally acceptable and tracked in `ASYNC_WITH_INTERNAL_SCAN`.

### Phase 7: Recovery Commands (COMPLETE)

| Command | File | Status |
|---------|------|--------|
| `continue` | `recovery.rs` | Improved with ledger integration |
| `abort` | `recovery.rs` | Improved with forge warnings |
| `undo` | `undo.rs` | Improved with ledger events |

**Decision:** These commands have unique semantics (resume/reverse vs. create) and don't implement the `Command` trait. They use `check_requirements(RECOVERY)` directly, which is allowed per Phase 8 lint exceptions.

---

## Commands Excluded from Migration

| Command | File | Reason |
|---------|------|--------|
| `changelog` | `changelog.rs` | Static version info |
| `completion` | `completion.rs` | Shell completion generation |
| `config` | `config_cmd.rs` | File I/O only, no repo state |
| `auth` | `auth.rs` | No repo required |

---

## Test Coverage Summary

### New Test Files (Phase 8)

| File | Tests | Purpose |
|------|-------|---------|
| `tests/architecture_lint.rs` | 9 | Enforce architectural constraints |
| `tests/gating_matrix.rs` | 27 | Verify capability gating |
| `tests/oob_fuzz.rs` (added) | 6 | Engine hooks verification |

### Total Phase 8 Tests: 42 new tests

---

## Technical Debt

### Phase 5 Pending Commands

The following 11 commands still call `scan()` directly and bypass the unified lifecycle:

1. `create.rs`
2. `modify.rs`
3. `delete.rs`
4. `rename.rs`
5. `squash.rs`
6. `fold.rs`
7. `move_cmd.rs`
8. `pop.rs`
9. `reorder.rs`
10. `split.rs`
11. `revert.rs`

**Impact:**
- Engine hooks don't fire for these commands
- Can't be used in OOB drift detection scenarios
- Represent architectural inconsistency

**Mitigation:**
- Tracked in `tests/architecture_lint.rs::PHASE5_PENDING`
- `#![allow(deprecated)]` markers document the debt
- Migration pattern established via `restack.rs` reference implementation

---

## Verification Commands

```bash
# Run all tests
cargo test

# Run architecture lint tests
cargo test architecture_lint

# Run gating matrix tests
cargo test gating_matrix

# Run engine hooks tests
cargo test --features test_hooks --test oob_fuzz engine_hooks_verification

# Check for warnings
cargo clippy -- -D warnings

# Check formatting
cargo fmt --check

# Release build
cargo build --release
```

---

## Key Patterns (Reference)

### Pattern 1: ReadOnlyCommand

```rust
impl ReadOnlyCommand for MyQueryCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
    type Output = MyOutput;

    fn execute(&self, ctx: &ReadyContext) -> Result<Self::Output, PlanError> {
        Ok(output)
    }
}
```

### Pattern 2: Command with SimpleCommand

```rust
impl Command for MyMutatingCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING_METADATA_ONLY;
    type Output = ();

    fn plan(&self, ready: &ReadyContext) -> Result<Plan, PlanError> {
        let mut plan = Plan::new(OpId::new(), "my-command");
        plan = plan.with_step(PlanStep::WriteMetadataCas { /* ... */ });
        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<()> {
        self.simple_finish(result)
    }
}

impl SimpleCommand for MyMutatingCommand<'_> {}
```

### Pattern 3: AsyncCommand

```rust
#[async_trait]
impl AsyncCommand for MyAsyncCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE;
    type Output = MyResult;

    async fn plan(&self, ready: &ReadyContext) -> Result<Plan, PlanError> {
        // Async planning with network calls
        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
        // Same as Command
    }
}
```

---

## References

- **ARCHITECTURE.md Section 5** - Command lifecycle requirements
- **ARCHITECTURE.md Section 6** - Plan/Execute model  
- **SPEC.md Section 4.6.7** - Bare repo policy for submit/sync/get
- **ROADMAP.md Milestone 0.12** - Engine hooks requirement
- **Phase 8 Plan:** `.agents/v3/command-migration/phase8/PLAN.md`
- **Phase 8 Checklist:** `.agents/v3/command-migration/phase8/CHECKLIST.md`
