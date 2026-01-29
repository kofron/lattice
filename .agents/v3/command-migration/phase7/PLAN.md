# Phase 7: Recovery Commands Review

## Status: PLANNING

**Started:** 2026-01-21  
**Branch:** `jared-fix-ledger-bug`  
**Prerequisite:** Phase 6 complete (async/remote commands migrated)

---

## Executive Summary

Phase 7 reviews and potentially refactors the recovery commands (`continue`, `abort`, `undo`) to ensure they:

1. Properly validate origin worktree (per SPEC.md §4.6.5)
2. Use controlled entry points with proper gating
3. Integrate correctly with the journal/ledger system
4. Handle the new forge-related `PlanStep` variants from Phase 6

**Key Decision:** Recovery commands have fundamentally different semantics from normal commands—they don't plan new operations, they resume or reverse existing ones. Per HANDOFF.md, these may not need the `Command` trait but must still follow architectural guidelines.

---

## Architecture Context

### Current State Analysis

| Command | File | Current Pattern | Assessment |
|---------|------|-----------------|------------|
| `continue` | `recovery.rs` | `check_requirements(RECOVERY)`, journal-based | Partially compliant |
| `abort` | `recovery.rs` | `check_requirements(RECOVERY)`, journal rollback | Partially compliant |
| `undo` | `undo.rs` | `check_requirements(RECOVERY)`, journal-based | Partially compliant |

### Architectural Requirements (per ARCHITECTURE.md and SPEC.md)

1. **Worktree Origin Validation** (SPEC.md §4.6.5):
   - `continue` and `abort` MUST run from the originating worktree when paused due to Git conflicts
   - If `origin_git_dir != current git_dir`, refuse with clear error and path to correct worktree

2. **Op-State Awareness** (ARCHITECTURE.md §3.3):
   - `continue`/`abort` require active op-state marker
   - `undo` requires no active op-state (operates on committed journals)

3. **Journal Integration** (SPEC.md §4.2.2):
   - `continue`: Resume remaining plan steps from journal
   - `abort`: Roll back using journal ref snapshots
   - `undo`: Reverse last committed operation's ref changes

4. **Gating** (ARCHITECTURE.md §5):
   - Recovery commands use `requirements::RECOVERY` (minimal - just `RepoOpen`)
   - This is intentional: recovery should work even when repo is in degraded state

5. **Forge Step Handling** (Phase 6 integration):
   - Recovery must handle new `PlanStep` variants: `ForgeFetch`, `ForgePush`, etc.
   - API-based forge steps cannot be continued through sync recovery path

---

## Why Recovery Commands Are Different

Recovery commands differ from normal commands in key ways:

| Aspect | Normal Commands | Recovery Commands |
|--------|-----------------|-------------------|
| **Purpose** | Create new plans, execute new operations | Resume/reverse existing operations |
| **Plan Source** | Generated from current snapshot | Loaded from journal |
| **Snapshot** | Fresh scan required | May operate on stale/paused state |
| **Op-State** | Created during execution | Consumed during recovery |
| **Gating** | Full capability requirements | Minimal requirements |
| **Engine Hooks** | Fire before execution | May not apply (already in-progress) |

**Recommendation:** Keep recovery commands as specialized functions but ensure they:
1. Use controlled entry points
2. Validate origin worktree
3. Integrate with ledger for event recording
4. Handle all `PlanStep` variants

---

## Current Implementation Review

### `continue_op()` - Current State

```rust
pub fn continue_op(ctx: &Context, all: bool) -> Result<()> {
    // 1. Open repo, get paths
    // 2. Check requirements (RECOVERY)
    // 3. Load op-state, verify phase is Paused
    // 4. Verify plan schema version compatibility
    // 5. Optionally stage all changes
    // 6. Continue git operation (rebase --continue, etc.)
    // 7. If remaining steps, execute them
    // 8. Complete operation, clear op-state
}
```

**Issues Identified:**
- ✅ Checks requirements via `check_requirements()`
- ✅ Validates plan schema version
- ✅ Handles multi-step continuation (Milestone 0.5)
- ⚠️ Origin worktree validation happens in `execute_remaining_steps()` but should be earlier
- ⚠️ Forge API steps fail with generic message during continuation

### `abort()` - Current State

```rust
pub fn abort(ctx: &Context) -> Result<()> {
    // 1. Open repo, get paths
    // 2. Check requirements (RECOVERY)
    // 3. Load op-state
    // 4. Validate origin worktree
    // 5. Abort git operation
    // 6. Roll back refs using journal
    // 7. Record Aborted event in ledger
    // 8. Clear op-state
}
```

**Assessment:**
- ✅ Validates origin worktree early
- ✅ Records abort event in ledger
- ✅ Uses journal for rollback
- ✅ Handles partial rollback gracefully

### `undo()` - Current State

```rust
pub fn undo(ctx: &Context) -> Result<()> {
    // 1. Open repo, get paths
    // 2. Check requirements (RECOVERY)
    // 3. Verify no op in progress
    // 4. Find most recent committed journal
    // 5. Apply rollback operations
}
```

**Issues Identified:**
- ✅ Checks requirements
- ✅ Verifies no op in progress
- ⚠️ Does not record undo event in ledger
- ⚠️ Does not check if operation is undoable (remote ops warning)
- ⚠️ Uses raw `git update-ref` instead of going through `Git` interface

---

## Implementation Tasks

### Task 7.1: Audit and Improve `continue_op()`

**Effort:** 0.5 days  
**Risk:** LOW

**Changes Required:**

1. **Move worktree validation earlier** (before attempting any git operations):
   ```rust
   pub fn continue_op(ctx: &Context, all: bool) -> Result<()> {
       // ... setup ...
       
       // Validate origin worktree IMMEDIATELY after loading op-state
       let op_state = OpState::read(&paths)?.ok_or_else(|| ...)?;
       
       if let Err(msg) = op_state.check_origin_worktree(&info.git_dir) {
           bail!("{}", msg);
       }
       
       // ... rest of function
   }
   ```

2. **Improve forge step error message** in `execute_single_step()`:
   ```rust
   PlanStep::ForgeCreatePr { .. } | ... => {
       Ok(ContinueStepResult::Abort {
           error: format!(
               "Cannot continue forge API operation '{}'. \
                The operation paused before completing remote changes. \
                Please re-run the original command (e.g., 'lattice submit').",
               step.description()
           ),
       })
   }
   ```

3. **Add debug logging** for continuation steps:
   ```rust
   if ctx.debug {
       eprintln!("[debug] continue: validating origin worktree");
       eprintln!("[debug] continue: origin={:?}, current={:?}", 
                 op_state.origin_git_dir, info.git_dir);
   }
   ```

**Acceptance Criteria:**
- [ ] Origin worktree validation happens before any git operations
- [ ] Forge API step failures have clear, actionable error messages
- [ ] Debug logging shows continuation flow
- [ ] Existing tests pass

### Task 7.2: Audit and Improve `abort()`

**Effort:** 0.25 days  
**Risk:** LOW

**Current State:** Already well-implemented. Minor improvements only.

**Changes Required:**

1. **Add forge step rollback consideration** (document limitation):
   - Forge API steps (PR creation, etc.) cannot be rolled back
   - Add warning if journal contains forge steps that were executed

2. **Improve partial rollback messaging**:
   ```rust
   if !rollback_result.complete {
       eprintln!();
       eprintln!("Warning: Partial rollback - some changes could not be reversed:");
       for (refname, error) in &rollback_result.failed {
           eprintln!("  {}: {}", refname, error);
       }
       
       // Check for executed forge steps
       if journal.has_executed_forge_steps() {
           eprintln!();
           eprintln!("Note: Remote operations (push, PR updates) cannot be undone.");
           eprintln!("You may need to manually revert changes on the forge.");
       }
       
       eprintln!();
       eprintln!("Run 'lattice doctor' for guidance on resolving this.");
   }
   ```

**Acceptance Criteria:**
- [ ] Abort warns about forge steps that cannot be rolled back
- [ ] Partial rollback messaging is clear and actionable
- [ ] Existing tests pass

### Task 7.3: Audit and Improve `undo()`

**Effort:** 0.5 days  
**Risk:** MEDIUM

**Changes Required:**

1. **Record undo event in ledger**:
   ```rust
   // After successful undo
   let ledger = EventLedger::new(&git);
   let event = Event::undo_applied(&journal.op_id, rollbacks.len());
   if let Err(e) = ledger.append(event) {
       if ctx.debug {
           eprintln!("[debug] Warning: Could not record undo event: {}", e);
       }
   }
   ```

2. **Check for remote operations and warn**:
   ```rust
   // Before applying rollbacks
   if journal.has_remote_operations() {
       eprintln!("Warning: This operation included remote changes that cannot be undone:");
       for step in journal.remote_steps() {
           eprintln!("  - {}", step.description());
       }
       eprintln!();
       if !ctx.force && ctx.interactive {
           if !confirm("Continue with local-only undo?")? {
               bail!("Undo cancelled");
           }
       }
   }
   ```

3. **Use Git interface instead of raw commands**:
   ```rust
   // Instead of:
   let status = std::process::Command::new("git")
       .args(["update-ref", refname, old])
       .current_dir(&cwd)
       .status()?;
   
   // Use:
   git.update_ref_cas(refname, &new_oid, Some(&old_oid), "undo")?;
   ```

4. **Add Event::undo_applied variant** if not present:
   ```rust
   // In ledger.rs
   impl Event {
       pub fn undo_applied(op_id: &str, refs_restored: usize) -> Self {
           Self {
               kind: EventKind::UndoApplied,
               op_id: op_id.to_string(),
               details: format!("{} refs restored", refs_restored),
               // ...
           }
       }
   }
   ```

**Acceptance Criteria:**
- [ ] Undo records event in ledger
- [ ] Undo warns about remote operations
- [ ] Undo uses Git interface for ref updates
- [ ] Existing tests pass

### Task 7.4: Add Recovery Integration Tests

**Effort:** 0.5 days  
**Risk:** LOW

**New Tests Required:**

1. **Worktree origin validation test**:
   ```rust
   #[test]
   fn continue_refuses_from_wrong_worktree() {
       // Create main repo and worktree
       // Start operation in main repo
       // Pause on conflict
       // Try to continue from worktree
       // Assert error with correct guidance
   }
   ```

2. **Forge step continuation test**:
   ```rust
   #[test]
   fn continue_fails_gracefully_on_forge_steps() {
       // Create operation with forge steps
       // Pause after local steps but before forge steps
       // Try to continue
       // Assert clear error message
   }
   ```

3. **Undo with remote operations test**:
   ```rust
   #[test]
   fn undo_warns_about_remote_operations() {
       // Create operation with push step
       // Complete operation
       // Run undo
       // Assert warning about remote operations
   }
   ```

4. **Ledger event recording tests**:
   ```rust
   #[test]
   fn abort_records_event_in_ledger() {
       // Start operation, pause
       // Abort
       // Assert Aborted event in ledger
   }
   
   #[test]
   fn undo_records_event_in_ledger() {
       // Complete operation
       // Undo
       // Assert UndoApplied event in ledger
   }
   ```

**Acceptance Criteria:**
- [ ] All new tests pass
- [ ] Tests cover worktree validation
- [ ] Tests cover forge step handling
- [ ] Tests verify ledger integration

### Task 7.5: Documentation Review

**Effort:** 0.25 days  
**Risk:** LOW

**Updates Required:**

1. **Update module docs** in `recovery.rs`:
   - Document worktree origin validation
   - Document forge step limitations
   - Reference SPEC.md sections

2. **Update module docs** in `undo.rs`:
   - Document remote operation limitations
   - Document ledger integration

3. **Verify SPEC.md compliance**:
   - Cross-reference §4.6.5 (worktree origin)
   - Cross-reference §8F (recovery commands)

**Acceptance Criteria:**
- [ ] Module docs are comprehensive
- [ ] SPEC.md references are correct
- [ ] Limitations are clearly documented

---

## Decision: Should Recovery Commands Implement Traits?

### Analysis

**Option A: Keep as specialized functions (RECOMMENDED)**

Pros:
- Recovery commands have unique semantics (resume/reverse vs. create)
- They already use `check_requirements()` for gating
- The journal/ledger integration is specialized
- Adding trait implementation would complicate without benefit

Cons:
- Less uniformity with other commands
- Engine hooks don't fire (but they shouldn't for recovery)

**Option B: Create RecoveryCommand trait**

Pros:
- Uniformity across codebase
- Could enable future extensibility

Cons:
- Adds complexity without clear benefit
- Recovery semantics don't fit the plan/execute model
- Would require rethinking journal integration

### Recommendation

**Keep recovery commands as specialized functions.** They already follow architectural guidelines through:
- `check_requirements()` for gating
- `EventLedger` for event recording
- Journal for state management
- Origin worktree validation

The improvements in this phase focus on:
1. Earlier validation (worktree check)
2. Better error messages (forge steps)
3. Ledger integration (undo event)
4. Using Git interface consistently

---

## Implementation Order

| Order | Task | Description | Effort |
|-------|------|-------------|--------|
| 1 | 7.1 | Improve `continue_op()` | 0.5 days |
| 2 | 7.2 | Improve `abort()` | 0.25 days |
| 3 | 7.3 | Improve `undo()` | 0.5 days |
| 4 | 7.4 | Add integration tests | 0.5 days |
| 5 | 7.5 | Documentation review | 0.25 days |

**Total estimated effort:** 2 days

---

## Verification Checklist

Before marking Phase 7 complete:

- [ ] `continue_op()` validates origin worktree before any git operations
- [ ] `continue_op()` has clear error messages for forge API step failures
- [ ] `abort()` warns about unrollable forge steps
- [ ] `undo()` records event in ledger
- [ ] `undo()` warns about remote operations
- [ ] `undo()` uses Git interface for ref updates
- [ ] All recovery commands use `check_requirements(RECOVERY)`
- [ ] New integration tests pass
- [ ] Module documentation is complete
- [ ] `cargo test` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Breaking existing recovery flow | LOW | HIGH | Comprehensive test coverage |
| Forge step edge cases | MEDIUM | LOW | Clear error messages, guide to re-run |
| Ledger event format changes | LOW | LOW | Use existing event patterns |

---

## Dependencies

### Depends On:
- Phase 6 complete (forge `PlanStep` variants in place)
- Event ledger infrastructure (already exists)
- Journal system (already exists)

### Required By:
- Phase 8 (Verification & Cleanup)

---

## References

- **ARCHITECTURE.md §3.3** - Op-state marker
- **ARCHITECTURE.md §6.2** - Executor contract (worktree revalidation)
- **SPEC.md §4.2.2** - Journal and crash safety
- **SPEC.md §4.6.5** - Cross-worktree continue/abort ownership
- **SPEC.md §8F** - Recovery commands specification
- **HANDOFF.md Phase 7** - Original migration spec
- `src/cli/commands/recovery.rs` - Current implementation
- `src/cli/commands/undo.rs` - Current implementation
