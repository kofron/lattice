# Milestone 1.1: Doctor Fix Execution

## Goal

Complete the `lattice doctor --fix` command to actually execute repair plans, replacing the current stub that prints "Execution not yet implemented."

**Core principle from ARCHITECTURE.md Section 8.1:** "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

---

## Background

The doctor framework infrastructure is **99% complete**:

- Diagnosis generation: working
- Plan preview display: working
- Plan generation from FixOptions: working
- Precondition validation: working
- Executor compatibility: all doctor step types are supported
- Event types (DoctorProposed, DoctorApplied): fully defined

**What's missing:** The final wiring in `src/cli/commands/mod.rs` at line ~357 that actually calls the executor.

---

## Spec References

- **ARCHITECTURE.md Section 8** - Doctor framework
- **ARCHITECTURE.md Section 8.1** - Doctor shares executor with commands
- **ARCHITECTURE.md Section 8.3** - Confirmation model (never auto-select)
- **ARCHITECTURE.md Section 8.4** - Post-verify and DoctorApplied event
- **ARCHITECTURE.md Section 3.4.2** - DoctorProposed and DoctorApplied events

---

## Implementation Steps

### Step 1: Record DoctorProposed Event

**File:** `src/cli/commands/mod.rs`

When diagnosis is displayed with available fixes, record a `DoctorProposed` event in the ledger.

**Location:** After `diagnosis.format()` is printed (around line 322)

```rust
// Record DoctorProposed event when fixes are available
if !diagnosis.fixes.is_empty() {
    let issue_ids: Vec<String> = diagnosis.issues.iter()
        .map(|i| i.id.to_string())
        .collect();
    let fix_ids: Vec<String> = diagnosis.fixes.iter()
        .map(|f| f.id.to_string())
        .collect();
    
    let ledger = EventLedger::new(&git);
    let event = Event::doctor_proposed(issue_ids, fix_ids);
    if let Err(e) = ledger.append(&event) {
        if ctx.debug {
            eprintln!("Warning: failed to record DoctorProposed event: {}", e);
        }
    }
}
```

### Step 2: Wire Executor for Fix Execution

**File:** `src/cli/commands/mod.rs`

Replace the stub at lines 352-360 with actual executor invocation.

**Current code (stub):**
```rust
// Execute the plan
// Note: For now, we just show the plan. Actual execution would use the executor.
if !ctx.quiet {
    println!(
        "Would apply {} fix(es). Execution not yet implemented.",
        parsed_fix_ids.len()
    );
}
```

**Replacement:**
```rust
// Execute the plan through the standard executor
use crate::engine::exec::{Executor, ExecuteResult};
use crate::engine::ledger::{Event, EventLedger};
use crate::engine::scan::compute_fingerprint;

let executor = Executor::new(&git);
let result = executor.execute(&plan, &ctx)?;

match result {
    ExecuteResult::Success { fingerprint } => {
        // Record DoctorApplied event
        let fix_id_strings: Vec<String> = parsed_fix_ids.iter()
            .map(|f| f.to_string())
            .collect();
        let ledger = EventLedger::new(&git);
        let event = Event::doctor_applied(fix_id_strings, &fingerprint.to_string());
        ledger.append(&event)?;

        if !ctx.quiet {
            println!("Successfully applied {} fix(es).", parsed_fix_ids.len());
        }
    }
    ExecuteResult::Paused { branch, git_state, .. } => {
        // Conflict during repair - transition to awaiting_user
        println!(
            "Repair paused: conflict on branch '{}' ({:?}).",
            branch, git_state
        );
        println!("Resolve conflicts and run 'lattice continue', or 'lattice abort' to cancel.");
        return Ok(());
    }
    ExecuteResult::Aborted { error, applied_steps } => {
        // Repair failed - some steps may have been applied
        eprintln!("Repair aborted: {}", error);
        if !applied_steps.is_empty() {
            eprintln!(
                "Warning: {} step(s) were applied before failure.",
                applied_steps.len()
            );
            eprintln!("Run 'lattice doctor' to check repository state.");
        }
        return Err(anyhow::anyhow!("Repair failed: {}", error));
    }
}
```

### Step 3: Add Required Imports

**File:** `src/cli/commands/mod.rs`

Ensure these imports are present at the top of the file:

```rust
use crate::engine::exec::{Executor, ExecuteResult, ExecuteError};
use crate::engine::ledger::{Event, EventLedger};
```

### Step 4: Handle Post-Verify

The executor already performs post-verify as part of its contract. However, doctor should run an additional diagnosis after successful repair to confirm the issue is resolved.

**Add after success message:**
```rust
// Post-verify: run diagnosis again to confirm fix worked
if !ctx.quiet {
    let new_snapshot = scan::scan(&git)?;
    let new_diagnosis = doctor.diagnose(&new_snapshot);
    
    // Check if the fixed issues are now resolved
    let fixed_issue_ids: std::collections::HashSet<_> = parsed_fix_ids.iter()
        .filter_map(|f| diagnosis.fixes.iter().find(|fix| &fix.id == f))
        .map(|fix| &fix.issue_id)
        .collect();
    
    let remaining: Vec<_> = new_diagnosis.issues.iter()
        .filter(|i| fixed_issue_ids.contains(&i.id))
        .collect();
    
    if remaining.is_empty() {
        println!("All targeted issues resolved.");
    } else {
        println!("Warning: {} issue(s) may not be fully resolved.", remaining.len());
        println!("Run 'lattice doctor' to check.");
    }
}
```

### Step 5: Update Error Handling

**File:** `src/cli/commands/mod.rs`

Ensure proper error conversion from ExecuteError to the command's error type:

```rust
// In the doctor command function, update error handling
impl From<ExecuteError> for anyhow::Error {
    // This should already exist via thiserror
}
```

### Step 6: Integration Tests

**File:** `tests/integration/doctor_fix.rs` (new)

```rust
//! Integration tests for doctor --fix execution

#[test]
fn test_doctor_fix_trunk_not_configured() {
    // Setup: repo without trunk configured
    // Run: lattice doctor --fix trunk-not-configured
    // Verify: trunk is now configured
    // Verify: DoctorApplied event recorded
}

#[test]
fn test_doctor_fix_metadata_parse_error() {
    // Setup: repo with corrupt metadata
    // Run: lattice doctor --fix metadata-parse-error:branch-name
    // Verify: metadata is repaired or removed
    // Verify: DoctorApplied event recorded
}

#[test]
fn test_doctor_fix_orphaned_metadata() {
    // Setup: metadata ref exists but branch doesn't
    // Run: lattice doctor --fix orphaned-metadata:branch-name
    // Verify: orphaned metadata deleted
    // Verify: DoctorApplied event recorded
}

#[test]
fn test_doctor_fix_dry_run_no_changes() {
    // Setup: repo with issues
    // Run: lattice doctor --fix <id> --dry-run
    // Verify: no changes made
    // Verify: no DoctorApplied event recorded
}

#[test]
fn test_doctor_fix_nonexistent_fix_id() {
    // Run: lattice doctor --fix nonexistent-fix-id
    // Verify: error returned
}

#[test]
fn test_doctor_proposed_event_recorded() {
    // Setup: repo with issues
    // Run: lattice doctor (no --fix)
    // Verify: DoctorProposed event recorded with issue/fix ids
}

#[test]
fn test_doctor_fix_multiple_fixes() {
    // Setup: repo with multiple issues
    // Run: lattice doctor --fix id1 --fix id2
    // Verify: both fixes applied
    // Verify: single DoctorApplied event with both fix ids
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/cli/commands/mod.rs` | MODIFY | Wire executor, record events |
| `src/engine/exec.rs` | READ ONLY | Reference for executor pattern |
| `src/engine/ledger.rs` | READ ONLY | Reference for event recording |
| `src/doctor/planner.rs` | READ ONLY | Plan generation (already complete) |
| `tests/integration/doctor_fix.rs` | NEW | Integration tests |

---

## Acceptance Gates

Per ROADMAP.md and ARCHITECTURE.md Section 8:

- [x] Doctor uses the same executor as other commands (no separate repair path)
- [x] `lattice doctor --fix trunk-not-configured` actually sets trunk
- [x] `lattice doctor --fix metadata-parse-error:<branch>` repairs or removes corrupt metadata
- [x] `lattice doctor --fix orphaned-metadata:<branch>` removes orphaned metadata ref
- [x] `DoctorApplied` event recorded in ledger after successful fix
- [x] `DoctorProposed` event recorded when diagnosis with fixes is displayed
- [x] Non-interactive mode requires explicit `--fix <id>` (never auto-selects)
- [x] If repair encounters conflict, transitions to `awaiting_user` op-state
- [x] Paused repair can be continued with `lattice continue`
- [x] Paused repair can be aborted with `lattice abort`
- [x] Post-verify runs after repair completes (via executor)
- [x] `--dry-run` shows plan but makes no changes and records no events
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

## Testing Rubric

### Unit Tests (existing, verify still pass)

- `src/doctor/planner.rs` - 9 tests for plan generation
- `src/doctor/mod.rs` - 32 tests for diagnosis and preview
- `src/engine/exec.rs` - 15 tests for executor
- `src/engine/ledger.rs` - 17 tests for events

### Integration Tests (new)

| Test | Description | Pass Criteria |
|------|-------------|---------------|
| `test_doctor_fix_trunk_not_configured` | Fix missing trunk | Trunk set in config |
| `test_doctor_fix_metadata_parse_error` | Fix corrupt metadata | Metadata parseable |
| `test_doctor_fix_orphaned_metadata` | Remove orphaned metadata | Metadata ref deleted |
| `test_doctor_fix_dry_run_no_changes` | Dry run safety | No repo changes |
| `test_doctor_fix_nonexistent_fix_id` | Invalid fix ID | Error returned |
| `test_doctor_proposed_event_recorded` | Event recording | Event in ledger |
| `test_doctor_fix_multiple_fixes` | Multiple fixes | All applied |

### Manual Verification

1. Create a repo without trunk configured
2. Run `lattice doctor` - should show `trunk-not-configured` issue
3. Run `lattice doctor --fix trunk-not-configured` - should set trunk
4. Run `lattice doctor` again - issue should be gone
5. Check event ledger for `DoctorApplied` event

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

# Specific doctor tests
cargo test doctor

# Format check
cargo fmt --check
```

---

## Notes

- **Simplicity principle**: The executor and plan infrastructure already exist and work. This milestone is primarily wiring.
- **No stubs principle**: The execution must be real, using the standard executor path.
- **Follow the leader**: Per ARCHITECTURE.md, doctor uses the same execution path as all other commands.
- **Purity principle**: Event recording is the only side effect outside the executor's transactional boundary.

---

## Estimated Scope

- **Lines of code changed**: ~50-100 in `mod.rs`
- **New test file**: ~150-200 lines
- **Risk**: Low - infrastructure is complete, this is wiring
