# Milestone 4: Engine Lifecycle - Implementation Notes

## Summary

Milestone 4 implements the full validated execution model - the architecture spine that all commands flow through. This establishes the Scan → Gate → Plan → Execute → Verify lifecycle with real implementations.

## Key Implementation Decisions

### 1. Capability System (`src/engine/capabilities.rs`)

Implemented capabilities as a simple enum with a `CapabilitySet` container. Key design choices:

- **Binary semantics**: Capabilities are either present or absent, no partial states
- **Copy trait**: Made `Capability` Copy for ergonomic use
- **Description method**: Each capability has a human-readable description for error messages
- **Set operations**: `has()`, `has_all()`, `missing()` for gating logic

### 2. Health Report (`src/engine/health.rs`)

The health report aggregates issues and capabilities discovered during scanning:

- **IssueId**: Deterministic hashing for stable issue identification across runs
- **Evidence enum**: Structured evidence types (Ref, ParseError, Cycle, etc.)
- **Severity levels**: Blocking, Warning, Info - only Blocking prevents commands
- **Capability blocking**: Issues can declare which capabilities they block

### 3. Event Ledger (`src/engine/ledger.rs`)

Implemented the append-only event log at `refs/lattice/event-log`:

- **Blob storage**: Events stored as JSON blobs, chained via parent commits
- **Event types**: IntentRecorded, Committed, Aborted, DivergenceObserved, DoctorProposed, DoctorApplied
- **Fingerprint tracking**: Each commit stores before/after fingerprints for divergence detection

### 4. Gating (`src/engine/gate.rs`)

Per-command requirement sets with predefined common sets:

- **RequirementSet**: Named set of required capabilities
- **Predefined sets**: READ_ONLY, NAVIGATION, MUTATING, REMOTE, RECOVERY, MINIMAL
- **GateResult**: Either `Ready(Box<ReadyContext>)` or `NeedsRepair(RepairBundle)`
- **Box optimization**: ReadyContext is boxed to reduce enum size (clippy recommendation)

### 5. Plan Steps (`src/engine/plan.rs`)

Typed, serializable plan steps:

- **CAS semantics**: UpdateRefCas, DeleteRefCas, WriteMetadataCas, DeleteMetadataCas
- **Git operations**: RunGit for shell-outs with expected effects
- **Checkpoints**: Named checkpoints for recovery
- **Box optimization**: BranchMetadataV1 boxed in WriteMetadataCas to reduce enum size

### 6. Executor (`src/engine/exec.rs`)

Full transactional executor implementing the ARCHITECTURE.md contract:

- **Locking**: Acquires RepoLock before mutations
- **Op-state marker**: Writes `.git/lattice/op-state.json` tracking in-progress operation
- **Intent recording**: Appends IntentRecorded event before executing
- **CAS execution**: All ref updates use compare-and-swap
- **Journal recording**: Writes journal entry for each step
- **Commit event**: Appends Committed event with new fingerprint on success

### 7. Fast Verify (`src/engine/verify.rs`)

Post-execution invariant checking:

- **Cycle detection**: Verifies stack graph is acyclic
- **Branch existence**: All tracked branches must exist as local refs
- **Base ancestry**: Base must be ancestor of tip, reachable from parent
- **Freeze state**: Validates frozen branches have proper scope

### 8. Engine Lifecycle (`src/engine/mod.rs`)

Wired up the full lifecycle:

- **Context struct**: Carries debug/quiet flags and cwd override
- **execute_hello**: Demonstrates full Scan → Gate → Plan → Execute → Verify flow
- **run_lifecycle**: Generic lifecycle runner for future commands

## Files Created/Modified

### New Files
- `src/engine/capabilities.rs` - Capability enum and CapabilitySet
- `src/engine/health.rs` - Health report, issues, severity
- `src/engine/ledger.rs` - Event ledger for audit trail

### Major Rewrites
- `src/engine/scan.rs` - Full RepoSnapshot with capabilities and fingerprint
- `src/engine/gate.rs` - Requirement sets and gating
- `src/engine/plan.rs` - Typed plan steps with CAS
- `src/engine/exec.rs` - Full transactional executor
- `src/engine/verify.rs` - Post-execution verification
- `src/engine/mod.rs` - Lifecycle orchestration

### Minor Updates
- `src/cli/commands/mod.rs` - Removed duplicate hello print

## Clippy Fixes Applied

1. **large_enum_variant**: Boxed `ReadyContext` in `GateResult` and `BranchMetadataV1` in `PlanStep::WriteMetadataCas`
2. **needless_borrows_for_generic_args**: Removed unnecessary borrow in `plan.digest()` call
3. **redundant_closure**: Changed `.map(|s| Oid::new(s))` to `.map(Oid::new)`
4. **for_kv_map**: Changed `for (_branch, scanned) in &map` to `for scanned in map.values()`

## Acceptance Gates

All gates pass:
- [x] `cargo fmt --check`
- [x] `cargo clippy -- -D warnings`
- [x] `cargo test` (325 unit tests + integration tests)
- [x] `cargo doc --no-deps`
- [x] `cargo run -- hello` outputs "Hello from Lattice!"

## Architecture Conformance

This implementation follows ARCHITECTURE.md Section 4 (Validated Execution Model):

1. **Scanner** produces `RepoSnapshot` with capabilities and health report
2. **Gating** checks capabilities against command requirements
3. **Planner** produces typed `Plan` with CAS steps
4. **Executor** is the single transactional write path
5. **Verifier** confirms post-conditions hold

The Event Ledger implements ARCHITECTURE.md Section 7 (Divergence Detection) with fingerprint-based change tracking.
