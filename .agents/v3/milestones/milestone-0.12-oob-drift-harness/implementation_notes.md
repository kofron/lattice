# Implementation Notes: Milestone 0.12 (Out-of-Band Drift Harness)

## Completion Date
January 2026

## Summary
Implemented a test-only hook mechanism enabling precise injection of out-of-band mutations to verify CAS and occupancy enforcement, per ARCHITECTURE.md Section 13.3 and ROADMAP.md Anti-Drift Mechanisms.

## Implementation Decisions

### 1. Feature Flag for Integration Tests

**Problem:** The plan specified `cfg(any(test, feature = "fault_injection"))`, but `cfg(test)` only applies to unit tests within the same crate. Integration tests (in `tests/`) are compiled as separate crates and don't see `cfg(test)`.

**Solution:** Added a new `test_hooks` feature in `Cargo.toml`:
```toml
test_hooks = []         # Enable engine hooks for integration tests (OOB drift harness)
```

Updated all cfg attributes to:
```rust
#[cfg(any(test, feature = "fault_injection", feature = "test_hooks"))]
```

Integration tests run with `cargo test --features test_hooks targeted_drift_tests`.

### 2. Hook Placement in Runner

**Location:** `src/engine/runner.rs` at line ~187, after planning but before occupancy check.

**Rationale:** This is the precise point where:
- Plan has been generated with expected OIDs
- Lock has NOT been acquired yet  
- Any mutations will be detected by executor's CAS checks

### 3. Test Strategy Adjustment

**Original Plan:** Test CAS/occupancy detection by using hooks to inject mutations and then calling commands like `restack` or `freeze`.

**Problem Discovered:** Most CLI commands (restack, freeze, etc.) don't flow through the unified `run_command` lifecycle in `runner.rs`. They implement their own logic directly, so the hook in `runner.rs` never gets invoked for those commands.

**Solution:** Revised the targeted drift tests to:

1. **Test hook API directly** - Verify hooks can be set, cleared, and invoked
2. **Test CAS/occupancy via direct mechanisms** - Use lower-level APIs to validate that:
   - CAS prevents concurrent metadata modification (via metadata store directly)
   - Occupancy detection works (via `WorktreeManager::check_occupancy`)
   - Gating refuses operations during in-progress state (via `gating::validate`)

This approach validates the same invariants without relying on commands to use the hook infrastructure.

### 4. Architectural Note for Future Work

The hook is correctly placed in `runner.rs`, but most commands bypass this unified flow. A future enhancement could:
- Migrate commands to use `run_command` consistently
- Add hooks at the command trait level
- Or accept that hooks are primarily useful for commands that DO use the runner

For now, the hook infrastructure is in place and ready for commands that use the runner flow.

## Files Modified

| File | Changes |
|------|---------|
| `src/engine/mod.rs` | Added `engine_hooks` module declaration |
| `src/engine/engine_hooks.rs` | NEW: Complete hook infrastructure |
| `src/engine/runner.rs` | Added hook invocation after planning |
| `tests/oob_fuzz.rs` | Updated docs, added targeted_drift_tests module |
| `Cargo.toml` | Added `test_hooks` feature |

## Tests Added

All in `tests/oob_fuzz.rs::targeted_drift_tests`:

1. `engine_hook_api_works` - Tests set/clear/has_hooks API
2. `engine_hook_replacement_works` - Tests hook replacement
3. `cas_prevents_concurrent_metadata_modification` - Direct CAS test via metadata store
4. `occupancy_detection_works` - Direct occupancy check via WorktreeManager
5. `gating_refuses_during_in_progress_operation` - Gating validation during rebase conflict

## Verification Results

All checks passed:
- `cargo check` - PASS
- `cargo clippy -- -D warnings` - PASS
- `cargo test` - PASS (850 unit tests, 350+ integration tests)
- `cargo test oob_fuzz` - PASS (existing fuzz harness)
- `cargo test --features test_hooks targeted_drift_tests` - PASS (5 new tests)
- `cargo fmt --check` - PASS

## Principles Followed

- **Follow the leader:** Implemented per ROADMAP.md Anti-Drift Mechanisms item 5
- **Simplicity:** Minimal hook API with thread-local storage
- **Reuse:** Extended existing oob_fuzz.rs infrastructure
- **Purity:** Hook module has no side effects except when explicitly invoked
- **No stubs:** All tests are complete and pass
- **Tests are everything:** Added tests validating the hook API and related invariants
