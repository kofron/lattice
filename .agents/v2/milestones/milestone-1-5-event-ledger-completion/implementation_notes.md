# Milestone 1.5: Implementation Notes

## Summary

Wired the `DivergenceObserved` event recording per ARCHITECTURE.md Section 7.2. The infrastructure was complete; this milestone integrated `detect_divergence()` into the scan flow and surfaced divergence info in `RepoHealthReport`.

## Key Implementation Decisions

### 1. Reuse Existing Infrastructure (CLAUDE.md)

Per the Reuse principle, all components already existed:
- `detect_divergence()` function (implemented at `src/engine/scan.rs:576-605`)
- `DivergenceInfo` struct (defined at `src/engine/scan.rs:534-541`)
- `DivergenceObserved` event type (defined at `src/engine/ledger.rs:130-143`)
- `Event::divergence_observed()` constructor (at `src/engine/ledger.rs`)
- `last_committed_fingerprint()` method (complete in ledger.rs)
- Fingerprint computation (complete in `src/core/types.rs`)

The implementation was purely integration work - calling existing functions at the right time.

### 2. Best-Effort Recording Pattern

Per ARCHITECTURE.md Section 7.3, divergence is informational, not blocking. The implementation uses best-effort recording:

```rust
fn detect_and_record_divergence(
    git: &Git,
    current_fingerprint: &Fingerprint,
) -> Result<Option<DivergenceInfo>, ScanError> {
    let divergence = detect_divergence(git, current_fingerprint)?;
    
    if let Some(ref info) = divergence {
        let ledger = EventLedger::new(git);
        let event = Event::divergence_observed(/*...*/);
        
        // Best-effort recording - don't fail scan if ledger write fails
        if let Err(e) = ledger.append(event) {
            eprintln!("Warning: failed to record DivergenceObserved event: {}", e);
        }
    }
    
    Ok(divergence)
}
```

This ensures scan never fails due to ledger issues, while still recording events when possible.

### 3. Divergence in RepoHealthReport

Added `divergence` field to `RepoHealthReport`:

```rust
pub struct RepoHealthReport {
    issues: Vec<Issue>,
    capabilities: CapabilitySet,
    divergence: Option<DivergenceInfo>,  // NEW
}
```

With accessor methods:
- `set_divergence()` - Set during scan
- `divergence()` - Get reference for commands
- `has_divergence()` - Convenience boolean check

### 4. Debug Output for Divergence

Added `surface_divergence_if_debug()` helper that prints divergence info when debug mode is enabled:

```rust
pub fn surface_divergence_if_debug(ctx: &Context, health: &RepoHealthReport) {
    if ctx.debug {
        if let Some(divergence) = health.divergence() {
            eprintln!("Note: Repository state has changed since last Lattice operation.");
            eprintln!("Prior fingerprint: {}", &divergence.prior_fingerprint[..12]);
            eprintln!("Current fingerprint: {}", &divergence.current_fingerprint[..12]);
            // List changed refs if any
        }
    }
}
```

This is called in doctor and can be used by other commands for visibility.

## Files Modified

### Engine
- `src/engine/health.rs` - Added `divergence` field to `RepoHealthReport` with accessors
- `src/engine/scan.rs` - Added `detect_and_record_divergence()` call in scan flow (Line ~434)
- `src/engine/mod.rs` - Exported `DivergenceInfo` (already present)

### CLI
- `src/cli/commands/mod.rs` - Added `surface_divergence_if_debug()` helper (Lines ~286-305)

## Test Coverage

### Existing Tests (verified passing)
- `src/engine/scan.rs` - Tests for `detect_divergence()` function
- `src/engine/ledger.rs` - Tests for event recording including `DivergenceObserved`

### Acceptance Gates Verified
- [x] `DivergenceObserved` recorded when fingerprint changes between operations
- [x] Divergence info available in `RepoHealthReport`
- [x] `DoctorProposed` recorded when fix options are presented (already complete from 1.1)
- [x] `DoctorApplied` recorded when fix is executed (already complete from 1.1)

## Notes

### How Divergence Detection Works

1. **During scan**: Compute fingerprint over trunk, tracked branches, and metadata refs
2. **Compare**: Check against last `Committed` event fingerprint in ledger
3. **If different**: Record `DivergenceObserved` with:
   - Prior fingerprint
   - Current fingerprint
   - List of changed refs

### When Divergence Occurs

Divergence is detected when:
- User runs `git commit`, `git rebase`, etc. directly
- GitHub UI actions modify refs
- Other tools modify the repository
- Any out-of-band change since last Lattice operation

### Divergence is Evidence, Not Error

Per ARCHITECTURE.md Section 3.4.1:
> "The ledger MUST NOT be replayed blindly to overwrite repository state."

And Section 7.3:
> "Divergence itself is not an error. It becomes evidence surfaced in doctor and in gated command failures."

The implementation follows this - divergence affects gating only when it prevents required capabilities from being established.
