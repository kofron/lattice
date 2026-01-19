# Milestone 1.5: Event Ledger Completion

## Goal

**Complete event recording per ARCHITECTURE.md Section 3.4 and Section 7 requirements, specifically wiring the `DivergenceObserved` event recording that is currently implemented but never called.**

The event ledger infrastructure is complete. All 6 event types are defined with proper schemas, fingerprint computation works correctly, and `IntentRecorded`, `Committed`, `Aborted`, `DoctorProposed`, and `DoctorApplied` are all being recorded. The missing piece is integrating `detect_divergence()` into the scan flow and recording `DivergenceObserved` events when divergence is detected.

**Governing Principle:** Per CLAUDE.md "Reuse" - the `detect_divergence()` function exists and works. We need to call it during scanning and record the event.

---

## Background

### What Already Exists (Infrastructure Complete)

| Component | Location | Status |
|-----------|----------|--------|
| Event ledger storage at `refs/lattice/event-log` | `src/engine/ledger.rs` | Complete |
| All 6 event types with JSON schemas | `src/engine/ledger.rs` (Lines 82-171) | Complete |
| Fingerprint computation (SHA-256) | `src/core/types.rs` (Lines 680-706) | Complete |
| `IntentRecorded` recording | `src/engine/exec.rs` (Lines 230-242) | Complete |
| `Committed` recording | `src/engine/exec.rs` (Lines 316-321) | Complete |
| `Aborted` recording | `src/engine/exec.rs` (Lines 297-300) | Complete |
| `DoctorProposed` recording | `src/cli/commands/mod.rs` (Lines 327-341) | Complete |
| `DoctorApplied` recording | `src/cli/commands/mod.rs` (Lines 378-386) | Complete |
| `detect_divergence()` function | `src/engine/scan.rs` (Lines 530-563) | Complete but not called |
| `DivergenceInfo` struct | `src/engine/scan.rs` (Lines 491-496) | Complete |
| `DivergenceObserved` event type | `src/engine/ledger.rs` (Lines 130-143) | Complete |
| `last_committed_fingerprint()` method | `src/engine/ledger.rs` | Complete |

### What's Missing (The Gap)

Per ARCHITECTURE.md Section 7.2:

> "On each command invocation, the engine compares: the current fingerprint, the last recorded Committed event fingerprint. If they differ, the engine records a DivergenceObserved event."

**Current state:** The `detect_divergence()` function exists but is never called. `RepoHealthReport` does not include divergence information.

**What we need:**
1. Call `detect_divergence()` during the scanning phase
2. Record `DivergenceObserved` event when divergence is detected
3. Include divergence info in `RepoHealthReport` for gating decisions and user visibility

---

## Spec References

### ARCHITECTURE.md Section 3.4.2 - Event Categories

```markdown
The ledger contains the following event categories:
* `IntentRecorded`
* `Committed`
* `Aborted`
* `DivergenceObserved`
* `DoctorProposed`
* `DoctorApplied`
```

### ARCHITECTURE.md Section 7 - Out-of-band Divergence Detection

#### Section 7.1 - Fingerprints

> The scanner computes a repository fingerprint over a stable set of ref values:
> * trunk ref value
> * all tracked branch ref values
> * all structural metadata ref values
> * repository config version
>
> The fingerprint is a stable hash of sorted `(refname, oid)` entries.

#### Section 7.2 - DivergenceObserved Event

> On each command invocation, the engine compares:
> * the current fingerprint
> * the last recorded `Committed` event fingerprint
>
> If they differ, the engine records a `DivergenceObserved` event including:
> * prior fingerprint
> * current fingerprint
> * a diff summary of changed refs
>
> Divergence itself is not an error. It becomes evidence surfaced in doctor and in gated command failures.

#### Section 7.3 - Divergence and Gating

> Divergence affects gating only insofar as it prevents required capabilities from being established.

### ARCHITECTURE.md Section 12 - Command Lifecycle

The lifecycle explicitly includes divergence detection as part of the scan phase:

> 1. **Scan**
>    * compute repo health report
>    * detect in-progress ops
>    * **compute fingerprint and record divergence if needed**

---

## Implementation Steps

### Step 1: Add Divergence Info to RepoHealthReport

**File:** `src/engine/health.rs`

**Current state (Lines 205-295):**
```rust
#[derive(Debug, Clone, Default)]
pub struct RepoHealthReport {
    issues: Vec<Issue>,
    capabilities: CapabilitySet,
}
```

**Add divergence field:**
```rust
#[derive(Debug, Clone, Default)]
pub struct RepoHealthReport {
    issues: Vec<Issue>,
    capabilities: CapabilitySet,
    divergence: Option<DivergenceInfo>,  // NEW
}
```

**Add accessor method:**
```rust
impl RepoHealthReport {
    // ... existing methods ...
    
    /// Returns divergence information if out-of-band changes were detected.
    pub fn divergence(&self) -> Option<&DivergenceInfo> {
        self.divergence.as_ref()
    }
    
    /// Returns true if divergence from last committed state was detected.
    pub fn has_divergence(&self) -> bool {
        self.divergence.is_some()
    }
}
```

**Add builder method:**
```rust
impl RepoHealthReport {
    // ... existing builder methods ...
    
    /// Sets the divergence information.
    pub fn with_divergence(mut self, divergence: Option<DivergenceInfo>) -> Self {
        self.divergence = divergence;
        self
    }
}
```

**Import DivergenceInfo:**
```rust
use crate::engine::scan::DivergenceInfo;
```

---

### Step 2: Integrate Divergence Detection into Scanner

**File:** `src/engine/scan.rs`

**Location:** In the main scanning function (likely `scan()` or `scan_repo()`)

The scanner already computes the fingerprint via `compute_fingerprint()`. We need to:

1. Call `detect_divergence()` with the computed fingerprint
2. Record `DivergenceObserved` event if divergence is detected
3. Pass divergence info to `RepoHealthReport`

**Add divergence detection to scan flow:**

```rust
pub fn scan(git: &Git, config: &RepoConfig) -> Result<RepoHealthReport, ScanError> {
    // ... existing scanning logic ...
    
    // Compute fingerprint over tracked state
    let fingerprint = compute_fingerprint(&branches, &metadata, trunk.as_ref());
    
    // Detect and record divergence per ARCHITECTURE.md Section 7.2
    let divergence = detect_and_record_divergence(git, &fingerprint)?;
    
    // Build health report with divergence info
    let report = RepoHealthReport::new()
        .with_issues(issues)
        .with_capabilities(capabilities)
        .with_divergence(divergence);  // NEW
    
    Ok(report)
}

/// Detect divergence and record DivergenceObserved event if needed.
/// Per ARCHITECTURE.md Section 7.2: "On each command invocation, the engine
/// compares the current fingerprint with the last recorded Committed event
/// fingerprint. If they differ, the engine records a DivergenceObserved event."
fn detect_and_record_divergence(
    git: &Git,
    current_fingerprint: &Fingerprint,
) -> Result<Option<DivergenceInfo>, ScanError> {
    let divergence = detect_divergence(git, current_fingerprint)?;
    
    if let Some(ref info) = divergence {
        // Record DivergenceObserved event
        let ledger = EventLedger::new(git);
        let event = Event::divergence_observed(
            &info.prior_fingerprint,
            &info.current_fingerprint,
            info.changed_refs.clone(),
        );
        
        // Best-effort recording - don't fail the scan if ledger write fails
        if let Err(e) = ledger.append(event) {
            // Log warning but continue - divergence detection is informational
            tracing::warn!("Failed to record DivergenceObserved event: {}", e);
        }
    }
    
    Ok(divergence)
}
```

---

### Step 3: Add Event Constructor for DivergenceObserved

**File:** `src/engine/ledger.rs`

Check if the constructor exists. Based on exploration, there should be constructors like `Event::intent_recorded()`, `Event::committed()`, etc.

**If missing, add:**
```rust
impl Event {
    /// Creates a DivergenceObserved event.
    pub fn divergence_observed(
        prior_fingerprint: &str,
        current_fingerprint: &str,
        changed_refs: Vec<String>,
    ) -> Self {
        Event::DivergenceObserved {
            prior_fingerprint: prior_fingerprint.to_string(),
            current_fingerprint: current_fingerprint.to_string(),
            changed_refs,
            timestamp: Utc::now().to_rfc3339(),
        }
    }
}
```

---

### Step 4: Surface Divergence in User-Facing Output

**File:** `src/cli/commands/mod.rs` (or relevant command entry point)

When divergence is detected, inform the user if in verbose/debug mode:

```rust
// After scanning, surface divergence info if present
if let Some(divergence) = health_report.divergence() {
    if ctx.debug || ctx.verbose {
        eprintln!(
            "Note: Repository state has changed since last Lattice operation.\n\
             {} refs changed out-of-band.",
            divergence.changed_refs.len()
        );
        if ctx.debug {
            for ref_name in &divergence.changed_refs {
                eprintln!("  - {}", ref_name);
            }
        }
    }
}
```

---

### Step 5: Update DivergenceInfo to be Public

**File:** `src/engine/scan.rs`

Ensure `DivergenceInfo` is exported and accessible:

```rust
/// Information about detected divergence from last committed state.
/// Per ARCHITECTURE.md Section 7.2.
#[derive(Debug, Clone)]
pub struct DivergenceInfo {
    /// Fingerprint from the last Committed event
    pub prior_fingerprint: String,
    /// Current computed fingerprint
    pub current_fingerprint: String,
    /// List of refs that changed between fingerprints
    pub changed_refs: Vec<String>,
}
```

Ensure it's exported in `src/engine/mod.rs`:
```rust
pub use scan::DivergenceInfo;
```

---

### Step 6: Add Tests for Divergence Detection Integration

**File:** `src/engine/scan.rs` (test module) or `tests/integration/divergence.rs`

```rust
#[cfg(test)]
mod divergence_tests {
    use super::*;
    
    #[test]
    fn test_no_divergence_when_no_prior_commit() {
        // Given: A fresh repo with no event ledger entries
        // When: scan is called
        // Then: divergence should be None (no prior state to compare)
    }
    
    #[test]
    fn test_no_divergence_when_fingerprints_match() {
        // Given: A repo with a Committed event and unchanged state
        // When: scan is called
        // Then: divergence should be None
    }
    
    #[test]
    fn test_divergence_detected_when_refs_changed() {
        // Given: A repo with a Committed event
        // And: Some refs have changed out-of-band (e.g., git commit directly)
        // When: scan is called
        // Then: divergence should be Some with changed refs
        // And: DivergenceObserved event should be recorded
    }
    
    #[test]
    fn test_divergence_info_in_health_report() {
        // Given: Divergence exists
        // When: scan produces a RepoHealthReport
        // Then: report.divergence() should return the info
    }
}
```

---

### Step 7: Update ROADMAP.md

**File:** `.agents/v2/ROADMAP.md`

**Update milestone 1.5 status from "Partially stubbed" to "Complete"**

**Mark all acceptance gates:**

```markdown
### Milestone 1.5: Event Ledger Completion

**Status:** Complete

...

**Acceptance gates (per ARCHITECTURE.md Section 3.4 and 7):**

- [x] `DivergenceObserved` recorded when fingerprint changes between operations
- [x] Divergence info available in `RepoHealthReport`
- [x] `DoctorProposed` recorded when fix options are presented
- [x] `DoctorApplied` recorded when fix is executed
- [x] `cargo test` passes
- [x] `cargo clippy` passes
```

---

## Critical Files Summary

| File | Action | Purpose |
|------|--------|---------|
| `src/engine/health.rs` | MODIFY | Add `divergence` field to `RepoHealthReport` |
| `src/engine/scan.rs` | MODIFY | Integrate `detect_divergence()` into scan flow |
| `src/engine/ledger.rs` | VERIFY/MODIFY | Ensure `divergence_observed()` constructor exists |
| `src/engine/mod.rs` | MODIFY | Export `DivergenceInfo` |
| `src/cli/commands/mod.rs` | MODIFY | Surface divergence in verbose/debug output |
| `.agents/v2/ROADMAP.md` | MODIFY | Update status to Complete |

---

## Acceptance Gates

Per ARCHITECTURE.md Section 3.4 and 7, and ROADMAP.md:

- [ ] `DivergenceObserved` recorded when fingerprint changes between operations
- [ ] Divergence info available in `RepoHealthReport`
- [ ] `DoctorProposed` recorded when fix options are presented (already complete)
- [ ] `DoctorApplied` recorded when fix is executed (already complete)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Rubric

### Unit Tests

| Test | File | Purpose |
|------|------|---------|
| `test_health_report_with_divergence` | `src/engine/health.rs` | Verify divergence field accessors |
| `test_divergence_observed_event_schema` | `src/engine/ledger.rs` | Verify event serialization |
| `test_detect_divergence_no_prior` | `src/engine/scan.rs` | No divergence when no prior state |
| `test_detect_divergence_matching` | `src/engine/scan.rs` | No divergence when fingerprints match |
| `test_detect_divergence_changed` | `src/engine/scan.rs` | Divergence detected correctly |

### Integration Tests

| Test | File | Purpose |
|------|------|---------|
| `test_divergence_recorded_after_oob_change` | `tests/integration/ledger.rs` | Full flow: OOB change triggers event |
| `test_divergence_in_health_report` | `tests/integration/scan.rs` | Health report contains divergence |
| `test_no_divergence_after_lattice_op` | `tests/integration/ledger.rs` | No divergence after clean Lattice op |

### Existing Verification (Already Passing)

| Component | Status | Notes |
|-----------|--------|-------|
| `IntentRecorded` recording | Complete | Tested in exec.rs |
| `Committed` recording | Complete | Tested in exec.rs |
| `Aborted` recording | Complete | Tested in exec.rs |
| `DoctorProposed` recording | Complete | Recorded in mod.rs |
| `DoctorApplied` recording | Complete | Recorded in mod.rs |
| Fingerprint computation | Complete | Tested in types.rs |

---

## Verification Commands

```bash
# Build and type check
cargo check

# Lint
cargo clippy -- -D warnings

# Run all tests
cargo test

# Run specific ledger tests
cargo test ledger::

# Run specific scan tests
cargo test scan::

# Run specific divergence tests
cargo test divergence

# Format check
cargo fmt --check
```

---

## Risk Assessment

**Low risk** - This milestone:
- Wires existing, tested infrastructure
- The `detect_divergence()` function is already implemented and has docstrings
- Event types and ledger operations are fully tested
- No new async code or complex state management
- Divergence is informational, not blocking (per ARCHITECTURE.md Section 7.3)

**Potential Issues:**
- Ledger write failures during divergence recording should not fail the scan
- Performance: fingerprint comparison happens on every scan (but it's just string comparison)

**Mitigations:**
- Use best-effort recording with warning on failure (already shown in implementation)
- Fingerprint comparison is O(1) string compare, negligible overhead

---

## Dependencies

No new dependencies required. Uses existing:
- `EventLedger` for event recording
- `Fingerprint` for comparison
- `detect_divergence()` for detection logic
- `chrono::Utc` for timestamps

---

## Notes

**Principles Applied:**

- **Reuse:** The entire infrastructure exists - we're just wiring it together
- **Follow the Leader:** Implements exactly what ARCHITECTURE.md Section 7.2 specifies
- **Simplicity:** Single call site addition, minimal code changes
- **Purity:** `detect_divergence()` is pure - it reads state and returns a result
- **Tests are Everything:** Comprehensive test coverage for the integration

**Key Insight:**

The `detect_divergence()` function was implemented with clear documentation but never called. This milestone connects the existing pieces:

```
Scanner → compute_fingerprint() → detect_divergence() → Event::divergence_observed() → ledger.append()
                                           ↓
                                   RepoHealthReport.divergence
```

**Why Divergence Matters:**

Per ARCHITECTURE.md, divergence detection enables:
1. Audit trail of out-of-band changes
2. Evidence for doctor diagnostics
3. Recovery hints when metadata is corrupted
4. User awareness of external modifications

**Divergence is Evidence, Not Error:**

From ARCHITECTURE.md Section 3.4.1:
> "The ledger MUST NOT be replayed blindly to overwrite repository state."

And Section 7.3:
> "Divergence itself is not an error. It becomes evidence surfaced in doctor and in gated command failures."

This means divergence recording is informational. Commands should proceed normally unless divergence causes specific capability failures (e.g., corrupt metadata blocking `MetadataReadable`).

---

## Sequence of Implementation

1. **First:** Add `divergence` field to `RepoHealthReport` (health.rs)
2. **Second:** Ensure `DivergenceInfo` is exported (scan.rs, mod.rs)
3. **Third:** Verify/add `Event::divergence_observed()` constructor (ledger.rs)
4. **Fourth:** Integrate `detect_and_record_divergence()` into scan flow (scan.rs)
5. **Fifth:** Surface divergence in debug/verbose output (commands/mod.rs)
6. **Sixth:** Add tests
7. **Finally:** Update ROADMAP.md
