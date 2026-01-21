# Milestone 0.1: Gating Integration + Scope Walking

## Status: PLANNED

---

## Overview

**Goal:** Wire the existing gating infrastructure into all commands so that the validated execution model from ARCHITECTURE.md is enforced. Additionally, complete the scope walking for `FrozenPolicySatisfied` capability derivation.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs, Tests are everything.

**Priority:** CRITICAL - Commands bypass all capability validation

---

## Problem Statement

The gating system at `src/engine/gate.rs` is **architecturally complete but completely disconnected from commands**. Every command follows this incorrect pattern:

```rust
let git = Git::open(&cwd)?;
let snapshot = scan(&git)?;  // Direct scan() call - BYPASSES gating
// Business logic proceeds without validation...
```

**Evidence:**
- Zero grep results for `gate(` in `src/cli/commands/`
- `ReadyContext` and `ValidatedData` types defined but never produced/consumed
- `FrozenPolicySatisfied` is unconditionally true at `scan.rs:385`
- TODO at `gate.rs:410`: "Walk graph to find all upstack branches"

**ARCHITECTURE.md violations:**
- Section 5.1: "A command executes only against a validated representation"
- Section 5.3: "Every non-doctor command declares its required capabilities"
- Section 12: Command lifecycle step 2 (Gate) is skipped entirely

---

## Spec References

- **ARCHITECTURE.md Section 5** - Validated execution model and capability gating
- **ARCHITECTURE.md Section 12** - Command lifecycle (7 steps)
- **SPEC.md Section 4.6.6** - Command category rules and capability requirements
- **SPEC.md Section 8B.4** - Freeze scope: "downstack ancestors up to trunk"
- **ROADMAP.md Milestone 0.1** - Detailed requirements and design questions

---

## Current State Analysis

### What Exists (Complete)
- `gate()` function at `src/engine/gate.rs:178`
- `gate_with_scope()` function at `src/engine/gate.rs:207`
- `RequirementSet` with predefined sets: `READ_ONLY`, `NAVIGATION`, `MUTATING`, `MUTATING_METADATA_ONLY`, `REMOTE`, `REMOTE_BARE_ALLOWED`, `RECOVERY`, `MINIMAL`
- `GateResult` enum: `Ready(ReadyContext)` or `NeedsRepair(RepairBundle)`
- `ReadyContext` struct with `snapshot` and `ValidatedData`
- All 11 capabilities defined in `src/engine/capabilities.rs`
- Scanner produces `RepoSnapshot` with `health.capabilities`

### What's Missing / Broken
- **No command calls `gate()`** - gating is dead code
- **Scope walking incomplete** - TODO at `gate.rs:410`
- **`FrozenPolicySatisfied` unconditional** - Always true at `scan.rs:385`
- **No Command trait** - Commands aren't type-enforced
- **No mode types** - Flag-dependent requirements not modeled
- **Module visibility not enforced** - Commands can import `scan()` directly

---

## Design Decisions

### D1: How to make "bypass is impossible" the default

**Decision:** Implement all three mechanisms from ROADMAP.md:
1. Restructure module visibility so commands cannot call `scan()` directly
2. Commands only receive `ReadyContext<C>` and never see raw snapshot
3. CI lint that fails if command module imports scanner

### D2: How to encode requirements per command

**Decision:** Use mode types for flag-dependent commands:
```rust
enum SubmitMode { WithRestack, NoRestack }
enum SyncMode { WithRestack, NoRestack }
enum GetMode { WithCheckout, NoCheckout }
```

Each mode has static requirements. Mode resolution happens before gating and may fail early for bare repos without explicit flags.

### D3: Doctor handoff for auth-type issues

**Decision:** Per ARCHITECTURE.md §8.2, `GateResult::Repair(bundle)` routes to doctor. Doctor presents fix options including user-action fixes that have no executor plan, just instructions (e.g., "run `lattice auth login`").

### D4: FrozenPolicySatisfied scope definition

**Decision:** Per SPEC.md §8B.4, freeze applies to "the target branch and its downstack ancestors up to trunk." For operations that modify upstack branches (restack, create --insert, move), the scope must include all branches that will be touched.

---

## Implementation Steps

### Phase 1: Foundation Types (No Behavioral Changes)

#### Step 1.1: Define Command Trait

**File:** `src/engine/command.rs` (NEW)

```rust
//! Command trait for lifecycle integration.
//!
//! Every command must implement this trait to participate in the
//! validated execution model from ARCHITECTURE.md Section 5.

use crate::engine::gate::{RequirementSet, ReadyContext};
use crate::engine::plan::Plan;

/// A command that can be executed through the engine lifecycle.
pub trait Command {
    /// The requirement set for this command.
    /// Must be a compile-time constant.
    const REQUIREMENTS: &'static RequirementSet;
    
    /// Output type produced by this command.
    type Output;
    
    /// Generate a plan from validated context.
    /// This is pure - no I/O, no mutations.
    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError>;
    
    /// Process execution result into command output.
    fn finish(&self, result: ExecuteResult) -> Result<Self::Output, CommandError>;
}
```

#### Step 1.2: Define Mode Types for Flag-Dependent Commands

**File:** `src/engine/modes.rs` (NEW)

```rust
//! Mode types for commands with flag-dependent requirements.
//!
//! Per ROADMAP.md, several commands have different requirement sets
//! depending on flags (--no-restack, --no-checkout) and repo context (bare).

use crate::engine::gate::RequirementSet;

/// Submit mode determines gating requirements.
#[derive(Debug, Clone, Copy)]
pub enum SubmitMode {
    /// Default: restack before submit (requires working directory)
    WithRestack,
    /// --no-restack: skip restack (bare-repo compatible)
    NoRestack,
}

impl SubmitMode {
    pub fn requirements(&self) -> &'static RequirementSet {
        match self {
            Self::WithRestack => &requirements::REMOTE,
            Self::NoRestack => &requirements::REMOTE_BARE_ALLOWED,
        }
    }
    
    /// Resolve mode from flags and repo context.
    /// Returns error if bare repo without explicit --no-restack.
    pub fn resolve(no_restack: bool, is_bare: bool) -> Result<Self, ModeError> {
        match (no_restack, is_bare) {
            (true, _) => Ok(Self::NoRestack),
            (false, false) => Ok(Self::WithRestack),
            (false, true) => Err(ModeError::BareRepoRequiresFlag {
                command: "submit",
                required_flag: "--no-restack",
            }),
        }
    }
}

// Similar for SyncMode and GetMode...
```

#### Step 1.3: Implement Scope Walking

**File:** `src/engine/gate.rs` (MODIFY)

Complete the TODO at line 410:

```rust
/// Compute the freeze scope for a branch operation.
/// 
/// Returns all branches that would be affected by an operation on `target`:
/// - The target branch itself
/// - All ancestors up to (but not including) trunk
/// - For upstack operations: all descendants that would be restacked
pub fn compute_freeze_scope(
    target: &BranchName,
    graph: &StackGraph,
    include_upstack: bool,
) -> Vec<BranchName> {
    let mut scope = Vec::new();
    
    // Always include target
    scope.push(target.clone());
    
    // Walk downstack (ancestors) to trunk
    let mut current = target.clone();
    while let Some(parent) = graph.parent(&current) {
        if graph.is_trunk(&parent) {
            break;
        }
        scope.push(parent.clone());
        current = parent;
    }
    
    // Optionally walk upstack (descendants)
    if include_upstack {
        fn collect_descendants(branch: &BranchName, graph: &StackGraph, out: &mut Vec<BranchName>) {
            for child in graph.children(branch) {
                out.push(child.clone());
                collect_descendants(&child, graph, out);
            }
        }
        collect_descendants(target, graph, &mut scope);
    }
    
    scope
}
```

Update `FrozenPolicySatisfied` capability derivation:

```rust
/// Check if frozen policy is satisfied for a scope.
pub fn check_frozen_policy(
    scope: &[BranchName],
    metadata: &HashMap<BranchName, ScannedMetadata>,
) -> Result<(), Vec<BranchName>> {
    let frozen: Vec<_> = scope
        .iter()
        .filter(|b| metadata.get(*b).map(|m| m.metadata.freeze.is_frozen()).unwrap_or(false))
        .cloned()
        .collect();
    
    if frozen.is_empty() {
        Ok(())
    } else {
        Err(frozen)
    }
}
```

---

### Phase 2: Engine Entrypoint

#### Step 2.1: Create Engine Runner

**File:** `src/engine/runner.rs` (NEW)

```rust
//! Engine runner - the only entry point for command execution.
//!
//! This module enforces the command lifecycle from ARCHITECTURE.md Section 12:
//! Scan -> Gate -> Repair (if needed) -> Plan -> Execute -> Verify -> Return

use crate::engine::scan::scan;
use crate::engine::gate::{gate, GateResult};
use crate::engine::command::Command;

/// Execute a command through the full lifecycle.
///
/// This is the ONLY way commands should execute. Direct use of `scan()`
/// from command modules is prohibited.
pub fn run_command<C: Command>(
    cmd: C,
    git: &Git,
    ctx: &Context,
) -> Result<C::Output, EngineError> {
    // Step 1: Scan (only place this happens)
    let snapshot = scan(git)?;
    
    // Step 2: Gate
    match gate(&snapshot, C::REQUIREMENTS) {
        GateResult::Ready(ready_ctx) => {
            // Step 4: Plan
            let plan = cmd.plan(&ready_ctx)?;
            
            if plan.is_empty() {
                // No-op, skip execution
                return cmd.finish(ExecuteResult::Success { 
                    fingerprint: snapshot.fingerprint.clone() 
                });
            }
            
            // Step 5: Execute
            let executor = Executor::new(git);
            let result = executor.execute(&plan, ctx)?;
            
            // Step 6: Verify (done inside executor)
            
            // Step 7: Return
            cmd.finish(result)
        }
        GateResult::NeedsRepair(bundle) => {
            // Step 3: Repair - route to doctor
            Err(EngineError::NeedsRepair(bundle))
        }
    }
}
```

#### Step 2.2: Update Module Visibility

**File:** `src/engine/mod.rs` (MODIFY)

```rust
// Make scan private to engine module
mod scan;  // NOT `pub mod scan`

// Public interface
pub mod command;
pub mod modes;
pub mod runner;

// Re-exports for commands
pub use runner::run_command;
pub use command::Command;
pub use modes::{SubmitMode, SyncMode, GetMode};
pub use gate::{ReadyContext, ValidatedData};
// Note: scan is NOT re-exported
```

---

### Phase 3: Command Migration (Incremental)

Each command must be converted to implement the `Command` trait. This is mechanical but touches every command file.

#### Step 3.1: Migration Pattern

**Before (current pattern in all commands):**
```rust
pub fn run_create(args: CreateArgs, ctx: &Context) -> Result<()> {
    let git = Git::open(&ctx.cwd)?;
    let snapshot = scan(&git)?;  // Direct scan - BAD
    
    // Manual checks
    if snapshot.trunk.is_none() {
        bail!("Trunk not configured");
    }
    // ... business logic
}
```

**After (gated pattern):**
```rust
pub struct CreateCommand {
    args: CreateArgs,
}

impl Command for CreateCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ();
    
    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        // ctx.snapshot is guaranteed valid per REQUIREMENTS
        // ctx.validated provides scope-specific data
        let trunk = ctx.snapshot.trunk.as_ref().unwrap(); // Safe - TrunkKnown satisfied
        // ... generate plan
    }
    
    fn finish(&self, result: ExecuteResult) -> Result<(), CommandError> {
        match result {
            ExecuteResult::Success { .. } => Ok(()),
            ExecuteResult::Paused { .. } => {
                println!("Paused for conflict resolution...");
                Ok(())
            }
            ExecuteResult::Aborted { error, .. } => Err(error.into()),
        }
    }
}

// Entry point called from CLI dispatch
pub fn run_create(args: CreateArgs, ctx: &Context) -> Result<()> {
    let git = Git::open(&ctx.cwd)?;
    let cmd = CreateCommand { args };
    run_command(cmd, &git, ctx)
}
```

#### Step 3.2: Migration Order (by complexity)

**Batch 1: Read-only commands (simplest, no plan needed)**
- `log`, `info`, `parent`, `children`, `trunk` (print only)
- These just need `READ_ONLY` requirements

**Batch 2: Metadata-only mutations**
- `init`, `track`, `untrack`, `freeze`, `unfreeze`, `unlink`
- Use `MUTATING_METADATA_ONLY` requirements

**Batch 3: Navigation commands**
- `checkout`, `up`, `down`, `top`, `bottom`
- Use `NAVIGATION` requirements

**Batch 4: Stack mutation commands**
- `create`, `modify`, `restack`, `move`, `reorder`, `split`, `squash`, `fold`, `pop`, `delete`, `rename`, `revert`
- Use `MUTATING` requirements with freeze scope checking

**Batch 5: Remote commands with modes**
- `submit`, `sync`, `get`, `merge`, `pr`
- Use mode types for flag-dependent requirements

**Batch 6: Recovery commands**
- `continue`, `abort`, `undo`
- Use `RECOVERY` requirements

---

### Phase 4: Testing Infrastructure

#### Step 4.1: Gating Matrix Tests

**File:** `tests/gating_matrix.rs` (NEW)

Table-driven tests anchoring intended semantics:

```rust
use test_case::test_case;

#[test_case("log", &[], Ok(()))]
#[test_case("log", &[missing(MetadataReadable)], Ok(degraded()))]
#[test_case("create", &[], Ok(()))]
#[test_case("create", &[missing(TrunkKnown)], Err(NeedsRepair))]
#[test_case("create", &[missing(WorkingDirectoryAvailable)], Err(NeedsRepair))]
#[test_case("restack", &[missing(FrozenPolicySatisfied)], Err(NeedsRepair))]
#[test_case("checkout", &[missing(WorkingDirectoryAvailable)], Err(NeedsRepair))]
#[test_case("submit", &[bare_repo()], Err(BareRepoRequiresFlag("--no-restack")))]
#[test_case("submit --no-restack", &[bare_repo()], Ok(()))]
fn gating_matrix(cmd: &str, conditions: &[Condition], expected: GateExpectation) {
    // Setup test repo with specified conditions
    // Run command through runner::run_command
    // Assert gate result matches expected
}
```

#### Step 4.2: Architecture Lint

**File:** `.github/workflows/ci.yml` (MODIFY or scripts/lint-arch.sh NEW)

```bash
#!/bin/bash
# Fail if any command module imports scan directly

if grep -r "use crate::engine::scan" src/cli/commands/; then
    echo "ERROR: Command modules must not import scan directly"
    echo "Use run_command() from engine::runner instead"
    exit 1
fi

if grep -r "scan(&git)" src/cli/commands/; then
    echo "ERROR: Command modules must not call scan() directly"
    exit 1
fi
```

#### Step 4.3: Scope Walking Tests

**File:** `src/engine/gate.rs` (ADD tests)

```rust
#[cfg(test)]
mod scope_tests {
    #[test]
    fn compute_freeze_scope_single_branch() {
        // Setup: trunk -> A -> B (target)
        // Expected: [B, A] (downstack only)
    }
    
    #[test]
    fn compute_freeze_scope_with_upstack() {
        // Setup: trunk -> A -> B (target) -> C -> D
        // Expected: [B, A, C, D] (include descendants)
    }
    
    #[test]
    fn compute_freeze_scope_branching_graph() {
        // Setup: trunk -> A -> B (target)
        //                 \-> C -> D
        // Expected with upstack: depends on what operation
    }
    
    #[test]
    fn frozen_policy_blocks_frozen_branch() {
        // Setup: trunk -> A (frozen) -> B (target)
        // Expected: FrozenPolicySatisfied NOT satisfied
    }
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/engine/command.rs` | NEW | Command trait definition |
| `src/engine/modes.rs` | NEW | Mode types for flag-dependent commands |
| `src/engine/runner.rs` | NEW | Engine entrypoint enforcing lifecycle |
| `src/engine/mod.rs` | MODIFY | Module visibility, re-exports |
| `src/engine/gate.rs` | MODIFY | Scope walking, frozen policy check |
| `src/engine/scan.rs` | MODIFY | Remove unconditional FrozenPolicySatisfied |
| `src/cli/commands/*.rs` | MODIFY | All 50+ command files migrate to Command trait |
| `tests/gating_matrix.rs` | NEW | Table-driven gating tests |
| `scripts/lint-arch.sh` | NEW | Architecture enforcement lint |

---

## Acceptance Gates

Per ROADMAP.md and ARCHITECTURE.md:

- [ ] Every command implements `Command` trait with declared requirements
- [ ] Mode types used for flag-dependent commands (submit/sync/get)
- [ ] Commands receive `ReadyContext` not raw snapshot
- [ ] `scan()` is private to engine; commands cannot import it
- [ ] `GateResult::NeedsRepair` routes to doctor (not silent failure)
- [ ] Scope walking implemented and used by `FrozenPolicySatisfied`
- [ ] Manual capability checks removed (replaced by gating)
- [ ] Gating matrix test covers all command × capability combinations
- [ ] Architecture lint prevents future bypass
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Rubric

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `scope_walking_downstack` | `gate.rs` | Walks ancestors to trunk |
| `scope_walking_upstack` | `gate.rs` | Includes descendants when requested |
| `frozen_policy_check` | `gate.rs` | Detects frozen branches in scope |
| `mode_resolution_bare_repo` | `modes.rs` | Refuses without explicit flag |
| `command_trait_requirements` | `command.rs` | Static requirements accessible |

### Integration Tests

| Test | File | Description |
|------|------|-------------|
| `gating_matrix_*` | `tests/gating_matrix.rs` | All command × capability combinations |
| `bare_repo_submit_refuses` | `tests/bare_repo.rs` | Submit without --no-restack |
| `frozen_branch_blocks_restack` | `tests/freeze.rs` | Gating catches frozen |
| `missing_trunk_routes_to_doctor` | `tests/doctor.rs` | Repair handoff works |

### Architecture Lint

| Check | Script | Description |
|-------|--------|-------------|
| No scan imports | `lint-arch.sh` | Commands don't import scan |
| No scan calls | `lint-arch.sh` | Commands don't call scan() |

---

## Migration Checklist

Commands to migrate (50+ files in `src/cli/commands/`):

### Batch 1: Read-Only
- [ ] `log.rs`
- [ ] `info.rs`
- [ ] `parent.rs`
- [ ] `children.rs`
- [ ] `trunk.rs` (print mode)

### Batch 2: Metadata-Only
- [ ] `init.rs`
- [ ] `track.rs`
- [ ] `untrack.rs`
- [ ] `freeze.rs`
- [ ] `unfreeze.rs`
- [ ] `unlink.rs`

### Batch 3: Navigation
- [ ] `checkout.rs`
- [ ] `up.rs`
- [ ] `down.rs`
- [ ] `top.rs`
- [ ] `bottom.rs`

### Batch 4: Stack Mutation
- [ ] `create.rs`
- [ ] `modify.rs`
- [ ] `restack.rs`
- [ ] `move_cmd.rs`
- [ ] `reorder.rs`
- [ ] `split.rs`
- [ ] `squash.rs`
- [ ] `fold.rs`
- [ ] `pop.rs`
- [ ] `delete.rs`
- [ ] `rename.rs`
- [ ] `revert.rs`

### Batch 5: Remote (with Modes)
- [ ] `submit.rs`
- [ ] `sync.rs`
- [ ] `get.rs`
- [ ] `merge.rs`
- [ ] `pr.rs`

### Batch 6: Recovery
- [ ] `continue.rs` (in `recovery.rs`)
- [ ] `abort.rs` (in `recovery.rs`)
- [ ] `undo.rs` (in `recovery.rs`)

### Other
- [ ] `auth.rs` (special - no repo required)
- [ ] `config.rs`
- [ ] `alias.rs`
- [ ] `completion.rs`
- [ ] `changelog.rs`
- [ ] `doctor.rs` (special - is the repair handler)

---

## Estimated Complexity

**HIGH** - This milestone touches every command file in the codebase.

However, the changes are **mechanical** once the foundation is in place:
1. Foundation types (Steps 1.1-1.3): ~300 lines new code
2. Engine runner (Steps 2.1-2.2): ~150 lines new code
3. Command migrations (Step 3): Repetitive pattern across 50+ files
4. Tests (Step 4): ~400 lines new tests

**Risk Mitigation:**
- Migrate commands in batches, keeping tests green
- Each batch is independently deployable
- Existing behavior preserved during migration

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

# Gating-specific tests
cargo test gating

# Architecture lint
./scripts/lint-arch.sh

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** The gating infrastructure already exists and is well-designed. This milestone is primarily integration work.
- **Simplicity:** We're not redesigning gating - just wiring it in.
- **No stubs:** Every command must actually go through gating after migration.
- **Purity:** The `Command::plan()` method must remain pure (no I/O).
- **Reuse:** Leverage existing `RequirementSet` definitions from `gate.rs`.

---

## Dependencies

**Blocking:**
- None - this is the first correctness milestone

**Blocked by this:**
- Milestone 0.2 (Worktree Occupancy) - needs `Plan::touched_branches()` which uses gating
- Milestone 0.3 (Journal Rollback) - needs gating in place
- All other correctness milestones

---

## Next Steps (After Completion)

Per ROADMAP.md execution order:
1. ✅ Milestone 0.1: Gating Integration + Scope Walking (this)
2. → Milestone 0.2 + 0.6: Occupancy + Post-Verify (bundled, both touch executor)
3. → Milestone 0.3: Journal Rollback
