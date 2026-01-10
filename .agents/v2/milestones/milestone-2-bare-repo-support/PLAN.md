# Milestone 12: Bare Repository and Worktree Support

## Status: PLANNED

---

## Overview

**Goal:** Enable Lattice to run safely and predictably in Git **bare repositories** and **linked worktrees**, without violating the prime invariant or the “single transactional write path” architecture.

This milestone delivers:

1. **Bare repo support (read-only + metadata-only):**

   * `lattice log`, `lattice info`, `lattice parent/children`, `lattice config`, `lattice init`, etc. work from inside a bare repo directory.
   * Commands that require a working directory refuse with a crisp, high-signal error and a recommended `git worktree add …` flow.

2. **Worktree correctness:**

   * Normal operations work from within worktrees.
   * Repo-scoped state (config, lock, op-state, journals) is correctly shared across worktrees using Git’s **common dir**.

3. **No cross-worktree footguns:**

   * Lattice remains **single-writer per repository** (repo-scoped lock + op-state), so concurrent mutations cannot create confusing or irrecoverable states.
   * Lattice detects common worktree-specific blockers (like “branch checked out in another worktree”) and surfaces them as first-class issues rather than leaking raw Git errors.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Correctness-first, Determinism, Single write path, No stubs, Tests are everything, UX that teaches (without guessing repairs).

**Dependencies:**

* Milestone 2 (complete): Git interface with CAS ref update primitives.
* Milestone 3 (complete): Persistence layer (repo lock, op-state/journals, config IO).
* Milestone 4+ (assumed present): Engine lifecycle (scanner → gate → plan → execute) and capability-based gating.

---

## Architecture Context

Per ARCHITECTURE.md:

1. **Validated execution model + capability gating**
   Commands run only when required capabilities are proven. Bare repositories are not “invalid,” they are missing the **WorkingDirectoryAvailable** capability.

2. **Single transactional mutation pathway**
   Mutations must flow through the executor, under a lock, with op-state, journaling, and CAS ref updates.

3. **Repo-scoped state must be repository-scoped, not worktree-scoped**
   Git worktrees introduce:

   * **common dir**: shared refs/objects/config namespace
   * **per-worktree git dir**: HEAD/index/rebase state
   * **optional workdir**: absent for bare repos

To preserve “one in-flight structural operation,” Lattice’s lock + op-state + journals must live in a location shared by all worktrees: **Git common dir**.

---

## Acceptance Gates

### Functional Gates

**Repository context detection**

* [ ] `Git::open()` succeeds when invoked inside:

  * [ ] normal repo workdir
  * [ ] bare repo directory
  * [ ] linked worktree directory (including nested subdirectories)
* [ ] `RepoInfo` includes:

  * [ ] `git_dir` (per-worktree)
  * [ ] `common_dir` (shared)
  * [ ] `work_dir: Option<PathBuf>` (None for bare)
  * [ ] `context: RepoContext::{Normal,Bare,Worktree}`
* [ ] `RepoContext` classification is correct for:

  * [ ] normal non-worktree repo
  * [ ] bare repo
  * [ ] worktree repo

**Repo-scoped persistence correctness**

* [ ] Repo config is loaded from and written to **`common_dir/lattice/…`** (canonical) and is visible from all worktrees
* [ ] Repo lock is acquired at **repo scope** (shared across worktrees)
* [ ] Op-state and journals are stored at **repo scope** (shared across worktrees)
* [ ] When an op-state marker exists, mutating commands refuse from *any* worktree

**Capability gating behavior**

* [ ] `WorkingDirectoryAvailable` capability is present iff `RepoInfo.work_dir.is_some()`
* [ ] Commands that require a working directory refuse in bare repos with a targeted message:

  * [ ] `lattice create`
  * [ ] `lattice checkout`
  * [ ] navigation commands (`up/down/top/bottom`)
  * [ ] stack mutation commands (`restack`, `modify`, etc.)
* [ ] Metadata-only mutators (`track`, `untrack`, `freeze`, `unfreeze`) can run in bare repos (no working dir required)

**Worktree-specific blockers handled**

* [ ] If a command would rewrite a branch that is checked out in another worktree, Lattice refuses with a **first-class issue** explaining:

  * [ ] which branch
  * [ ] which worktree path is holding it
  * [ ] what the user should do (switch worktrees or detach)
* [ ] `continue/abort` refuse when run from a different worktree than the one that owns the in-progress Git state (rebase/merge), with a message that points to the correct worktree

**UX behavior**

* [ ] From bare repo, `lattice create` prints guidance:

  * [ ] explains why it cannot proceed
  * [ ] provides a concrete `git worktree add …` example
  * [ ] explicitly states which commands work from bare (ex: `log`, `info`)
* [ ] From bare repo, read-only commands do not attempt working-tree-dependent probes (no “clean status” lie)

### Quality Gates

* [ ] `cargo fmt --check` passes
* [ ] `cargo clippy -- -D warnings` passes
* [ ] `cargo test` passes
* [ ] `cargo doc --no-deps` succeeds
* [ ] All new public APIs have module docs and doctests
* [ ] Integration tests use real Git repositories with real `git worktree` operations

### Architectural Gates

* [ ] No direct parsing of `.git` internals outside the Git interface
* [ ] All ref updates remain CAS-based through the Git interface
* [ ] Path decisions are centralized (no scattered `common_dir.join("lattice")` ad hoc logic)
* [ ] Lock + op-state semantics remain consistent with ARCHITECTURE.md (“single in-flight structural operation”)

---

## Implementation Steps

### Step 1: Expand Git Repository Context (`RepoInfo` + `RepoContext`)

**Goal:** Allow `Git::open()` to succeed in bare repos and worktrees, and provide enough information for scanner/gating to make correct decisions.

**Files:**

* `src/git/interface.rs` (or equivalent git abstraction)

**Implementation:**

* Add:

```rust
pub enum RepoContext { Normal, Bare, Worktree }

pub struct RepoInfo {
    pub git_dir: PathBuf,           // per-worktree git dir
    pub common_dir: PathBuf,        // shared refs/objects dir
    pub work_dir: Option<PathBuf>,  // None for bare
    pub context: RepoContext,
}
```

* Update `Git::open()` to:

  * remove any “bare repo is an error” behavior
  * populate `common_dir` via libgit2 `commondir()` (or equivalent)
  * populate `git_dir` via repo path / gitdir resolution
  * populate `work_dir` from repo workdir if present
  * set `context`:

    * `Bare` if `is_bare()`
    * `Worktree` if `is_worktree()` (or `.git` is a gitfile pointing into `worktrees/`)
    * else `Normal`

* Add helpers:

  * `has_workdir() -> bool`
  * `common_dir() -> &Path`

**Acceptance Criteria (Step 1)**

* [ ] `Git::open()` works from bare/worktree/normal
* [ ] `RepoInfo` fields are correct and stable under `--cwd`

**Tests Required**

* Unit-ish integration (real git repos):

  * [ ] `open_normal_repo_context_is_normal`
  * [ ] `open_bare_repo_context_is_bare`
  * [ ] `open_worktree_context_is_worktree`
  * [ ] `open_from_nested_subdir_discovers_repo`
* Behavior tests:

  * [ ] `worktree_status_unavailable_in_bare` returns `Unavailable` (not “clean”)

---

### Step 2: Introduce Centralized Path Routing (`LatticePaths`)

**Goal:** Prevent “some code uses git_dir, some uses common_dir” drift. Make all Lattice storage locations explicit and consistent.

**Files:**

* `src/core/paths.rs` (new)
* plus call-site updates across config/lock/journal/op-state modules

**Implementation:**
Create a centralized path helper:

```rust
pub struct LatticePaths {
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

impl LatticePaths {
    pub fn repo_lattice_dir(&self) -> PathBuf {
        self.common_dir.join("lattice")
    }
    pub fn repo_config_path(&self) -> PathBuf {
        self.repo_lattice_dir().join("config.toml")
    }
    pub fn repo_lock_path(&self) -> PathBuf {
        self.repo_lattice_dir().join("lock")
    }
    pub fn repo_op_state_path(&self) -> PathBuf {
        self.repo_lattice_dir().join("op-state.json")
    }
    pub fn repo_ops_dir(&self) -> PathBuf {
        self.repo_lattice_dir().join("ops")
    }
}
```

Then attach `paths: LatticePaths` to `RepoInfo` or compute it once in the engine and pass downward.

**Acceptance Criteria (Step 2)**

* [ ] No production code computes `.join("lattice")` outside the path helper
* [ ] Path usage is explicit: repo-scoped storage uses `common_dir`, worktree-scoped (if any) uses `git_dir`

**Tests Required**

* [ ] `paths_repo_scoped_locations_use_common_dir`
* [ ] `paths_normal_repo_common_equals_git_dir` (sanity)
* [ ] `paths_worktree_common_differs_from_git_dir` (sanity)

---

### Step 3: Move Lock + Op-State + Journals to Repo Scope

**Goal:** Ensure “one in-flight structural operation” holds across worktrees, and failures are understandable.

**Files:**

* `src/core/ops/lock.rs`
* `src/core/ops/journal.rs` (or op-state implementation)
* `src/engine/executor.rs` (where op-state is written)
* any code that assumes these live under per-worktree `.git`

**Implementation Changes**

1. Change `RepoLock::acquire()` to take `LatticePaths` (or `common_dir`) and lock `repo_lock_path()`.
2. Change journal/op-state persistence functions to read/write in `repo_ops_dir()` and `repo_op_state_path()`.
3. Extend op-state schema with “originating worktree identity”:

```rust
pub struct OpState {
    pub op_id: String,
    pub phase: Phase,
    pub origin_git_dir: PathBuf,
    pub origin_work_dir: Option<PathBuf>,
    // ... existing fields: touched refs, expected olds, plan digest, etc.
}
```

4. In `continue/abort` gating:

   * if op-state exists and `origin_git_dir != current git_dir`, refuse with guidance:

     * “Run this from the worktree at …”

**Acceptance Criteria (Step 3)**

* [ ] A mutating command in worktree A prevents mutating commands in worktree B via repo-scoped lock
* [ ] Op-state visibility is global across worktrees
* [ ] `continue/abort` correctly detect and refuse “wrong worktree” execution

**Tests Required**

* Integration tests (real worktrees):

  * [ ] `lock_is_shared_across_worktrees`
  * [ ] `op_state_blocks_mutations_in_other_worktrees`
  * [ ] `continue_refuses_from_non_origin_worktree`
  * [ ] `abort_refuses_from_non_origin_worktree`
* Regression tests:

  * [ ] normal repo still stores lock/op-state under `.git/lattice/…`

---

### Step 4: Add `WorkingDirectoryAvailable` Capability + Bare Repo Issue

**Goal:** Convert “bare repo” from a Git error into a clean capability absence.

**Files:**

* `src/engine/capabilities.rs`
* `src/engine/health.rs`

**Implementation**

* Add capability:

```rust
WorkingDirectoryAvailable,
```

* Add health issue:

```rust
pub fn no_working_directory() -> Issue {
    Issue::new(
        "no-working-directory",
        Severity::Blocking,
        "No working directory available (bare repository).",
    ).blocks(Capability::WorkingDirectoryAvailable)
}
```

**Acceptance Criteria (Step 4)**

* [ ] Scanner emits `no-working-directory` issue in bare repos
* [ ] Commands that require workdir are gated by `WorkingDirectoryAvailable`

**Tests Required**

* [ ] `bare_repo_missing_workdir_blocks_workdir_commands`
* [ ] `worktree_repo_has_workdir_capability`

---

### Step 5: Detect Worktree Branch Occupancy and Surface “Checked Out Elsewhere” Blockers

**Goal:** Prevent Git worktree constraints from showing up as mysterious failures during rewrite operations.

**Files:**

* `src/git/interface.rs` (add `worktree_list_porcelain()` helper)
* `src/engine/scan.rs` (store occupancy evidence)
* `src/engine/gate.rs` or command-specific gating (enforce for rewrite plans)
* `src/engine/health.rs` (issue factory)

**Implementation Approach**

1. In Git interface: add a method that returns structured worktree info from:

* `git worktree list --porcelain`

Model:

```rust
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: Option<Oid>,
    pub branch: Option<BranchName>, // None for detached
    pub is_bare: bool,
}
```

2. Scanner stores `Vec<WorktreeEntry>` in the health report as evidence (not necessarily blocking by itself).

3. Gating for commands that rewrite branches (restack/modify/fold/etc.) checks:

   * computed “touched branches” set (from validated context / planned stack)
   * intersect with `branch -> worktree path` map (excluding current worktree)
   * if intersection non-empty: gate fails with a blocking issue that lists conflicts.

Issue factory (shape):

```rust
pub fn branches_checked_out_elsewhere(conflicts: Vec<(BranchName, PathBuf)>) -> Issue { ... }
```

**Acceptance Criteria (Step 5)**

* [ ] Worktree list parsing is robust across platforms
* [ ] Rewrite commands refuse with a clear Lattice issue when any target branch is checked out in another worktree
* [ ] The refusal message names both the branch and the blocking worktree path

**Tests Required**

* Parsing tests:

  * [ ] `parse_worktree_porcelain_basic`
  * [ ] `parse_worktree_porcelain_detached_head`
* Integration tests:

  * [ ] `restack_refuses_if_target_branch_checked_out_elsewhere`
  * [ ] `modify_refuses_if_descendant_checked_out_elsewhere`
  * [ ] `rewrite_allowed_when_conflicts_absent`

---

### Step 6: Update Requirement Sets and Command Gating

**Goal:** Make command eligibility match reality, with minimal ad hoc branching.

**Files:**

* `src/engine/gate.rs`
* command registration mapping (if present)

**Implementation**

1. Add / update requirement sets:

* `MUTATING` includes `WorkingDirectoryAvailable`
* `NAVIGATION` includes `WorkingDirectoryAvailable`
* Add `MUTATING_METADATA_ONLY` (no workdir required) for:

  * `track`, `untrack`
  * `freeze`, `unfreeze`

2. For `submit`/`sync` policy, choose and encode explicitly. Canonical recommended policy for this milestone:

**Policy (Correctness-first, still useful in bare):**

* `submit` and `sync` are allowed from bare repos only when:

  * they do not require working tree mutation, and
  * they are not going to perform a restack, or restack is not needed
* otherwise they refuse with “run from a worktree” guidance

This keeps “submit from bare” possible, but never silently downgrades correctness.

**Acceptance Criteria (Step 6)**

* [ ] All workdir-required commands are gated in bare repos
* [ ] Metadata-only commands succeed in bare repos
* [ ] Submit/sync behavior in bare is explicit and deterministic (no surprise restacks)

**Tests Required**

* [ ] `metadata_only_commands_run_in_bare_repo`
* [ ] `navigation_commands_refuse_in_bare_repo`
* [ ] `submit_from_bare_refuses_if_restack_needed` (or equivalent)
* [ ] `submit_from_bare_succeeds_if_no_restack_needed` (fixture where aligned)

---

### Step 7: UX: `lattice create` Bare Repo Guidance + “Unavailable working copy” correctness

**Goal:** Make the bare repo experience self-explanatory without inventing magical repairs.

**Files:**

* `src/cli/commands/create.rs`
* any shared error rendering module (`src/ui/output.rs` etc.)

**Implementation**
When `create` is gated only by missing `WorkingDirectoryAvailable`, show a tailored message:

* states the reason (bare repo has no working directory)
* provides an actionable `git worktree add` example
* lists a few commands that do work from bare

**Acceptance Criteria (Step 7)**

* [ ] `create` from bare prints the expected guidance message
* [ ] Message does not claim Lattice can auto-create worktrees
* [ ] Message is stable for snapshot testing

**Tests Required**

* Snapshot / integration:

  * [ ] `create_from_bare_prints_worktree_instructions`
* Behavioral:

  * [ ] `log_from_bare_does_not_attempt_working_copy_status`

---

### Step 8: Integration Test Suite for Bare + Worktree Matrix

**Goal:** Prevent regressions by locking in behavior across all repo contexts.

**Files:**

* `tests/worktree_support_integration.rs` (new)
* test harness helpers (`tests/support/*` if you have them)

**Test Scenarios (Required Minimum)**

1. **Bare repo read-only works**

   * [ ] `lattice log` succeeds inside bare
   * [ ] `lattice info` succeeds inside bare

2. **Bare repo workdir-required commands refuse**

   * [ ] `lattice create` refuses with guidance
   * [ ] `lattice checkout` refuses
   * [ ] `lattice restack` refuses

3. **Worktree operations behave normally**

   * [ ] `lattice create` succeeds in a worktree
   * [ ] `lattice log` works in all worktrees

4. **Repo config shared via common dir**

   * [ ] `lattice init` in worktree A is observed by `lattice trunk` in worktree B

5. **Metadata shared**

   * [ ] `track` in worktree A changes `log` output in worktree B

6. **Lock shared**

   * [ ] Acquire lock in A (or simulate) and ensure mutating command in B fails fast

7. **Op-state shared and worktree-origin enforced**

   * [ ] Create op-state (or pause a real operation) in worktree A
   * [ ] Verify worktree B refuses mutation
   * [ ] Verify `continue/abort` from B refuses and points to A

8. **Checked-out-elsewhere refusal**

   * [ ] Create two worktrees with different checked-out branches
   * [ ] Attempt a rewrite that touches the other worktree’s checked-out branch
   * [ ] Assert Lattice surfaces a structured issue (not raw git spew)

---

### Step 9: Documentation Updates

**Goal:** Make the behavior discoverable and predictable.

**Files:**

* `docs/commands/create.md`
* `docs/commands/log.md`
* `docs/commands/submit.md` (if bare-mode limitations exist)
* `docs/commands/doctor.md` (optional notes on worktrees)

**Acceptance Criteria (Step 9)**

* [ ] Docs explicitly state which commands work from bare repos
* [ ] Docs explain worktree recommendation and show a canonical command
* [ ] Any “submit from bare” constraints are documented clearly

**Tests Required**

* [ ] Documentation examples that are intended to be validated are mirrored in integration tests (no drifting docs)

---

## Files to Create/Modify

| File                                    | Action | Description                                                                        |
| --------------------------------------- | ------ | ---------------------------------------------------------------------------------- |
| `src/git/interface.rs`                  | Modify | Add `RepoContext`, expand `RepoInfo`, add worktree listing helper                  |
| `src/core/paths.rs`                     | Create | Central `LatticePaths` (repo-scoped vs worktree-scoped paths)                      |
| `src/core/config/mod.rs`                | Modify | Load/write repo config from `common_dir` via `LatticePaths`                        |
| `src/core/ops/lock.rs`                  | Modify | Lock path becomes repo-scoped (`common_dir`)                                       |
| `src/core/ops/journal.rs`               | Modify | Journals + op-state repo-scoped; add origin worktree fields                        |
| `src/engine/capabilities.rs`            | Modify | Add `WorkingDirectoryAvailable`                                                    |
| `src/engine/scan.rs`                    | Modify | Populate capability + capture worktree occupancy evidence                          |
| `src/engine/gate.rs`                    | Modify | Update requirement sets; add metadata-only mutating set; enforce worktree blockers |
| `src/engine/health.rs`                  | Modify | Add issues: no-workdir, branch-checked-out-elsewhere                               |
| `src/cli/commands/create.rs`            | Modify | Bare-repo guidance message                                                         |
| `tests/worktree_support_integration.rs` | Create | Full bare/worktree matrix integration tests                                        |
| `docs/commands/*.md`                    | Modify | Behavior documentation updates                                                     |

---

## Test Count Target

| Category                         | Count      |
| -------------------------------- | ---------- |
| Existing tests                   | (baseline) |
| Repo context detection tests     | ~6         |
| Path routing tests               | ~3         |
| Lock/op-state scope tests        | ~6         |
| Worktree occupancy parsing tests | ~4         |
| Rewrite gating conflict tests    | ~4         |
| CLI UX snapshots                 | ~2         |
| Integration matrix tests         | ~10–14     |
| **Target New Tests**             | **~35–45** |

(If you already have a strong integration harness, skew toward more integration and fewer unit tests for Git worktree behavior. Worktrees are a “real Git” feature; trust reality.)

---

## Implementation Notes

### Correctness stance on “concurrent worktrees”

This milestone intentionally keeps Lattice as **single-writer per repository**. That means:

* concurrent read-only commands: allowed
* concurrent mutating commands across worktrees: serialized (lock), not “parallel”

This is not a limitation of Git; it’s an intentional choice to preserve:

* unambiguous op-state semantics
* deterministic gating
* clean recovery (`continue/abort`) behavior

If we ever want *true parallel mutation* across worktrees, it would require an architectural extension (multi-op-state, per-ref locking, or partitioned operation domains). That is explicitly out of scope here.

### “Bare repo” should not pretend to have a clean working tree

Bare repos don’t have a working copy. Any API that returns “clean” for bare repos is a future bug magnet. Prefer an explicit `Unavailable` state.

### Worktree branch occupancy is a first-class constraint

Git will refuse some operations when a branch is checked out elsewhere. Detect it early and explain it clearly. The best error is the one users never have to decode from porcelain.
