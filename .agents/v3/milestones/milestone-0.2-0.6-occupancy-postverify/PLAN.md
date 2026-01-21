# Milestone 0.2 + 0.6: Worktree Occupancy Checks + Executor Post-Verification

## Status: COMPLETE

---

## Overview

**Goal:** Enforce worktree occupancy constraints and post-verification as executor invariants, ensuring commands cannot mutate branches checked out in other worktrees and that post-execution verification is self-enforcing.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs, Tests are everything.

**Priority:** CRITICAL - Ref mutations can corrupt other worktrees; verification not self-enforcing

**Bundle Rationale:** Per ROADMAP.md, these milestones are bundled because both touch the executor contract. Implementing them together ensures a single coherent change to the executor.

---

## Problem Statement

### Problem 1: Worktree Occupancy (Milestone 0.2)

The Git interface provides `branch_checked_out_elsewhere()` and `branches_checked_out_elsewhere()` methods, but they are **not enforced before mutations**. Commands can mutate branches checked out in other worktrees, violating Git safety semantics.

**SPEC.md Section 4.6.8 requirements:**
> "For any command that would update, rebase, delete, or rename a branch ref, the command MUST compute the set of touched branches and refuse if any is checked out in a different worktree"

**ARCHITECTURE.md Section 6.2 requirements:**
> "revalidate worktree occupancy constraints under lock before ref-mutating steps"

**Current State:**
- `branch_checked_out_elsewhere()` exists at `src/git/interface.rs:1120`
- `branches_checked_out_elsewhere()` exists at `src/git/interface.rs:1150`
- Called only in `src/engine/health.rs:802` for issue generation
- **NOT enforced before mutations in the executor**

### Problem 2: Post-Verification (Milestone 0.6)

Post-verification is performed in `runner.rs` after execution, but this is **not self-enforcing**. If a caller bypasses the runner or forgets verification, invariants may be violated silently.

**ARCHITECTURE.md Section 6.2 requirements:**
> "After success: re-scan, verify invariants, record `Committed`"

**Current State:**
- `fast_verify()` called in `runner.rs:195-199` after execution
- Executor computes fingerprint but doesn't verify invariants
- Verification is the caller's responsibility, not executor-enforced

---

## Spec References

- **SPEC.md Section 4.6.8** - Worktree branch occupancy: "checked out elsewhere" is a first-class blocker
- **ARCHITECTURE.md Section 6.2** - The Executor contract (8 requirements)
- **ARCHITECTURE.md Section 5.2** - Capabilities (WorkingDirectoryAvailable)
- **ROADMAP.md Milestone 0.2** - Worktree Occupancy Checks
- **ROADMAP.md Milestone 0.6** - Executor Post-Verification + Occupancy Revalidation

---

## Current State Analysis

### Worktree Occupancy Infrastructure

**What Exists:**
- `Git::branch_checked_out_elsewhere(&self, branch: &BranchName) -> Result<Option<PathBuf>>` (interface.rs:1120-1149)
- `Git::branches_checked_out_elsewhere(&self) -> Result<HashMap<BranchName, PathBuf>>` (interface.rs:1150-1175)
- `issues::branches_checked_out_elsewhere()` creates a blocking `Issue` (health.rs)
- `Plan::touched_refs() -> &[String]` returns all refs touched by plan (plan.rs:351)

**What's Missing:**
- `Plan::touched_branches()` method to extract branch refs
- Engine post-plan occupancy check (nice UX before lock)
- Executor under-lock occupancy revalidation (correctness after lock)
- `ExecuteError::OccupancyViolation` error variant

### Post-Verification Infrastructure

**What Exists:**
- `fast_verify()` function in `verify.rs`
- Called in `runner.rs` after successful execution
- Executor computes `compute_current_fingerprint()` after all steps

**What's Missing:**
- Verification inside executor (self-enforcing)
- Rollback attempt on verify failure
- Proper error handling when post-verify fails

---

## Design Decisions

### D1: Where does "touched branches" come from?

**Decision:** Derive from `Plan::touched_refs()`. This is the single source of truth.

```rust
impl Plan {
    pub fn touched_branches(&self) -> Vec<BranchName> {
        self.touched_refs()
            .iter()
            .filter_map(|r| r.strip_prefix("refs/heads/"))
            .filter_map(|name| BranchName::new(name).ok())
            .collect()
    }
}
```

### D2: Where do occupancy checks happen?

**Decision:** Two locations per ROADMAP.md:

1. **Engine post-plan check (nice UX):** After Plan exists, before acquiring lock
   - Uses `plan.touched_branches()`
   - Returns Doctor issue with worktree paths (actionable UX)

2. **Executor hard check (correctness):** After lock acquired, before first ref mutation
   - Uses same `plan.touched_branches()`
   - Aborts with "precondition failed, re-run" error
   - Records `Aborted` event per ARCHITECTURE.md

**Rationale:** Gate is about capabilities, occupancy is a plan precondition. Keeping them separate maintains clean architecture.

### D3: How to represent occupancy violations?

**Decision:**
- **Pre-lock (Engine):** Return `RunError::NeedsRepair(bundle)` with `BranchCheckedOutElsewhere` issue
- **Under-lock (Executor):** Return `ExecuteError::OccupancyViolation` with details

### D4: What if post-verify fails after changes are applied?

**Decision:** Per ROADMAP.md, attempt rollback first:

1. Attempt rollback using journal (same as abort)
2. If rollback succeeds: record `Aborted` event with verification failure evidence, clear op-state, return error
3. If rollback fails (CAS mismatch): transition to `awaiting_user` with `AwaitingReason::VerificationFailed`, route to doctor
4. Do NOT claim success in any case

### D5: Should occupancy check happen for metadata-only steps?

**Decision:** No. Per SPEC.md §4.6.8:
> "Metadata-only commands (`track`, `freeze`, etc.) do not change branch refs and MUST NOT be blocked by occupancy."

The `touched_branches()` method only extracts `refs/heads/` refs, so metadata refs (`refs/branch-metadata/`) are naturally excluded.

---

## Implementation Steps

### Phase 1: Plan Enhancement

#### Step 1.1: Add `touched_branches()` to Plan

**File:** `src/engine/plan.rs` (MODIFY)

```rust
impl Plan {
    /// Get all branch refs that will be touched by this plan.
    ///
    /// Returns branch names (not full refs) for branches under `refs/heads/`.
    /// This is used for worktree occupancy checking - branches checked out
    /// in other worktrees cannot be mutated.
    ///
    /// # Note
    ///
    /// Metadata refs (`refs/branch-metadata/`) are NOT included because
    /// metadata-only operations don't require occupancy checks per SPEC.md §4.6.8.
    pub fn touched_branches(&self) -> Vec<BranchName> {
        self.touched_refs()
            .iter()
            .filter_map(|r| r.strip_prefix("refs/heads/"))
            .filter_map(|name| BranchName::new(name).ok())
            .collect()
    }
    
    /// Check if this plan touches any branch refs.
    ///
    /// Plans that only touch metadata refs don't need occupancy checks.
    pub fn touches_branch_refs(&self) -> bool {
        self.touched_refs()
            .iter()
            .any(|r| r.starts_with("refs/heads/"))
    }
}
```

#### Step 1.2: Add Tests for `touched_branches()`

**File:** `src/engine/plan.rs` (ADD to tests module)

```rust
#[test]
fn touched_branches_extracts_branch_names() {
    let mut plan = Plan::new("test", "op-123");
    plan = plan.with_step(PlanStep::UpdateRefCas {
        refname: "refs/heads/feature".to_string(),
        old_oid: Some("abc".to_string()),
        new_oid: "def".to_string(),
        reason: "test".to_string(),
    });
    plan = plan.with_step(PlanStep::WriteMetadataCas {
        branch: "feature".to_string(),
        old_ref_oid: None,
        metadata: /* ... */,
    });
    
    let branches = plan.touched_branches();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].as_str(), "feature");
}

#[test]
fn touches_branch_refs_true_when_branch_refs() {
    let plan = Plan::new("test", "op-123").with_step(PlanStep::UpdateRefCas {
        refname: "refs/heads/feature".to_string(),
        /* ... */
    });
    assert!(plan.touches_branch_refs());
}

#[test]
fn touches_branch_refs_false_for_metadata_only() {
    let plan = Plan::new("test", "op-123").with_step(PlanStep::WriteMetadataCas {
        branch: "feature".to_string(),
        /* ... */
    });
    assert!(!plan.touches_branch_refs());
}
```

---

### Phase 2: Executor Enhancements

#### Step 2.1: Add `OccupancyViolation` Error Variant

**File:** `src/engine/exec.rs` (MODIFY)

```rust
/// Errors from execution.
#[derive(Debug, Error)]
pub enum ExecuteError {
    // ... existing variants ...

    /// Branch is checked out in another worktree.
    ///
    /// This error occurs when the executor revalidates occupancy under lock
    /// and finds that a touched branch is now checked out elsewhere.
    /// This is a precondition failure - the user should close the worktree
    /// or switch it to a different branch, then re-run the command.
    #[error("branch '{branch}' is checked out in worktree at {worktree_path}")]
    OccupancyViolation {
        /// The branch that is checked out elsewhere.
        branch: String,
        /// Path to the worktree where it's checked out.
        worktree_path: String,
    },

    /// Post-execution verification failed.
    ///
    /// This error occurs when invariants are violated after execution.
    /// The executor attempts rollback before returning this error.
    #[error("post-execution verification failed: {message}")]
    VerificationFailed {
        /// Description of the verification failure.
        message: String,
        /// Whether rollback was successful.
        rollback_succeeded: bool,
    },
}
```

#### Step 2.2: Add Occupancy Revalidation Method

**File:** `src/engine/exec.rs` (ADD)

```rust
impl<'a> Executor<'a> {
    /// Revalidate worktree occupancy for touched branches.
    ///
    /// Per ARCHITECTURE.md Section 6.2, this MUST be called after acquiring
    /// the lock and before any ref-mutating steps. Worktree occupancy can
    /// change out-of-band between scan and execution.
    ///
    /// # Arguments
    ///
    /// * `plan` - The plan to check
    ///
    /// # Returns
    ///
    /// `Ok(())` if no conflicts, or `ExecuteError::OccupancyViolation` if
    /// any touched branch is checked out in another worktree.
    fn revalidate_occupancy(&self, plan: &Plan) -> Result<(), ExecuteError> {
        // Skip if plan doesn't touch branch refs
        if !plan.touches_branch_refs() {
            return Ok(());
        }

        let touched = plan.touched_branches();
        if touched.is_empty() {
            return Ok(());
        }

        // Check each touched branch
        for branch in &touched {
            if let Some(worktree_path) = self.git.branch_checked_out_elsewhere(branch)
                .map_err(|e| ExecuteError::Internal(format!(
                    "failed to check worktree occupancy: {}", e
                )))?
            {
                return Err(ExecuteError::OccupancyViolation {
                    branch: branch.to_string(),
                    worktree_path: worktree_path.display().to_string(),
                });
            }
        }

        Ok(())
    }
}
```

#### Step 2.3: Add Post-Verification Inside Executor

**File:** `src/engine/exec.rs` (MODIFY `execute()` method)

Add verification after all steps complete but before recording `Committed`:

```rust
pub fn execute(&self, plan: &Plan, ctx: &Context) -> Result<ExecuteResult, ExecuteError> {
    // ... existing lock acquisition, op-state, etc. ...

    // NEW: Revalidate occupancy under lock
    if ctx.debug {
        eprintln!("[debug] Revalidating worktree occupancy under lock");
    }
    self.revalidate_occupancy(plan)?;

    // ... existing step execution loop ...

    // After all steps complete successfully:

    // NEW: Post-execution verification
    if ctx.debug {
        eprintln!("[debug] Running post-execution verification");
    }
    if let Err(verify_error) = self.post_verify() {
        // Attempt rollback
        if ctx.debug {
            eprintln!("[debug] Verification failed, attempting rollback");
        }
        
        let rollback_result = self.attempt_rollback(&journal, &paths, &ledger, plan);
        
        match rollback_result {
            Ok(()) => {
                // Rollback succeeded - clear op-state and return error
                OpState::remove(&paths)?;
                return Err(ExecuteError::VerificationFailed {
                    message: verify_error.to_string(),
                    rollback_succeeded: true,
                });
            }
            Err(rollback_error) => {
                // Rollback failed - transition to awaiting_user
                let mut op_state = OpState::from_journal(&journal, &paths, info.work_dir.clone());
                op_state.phase = OpPhase::Paused;
                // Note: Full AwaitingReason support comes in Milestone 0.4
                op_state.write(&paths)?;
                
                return Err(ExecuteError::VerificationFailed {
                    message: format!(
                        "verification failed ({}) and rollback failed ({})",
                        verify_error, rollback_error
                    ),
                    rollback_succeeded: false,
                });
            }
        }
    }

    // ... existing fingerprint computation and committed event ...
}
```

#### Step 2.4: Add Post-Verify Method

**File:** `src/engine/exec.rs` (ADD)

```rust
impl<'a> Executor<'a> {
    /// Run post-execution verification.
    ///
    /// Per ARCHITECTURE.md Section 6.2, the executor MUST verify invariants
    /// after successful execution. This is now self-enforcing.
    fn post_verify(&self) -> Result<(), super::verify::VerifyError> {
        // Re-scan repository
        let snapshot = super::scan::scan(self.git)
            .map_err(|e| super::verify::VerifyError::ScanFailed(e.to_string()))?;
        
        // Run fast verification
        super::verify::fast_verify(self.git, &snapshot)
    }
    
    /// Attempt to rollback changes using journal.
    ///
    /// This is called when post-verification fails. It attempts to restore
    /// refs to their pre-operation state.
    fn attempt_rollback(
        &self,
        journal: &Journal,
        paths: &LatticePaths,
        ledger: &EventLedger,
        plan: &Plan,
    ) -> Result<(), ExecuteError> {
        // Get rollback entries from journal (reverse order)
        let rollback_entries = journal.ref_updates_for_rollback();
        
        for (refname, old_oid, expected_current) in rollback_entries {
            // Attempt CAS update to restore old value
            let old = if old_oid.is_empty() {
                None
            } else {
                Some(Oid::new(&old_oid).map_err(|e| ExecuteError::Internal(e.to_string()))?)
            };
            
            let expected = if expected_current.is_empty() {
                None
            } else {
                Some(Oid::new(&expected_current).map_err(|e| ExecuteError::Internal(e.to_string()))?)
            };
            
            // For rollback, we're going from expected_current back to old_oid
            if let Some(old_val) = old {
                self.git
                    .update_ref_cas(&refname, &old_val, expected.as_ref(), "lattice rollback")
                    .map_err(|e| ExecuteError::Internal(format!("rollback failed: {}", e)))?;
            } else {
                // Original was None (ref didn't exist), so delete it
                if let Some(exp) = expected {
                    self.git
                        .delete_ref_cas(&refname, &exp)
                        .map_err(|e| ExecuteError::Internal(format!("rollback failed: {}", e)))?;
                }
            }
        }
        
        // Record Aborted event
        let _ = ledger.append(Event::aborted(plan.op_id.as_str(), "verification failed, rolled back"));
        
        Ok(())
    }
}
```

---

### Phase 3: Engine Integration

#### Step 3.1: Add Post-Plan Occupancy Check in Runner

**File:** `src/engine/runner.rs` (MODIFY `run_command_internal`)

Add occupancy check after planning, before execution:

```rust
fn run_command_internal<C: Command>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &RequirementSet,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    // ... existing scan and gate ...

    // Step 3: Plan
    if ctx.debug {
        eprintln!("[debug] Step 3: Plan");
    }
    let plan = command.plan(&ready)?;

    if ctx.debug {
        eprintln!("[debug] Plan has {} steps", plan.step_count());
    }

    // NEW: Step 3.5: Pre-execution occupancy check (nice UX)
    if plan.touches_branch_refs() {
        if ctx.debug {
            eprintln!("[debug] Step 3.5: Pre-execution occupancy check");
        }
        check_occupancy_for_plan(git, &plan)?;
    }

    // ... existing execution ...
}

/// Check worktree occupancy for a plan before execution.
///
/// This is the "nice UX" check before acquiring the lock. It provides
/// actionable guidance to the user. The executor also revalidates
/// under lock for correctness.
fn check_occupancy_for_plan(git: &Git, plan: &Plan) -> Result<(), RunError> {
    let touched = plan.touched_branches();
    if touched.is_empty() {
        return Ok(());
    }

    let mut conflicts = Vec::new();
    
    for branch in touched {
        if let Ok(Some(worktree_path)) = git.branch_checked_out_elsewhere(&branch) {
            conflicts.push((branch, worktree_path));
        }
    }
    
    if !conflicts.is_empty() {
        let issue = super::health::issues::branches_checked_out_elsewhere(conflicts);
        let bundle = super::gate::RepairBundle {
            command: plan.command.clone(),
            missing_capabilities: vec![],
            blocking_issues: vec![issue],
        };
        return Err(RunError::NeedsRepair(bundle));
    }
    
    Ok(())
}
```

#### Step 3.2: Remove Redundant Verification from Runner

**File:** `src/engine/runner.rs` (MODIFY)

Since verification is now inside the executor, remove the redundant call:

```rust
// Step 4: Execute
if ctx.debug {
    eprintln!("[debug] Step 4: Execute");
}
let executor = Executor::new(git);
let result = executor.execute(&plan, ctx)?;

// REMOVED: Step 5 verification - now done inside executor
// The executor is self-enforcing per ARCHITECTURE.md

// Step 5 (was 6): Finish
if ctx.debug {
    eprintln!("[debug] Step 5: Finish");
}
Ok(command.finish(result))
```

---

### Phase 4: Verify Module Enhancement

#### Step 4.1: Add ScanFailed Error Variant

**File:** `src/engine/verify.rs` (MODIFY if needed)

Ensure VerifyError can represent scan failures:

```rust
#[derive(Debug, Error)]
pub enum VerifyError {
    // ... existing variants ...
    
    /// Scan failed during verification.
    #[error("scan failed during verification: {0}")]
    ScanFailed(String),
}
```

---

### Phase 5: Testing

#### Step 5.1: Unit Tests for Plan Methods

**File:** `src/engine/plan.rs` (ADD to tests)

```rust
#[cfg(test)]
mod occupancy_tests {
    use super::*;

    #[test]
    fn touched_branches_extracts_branch_names() {
        let mut plan = Plan::new("test", "op-123");
        plan = plan.with_step(PlanStep::UpdateRefCas {
            refname: "refs/heads/feature".to_string(),
            old_oid: Some("abc".to_string()),
            new_oid: "def".to_string(),
            reason: "test".to_string(),
        });
        
        let branches = plan.touched_branches();
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].as_str(), "feature");
    }

    #[test]
    fn touched_branches_ignores_metadata_refs() {
        let plan = Plan::new("test", "op-123").with_step(PlanStep::WriteMetadataCas {
            branch: "feature".to_string(),
            old_ref_oid: None,
            metadata: test_metadata(),
        });
        
        let branches = plan.touched_branches();
        assert!(branches.is_empty());
    }

    #[test]
    fn touches_branch_refs_false_for_metadata_only() {
        let plan = Plan::new("test", "op-123").with_step(PlanStep::WriteMetadataCas {
            branch: "feature".to_string(),
            old_ref_oid: None,
            metadata: test_metadata(),
        });
        
        assert!(!plan.touches_branch_refs());
    }

    #[test]
    fn touches_branch_refs_true_when_has_branch_ref() {
        let plan = Plan::new("test", "op-123").with_step(PlanStep::UpdateRefCas {
            refname: "refs/heads/feature".to_string(),
            old_oid: Some("abc".to_string()),
            new_oid: "def".to_string(),
            reason: "test".to_string(),
        });
        
        assert!(plan.touches_branch_refs());
    }
}
```

#### Step 5.2: Integration Tests for Occupancy

**File:** `tests/worktree_occupancy.rs` (NEW)

```rust
//! Integration tests for worktree occupancy checking.
//!
//! Per SPEC.md §4.6.8, commands that mutate branch refs must refuse
//! when the target branch is checked out in another worktree.

use std::process::Command;
use tempfile::TempDir;

/// Create a worktree, attempt restack from main repo → blocked
#[test]
fn restack_blocked_when_branch_checked_out_elsewhere() {
    let (repo_dir, _guard) = setup_repo_with_worktree();
    
    // In main repo, try to restack a branch checked out in worktree
    let output = Command::new("cargo")
        .args(["run", "--", "restack", "--branch", "feature"])
        .current_dir(&repo_dir)
        .output()
        .expect("failed to run lattice");
    
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("checked out") || stderr.contains("worktree"));
}

/// Occupancy changes between scan and execute → executor aborts
#[test]
fn occupancy_change_between_plan_and_execute_aborts() {
    // This test requires the fault injection hook (Milestone 0.12)
    // For now, document the expected behavior
}

/// Metadata-only operations are NOT blocked by occupancy
#[test]
fn track_allowed_when_branch_checked_out_elsewhere() {
    let (repo_dir, _guard) = setup_repo_with_worktree();
    
    // Track should succeed even if branch is checked out elsewhere
    let output = Command::new("cargo")
        .args(["run", "--", "track", "feature", "--parent", "main"])
        .current_dir(&repo_dir)
        .output()
        .expect("failed to run lattice");
    
    // Track is metadata-only, should succeed
    assert!(output.status.success());
}
```

#### Step 5.3: Unit Tests for Executor Verification

**File:** `src/engine/exec.rs` (ADD to tests)

```rust
#[cfg(test)]
mod verification_tests {
    use super::*;

    #[test]
    fn execute_error_occupancy_violation_display() {
        let err = ExecuteError::OccupancyViolation {
            branch: "feature".to_string(),
            worktree_path: "/worktrees/feature".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("feature"));
        assert!(msg.contains("/worktrees/feature"));
    }

    #[test]
    fn execute_error_verification_failed_display() {
        let err = ExecuteError::VerificationFailed {
            message: "graph invalid".to_string(),
            rollback_succeeded: true,
        };
        let msg = err.to_string();
        assert!(msg.contains("verification failed"));
        assert!(msg.contains("graph invalid"));
    }
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/engine/plan.rs` | MODIFY | Add `touched_branches()` and `touches_branch_refs()` |
| `src/engine/exec.rs` | MODIFY | Add occupancy revalidation, post-verify, rollback |
| `src/engine/runner.rs` | MODIFY | Add pre-execution occupancy check, remove redundant verify |
| `src/engine/verify.rs` | MODIFY | Add `ScanFailed` error variant if needed |
| `tests/worktree_occupancy.rs` | NEW | Integration tests for occupancy |

---

## Acceptance Gates

Per ROADMAP.md and ARCHITECTURE.md:

### Milestone 0.2 (Occupancy)
- [ ] `Plan::touched_branches()` returns all branches affected by plan
- [ ] `Plan::touches_branch_refs()` correctly identifies branch-mutating plans
- [ ] Post-plan occupancy check in Engine (before lock)
- [ ] Under-lock revalidation in Executor (after lock, before mutations)
- [ ] `BranchCheckedOutElsewhere` issue includes worktree path and guidance
- [ ] Metadata-only plans are NOT blocked by occupancy

### Milestone 0.6 (Post-Verification)
- [ ] Executor calls `post_verify()` after successful execution
- [ ] Executor revalidates occupancy under lock
- [ ] Verification failure attempts rollback first
- [ ] Successful rollback on verify failure records `Aborted` event
- [ ] Failed rollback on verify failure transitions to `awaiting_user`
- [ ] Never claims success when verification fails

### General
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes
- [ ] `cargo fmt --check` passes

---

## Testing Rubric

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `touched_branches_extracts_branch_names` | plan.rs | Extracts branch names from refs/heads/ |
| `touched_branches_ignores_metadata_refs` | plan.rs | Doesn't extract refs/branch-metadata/ |
| `touches_branch_refs_*` | plan.rs | Correctly identifies branch-mutating plans |
| `execute_error_occupancy_violation_display` | exec.rs | Error message formatting |
| `execute_error_verification_failed_display` | exec.rs | Error message formatting |

### Integration Tests

| Test | File | Description |
|------|------|-------------|
| `restack_blocked_when_branch_checked_out_elsewhere` | worktree_occupancy.rs | Main repo restack blocked |
| `track_allowed_when_branch_checked_out_elsewhere` | worktree_occupancy.rs | Metadata-only not blocked |
| `checkout_refuses_when_branch_checked_out_elsewhere` | worktree_occupancy.rs | Checkout blocked |

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

# Specific tests
cargo test touched_branches
cargo test occupancy
cargo test verification

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** The occupancy infrastructure exists. This milestone wires it into enforcement.
- **Simplicity:** We're adding two checks (pre-lock UX, under-lock correctness) to existing code paths.
- **No stubs:** Rollback must actually work. Post-verify must actually verify.
- **Purity:** The `touched_branches()` method is pure; enforcement is in executor.

---

## Dependencies

**Depends on:**
- Milestone 0.1 (Gating Integration) - COMPLETE

**Blocked by this:**
- Milestone 0.3 (Journal Rollback) - uses similar rollback logic
- Milestone 0.12 (Drift Harness) - validates occupancy under concurrent modification

---

## Risk Assessment

**Risk: Rollback complexity**
- Mitigation: Rollback logic follows same pattern as abort (Milestone 0.3)
- If full rollback is complex, can implement basic version that records `Aborted` and clears op-state

**Risk: Performance overhead**
- Mitigation: `branch_checked_out_elsewhere()` is a simple git command
- Only called when plan touches branch refs (not metadata-only)

---

## Next Steps (After Completion)

Per ROADMAP.md execution order:
1. ✅ Milestone 0.1: Gating Integration + Scope Walking
2. ✅ Milestone 0.2 + 0.6: Occupancy + Post-Verify (this)
3. → Milestone 0.3: Journal Rollback
4. → Milestone 0.9: Journal Fsync
