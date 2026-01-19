# Milestone 1.1: Implementation Notes

## Summary

Completed the `lattice doctor --fix` command to execute repair plans through the standard executor, replacing the stub that printed "Execution not yet implemented." Also wired `DoctorProposed` and `DoctorApplied` event recording per ARCHITECTURE.md Section 3.4.2.

## Key Implementation Decisions

### 1. Executor Reuse (ARCHITECTURE.md Section 8.1)

Per the architecture requirement "Doctor shares the same scanner, planner model, executor, event recording. There is no separate 'repair mutation path.'"

The implementation uses the existing `Executor::execute()` method directly:

```rust
let executor = Executor::new(&git);
let result = executor.execute(&plan, ctx)?;
```

This ensures doctor repairs have the same transactional guarantees (lock acquisition, journaling, CAS ref updates) as all other commands.

### 2. Event Recording (ARCHITECTURE.md Section 3.4.2)

Added two event recording points:

1. **DoctorProposed** - Recorded when diagnosis is displayed with available fixes (before any fix is applied). Records all issue IDs and available fix IDs.

2. **DoctorApplied** - Recorded after successful fix execution. Records which fix IDs were applied and the resulting fingerprint.

Both use best-effort recording - failures are logged as warnings but don't fail the command:

```rust
if let Err(e) = ledger.append(event) {
    eprintln!("Warning: failed to record DoctorApplied event: {}", e);
}
```

### 3. Post-Verify After Repair

After successful execution, the implementation performs a post-verify by re-running diagnosis:

1. Re-scan the repository
2. Re-diagnose
3. Check if targeted issues are resolved
4. Report status to user

This fulfills ARCHITECTURE.md Section 8.4: "After applying a repair plan, doctor performs a full post-verify."

### 4. ExecuteResult Handling

The implementation handles all three `ExecuteResult` variants:

- **Success**: Record `DoctorApplied` event, run post-verify
- **Paused**: Inform user of conflict and provide continue/abort instructions
- **Aborted**: Report failure and warn about partial application

## Files Modified

### Primary Change
- `src/cli/commands/mod.rs` (Lines ~355-450) - Replaced stub with executor call, added event recording, added post-verify

### Supporting (already implemented)
- `src/engine/ledger.rs` - `Event::doctor_proposed()` and `Event::doctor_applied()` constructors
- `src/engine/exec.rs` - Executor already supported all doctor plan step types
- `src/doctor/planner.rs` - Plan generation was already complete

## Test Coverage

### Existing Tests (verified passing)
- `src/doctor/planner.rs` - 9 tests for plan generation
- `src/doctor/mod.rs` - 32 tests for diagnosis and preview
- `src/engine/exec.rs` - 15 tests for executor
- `src/engine/ledger.rs` - 17 tests for events (including DoctorProposed/DoctorApplied)

### Acceptance Gates Verified
- [x] Doctor uses the same executor as other commands
- [x] `--fix trunk-not-configured` sets trunk
- [x] `--fix metadata-parse-error:<branch>` repairs corrupt metadata
- [x] `DoctorApplied` event recorded after successful fix
- [x] `DoctorProposed` event recorded when diagnosis displayed
- [x] Non-interactive requires explicit `--fix <id>`
- [x] Conflict handling transitions to `awaiting_user`
- [x] Post-verify runs after repair

## Notes

The implementation was straightforward because the infrastructure was nearly complete. The doctor framework, planner, executor, and event types were all in place - this milestone was primarily wiring them together at the CLI layer.

The `--list` flag was added for machine-readable output, useful for CI/automation where issue and fix IDs need to be parsed programmatically.
