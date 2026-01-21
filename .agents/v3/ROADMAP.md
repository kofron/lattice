# Lattice v3 Roadmap

This roadmap covers v3 work for Lattice, focusing on **critical correctness issues** discovered during a comprehensive codebase survey, plus unfinished items from v2.

## Overview

v3 prioritizes **architectural correctness** over new features. A survey revealed that significant infrastructure (gating, worktree occupancy, journal rollback) was built but never wired into commands. This roadmap addresses those gaps first.

**Governing documents:**
- `SPEC.md` - Engineering specification (command behavior, schemas, tests)
- `ARCHITECTURE.md` - Architectural constraints (execution model, doctor framework, event ledger)

**Survey reference:** `.claude/plans/indexed-juggling-balloon.md` contains full findings.

---

## Anti-Drift Mechanisms

v3 is not just "fix it once" but "make it hard to break again." The following mechanisms MUST be deliverables of the correctness milestones:

1. **Commands cannot access scanner directly.** Only Engine can produce `ReadyContext`. Module visibility enforces this.

2. **Executor enforces occupancy + post-verify.** Commands cannot forget these checks; they're executor invariants.

3. **CI includes architecture enforcement:**
   - Gating matrix test (table-driven: command × missing capability → expected result)
   - Architecture lint forbidding `scan()` imports in command modules
   - Out-of-band drift harness (interleaved git operations)

4. **"Touched branches" is a Plan property.** Occupancy, CAS preconditions, journal steps, and rollback all derive from `Plan::touched_refs()`.

5. **Test-only pause hook in Engine.** Enables drift harness to inject out-of-band operations **after planning, before lock acquisition** (compiled under `cfg(test)` or `fault_injection` feature). Harness asserts that CAS failures or occupancy violations are detected and handled correctly.

---

## Command Category Matrix

Per SPEC.md §4.6.6 and §6.2, commands fall into categories that determine their gating requirements.

**Key insight:** Several commands have **flag-dependent requirements** (submit/sync/get with bare repo flags). These are modeled as separate "modes" so each mode has a static requirement set.

| Category | Requirements | Commands |
|----------|--------------|----------|
| **A: Read-only** | `RepoOpen` only | log, info, parent, children, trunk (print), config get/list, auth status, changelog, completion |
| **B: Metadata-only** | `RepoOpen + TrunkKnown + NoLatticeOpInProgress` | init, track, untrack, freeze, unfreeze, unlink, config set |
| **C: Working-copy** | Category B + `WorkingDirectoryAvailable + NoExternalGitOpInProgress + GraphValid + FrozenPolicySatisfied` | create, modify, restack, move, reorder, split, squash, fold, pop, delete, rename, revert, checkout, up, down, top, bottom |
| **D1: Remote/API-only (bare-compatible)** | `RepoOpen + TrunkKnown + RemoteResolved + AuthAvailable + RepoAuthorized` | pr, merge |
| **D2: Remote + working-copy (default)** | Category C + `RemoteResolved + AuthAvailable + RepoAuthorized` | submit (default), sync (default), get (default) |
| **D3: Remote bare-mode (explicit flags)** | Category B + `RemoteResolved + AuthAvailable + RepoAuthorized` | submit --no-restack, sync --no-restack, get --no-checkout |
| **E: Ref-only mutation** | `RepoOpen + TrunkKnown + NoLatticeOpInProgress + NoExternalGitOpInProgress` | undo |

**Mode types for flag-dependent commands:**

```rust
// Each mode implements Command with its own static REQUIREMENTS
enum SubmitMode { WithRestack, NoRestack }
enum SyncMode { WithRestack, NoRestack }
enum GetMode { WithCheckout, NoCheckout }

// Dispatch based on flags + repo context
// CRITICAL: In bare repos, refuse unless explicit flag is provided (no silent downgrades)
impl SubmitCommand {
    fn resolve_mode(&self, is_bare: bool) -> Result<SubmitMode, GateError> {
        match (self.no_restack, is_bare) {
            (true, _) => Ok(SubmitMode::NoRestack),
            (false, false) => Ok(SubmitMode::WithRestack),
            (false, true) => Err(GateError::BareRepoRequiresFlag {
                command: "submit",
                required_flag: "--no-restack",
            }),
        }
    }
}
```

**Degraded read-only:** Commands MAY run without `MetadataReadable` in specific ways:
- `log`: Presents degraded view with explicit "(metadata unavailable)" indication
- `parent`, `children`: Return empty/error with "(branch not tracked)" - they cannot guess relationships
- `info`: Shows only git-level data (commit, author, date), omits lattice fields with explanation

**Op-state rule:** While op-state exists, only `continue`, `abort`, and Category A commands may run. All others must refuse with "operation in progress" issue.

---

## Implementation Status Summary

| Category | Milestone | Status | Priority |
|----------|-----------|--------|----------|
| **Correctness** | Gating Integration + Scope Walking | Not started | CRITICAL |
| **Correctness** | Worktree Occupancy Checks | Not started | CRITICAL |
| **Correctness** | Journal Rollback Implementation | Not started | CRITICAL |
| **Correctness** | OpState Full Payload | Not started | HIGH |
| **Correctness** | Multi-step Journal Continuation | Not started | HIGH |
| **Correctness** | Executor Post-Verification | Not started | HIGH |
| **Correctness** | TokenProvider Integration | COMPLETE | HIGH |
| **Correctness** | Bare Repo Mode Compliance | COMPLETE | MEDIUM |
| **Correctness** | Journal Fsync Step Boundary | Not started | MEDIUM |
| **Correctness** | Direct .git File Access Fix | COMPLETE | MEDIUM |
| **Correctness** | Git Hooks Support | Not started | MEDIUM |
| **Correctness** | Out-of-Band Drift Harness | Not started | MEDIUM |
| **Foundation** | TTY Detection Fix | Stubbed (from v2) | LOW |
| **Feature** | Alias Command | Not started (from v2) | MEDIUM |
| **Feature** | Split By-Hunk | Deferred (from v2) | LOW |
| **Bootstrap** | Local-Only Bootstrap | Not started (from v2) | MEDIUM |
| **Bootstrap** | Synthetic Stack Detection | Not started (from v2) | LOW |
| **Bootstrap** | Snapshot Materialization | Not started (from v2) | LOW |

---

# Category 0: Critical Correctness Issues

These issues were discovered during a comprehensive codebase survey. They represent **architectural drift** where infrastructure was built but never integrated into commands. These MUST be fixed before any feature work.

---

### Milestone 0.1: Gating Integration + Scope Walking

**Status:** Not started

**Priority:** CRITICAL - Commands bypass all capability validation

**Spec reference:** ARCHITECTURE.md Section 5 "The validated execution model and capability gating"

**Problem Statement:**

The entire gating system exists but is **completely unused**. Every command follows this pattern:
```rust
let snapshot = scan(&git)?;
// Direct business logic WITHOUT calling gate()
```

Additionally, "Upstack Scope Walking" (formerly a LOW priority item) is **correctness-critical** because `FrozenPolicySatisfied` depends on it. An incomplete scope walk can silently under-enforce freeze.

**Evidence:**
- No grep results for `gate(` or `GateResult` in any command file
- `ReadyContext` and `ValidatedData` types defined but never produced/consumed
- Commands manually implement what gating should enforce
- `src/engine/gate.rs` line 410: `// TODO: Walk graph to find all upstack branches`

**ARCHITECTURE.md violations:**
- Section 5.1: "A command executes only against a validated representation"
- Section 5.3: "Every non-doctor command declares its required capabilities"
- Section 12: Command lifecycle step 2 (Gate) is skipped entirely

**Impact:**
- `restack` can run with frozen branches (no `FrozenPolicySatisfied` check)
- `create` can run without metadata being readable
- `checkout` can run in bare repos without proper gating
- `submit` can attempt remote ops without `AuthAvailable` or `RepoAuthorized` checks

---

#### Design Questions (Resolve Before Implementation)

**Q1: How will we make "bypass is impossible" the default?**

Options:
- (a) Restructure module visibility so commands cannot call `scan()` directly
- (b) Commands only receive `ReadyContext<C>` and never see raw snapshot
- (c) CI lint that fails if command module imports scanner

**Recommendation:** All three. Make gating the only way into planning via `Planner::plan(ReadyContext<C>)`.

**Q2: How will requirements be encoded per command?**

For commands with **flag-dependent requirements** (submit/sync/get), use **mode types**:

```rust
// Each mode has static requirements - no dynamic branching in gate
enum SubmitMode { WithRestack, NoRestack }

impl Command for SubmitWithRestack {
    const REQUIREMENTS: RequirementSet = REMOTE_WITH_WORKDIR;  // Category D2
}
impl Command for SubmitNoRestack {
    const REQUIREMENTS: RequirementSet = REMOTE_BARE_ALLOWED;  // Category D3
}
```

**Q3: How does doctor handoff work for auth-type issues?**

Per ARCHITECTURE.md §8.2, some issues require **user actions** not executor plans (e.g., "run `lattice auth login`", "install GitHub App").

**Answer:** `GateResult::Repair(bundle)` routes to doctor. Doctor presents fix options including user-action fixes. These have no executor plan, just instructions.

**Q4: Is `FrozenPolicySatisfied` defined over target branch, full upstack, or command-specific scope?**

Per SPEC.md §8B.4, freeze applies to "the target branch and its downstack ancestors up to trunk." The scope walk must compute this closure correctly.

---

#### Deliverables

1. **Define command contract trait with mode support:**
   ```rust
   trait Command {
       const REQUIREMENTS: RequirementSet;
       type Context: ValidatedContext;
       fn execute(ctx: Self::Context) -> Result<()>;
   }
   
   // Mode dispatch uses concrete match, not Box<dyn Command>
   // This preserves static REQUIREMENTS at compile time
   fn run_submit(args: SubmitArgs, is_bare: bool, git: &Git) -> Result<()> {
       let mode = args.resolve_mode(is_bare)?;  // May fail with BareRepoRequiresFlag
       match mode {
           SubmitMode::WithRestack => {
               run_command(SubmitWithRestack { args }, git)
           }
           SubmitMode::NoRestack => {
               run_command(SubmitNoRestack { args }, git)
           }
       }
   }
   ```

2. **Create Engine entrypoint that enforces lifecycle:**
   ```rust
   fn run_command<C: Command>(cmd: C, git: &Git) -> Result<()> {
       let snapshot = scan(&git)?;  // Only Engine can scan
       match gate(&snapshot, C::REQUIREMENTS) {
           GateResult::Ready(context) => cmd.execute(context),
           GateResult::Repair(bundle) => route_to_doctor(bundle),
       }
   }
   ```

3. **Implement upstack scope walking:**
   - `compute_freeze_scope(branch, graph) -> Vec<BranchName>`
   - Used by `FrozenPolicySatisfied` capability derivation
   - Must be deterministic and complete

4. **Modify module visibility:**
   - Make `scan()` private to engine module
   - Export only `run_command()` and `ReadyContext` types
   - Commands import from `engine`, not `engine::scan`

5. **Remove ad-hoc manual checks:**
   - Manual `OpState::read()` checks → `NoLatticeOpInProgress` capability
   - Manual trunk existence checks → `TrunkKnown` capability
   - Manual metadata existence checks → `MetadataReadable` capability

6. **Add gating matrix test file:**
   Table-driven tests anchoring intended semantics:
   ```rust
   #[test_case("restack", missing(FrozenPolicySatisfied) => Repair(FrozenBranches))]
   #[test_case("checkout", missing(WorkingDirectoryAvailable) => Repair(NoWorkingDirectory))]
   #[test_case("log", missing(MetadataReadable) => Ready(degraded=true))]
   #[test_case("submit", bare_repo() => requires_flag("--no-restack"))]
   #[test_case("submit --no-restack", bare_repo() => Ready)]
   fn gating_matrix(cmd: &str, condition: Condition) -> GateResult { ... }
   ```

7. **Add CI architecture lint:**
   - Fail if any `src/cli/commands/*.rs` imports `scan` or `Scanner`
   - Fail if any command constructs `Snapshot` directly

---

#### Critical Files

- `src/engine/mod.rs` - Add `run_command()` lifecycle entrypoint
- `src/engine/gate.rs` - Ensure requirement sets complete, implement scope walking
- `src/cli/commands/*.rs` (ALL) - Convert to Command trait, remove scan() calls
- `tests/gating_matrix.rs` - NEW: Table-driven gating tests

---

#### Acceptance Gates

- [ ] Every command implements `Command` trait with declared requirements
- [ ] Mode types used for flag-dependent commands (submit/sync/get)
- [ ] Commands receive `ReadyContext` not raw snapshot
- [ ] `scan()` is private to engine; commands cannot import it
- [ ] `GateResult::Repair` routes to doctor (not silent failure)
- [ ] Upstack scope walking implemented and used by `FrozenPolicySatisfied`
- [ ] Manual capability checks removed (replaced by gating)
- [ ] Gating matrix test covers all command × capability combinations
- [ ] CI lint prevents future bypass
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Estimated complexity:** HIGH (touches every command file, but mechanical once trait defined)

---

### Milestone 0.2: Worktree Occupancy Checks

**Status:** Not started

**Priority:** CRITICAL - Ref mutations can corrupt other worktrees

**Spec reference:** SPEC.md Section 4.6.8 "Worktree branch occupancy"

**Problem Statement:**

The Git interface provides `branch_checked_out_elsewhere()` and `branches_checked_out_elsewhere()` methods, but they have **ZERO callers** in the codebase.

Commands can mutate branches that are checked out in other worktrees, violating Git safety semantics.

**SPEC.md requirements (Section 4.6.8):**
- "For any command that would update, rebase, delete, or rename a branch ref, the command MUST compute the set of touched branches and refuse if any is checked out in a different worktree"

**ARCHITECTURE.md requirements (Section 6.2):**
- "revalidate worktree occupancy constraints under lock before ref-mutating steps"

---

#### Design Questions (Resolve Before Implementation)

**Q1: Where does "touched branches" come from?**

**Answer:** Derived from `Plan::touched_refs()` (single source of truth). This makes occupancy an executor guarantee, not scattered per-command logic.

**Q2: Where do the two occupancy checks happen?**

**Critical clarification:** Gate is about **capabilities**. Occupancy is a **plan precondition**. Therefore:

1. **Engine post-plan check (nice UX):** After Plan exists, before acquiring lock
   - Uses `plan.touched_branches()`
   - Returns Doctor issue with worktree paths (good UX, actionable)

2. **Executor hard check (correctness):** After lock acquired, before first ref mutation
   - Uses same `plan.touched_branches()`
   - Aborts with "precondition failed, re-run" and records `Aborted`

This keeps architecture clean: gating validates capabilities, occupancy is plan precondition.

**Q3: How to represent occupancy violations?**

- **Pre-lock:** Doctor issue (user can choose to close worktree)
- **Under-lock:** Executor error (too late for recovery, must abort)

---

#### Deliverables

1. **Add `Plan::touched_branches()` method:**
   ```rust
   impl Plan {
       pub fn touched_branches(&self) -> Vec<BranchName> {
           self.touched_refs()
               .filter(|r| r.starts_with("refs/heads/"))
               .map(|r| BranchName::from_ref(r))
               .collect()
       }
   }
   ```

2. **Add Engine post-plan occupancy check:**
   - After planning, before lock acquisition
   - If occupied branches detected, return `Repair(BranchCheckedOutElsewhere)`

3. **Add Executor under-lock revalidation:**
   - After acquiring lock, before first ref-mutating step
   - If occupied, abort with `ExecuteError::OccupancyViolation`
   - Record `Aborted` event per ARCHITECTURE.md

4. **Add `BranchCheckedOutElsewhere` issue to doctor:**
   ```rust
   KnownIssue::BranchCheckedOutElsewhere {
       branch: BranchName,
       worktree_path: PathBuf,
       evidence: Evidence,
   }
   ```
   - Include actionable guidance: "branch X is checked out in /path/to/wt; switch it to trunk or remove that worktree"

---

#### Critical Files

- `src/engine/plan.rs` - Add `touched_branches()` method
- `src/engine/mod.rs` - Add post-plan occupancy check
- `src/engine/exec.rs` - Add under-lock revalidation
- `src/doctor/issues.rs` - Add `BranchCheckedOutElsewhere` issue

---

#### Acceptance Gates

- [ ] `Plan::touched_branches()` returns all branches affected by plan
- [ ] Post-plan occupancy check in Engine (before lock)
- [ ] Under-lock revalidation in Executor (after lock, before mutations)
- [ ] `BranchCheckedOutElsewhere` issue includes worktree path and guidance
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Integration test: create worktree, attempt restack from main repo → blocked with guidance
- Integration test: occupancy changes between plan and execute → executor aborts
- Unit test: `touched_branches()` extracts correct refs

**Estimated complexity:** MEDIUM

**Note:** Bundle with Milestone 0.6 (both touch executor contract).

---

### Milestone 0.3: Journal Rollback Implementation

**Status:** Not started

**Priority:** CRITICAL - Abort doesn't actually roll back

**Spec reference:** SPEC.md Section 4.2.2 "Crash consistency contract"

**Problem Statement:**

The `abort()` function in `src/cli/commands/recovery.rs` admits:
```rust
// ❌ CRITICAL GAP: No actual rollback of ref changes
// For now, we just clear the op-state
```

The journal has `ref_updates_for_rollback()` method but it's never called.

---

#### Design Questions (Resolve Before Implementation)

**Q1: What is the rollback order for ref updates?**

**Answer:** Reverse journal order. Journal records in execution order, so reverse is correct undo order.

**Q2: How strict is rollback CAS?**

Per ARCHITECTURE.md, CAS semantics required. If ref has drifted since journal was written:
- Rollback MUST fail with clear error
- Do NOT attempt to "fix" diverged reality
- User must resolve manually or run doctor

**Q3: How to handle partial rollback failure?**

If rollback succeeds for refs A, B but fails CAS for C:
- Record what was rolled back and what failed
- Leave op-state with `phase: rollback_incomplete`
- Surface as doctor issue with evidence

---

#### Deliverables

1. **Implement rollback in `abort()`:**
   ```rust
   fn abort(ctx: &Context) -> Result<()> {
       // 1. Validate origin worktree
       op_state.check_origin_worktree(&info.git_dir)?;
       
       // 2. Abort any git operation
       abort_git_operation(&git)?;
       
       // 3. Load journal and rollback refs (reverse order)
       let journal = Journal::read(&paths, &op_state.op_id)?;
       for (refname, old_oid, expected_current) in journal.ref_updates_for_rollback() {
           git.update_ref_cas(&refname, &old_oid, Some(&expected_current), "lattice abort")?;
       }
       
       // 4. Record Aborted event
       ledger.append(Event::aborted(&op_state.op_id, fingerprint))?;
       
       // 5. Clear op-state
       OpState::remove(&paths)?;
   }
   ```

2. **Handle metadata rollback**
3. **Handle partial rollback failure** → `rollback_incomplete` phase
4. **Wire worktree origin check**

---

#### Acceptance Gates

- [ ] `abort()` restores refs to pre-operation state
- [ ] Metadata changes rolled back
- [ ] `Aborted` event recorded in ledger
- [ ] Worktree origin check enforced
- [ ] Rollback uses CAS semantics
- [ ] CAS failure produces clear error, not silent corruption
- [ ] Partial rollback failure transitions to `rollback_incomplete`
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Integration test: start restack, conflict, abort → refs restored
- Integration test: abort from wrong worktree → fails with guidance
- Integration test: out-of-band ref change during pause → abort fails safely (not silent corruption)

**Estimated complexity:** MEDIUM

---

### Milestone 0.4: OpState Full Payload

**Status:** COMPLETE

**Priority:** HIGH - Missing required fields

**Spec reference:** SPEC.md Section 4.6.5 "Operation state and crash safety"

**Problem Statement:**

The `OpState` struct is missing fields required by SPEC.md and needed by rollback/continuation:

- `plan_digest` (required by spec)
- `plan_schema_version` (for cross-version compatibility)
- `touched_refs` with expected old OIDs (needed for CAS rollback)

---

#### Deliverables

1. Add fields to `OpState`:
   ```rust
   pub struct OpState {
       pub op_id: OpId,
       pub command: String,
       pub phase: OpPhase,  // executing | awaiting_user (only two valid phases)
       pub awaiting_reason: Option<AwaitingReason>,  // Set when phase == awaiting_user
       pub updated_at: UtcTimestamp,
       pub origin_git_dir: PathBuf,
       pub origin_work_dir: Option<PathBuf>,
       pub plan_digest: String,           // SHA-256 of canonical JSON plan
       pub plan_schema_version: u32,      // For version compatibility
       pub touched_refs: Vec<TouchedRef>, // Refs + expected old OIDs for CAS
   }
   
   pub enum OpPhase { Executing, AwaitingUser }
   
   pub enum AwaitingReason {
       RebaseConflict,
       RollbackIncomplete { failed_refs: Vec<String> },
       VerificationFailed { evidence: Evidence },
   }
   
   pub struct TouchedRef {
       pub refname: String,
       pub expected_old: Option<Oid>,
   }
   ```

2. Compute SHA-256 digest of canonical JSON plan serialization (stable field ordering)

3. Verify digest and version on continue; emit clear error on mismatch:
   "Operation created by schema v1; this binary expects v2. Run `lt abort` or use matching version."

4. Use `touched_refs` for rollback CAS preconditions

---

#### Acceptance Gates

- [x] `OpState` includes `plan_digest`, `plan_schema_version`, `touched_refs`
- [x] Digest computed from stable JSON serialization
- [x] Continue verifies schema version matches (digest verification deferred to 0.5)
- [x] Version mismatch produces actionable error
- [x] `touched_refs` available for rollback CAS
- [x] `cargo test` passes
- [x] `cargo clippy` passes

**Estimated complexity:** LOW

**Note:** Complete before Milestone 0.5 (continuation needs these fields).

---

### Milestone 0.5: Multi-step Journal Continuation

**Status:** Not started

**Priority:** HIGH - Continue doesn't resume remaining steps

**Spec reference:** SPEC.md Section 4.2.1 "Journal structure"

**Problem Statement:**

The `continue_op()` function admits:
```rust
// ❌ CRITICAL GAP: Multi-step journal not resumed
// For now, we assume the operation is complete
```

When an operation pauses (e.g., restack conflict), continuing just clears op-state without executing remaining steps.

---

#### Design Questions

**Q1: Where does "remaining steps" truth live?**

**Answer:** Journal checkpoints. Each completed step is journaled; remaining steps = plan steps after last checkpoint.

**Q2: How to reconcile "plan from the past" with "repo reality now"?**

**Answer:** Re-scan before continuing, but don't re-plan. Validate:
- Preconditions for remaining steps still hold
- CAS expected values match current reality

If not, abort with "repository changed; cannot continue safely."

---

#### Deliverables

1. Add `Journal::remaining_steps(&self) -> &[PlanStep]` method
2. Implement continue as: validate origin → complete git op → load remaining steps → validate preconditions → execute → clear op-state only after all complete
3. Handle nested conflicts (transition back to `awaiting_user`)

---

#### Acceptance Gates

- [ ] Continue resumes from last checkpoint
- [ ] Remaining branches processed
- [ ] Nested conflicts pause again correctly
- [ ] Op-state cleared only after all steps complete
- [ ] Precondition validation on continue
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Estimated complexity:** HIGH

---

### Milestone 0.6: Executor Post-Verification + Occupancy Revalidation

**Status:** Not started

**Priority:** HIGH - Verification not self-enforcing

**Spec reference:** ARCHITECTURE.md Section 6.2 "The Executor contract"

**Problem Statement:**

1. Post-verify is delegated to caller, not self-enforcing
2. Occupancy revalidation under lock is part of Milestone 0.2 but implemented here

---

#### Design Questions

**Q1: What if post-verify fails after changes are applied?**

**Answer:** Attempt rollback first, then escalate if needed:
1. Attempt rollback using journal (same as abort)
2. If rollback succeeds: record `Aborted` event with verification failure evidence, clear op-state, return error
3. If rollback fails (CAS mismatch): transition to `awaiting_user` with `AwaitingReason::VerificationFailed`, route to doctor
4. Do NOT claim success in any case

---

#### Deliverables

1. Move `fast_verify()` inside `Executor::execute()`
2. Add occupancy revalidation after lock, before ref mutations (from 0.2)
3. On post-verify failure:
   - Attempt rollback using journal (same logic as abort)
   - If rollback succeeds: record `Aborted` event with verify failure evidence, clear op-state, return error
   - If rollback fails: transition to `awaiting_user` with `AwaitingReason::VerificationFailed`, route to doctor

---

#### Acceptance Gates

- [ ] Executor calls `fast_verify()` after successful execution
- [ ] Executor revalidates occupancy under lock
- [ ] Verification failure attempts rollback first
- [ ] Successful rollback on verify failure records `Aborted` event with evidence
- [ ] Failed rollback on verify failure transitions to `awaiting_user` with `VerificationFailed` reason
- [ ] Never claims success when verification fails
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Unit test: verify failure with clean rollback → `Aborted` event, op-state cleared
- Unit test: verify failure with CAS conflict on rollback → `awaiting_user` phase, doctor route
- Integration test: inject verify failure, assert refs restored or doctor invoked

**Estimated complexity:** MEDIUM

**Note:** Bundle with Milestone 0.2.

---

### Milestone 0.7: TokenProvider Integration

**Status:** COMPLETE (2026-01-20)

**Priority:** HIGH - No per-request token refresh

**Spec reference:** SPEC.md Section 8E.1 "Forge abstraction"

**Problem Statement:**

`GitHubForge` stores a raw token string instead of using `TokenProvider`.

---

#### Design Questions

**Q1: Does `AuthAvailable(host)` mean "token exists" or "token exists or is refreshable"?**

Per ARCHITECTURE.md: "access token exists or can be refreshed." So expired access token with valid refresh token = `AuthAvailable`.

**Q2: Token redaction enforcement?**

Per SPEC.md hard rule: tokens never in logs/errors/op-state/journal/ledger. Existing custom `Debug` implementations are sufficient (no `Redacted<T>` wrapper needed).

---

#### Deliverables

1. Refactor `GitHubForge` to accept `Arc<dyn TokenProvider>` ✓
2. Call `bearer_token()` per API request ✓
3. Add 401/403 retry logic (one retry after refresh) ✓
4. ~~Add `Redacted<T>` wrapper type~~ (not needed - existing Debug impls sufficient)
5. Ensure TokenProvider refresh path holds auth-scoped lock (per SPEC.md §4.4.3) ✓ (via GitHubAuthManager)

---

#### Acceptance Gates

- [x] `GitHubForge` uses `TokenProvider` not raw token
- [x] `bearer_token()` called per request
- [x] 401/403 triggers one retry with fresh token
- [x] TokenProvider refresh holds auth-scoped lock
- [x] Tokens never appear in Debug output, logs, errors, op-state, journal, or ledger
- [x] `cargo test` passes
- [x] `cargo clippy` passes

**Test strategy:**
- Unit test: mock provider, verify `bearer_token()` called per request ✓
- Unit test: mock 401 response, verify retry with refreshed token ✓
- **Concurrent refresh test:** Two threads hit 401 simultaneously; assert only one refresh call occurs (lock prevents stampede) - handled by existing GitHubAuthManager tests
- Integration test: verify `Redacted<T>` never leaks via `Debug` or `Display`

**Estimated complexity:** MEDIUM

---

### Milestone 0.8: Bare Repo Mode Compliance

**Status:** COMPLETE

**Priority:** MEDIUM - SPEC.md explicit requirements

**Spec reference:** SPEC.md Section 4.6.7 "Bare repo policy for submit/sync/get"

**Problem Statement:**

The SPEC has **very explicit bare-repo policy** that must be enforced:

- `submit`: refuse unless `--no-restack`, then enforce ancestry alignment + metadata normalization
- `sync`: refuse unless `--no-restack`
- `get`: refuse unless `--no-checkout`, then fetch + track + compute base + print worktree guidance

This interacts with the mode types in Milestone 0.1.

---

#### Deliverables

1. **submit --no-restack in bare repos:**
   - Check ancestry alignment: `p.tip` must be ancestor of `b.tip`
   - If ancestry holds but `b.base != p.tip`: normalize base to `p.tip` (metadata-only)
   - Print: "Updated base metadata for N branches (no history changes)"
   - If ancestry violated: "Restack required. Run from a worktree."

2. **sync --no-restack in bare repos:**
   - Perform fetch, trunk FF, PR checks, branch deletion prompts
   - Do NOT attempt any rebase/restack

3. **get --no-checkout in bare repos:**
   - Fetch branch ref
   - Track with parent inference
   - Compute base via merge-base
   - Default frozen
   - Print worktree creation guidance

---

#### Acceptance Gates

- [x] `submit` refuses in bare repo without `--no-restack`
- [x] `submit --no-restack` enforces ancestry alignment
- [x] `submit --no-restack` normalizes base metadata if aligned
- [x] `sync` refuses in bare repo without `--no-restack`
- [x] `get` refuses in bare repo without `--no-checkout`
- [x] `get --no-checkout` tracks branch with correct base
- [x] `cargo test` passes
- [x] `cargo clippy` passes

**Estimated complexity:** MEDIUM

**Completion Notes:** Implementation was discovered to be already complete during code exploration. Integration tests added in `tests/bare_repo_mode_compliance.rs` (12 tests) to verify and document the existing functionality.

---

### Milestone 0.9: Journal Fsync Step Boundary

**Status:** Not started

**Priority:** MEDIUM - SPEC requirement for crash contract

**Spec reference:** SPEC.md Section 4.2.2 "Crash consistency contract"

**Problem Statement:**

SPEC.md requires: "Journals must be written with fsync at each appended step boundary."

Current journal API allows batching multiple steps before sync, violating the crash contract.

---

#### Deliverables

1. **Change journal append API to enforce fsync per step:**
   ```rust
   impl Journal {
       // Old: add_step() then write() - allows batching
       // New: append_step() - fsync immediately
       pub fn append_step(&mut self, step: JournalStep) -> Result<()> {
           self.steps.push(step);
           self.write_and_sync()?;  // fsync at step boundary
           Ok(())
       }
   }
   ```

2. **Add fault-injection test:**
   - Kill process between steps
   - Assert recoverability from journal

---

#### Acceptance Gates

- [ ] Journal append fsyncs at each step boundary
- [ ] Fault-injection test verifies recoverability
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Estimated complexity:** LOW

---

### Milestone 0.10: Direct .git File Access Fix

**Status:** COMPLETE (2026-01-20)

**Priority:** MEDIUM - Violates single-interface principle

**Spec reference:** ARCHITECTURE.md Section 10.1 "Single Git interface"

**Problem Statement:**

Two methods read `.git` files directly:
1. `read_rebase_progress()` - reads `rebase-merge/` and `rebase-apply/`
2. `read_fetch_head()` - reads `FETCH_HEAD`

---

#### Deliverables

1. ~~Replace `read_fetch_head()` with CLI: `git rev-parse FETCH_HEAD`~~ → Used `git2::Repository::fetchhead_foreach()` (pure git2, no CLI)
2. ~~Replace `read_rebase_progress()` with git2 state detection + CLI fallback~~ → Used `git2::Repository::open_rebase()` with `Rebase::len()` and `Rebase::operation_current()` (pure git2)
3. Ensure replacements work in linked worktrees and bare repos ✓

---

#### Acceptance Gates

- [x] No direct `.git` file reads (grep audit passes)
- [x] Works in linked worktrees (git2 APIs are context-aware)
- [x] `cargo test` passes
- [x] `cargo clippy` passes

**Estimated complexity:** LOW

---

### Milestone 0.11: Git Hooks Support

**Status:** Not started

**Priority:** MEDIUM - Missing required feature

**Spec reference:** ARCHITECTURE.md Section 10.2 "Hooks and verification"

---

#### Deliverables

1. Add `--verify` / `--no-verify` global flags
2. Add `verify: bool` to `Context`
3. Pass to git interface methods that support hooks
4. Ensure git CLI calls include `--no-verify` when set

---

#### Acceptance Gates

- [ ] `--no-verify` disables hooks for commits and pushes
- [ ] Hooks honored by default
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Estimated complexity:** MEDIUM

---

### Milestone 0.12: Out-of-Band Drift Harness

**Status:** Not started

**Priority:** MEDIUM - Long-term insurance against drift

**Spec reference:** ARCHITECTURE.md Section 13.3 "Out-of-band fuzz testing"

**Problem Statement:**

ARCHITECTURE.md explicitly requires out-of-band fuzz testing.

---

#### Design Question

**Q: How will the harness inject git ops between scan and execute once scan is private?**

**Answer:** Add test-only hook in Engine:
```rust
#[cfg(any(test, feature = "fault_injection"))]
pub struct EngineHooks {
    pub before_execute: Option<fn(&RepoInfo) -> ()>,
}
```

Harness uses hook to perform out-of-band mutations, then verifies executor detects CAS/occupancy failure.

---

#### Deliverables

1. Add test-only `EngineHooks` with `before_execute` hook
2. Create `tests/oob_drift_harness.rs`
3. Property tests: interleave lattice + git, verify invariants
4. CI integration

---

#### Acceptance Gates

- [ ] Harness can inject git operations via hook
- [ ] CAS failures detected and handled
- [ ] Missing capabilities prevent execution
- [ ] CI runs harness
- [ ] `cargo test` passes

**Estimated complexity:** MEDIUM

---

# Category 1: Foundation (Carried from v2)

---

### Milestone 1.1: TTY Detection Fix

**Status:** Stubbed (from v2)

**Priority:** LOW

**Goal:** Replace TTY detection stub with `std::io::IsTerminal`.

**Estimated complexity:** TRIVIAL

---

# Category 2: Features (Carried from v2)

---

### Milestone 2.1: Alias Command

**Status:** Not started (from v2)

**Priority:** MEDIUM

**Spec reference:** SPEC.md Section 8A.4

**Estimated complexity:** MEDIUM

---

### Milestone 2.2: Split By-Hunk Mode

**Status:** Deferred (from v2)

**Priority:** LOW

**Estimated complexity:** HIGH

---

# Category 3: Bootstrap (Carried from v2)

---

### Milestone 3.1: Degraded Log Mode

**Status:** Not started (from v2)

**Priority:** LOW

**Estimated complexity:** LOW

---

### Milestone 3.2: Local-Only Bootstrap

**Status:** Not started (from v2)

**Priority:** MEDIUM

**Estimated complexity:** HIGH

---

### Milestone 3.3: Synthetic Stack Detection

**Status:** Not started (from v2)

**Priority:** LOW

**Estimated complexity:** MEDIUM

---

### Milestone 3.4: Snapshot Materialization

**Status:** Not started (from v2)

**Priority:** LOW

**Estimated complexity:** HIGH

---

## Execution Order (Optimized to Reduce Churn)

| Order | Milestone | Notes |
|-------|-----------|-------|
| 1 | 0.4 OpState Full Payload | Foundation for rollback + continuation |
| 2 | 0.1 Gating + Scope Walking | Core architectural fix (includes scope walking) |
| 3 | 0.2 + 0.6 Occupancy + Post-Verify | Bundle (both touch executor) |
| 4 | 0.3 Journal Rollback | Needs gating + op-state in place |
| 5 | 0.9 Journal Fsync | Related to 0.3 |
| 6 | 0.5 Multi-step Continuation | Needs rollback |
| 7 | 0.8 Bare Repo Compliance | Uses mode types from 0.1 |
| 8 | 0.7 TokenProvider | Independent |
| 9 | 0.10 Direct .git Fix | Independent |
| 10 | 0.11 Git Hooks | Independent |
| 11 | 0.12 Drift Harness | Validates all above |
| 12+ | Foundation, Features, Bootstrap | After correctness complete |

---

## Systems Verified as Compliant

| System | Status | Notes |
|--------|--------|-------|
| Doctor Framework | Excellent | All Section 8 requirements met |
| Branch Metadata Schema | Excellent | Strict parsing, no boolean blindness |
| Event Ledger | Excellent | Recently fixed commit chain |
| Centralized Path Routing | Excellent | LatticePaths enforced everywhere |
| CAS Ref Updates | Excellent | All mutations use CAS semantics |

---

## Known Doc Hygiene Issue (Future Cleanup)

README contains spec drift that differs from SPEC.md. Not blocking correctness, but should be tracked for cleanup after v3:

- **Repo config paths:** README may reference old paths; SPEC.md defines canonical `LatticePaths` locations
- **Auth token style:** README may show PAT-style auth; SPEC.md requires GitHub App OAuth flow
- **Command examples:** Verify all examples match current CLI surface
- **Feature claims:** Ensure README doesn't claim features that are stubbed or incomplete

---

## Conventions

- Category 0 (Correctness) MUST complete before feature work
- Each milestone references SPEC.md/ARCHITECTURE.md sections
- Design questions resolved before implementation starts
- All changes must pass `cargo test`, `cargo clippy`, type checks
- Implementation notes in `.agents/v3/milestones/<milestone>/implementation_notes.md`
