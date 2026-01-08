# Milestone 5: Doctor Framework - Implementation Plan

## Summary

Implement the Doctor framework as specified in ARCHITECTURE.md Section 8. Doctor is the unified repair broker - not a special-case command, but the standard way to handle repository issues. It uses the same scanner, planner, and executor as regular commands.

**Core principle from ARCHITECTURE.md:** "Doctor MUST never apply a fix without explicit confirmation."

---

## Architecture Conformance

Per ARCHITECTURE.md Section 8:

1. **Doctor is a framework, not a special-case command** (8.1)
   - Shares scanner, planner model, executor, event recording
   - No separate "repair mutation path"

2. **Issues and fix options** (8.2)
   - `IssueId` (stable, deterministic from evidence)
   - Severity (Blocking, Warning, Info)
   - Evidence (refs, object ids, parse failures, cycle traces)
   - One or more `FixOption`s per issue

3. **Confirmation model** (8.3)
   - Interactive: present issues and fix options, user selects, preview, confirm
   - Non-interactive: emit issues with ids, apply only explicitly provided fix ids
   - **Never auto-select fixes**

4. **Repair outcomes** (8.4)
   - After applying, perform full post-verify
   - Record `DoctorApplied` in event ledger
   - If conflicts arise, transition to `awaiting_user` op-state

---

## Issue Catalog (Per ROADMAP Section 5.1)

Implement deterministic issues with stable IDs:

| Issue | ID Pattern | Severity | Fix Options |
|-------|------------|----------|-------------|
| Missing trunk config | `trunk-not-configured` | Blocking | Set trunk (prompt or `--trunk`) |
| Metadata parse failure | `metadata-parse-error:{branch}` | Blocking | Untrack+retrack, clear metadata |
| Parent ref missing | `parent-missing:{child}` | Blocking | Reparent to trunk, reparent to nearest ancestor |
| Cycle detected | `graph-cycle:{branches}` | Blocking | Break cycle by untracking, reparent |
| Base ancestry violated | `base-not-ancestor:{branch}` | Warning | Recompute base, force update |
| Orphaned metadata | `orphaned-metadata:{branch}` | Warning | Delete metadata ref |
| Branch exists without metadata | `untracked-branch:{branch}` | Info | Track branch, ignore |
| Lattice op in progress | `lattice-op-in-progress:{op_id}` | Blocking | Continue, abort |
| External Git op in progress | `git-op-in-progress:{state}` | Blocking | Complete via git, abort |
| Config file migration needed | `config-migration:{path}` | Warning | Migrate to canonical path |

---

## Implementation Steps

### Step 1: Extend Issue Types in `engine/health.rs`

Add missing issue factory functions:
- `orphaned_metadata(branch)` - metadata ref exists but branch doesn't
- `config_migration_needed(old_path, new_path)` - repo.toml → config.toml

### Step 2: Create `doctor/fixes.rs` - Fix Option Types

```rust
// FixId - stable identifier for a fix option
pub struct FixId(String);

// FixOption - a concrete repair option
pub struct FixOption {
    pub id: FixId,
    pub issue_id: IssueId,
    pub description: String,
    pub preview: FixPreview,
    pub preconditions: Vec<Capability>,
}

// FixPreview - what the fix will do
pub struct FixPreview {
    pub ref_changes: Vec<RefChange>,
    pub metadata_changes: Vec<MetadataChange>,
    pub config_changes: Vec<ConfigChange>,
}

// RefChange, MetadataChange, ConfigChange - preview items
pub enum RefChange {
    Update { ref_name: String, old_oid: Option<String>, new_oid: String },
    Delete { ref_name: String, old_oid: String },
    Create { ref_name: String, new_oid: String },
}
```

### Step 3: Create `doctor/generators.rs` - Fix Generators

Each issue type maps to one or more fix generators:

```rust
// Trait for generating fixes from issues
pub trait FixGenerator {
    fn generate(&self, issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption>;
}

// Implementations
pub struct TrunkNotConfiguredFixer;
pub struct MetadataParseErrorFixer;
pub struct ParentMissingFixer;
pub struct CycleDetectedFixer;
pub struct BaseAncestryFixer;
pub struct OrphanedMetadataFixer;
pub struct OperationInProgressFixer;
pub struct ExternalGitOpFixer;
pub struct ConfigMigrationFixer;
```

### Step 4: Create `doctor/planner.rs` - Repair Plan Generation

Convert fix options to executable plans:

```rust
// Generate a Plan from selected FixOptions
pub fn generate_repair_plan(
    fixes: &[&FixOption],
    snapshot: &RepoSnapshot,
) -> Result<Plan, RepairPlanError>;
```

### Step 5: Create `doctor/mod.rs` - Doctor Struct

The main Doctor orchestrator:

```rust
pub struct Doctor {
    // Configuration
    interactive: bool,
}

impl Doctor {
    // Analyze repository and return issues with fixes
    pub fn diagnose(&self, snapshot: &RepoSnapshot) -> DiagnosisReport;
    
    // Apply selected fixes (by FixId)
    pub fn apply_fixes(
        &self,
        fix_ids: &[FixId],
        snapshot: &RepoSnapshot,
        git: &Git,
    ) -> Result<RepairOutcome, DoctorError>;
    
    // Interactive repair session
    pub fn interactive_repair(
        &self,
        snapshot: &RepoSnapshot,
        git: &Git,
    ) -> Result<RepairOutcome, DoctorError>;
}

pub struct DiagnosisReport {
    pub issues: Vec<Issue>,
    pub fixes: Vec<FixOption>,
    pub summary: DiagnosisSummary,
}

pub struct RepairOutcome {
    pub applied_fixes: Vec<FixId>,
    pub events_recorded: Vec<EventId>,
    pub final_health: RepoHealthReport,
}
```

### Step 6: Integrate with Gate - RepairBundle

Update gating to return a `RepairBundle` that can be passed to Doctor:

```rust
// In engine/gate.rs
pub struct RepairBundle {
    pub blocking_issues: Vec<Issue>,
    pub available_fixes: Vec<FixOption>,
    pub original_command: String,
}

// Gate handoff
pub fn handoff_to_doctor(bundle: RepairBundle) -> DoctorSession;
```

### Step 7: Add `doctor` CLI Command

In `cli/commands/mod.rs`:

```rust
// lattice doctor [--fix <fix-id>...] [--all] [--dry-run]
pub fn doctor(ctx: &Context, fix_ids: &[String], all: bool, dry_run: bool) -> Result<()>;
```

Flags:
- `--fix <id>`: Apply specific fix by ID (can repeat)
- `--all`: Apply all available fixes (requires confirmation in interactive)
- `--dry-run`: Show what would be done without applying
- No flags in interactive: prompt for selections

### Step 8: Update CLI Args

In `cli/args.rs`, add the doctor subcommand with its flags.

### Step 9: Event Ledger Integration

Ensure `DoctorProposed` and `DoctorApplied` events are recorded:

```rust
// In engine/ledger.rs - already defined but ensure usage
LedgerEvent::DoctorProposed { issues, fixes }
LedgerEvent::DoctorApplied { fixes_applied, outcome }
```

### Step 10: Tests

#### Unit Tests
- Fix generators produce correct options for each issue type
- Fix IDs are deterministic
- Repair plans are valid
- Confirmation model enforced (non-interactive without fix IDs does nothing)

#### Integration Tests
- Doctor diagnoses issues correctly
- Doctor applies fixes transactionally
- Doctor records events in ledger
- Gating hands off to doctor properly
- Interactive and non-interactive modes work correctly

---

## File Structure

```
src/doctor/
├── mod.rs           # Doctor struct and public API
├── issues.rs        # (existing) Extend with orphaned_metadata etc.
├── fixes.rs         # FixId, FixOption, FixPreview types
├── generators.rs    # Fix generators for each issue type
└── planner.rs       # Repair plan generation

src/cli/commands/mod.rs  # Add doctor command
src/cli/args.rs          # Add doctor subcommand args
src/engine/gate.rs       # RepairBundle for handoff
src/engine/ledger.rs     # Ensure DoctorProposed/Applied events
```

---

## Acceptance Gates

- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes
- [x] `cargo doc --no-deps` succeeds
- [x] For each blocking scanner issue, doctor offers fix options
- [x] No repair is applied without explicit confirmation (interactive) or explicit fix id (non-interactive)
- [ ] `DoctorApplied` event is recorded in ledger after repairs (deferred to future milestone - requires execution integration)

---

## Notes

- **Simplicity principle**: Start with the most common issues (trunk, metadata parse, parent missing, op in progress)
- **No stubs principle**: All fix generators must produce real, executable plans
- **Purity principle**: Fix generation is pure - takes issue + snapshot, returns options
- **Follow the leader**: Strictly following ARCHITECTURE.md Section 8 confirmation model
