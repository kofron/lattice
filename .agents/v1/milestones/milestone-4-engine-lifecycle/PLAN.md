# Milestone 4: Engine Lifecycle (Scan -> Gate -> Plan -> Execute -> Verify)

## Status: PLANNED

---

## Overview

**Goal:** Build the architecture "spine" - the validated execution model that all commands flow through. This transforms the stub engine modules into a fully functional lifecycle that enforces the correctness-by-design principles from ARCHITECTURE.md.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs, Tests are everything.

**Dependencies:**
- Milestone 0 (complete) - Crate structure
- Milestone 1 (complete) - Core domain types
- Milestone 2 (complete) - Git interface with CAS
- Milestone 3 (complete) - Persistence layer (MetadataStore, Journal, RepoLock)

---

## Architecture Context

Per ARCHITECTURE.md Section 4 and ROADMAP.md Section 4.1-4.6:

1. **Scanner**: Reads repository state, produces `RepoSnapshot` with capabilities and health report
2. **Event Ledger**: Append-only commit chain at `refs/lattice/event-log` for divergence detection
3. **Gating**: Per-command requirement sets, produces `ReadyContext` or `RepairBundle`
4. **Planner**: Pure, deterministic plan generation from validated context
5. **Executor**: Single transactional write path with CAS, journaling, and op-state
6. **Fast Verify**: Post-execution invariant checking

The key insight: **There is no global "repo is valid" boolean. Each command has its own validation contract.**

---

## Acceptance Gates

### Functional Gates
- [ ] Scanner produces `RepoSnapshot` with refs, metadata, config, and capabilities
- [ ] Scanner detects in-progress Git operations (rebase/merge/etc.)
- [ ] Scanner detects Lattice op-state marker and refuses conflicting operations
- [ ] Scanner computes repository fingerprint
- [ ] Event ledger records `IntentRecorded`, `Committed`, `Aborted`, `DivergenceObserved`
- [ ] Event ledger uses CAS for append-only updates
- [ ] Gating produces `ReadyContext` when requirements satisfied
- [ ] Gating produces `RepairBundle` when requirements not satisfied
- [ ] Planner generates deterministic, serializable plans
- [ ] Executor acquires lock before mutations
- [ ] Executor writes op-state marker before first mutation
- [ ] Executor records `IntentRecorded` before mutations
- [ ] Executor applies all ref updates with CAS semantics
- [ ] Executor aborts cleanly when CAS fails (no partial state)
- [ ] Executor records `Committed` on success, clears op-state
- [ ] Fast verify checks core invariants after execution

### Quality Gates
- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes (target: 350+ tests)
- [ ] `cargo doc --no-deps` succeeds
- [ ] All public types have doctests

### Architectural Gates
- [ ] Scanner is read-only (never mutates repository)
- [ ] Planner is pure (no I/O, no mutations)
- [ ] Executor is the ONLY component that mutates repository
- [ ] All capabilities are composable proofs (not partial states)
- [ ] Event ledger is evidence, not authority

---

## Implementation Steps

### Step 1: Define Capability System

Create the capability type system that represents what is known to be true about the repository.

**File:** `src/engine/capabilities.rs` (new)

```rust
/// Capabilities are composable proofs about repository state.
/// A capability either exists or does not - there is no "partial" capability.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Repository can be opened
    RepoOpen,
    /// Trunk branch is configured
    TrunkKnown,
    /// No Lattice operation in progress
    NoLatticeOpInProgress,
    /// No external Git operation in progress (rebase/merge/etc.)
    NoExternalGitOpInProgress,
    /// All metadata refs are readable and parseable
    MetadataReadable,
    /// Stack graph is valid (acyclic, all branches exist)
    GraphValid,
    /// Working copy state is known
    WorkingCopyStateKnown,
    /// Authentication is available for remote operations
    AuthAvailable,
    /// Remote is configured and resolvable
    RemoteResolved,
    /// Frozen policy is satisfied for target branches
    FrozenPolicySatisfied,
}

/// A set of capabilities.
#[derive(Debug, Clone, Default)]
pub struct CapabilitySet {
    capabilities: HashSet<Capability>,
}

impl CapabilitySet {
    pub fn has(&self, cap: &Capability) -> bool;
    pub fn has_all(&self, caps: &[Capability]) -> bool;
    pub fn insert(&mut self, cap: Capability);
    pub fn missing(&self, required: &[Capability]) -> Vec<Capability>;
}
```

**Tests:**
- `capability_set_insert_and_check`
- `capability_set_has_all`
- `capability_set_missing`

---

### Step 2: Define Health Report and Issues

Create the issue system for tracking problems found during scanning.

**File:** `src/engine/health.rs` (new)

```rust
/// Severity of an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    /// Blocks command execution
    Blocking,
    /// Warning but doesn't block
    Warning,
    /// Informational only
    Info,
}

/// A stable, deterministic issue identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueId(String);

/// Evidence supporting an issue.
#[derive(Debug, Clone)]
pub enum Evidence {
    /// A ref that's involved
    Ref { name: String, oid: Option<String> },
    /// A parse error
    ParseError { ref_name: String, message: String },
    /// A cycle in the graph
    Cycle { branches: Vec<String> },
    /// Missing branch
    MissingBranch { name: String },
    /// Git state
    GitState { state: String },
}

/// An issue found during scanning.
#[derive(Debug, Clone)]
pub struct Issue {
    pub id: IssueId,
    pub severity: Severity,
    pub message: String,
    pub evidence: Vec<Evidence>,
    /// Which capabilities this issue blocks
    pub blocks: Vec<Capability>,
}

/// Repository health report from scanning.
#[derive(Debug, Clone, Default)]
pub struct RepoHealthReport {
    pub issues: Vec<Issue>,
    pub capabilities: CapabilitySet,
}
```

**Tests:**
- `issue_id_deterministic`
- `health_report_aggregates_issues`
- `blocking_issues_reduce_capabilities`

---

### Step 3: Define Repository Snapshot

Expand `RepoSnapshot` to contain all scanned state.

**File:** `src/engine/scan.rs` (modify)

```rust
/// Complete snapshot of repository state.
#[derive(Debug)]
pub struct RepoSnapshot {
    /// Repository info (paths)
    pub info: RepoInfo,
    /// Current Git state (rebase/merge/etc.)
    pub git_state: GitState,
    /// Worktree status
    pub worktree_status: WorktreeStatus,
    /// Current branch (if on a branch)
    pub current_branch: Option<BranchName>,
    /// All local branches with their tips
    pub branches: HashMap<BranchName, Oid>,
    /// All metadata entries (tracked branches)
    pub metadata: HashMap<BranchName, MetadataEntry>,
    /// Repository config
    pub repo_config: Option<RepoConfig>,
    /// Stack graph derived from metadata
    pub graph: StackGraph,
    /// Repository fingerprint
    pub fingerprint: Fingerprint,
    /// Health report with issues and capabilities
    pub health: RepoHealthReport,
}
```

**New functions:**
- `scan(git: &Git, config: Option<&GlobalConfig>) -> Result<RepoSnapshot, ScanError>`
- `compute_fingerprint(branches: &HashMap<BranchName, Oid>, metadata: &HashMap<BranchName, Oid>, trunk: Option<&BranchName>) -> Fingerprint`

**Tests:**
- `scan_empty_repo`
- `scan_with_tracked_branches`
- `scan_detects_git_operation_in_progress`
- `scan_detects_lattice_op_in_progress`
- `scan_builds_stack_graph`
- `fingerprint_deterministic`
- `fingerprint_changes_when_refs_change`

---

### Step 4: Implement Event Ledger

Create the append-only event log stored in Git.

**File:** `src/engine/ledger.rs` (new)

Per ARCHITECTURE.md Section 3.4 and ROADMAP.md Section 4.2:

```rust
/// Event categories for the ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Intent to perform an operation was recorded
    IntentRecorded {
        op_id: String,
        command: String,
        plan_digest: String,
        fingerprint_before: String,
    },
    /// Operation completed successfully
    Committed {
        op_id: String,
        fingerprint_after: String,
    },
    /// Operation was aborted
    Aborted {
        op_id: String,
        reason: String,
    },
    /// Out-of-band divergence detected
    DivergenceObserved {
        prior_fingerprint: String,
        current_fingerprint: String,
        changed_refs: Vec<String>,
    },
    /// Doctor proposed a repair
    DoctorProposed {
        issue_ids: Vec<String>,
        fix_ids: Vec<String>,
    },
    /// Doctor applied a repair
    DoctorApplied {
        fix_ids: Vec<String>,
        fingerprint_after: String,
    },
}

/// The event ledger stored at refs/lattice/event-log.
pub struct EventLedger<'a> {
    git: &'a Git,
}

impl<'a> EventLedger<'a> {
    pub fn new(git: &'a Git) -> Self;
    
    /// Append an event to the ledger with CAS.
    pub fn append(&self, event: Event) -> Result<Oid, LedgerError>;
    
    /// Read the most recent event.
    pub fn latest(&self) -> Result<Option<(Oid, Event)>, LedgerError>;
    
    /// Read the last N events.
    pub fn recent(&self, count: usize) -> Result<Vec<Event>, LedgerError>;
    
    /// Get the fingerprint from the last Committed event.
    pub fn last_committed_fingerprint(&self) -> Result<Option<String>, LedgerError>;
}
```

Event storage: Each event is a commit with:
- Tree containing `event.json` blob
- Parent pointing to previous event commit
- Ref `refs/lattice/event-log` updated with CAS

**Tests:**
- `ledger_append_first_event`
- `ledger_append_multiple_events`
- `ledger_cas_fails_on_concurrent_write`
- `ledger_read_latest`
- `ledger_read_recent`
- `ledger_last_committed_fingerprint`

---

### Step 5: Implement Divergence Detection

Detect when repository changed outside Lattice.

**File:** `src/engine/scan.rs` (add to scan)

```rust
/// Check for divergence from last committed fingerprint.
pub fn detect_divergence(
    git: &Git,
    current_fingerprint: &Fingerprint,
) -> Result<Option<DivergenceInfo>, ScanError>;

#[derive(Debug, Clone)]
pub struct DivergenceInfo {
    pub prior_fingerprint: String,
    pub current_fingerprint: String,
    pub changed_refs: Vec<String>,
}
```

Called during scan. If divergence detected, records `DivergenceObserved` event.

**Tests:**
- `no_divergence_when_fingerprints_match`
- `divergence_detected_when_refs_change`
- `divergence_records_changed_refs`

---

### Step 6: Define Command Requirement Sets

Create the requirement system for gating.

**File:** `src/engine/gate.rs` (modify)

```rust
/// Requirements for a command to execute.
#[derive(Debug, Clone)]
pub struct RequirementSet {
    /// Required capabilities
    pub capabilities: Vec<Capability>,
    /// Human-readable command name
    pub command_name: &'static str,
}

impl RequirementSet {
    /// Check if requirements are satisfied.
    pub fn check(&self, caps: &CapabilitySet) -> GateResult;
}

/// Result of gating check.
pub enum GateResult {
    /// Ready to execute with validated context
    Ready(ReadyContext),
    /// Repair needed before execution
    NeedsRepair(RepairBundle),
}

/// Validated context for a command.
#[derive(Debug)]
pub struct ReadyContext {
    /// The snapshot (for reference)
    pub snapshot: RepoSnapshot,
    /// Command-specific validated data
    pub validated: ValidatedData,
}

/// Command-specific validated data.
#[derive(Debug)]
pub enum ValidatedData {
    /// No specific data needed (e.g., info commands)
    None,
    /// Scope-resolved data for stack operations
    StackScope {
        trunk: BranchName,
        branches: Vec<BranchName>,
    },
}

/// Bundle of issues requiring repair.
#[derive(Debug)]
pub struct RepairBundle {
    pub command: String,
    pub missing_capabilities: Vec<Capability>,
    pub blocking_issues: Vec<Issue>,
}
```

**Predefined requirement sets:**

```rust
pub mod requirements {
    /// Read-only commands (log, info, parent, children)
    pub const READ_ONLY: RequirementSet = RequirementSet {
        capabilities: vec![Capability::RepoOpen],
        command_name: "read-only",
    };
    
    /// Commands that need the stack graph (checkout, up, down)
    pub const NAVIGATION: RequirementSet = RequirementSet {
        capabilities: vec![
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::MetadataReadable,
            Capability::GraphValid,
        ],
        command_name: "navigation",
    };
    
    /// Mutating commands (create, track, restack, etc.)
    pub const MUTATING: RequirementSet = RequirementSet {
        capabilities: vec![
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::NoLatticeOpInProgress,
            Capability::NoExternalGitOpInProgress,
            Capability::MetadataReadable,
            Capability::GraphValid,
            Capability::FrozenPolicySatisfied,
        ],
        command_name: "mutating",
    };
    
    /// Remote commands (submit, sync, get)
    pub const REMOTE: RequirementSet = RequirementSet {
        capabilities: vec![
            // All of MUTATING plus:
            Capability::RemoteResolved,
            Capability::AuthAvailable,
        ],
        command_name: "remote",
    };
}
```

**Tests:**
- `gate_passes_when_all_capabilities_present`
- `gate_fails_when_capability_missing`
- `repair_bundle_contains_blocking_issues`
- `ready_context_contains_snapshot`

---

### Step 7: Expand Plan Types

Enhance the plan system with typed steps.

**File:** `src/engine/plan.rs` (modify)

```rust
/// A typed plan step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanStep {
    /// Update a ref with CAS
    UpdateRefCas {
        refname: String,
        old_oid: Option<String>,
        new_oid: String,
        reason: String,
    },
    /// Delete a ref with CAS
    DeleteRefCas {
        refname: String,
        old_oid: String,
        reason: String,
    },
    /// Write metadata with CAS
    WriteMetadataCas {
        branch: String,
        old_ref_oid: Option<String>,
        metadata: BranchMetadataV1,
    },
    /// Run a git command
    RunGit {
        args: Vec<String>,
        description: String,
        expected_effects: Vec<String>,
    },
    /// Checkpoint for recovery
    Checkpoint {
        name: String,
    },
}

/// A complete execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Operation ID (for journal correlation)
    pub op_id: OpId,
    /// Command that generated this plan
    pub command: String,
    /// Ordered steps to execute
    pub steps: Vec<PlanStep>,
    /// Refs that will be touched (for CAS validation)
    pub touched_refs: Vec<String>,
}

impl Plan {
    /// Compute a digest of the plan for integrity checking.
    pub fn digest(&self) -> String;
    
    /// Check if the plan is empty (no-op).
    pub fn is_empty(&self) -> bool;
    
    /// Get a preview string for user confirmation.
    pub fn preview(&self) -> String;
}
```

**Tests:**
- `plan_digest_deterministic`
- `plan_digest_changes_with_content`
- `plan_preview_formatting`
- `plan_serialization_roundtrip`

---

### Step 8: Implement Full Executor

Transform the stub executor into the real transactional executor.

**File:** `src/engine/exec.rs` (modify)

```rust
/// Execution result.
pub enum ExecuteResult {
    /// Plan executed successfully
    Success {
        /// Post-execution fingerprint
        fingerprint: Fingerprint,
    },
    /// Execution paused for conflict resolution
    Paused {
        /// Branch with conflict
        branch: String,
        /// Git state (rebase, merge, etc.)
        git_state: GitState,
        /// Remaining work
        remaining_steps: Vec<PlanStep>,
    },
    /// Execution aborted due to error
    Aborted {
        /// Error that caused abort
        error: ExecuteError,
        /// Steps that were successfully applied (for potential undo)
        applied_steps: Vec<PlanStep>,
    },
}

/// The executor.
pub struct Executor<'a> {
    git: &'a Git,
    ledger: &'a EventLedger<'a>,
}

impl<'a> Executor<'a> {
    pub fn new(git: &'a Git, ledger: &'a EventLedger<'a>) -> Self;
    
    /// Execute a plan transactionally.
    ///
    /// This is the ONLY mutation path in Lattice.
    pub fn execute(&self, plan: &Plan, ctx: &Context) -> Result<ExecuteResult, ExecuteError>;
}
```

**Executor contract (from ARCHITECTURE.md):**
1. Acquire lock
2. Write op-state marker
3. Record `IntentRecorded` event
4. For each step:
   - Apply with CAS
   - If CAS fails: abort, record `Aborted`, return error
   - If conflict: pause, update op-state, return Paused
   - Record step in journal with fsync
5. Re-scan and verify
6. Record `Committed` event
7. Remove op-state marker
8. Release lock

**Tests:**
- `executor_acquires_lock`
- `executor_writes_op_state_before_mutation`
- `executor_records_intent_event`
- `executor_applies_ref_update_cas`
- `executor_aborts_on_cas_failure`
- `executor_records_committed_on_success`
- `executor_clears_op_state_on_success`
- `executor_pauses_on_conflict`
- `executor_rollback_on_error`

---

### Step 9: Implement Fast Verify

Post-execution invariant checking.

**File:** `src/engine/verify.rs` (modify)

```rust
/// Verification errors.
#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("metadata unparseable for {branch}: {message}")]
    MetadataUnparseable { branch: String, message: String },
    
    #[error("cycle detected in stack graph: {branches:?}")]
    CycleDetected { branches: Vec<String> },
    
    #[error("tracked branch does not exist: {branch}")]
    BranchMissing { branch: String },
    
    #[error("base not ancestor of tip for {branch}")]
    BaseNotAncestor { branch: String },
    
    #[error("base not reachable from parent for {branch}")]
    BaseNotReachableFromParent { branch: String },
}

/// Fast verification of core invariants.
///
/// Called after execution to confirm the repository is in a valid state.
pub fn fast_verify(git: &Git, snapshot: &RepoSnapshot) -> Result<(), VerifyError>;
```

**Invariants checked (from SPEC.md Section 2.1):**
1. All metadata is parseable
2. Stack graph is acyclic
3. All tracked branches exist as local refs
4. For each tracked branch: base is ancestor of tip
5. For each tracked branch: base is reachable from parent tip
6. Freeze state is structurally valid

**Tests:**
- `verify_passes_valid_repo`
- `verify_fails_unparseable_metadata`
- `verify_fails_cycle`
- `verify_fails_missing_branch`
- `verify_fails_base_not_ancestor`
- `verify_fails_base_not_reachable_from_parent`

---

### Step 10: Wire Up Engine Lifecycle

Update the engine module to orchestrate the full lifecycle.

**File:** `src/engine/mod.rs` (modify)

```rust
pub mod capabilities;
pub mod health;
pub mod ledger;

// Re-exports
pub use capabilities::{Capability, CapabilitySet};
pub use health::{Issue, IssueId, RepoHealthReport, Severity};
pub use ledger::{Event, EventLedger};

/// Execute a command through the full lifecycle.
pub fn execute_command<C: Command>(
    command: &C,
    git: &Git,
    ctx: &Context,
) -> Result<C::Output, EngineError>;

/// The Command trait for lifecycle integration.
pub trait Command {
    type Output;
    
    /// Get the requirement set for this command.
    fn requirements(&self) -> &RequirementSet;
    
    /// Generate a plan from validated context.
    fn plan(&self, ready: &ReadyContext) -> Result<Plan, PlanError>;
    
    /// Process execution result into command output.
    fn finish(&self, result: ExecuteResult) -> Result<Self::Output, CommandError>;
}
```

**Tests:**
- `lifecycle_scans_before_gate`
- `lifecycle_gates_before_plan`
- `lifecycle_plans_before_execute`
- `lifecycle_verifies_after_execute`
- `lifecycle_handles_gate_failure`
- `lifecycle_handles_execute_failure`

---

### Step 11: Create Integration Tests

Comprehensive tests exercising the full lifecycle.

**File:** `tests/engine_integration.rs` (new)

**Test scenarios:**
1. Simple ref update through full lifecycle
2. Multiple ref updates in single plan
3. CAS failure mid-execution (simulated concurrent change)
4. Divergence detection and event recording
5. Op-state prevents concurrent operations
6. Crash recovery (op-state present on startup)

---

### Step 12: Update Hello Command

Update the hello command to use the real lifecycle.

**File:** `src/engine/mod.rs` (modify `execute_hello`)

The hello command becomes a real lifecycle test that:
1. Scans the repository
2. Gates with minimal requirements
3. Creates an empty plan
4. Executes (no-op)
5. Verifies
6. Prints "Hello from Lattice!"

---

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `src/engine/capabilities.rs` | Create | Capability type system |
| `src/engine/health.rs` | Create | Health report and issues |
| `src/engine/ledger.rs` | Create | Event ledger implementation |
| `src/engine/scan.rs` | Modify | Full scanner with RepoSnapshot |
| `src/engine/gate.rs` | Modify | Gating with requirement sets |
| `src/engine/plan.rs` | Modify | Typed plan steps |
| `src/engine/exec.rs` | Modify | Full transactional executor |
| `src/engine/verify.rs` | Modify | Fast verify implementation |
| `src/engine/mod.rs` | Modify | Lifecycle orchestration |
| `src/core/graph.rs` | Modify | Add methods for verification |
| `tests/engine_integration.rs` | Create | Integration tests |

---

## Test Count Target

| Category | Count |
|----------|-------|
| Existing tests | ~263 |
| Capability tests | ~10 |
| Health/Issue tests | ~10 |
| Scanner tests | ~15 |
| Ledger tests | ~15 |
| Gating tests | ~12 |
| Plan tests | ~10 |
| Executor tests | ~20 |
| Verify tests | ~10 |
| Integration tests | ~15 |
| **Target Total** | **~380** |

---

## Implementation Sequence

Recommended order to maintain working code:

1. **Step 1-2**: Capabilities and Health (foundational types)
2. **Step 3**: RepoSnapshot (depends on 1-2)
3. **Step 4-5**: Event Ledger and Divergence (can be parallel with 3)
4. **Step 6**: Gating (depends on 1-3)
5. **Step 7**: Plan types (independent)
6. **Step 8**: Executor (depends on 4, 7)
7. **Step 9**: Fast Verify (depends on 3)
8. **Step 10**: Wire up lifecycle (depends on all above)
9. **Step 11-12**: Integration tests and hello command update

---

## Notes

### CAS Everywhere

All ref mutations use CAS semantics. The executor validates preconditions before each step. This ensures:
- No partial updates if repository changes mid-operation
- Clean abort with rollback information
- Deterministic behavior even with concurrent access

### Event Ledger as Evidence

The event ledger is **evidence, not authority**. It records what Lattice intended and observed, but does not replace repository state as the source of truth. Divergence detection uses it to know when out-of-band changes occurred.

### Capability Proofs

Capabilities are boolean proofs. Either the scanner can establish a capability or it cannot. There is no "partial" capability - that would be represented as absence plus an issue in the health report.

### Doctor Integration

This milestone prepares for but does not implement Doctor. When gating fails, it produces a `RepairBundle` that Doctor (Milestone 5) will consume. For now, gating failure returns an error to the user.

---

## Next Steps (Milestone 5)

Per ROADMAP.md, proceed to **Milestone 5: Doctor Framework**:
- Issue catalog with stable IDs
- Fix options with repair plans
- Explicit confirmation model
- Doctor command and doctor-as-handoff
