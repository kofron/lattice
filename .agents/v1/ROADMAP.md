## Roadmap to fully implement **Lattice** (Rust single-binary) per the **Spec**, enforced by the **Architecture Reference**

This roadmap is written like a TPM plan you can drop into Jira/Linear: epics, milestones, sequencing, hard acceptance gates, and the engineering “shape” of the work. It assumes we will **not** cut corners on the Architecture Reference, even where it slightly diverges from the CLI spec. Where there are mismatches, I call them out and propose a concrete reconciliation that preserves correctness.

---

# 0) Reconcile spec vs architecture (do this first, in writing, then code)

Before anyone writes serious code, we lock these reconciliations so the codebase doesn’t become a hydra with two heads.

### 0.1 Operation journal vs op-state marker (spec `.git/lattice/ops/*` + `current-op` vs arch `.git/lattice/op-state.json`)

**Decision (recommended): implement both as one system with two representations.**

* **Journal** (spec) is the durable, append-only step ledger:

  * `.git/lattice/ops/<op_id>.json` (append-only + fsync at step boundaries)
* **Op-state** (architecture) is the authoritative “traffic light” during execution:

  * `.git/lattice/op-state.json` exists only while executing or awaiting user
  * contains: `op_id`, command, phase, touched refs + expected old OIDs, plan digest
* We do **not** need `.git/lattice/current-op` if op-state exists, but we may optionally write it as a compatibility shim:

  * If present, it must match `op_id` in op-state, otherwise it is treated as corruption and triggers doctor.

**Why this is correct:** architecture wants a single explicit marker; spec wants crash-safe journaling. You get both without duplicated truth.

### 0.2 Repo config path mismatch (spec `.git/lattice/repo.toml` vs arch `.git/lattice/config.toml`)

**Decision:** canonicalize on architecture’s `.git/lattice/config.toml`, but support spec path as a compatibility read.

* Writes go to `.git/lattice/config.toml` (atomic rename).
* Reads:

  1. `.git/lattice/config.toml`
  2. if absent, `.git/lattice/repo.toml` (warn, offer `lattice doctor` fix to migrate)

### 0.3 Metadata schema differences (spec “kind/schema_version/timestamps/structured states” vs arch “structural vs cached”)

**Decision:** implement the spec schema as the on-disk format (JSON), but split parsing into:

* **Structural view** (used for graph + invariants): parent/base/freeze and versioning
* **Cached view** (PR linkage/status): must never justify structural changes

Also enforce **strict parsing** (architecture): `deny_unknown_fields` per schema version.

### 0.4 Git interface requirement 

**Decision:** implement all git operations via `git2` crate.

* No direct calling of `git`.  We don't want to deal with different systems.

---

# 1) Program-level milestones (critical path first)

## Milestone 0: Repo bootstrap and guardrails (foundation)

**Goal:** create the rails so the train can’t drive into the ocean.

### Deliverables

* Workspace scaffold matching the recommended crate layout, plus required architecture components:

  * `engine/` (Scan/Gate/Plan/Execute loop)
  * `scan/`, `gate/`, `plan/`, `exec/`, `doctor/`
* CI:

  * `cargo fmt`, `cargo clippy -D warnings`
  * unit + integration tests
  * doc tests
  * minimal cross-platform (linux + mac) because secret storage + file permissions behave differently
* Feature flags:

  * `keychain` (optional, via `keyring`)
  * `fault_injection` (required for journaling/executor tests)
  * `live_github_tests` (optional harness behind env vars)
* Repo docs skeleton:

  * `docs/commands/` directory exists (even empty)
  * `docs/references.md` placeholder

### Acceptance gates

* `cargo test` passes in CI.
* `cargo doc` succeeds.
* A “hello command” runs through CLI parse and Engine lifecycle (even if it no-ops).

---

## Milestone 1: Core domain types, schemas, and deterministic serialization

**Goal:** define the vocabulary of the system so everything else composes cleanly.

### Workstreams

#### 1.1 Strong types (core correctness)

Implement in `core/`:

* `BranchName`:

  * validated refname-compatible
  * prohibits `..`, spaces, `~`, `^`, `:`, leading `-`, etc
  * canonical string form
* `Oid` wrapper for git object ids (40-hex SHA1 and future-proof to SHA256 if desired)
* `RefName` wrapper (validated)
* `UtcTimestamp` wrapper (RFC3339)

Unit tests: exhaustive branch name validation cases.

#### 1.2 Config schema (global + repo)

Implement:

* Global config loader with precedence:

  1. `$LATTICE_CONFIG`
  2. `$XDG_CONFIG_HOME/lattice/config.toml`
  3. `~/.lattice/config.toml` (canonical write)
* Repo config:

  * canonical write: `.git/lattice/config.toml`
  * compatibility read: `.git/lattice/repo.toml` and `.lattice/repo.toml` (warn)
* Merge semantics: repo overrides global; CLI flags override both

Tests:

* precedence tests
* invalid schema rejection
* atomic write test (write temp + rename)

#### 1.3 Metadata schema (v1)

Implement serde structs reflecting Appendix A (spec), with `deny_unknown_fields`:

* required: `kind`, `schema_version`, `branch`, `parent`, `base`, `freeze`, `pr`, `timestamps`
* `freeze.state`: `frozen | unfrozen` and structured fields when frozen
* `pr.state`: `none | linked` with required fields when linked

Also provide:

* `StructuralMetadataView` (parent/base/freeze)
* `CachedMetadataView` (PR linkage/status)

Tests:

* round-trip JSON
* unknown field rejection
* schema_version mismatch rejection
* structural extraction correctness

### Acceptance gates

* Doctests exist for pure functions (naming, parsing).
* Schema parsing is strict and tested.

---

## Milestone 2: Single Git interface (the only doorway to repo state)

**Goal:** everything Git goes through one chokepoint, with typed errors.

### 2.1 `git::Git` interface

Implement a single module (example shape):

* `Git::run(args, opts) -> Result<GitOutput, GitErrorCategory>`
* Options include:

  * cwd
  * env overrides
  * interactive policy
  * hook verification policy (`--no-verify` support where applicable)

Typed error categories:

* `NotARepo`
* `DirtyWorktree`
* `ConflictsInProgress` (rebase/merge/cherry-pick/revert detected)
* `RefNotFound`
* `CommandFailed { code, stderr }`

### 2.2 Git queries needed for scanner + commands

Implement helpers (shelling out):

* repo discovery:

  * `git rev-parse --git-dir`
  * `git rev-parse --show-toplevel`
* refs:

  * list local branches: `git for-each-ref refs/heads --format=...`
  * get ref oid: `git rev-parse <ref>`
  * CAS updates: `git update-ref <ref> <new> <old>`
  * delete ref with expected old: `git update-ref -d <ref> <old>`
* object creation:

  * write blob: `git hash-object -w --stdin`
* ancestry:

  * `git merge-base --is-ancestor <a> <b>`
  * `git merge-base <a> <b>`
* status:

  * `git status --porcelain=v1 -uno` (or v2) for cleanliness
* in-progress ops detection:

  * `git rev-parse --git-path rebase-merge`, etc then filesystem check inside Git module
  * and/or `git status` hints (still confirm with git-path)

### 2.3 Git mutation primitives for executor

* `rebase_onto(onto, upstream, branch)`:

  * `git rebase --onto <onto> <upstream> <branch>`
* continue/abort:

  * `git rebase --continue`, `git rebase --abort`
  * same for merge, cherry-pick, revert

Tests:

* golden tests that parse git output robustly (avoid brittle string parsing where possible).

### Acceptance gates

* No other module is allowed to call `Command::new("git")` directly (enforce via a lint pattern, or a “git is private” module).
* Integration tests can create a repo and query branches via Git interface only.

---

## Milestone 3: Persistence layer (metadata refs, secrets, config writes)

**Goal:** implement repo-resident storage and user-resident secrets behind stable abstractions.

### 3.1 Metadata store (refs/branch-metadata/*)

Implement `core::metadata::Store`:

* `read(branch) -> Option<(oid, MetadataV1)>` where `oid` is the blob oid pointed to by the metadata ref
* `write_cas(branch, expected_old_ref_oid, new_metadata_json) -> new_ref_oid`

  * create blob with `hash-object -w`
  * update ref with CAS to blob oid
* `delete_cas(branch, expected_old_ref_oid)`

Important: metadata ref CAS is against the metadata ref’s current value (blob oid), not the branch tip.

Tests:

* strict parsing exercised via real git objects
* CAS failure handling

### 3.2 SecretStore abstraction

Implement trait exactly as spec.
Provide:

* `FileSecretStore` writing to `~/.lattice/secrets.toml`

  * enforce 0600 on unix
  * never print secrets
* `KeychainSecretStore` behind feature flag `keychain`

Tests:

* file permissions (unix)
* token not present in stdout/stderr snapshots

### 3.3 Repo lock

Implement executor lock:

* file path: `.git/lattice/lock`
* use OS file locking (`fs2` or similar)
* lock is held for entire plan execution

Tests:

* lock contention test (spawn two lattice processes in integration test; second must fail fast with exit code 3 or a dedicated lock code)

### Acceptance gates

* We can atomically write metadata via refs with CAS.
* We can store/retrieve PAT via SecretStore with mock + real store tests.

---

## Milestone 4: Engine lifecycle (Scan → Gate → Plan → Execute → Verify), plus event ledger

**Goal:** build the architecture “spine” before implementing a zoo of commands.

### 4.1 Scanner

Implements:

* reads repo config, global config
* reads metadata refs and branch refs
* detects in-progress external git ops (rebase/merge/etc)
* detects lattice op in progress (`.git/lattice/op-state.json`)
* produces:

  * `RepoSnapshot` (refs, metadata, config)
  * `RepoHealthReport` (issues + evidence)
  * `Capabilities` set (the composable proofs)

Also computes:

* `Fingerprint` hash over:

  * trunk ref
  * all tracked branch tips
  * all structural metadata ref oids
  * repo config version (or config file hash)

### 4.2 Event ledger (`refs/lattice/event-log`)

Implement append-only commit chain:

* each event is a JSON blob stored in a commit message or a tree file (prefer tree file `event.json` for future-proofing)
* update `refs/lattice/event-log` with CAS

Events required by architecture:

* `IntentRecorded`
* `Committed`
* `Aborted`
* `DivergenceObserved`
* `DoctorProposed`
* `DoctorApplied`

When scanning:

* compare current fingerprint with last `Committed` fingerprint
* if different, record `DivergenceObserved` (evidence, diff summary)

### 4.3 Gating

Implement:

* command requirement sets as code (per command)
* `gate(command, snapshot, capabilities) -> ReadyContext | RepairBundle`

Important: there is no global “valid repo”. Each command has its own contract.

### 4.4 Planner

Implement deterministic plan generation:

* `Plan` is serializable, previewable, stable ordering
* `PlanDigest` is hash of canonical plan JSON
* Steps are typed, with explicit touched refs:

  * `UpdateRefCAS`
  * `DeleteRefCAS`
  * `WriteMetadataCAS`
  * `RunGit { args, expected_effects }`
  * `Checkpoint { name }`

Planner does not do I/O. It consumes validated contexts and emits plans.

### 4.5 Executor

Implements single mutation path:

* acquires lock
* writes `.git/lattice/op-state.json` before first mutation
* writes journal `.git/lattice/ops/<op_id>.json` with fsync per step boundary
* records `IntentRecorded` in event ledger
* applies plan:

  * all ref updates are CAS
  * if CAS fails: stop immediately, record `Aborted`, leave repo unchanged (or roll back what was already done)
  * if git conflict pauses: mark op-state `awaiting_user`, record pause in journal, exit code 1 with instructions
* post-apply scan + verify required invariants
* record `Committed`, remove op-state

### 4.6 Fast verify (core invariant enforcement)

Implement `core::verify::fast_verify(snapshot) -> Result<()>`:

* parseability
* acyclic graph
* tracked branches exist
* base ancestry constraints (base is ancestor of tip; base reachable from parent tip)
* freeze invariants sanity (freeze state present)

### Acceptance gates

* A dummy mutating plan can run transactionally with CAS updates and journaling.
* Crash simulation (kill process mid-plan) results in:

  * op-state present
  * next invocation refuses other mutations and routes to continue/abort
* DivergenceObserved is recorded when out-of-band changes occur.

---

## Milestone 5: Doctor framework (explicit repair, never guessing)

**Goal:** when reality is messy, Lattice is calm and explicit.

### 5.1 Issue catalog (initial)

Implement deterministic issues with stable ids:

* Missing trunk config
* Metadata parse failure
* Parent ref missing
* Cycle detected (include cycle trace)
* Base ancestry violated (include evidence: base oid, parent tip, child tip)
* Metadata ref exists but branch missing (or vice versa)
* Op-state exists (must resolve via continue/abort)
* External git op in progress (must resolve via git or lattice abort/continue depending)

### 5.2 Fix options and repair plans

Each issue provides `FixOption`s that produce concrete plans:

* Set trunk (prompt or non-interactive requires explicit `--trunk`)
* Untrack + retrack branch
* Move branch onto a different parent
* Clear metadata namespace (init --reset style) with confirmation
* Migrate repo config file path
* Resolve stale op-state (if journal missing, require explicit user confirmation and record DoctorApplied)

Non-interactive behavior:

* Doctor lists issue ids and fix ids
* Applies only when fix ids explicitly provided

### 5.3 Doctor command and doctor-as-handoff

Implement:

* `lattice doctor` command
* automatic doctor handoff when gating fails for any other command

### Acceptance gates

* For each blocking scanner issue, a user gets fix options, not a shrug.
* No repair is applied without explicit confirmation (interactive) or explicit fix id (non-interactive).

---

# 2) Implement commands in properly sequenced layers (Phase 1 → Phase 4)

Below is the command implementation roadmap, ordered by dependency and “surface area”.

## Milestone 6: Phase 1 commands (core local stack engine, shippable)

This milestone maps to the spec’s Phase 1, but implemented through the architecture lifecycle.

### 6.1 Read-only and low-risk commands first

These exercise Scan/Gate without heavy execution.

#### Commands

* `lattice log` (short/long, stack filtering, snapshots)
* `lattice info [branch]` (+ diff/stat/patch/body via git)
* `lattice parent`, `lattice children`
* `lattice trunk` (print trunk)

#### Gating requirements (examples)

* `log`: `RepoOpen` only, degrade gracefully if metadata unreadable
* `info`: `RepoOpen`, optionally `MetadataReadable` if requesting stack relations

#### Tests

* snapshot tests for `log` formats
* integration tests for `info --diff`

---

### 6.2 Repo setup and config commands

#### Commands

* `lattice init [--trunk] [--reset]`
* `lattice config get/set/list` (and `edit` optional)
* `lattice completion`
* `lattice changelog`
* `lattice alias` (expansion rules, shadowing prevention)

#### Key implementation details

* `init --reset` must be a plan:

  * delete all `refs/branch-metadata/*` with CAS (or in a controlled sweep)
  * clear `.git/lattice/ops/` history if desired (spec says clear operation history)
  * record event ledger entry

#### Tests

* reset requires force in non-interactive
* invalid trunk errors

---

### 6.3 Tracking and structure commands

#### Commands

* `lattice track [branch] [--parent] [--force] [--as-frozen]`
* `lattice untrack [branch] [--force]` (descendants prompt)
* `lattice freeze [branch]`, `lattice unfreeze [branch]` with downstack inclusive default

#### Planning/execution notes

* Track plan:

  * compute base = parent tip
  * write metadata ref CAS (expected old = none or existing if updating)
* Untrack plan:

  * compute subtree (branch + descendants) from graph
  * delete only metadata refs, not branches

Freeze enforcement:

* add a gating capability: `FrozenPolicySatisfied(scope)` for each command that rewrites
* implement a reusable “affected branches” computation in planner:

  * if the plan would move tips of branches X, verify none are frozen

#### Tests

* `--force` nearest ancestor selection by commit ancestry
* freeze blocks rewriting commands later (start writing these tests now, even if commands not implemented yet)

---

### 6.4 Navigation commands

#### Commands

* `lattice checkout [branch] [--trunk] [--stack]`
* `lattice up/down/top/bottom`

Implementation:

* mostly read-only planning that produces `RunGit checkout/switch` steps
* non-interactive ambiguity errors

Tests:

* multi-child prompt errors in non-interactive

---

### 6.5 Core mutating engine commands (the heart)

#### Commands

* `lattice create [name]` including empty create and `--insert`
* `lattice restack` (base-driven, bottom-up traversal)
* `lattice continue`
* `lattice abort`
* `lattice undo`

#### Implementation sequencing (recommended)

1. **restack** first (it sets the template for conflicts + pausing)
2. **continue/abort** next (so restack can pause safely)
3. **undo** (journal replay locally)
4. **create** (branch + metadata + optional insert)

#### Conflict-pausing mechanics

* Executor detects `git rebase` conflict exit code
* Writes journal conflict block and op-state `awaiting_user`
* Exits with code 1 and precise instructions

Continue:

* reads op-state + journal
* runs the correct git continue command
* resumes remaining steps

Abort:

* runs git abort if needed
* rolls back ref updates from journal snapshots (CAS where possible, otherwise doctor)

Undo:

* replays last committed op journal snapshots to restore refs and metadata
* explicitly cannot undo remote operations (Phase 2+)

#### Tests (must-have)

* restack no-op
* restack conflict pause + continue success
* abort restores pre-op after multiple rebases
* crash simulation mid-restack:

  * op-state exists
  * next command invocation refuses with exit code 3
  * continue/abort resolves it

### Phase 1 acceptance gate (hard)

* All Phase 1 commands have integration tests.
* Fault injection tests exist for executor step boundaries:

  * simulated failure after journal append
  * simulated failure after ref update
  * verify rollback or recoverable awaiting_user state
* Docs exist for each Phase 1 command:

  * `docs/commands/<cmd>.md` with Summary/Examples/Flags/Semantics/Pitfalls/Recovery/Parity notes

---

## Milestone 7: Robustness test harnesses (out-of-band reality)

**Goal:** prove the architecture promise: Lattice stays correct when users do random git things.

### 7.1 Out-of-band fuzz harness

Build an integration harness that interleaves:

* lattice operations
* direct git operations:

  * manual rebase
  * branch rename/delete
  * editing metadata refs (simulate corruption)
  * force-updating branch tips

Asserts:

* gating never produces ReadyContext when requirements not met
* doctor offers repair options
* executor never applies plan when CAS fails
* after any reported success, fast verify passes

### 7.2 Property-based graph tests

Use `proptest` to generate DAG shapes and validate:

* cycle detection correctness
* descendant computation correctness
* restack traversal ordering

### Acceptance gate

* A nightly CI job runs fuzz harness (even with limited iterations).
* A small deterministic seed suite runs in PR CI.

---

## Milestone 8: Phase 2 GitHub integration (Forge + auth + PR workflows)

This is the first time we touch network and remote state. Architecture requires remote interactions are a later phase in a plan and must not be needed to restore local invariants.

### 8.1 Forge abstraction + GitHub adapter

Implement `forge::Forge` trait as spec, plus:

* `GitHubForge` implementation:

  * REST: create/update/get/find by head, request reviewers, merge
  * GraphQL: draft toggling (`set_draft`)
* Remote repo identification:

  * parse `origin` URL (SSH/HTTPS) to org/repo
  * allow repo override in config

Tests:

* mock Forge for deterministic behavior (most tests)
* adapter tests for error mapping (401/403/404/429/5xx)

### 8.2 `lattice auth`

Implement:

* `auth --token` non-interactive
* interactive masked prompt otherwise
* stores under SecretStore key `github.pat`

Tests:

* token never printed
* provider override works

### 8.3 Remote-aware scanner capabilities

Add scanner capabilities:

* `RemoteResolved`
* `AuthAvailable` (token present)
* optionally `GitHubRepoResolved`

### 8.4 Implement GitHub commands in dependency order

#### 1) `lattice pr` and `lattice unlink`

* mostly read-only plus cached metadata updates
* `pr` resolves PR by linked metadata else `find_pr_by_head`

#### 2) `lattice submit` (largest surface area)

Implement with strict sequencing:

1. Gate requires:

   * restack requirements
   * remote resolved
   * auth available
2. Plan phases:

   * Local structural:

     * optional restack (default on)
     * verify success before any remote operations
   * Remote phase:

     * push branches with skip-unchanged logic
     * PR create/update in stack order
     * GraphQL draft/publish toggles
   * Cached metadata writes:

     * write PR linkage updates via metadata CAS

Key semantics to implement faithfully:

* branch set selection (ancestors; `--stack` adds descendants)
* skip pushing unchanged
* `--always` forces push
* default push uses force-with-lease
* `--force` overrides lease failure (explicit danger)
* idempotent submit:

  * link existing PR by head if found
  * update instead of duplicate create

Tests (minimum set from spec):

* new stack creates PRs with correct bases
* rerun submit updates existing PRs, no duplicates
* skip unchanged push works
* `--always` pushes regardless
* `--force` overwrites remote divergence (simulate)
* `--dry-run` no changes
* draft create + publish toggling calls GraphQL path
* missing auth returns exit code 1 with clear message

#### 3) `lattice sync`

* `git fetch`
* trunk FF update, diverged trunk requires prompt or `--force`
* detect merged/closed PRs and prompt to delete local branches
* optional restack afterwards

#### 4) `lattice get`

* fetch by branch or PR number
* track fetched branch, default frozen unless `--unfrozen`
* choose parent by:

  1. metadata refs if enabled and present
  2. PR base branch
  3. trunk
* force overwrite local divergence only with `--force`

#### 5) `lattice merge`

* merges PRs in order using GitHub merge API
* dry-run and confirm gating

### Phase 2 acceptance gate

* Full suite passes with mocked Forge.
* Optional “live” harness exists behind env vars and is non-blocking for CI.
* Documentation for each remote command includes failure modes and recovery.

---

## Milestone 9: Phase 3 advanced rewriting and structural mutation commands

These are the “sharp knives”. Implement after executor + restack + continue/abort are rock solid.

### Recommended sequencing (each relies on restack + journal + conflict handling)

1. `lattice modify` (amend / create-first-commit + auto-restack descendants)
2. `lattice move` (reparent + rebase + cycle prevention)
3. `lattice rename` (ref rename + metadata ref rename + fix parent pointers)
4. `lattice delete` (reparent children + optional upstack/downstack)
5. `lattice squash` (collapse branch commits + restack descendants)
6. `lattice fold` (merge into parent + delete + optional `--keep`)
7. `lattice pop` (net diff into parent as uncommitted changes)
8. `lattice reorder` (editor-driven reorder + rebase sequence)
9. `lattice split`:

   * implement `--by-commit` and `--by-file` in v1
   * `--by-hunk` can be explicitly deferred with stable NotImplemented error and tests if needed
10. `lattice revert <sha>` (new branch off trunk + git revert + conflict pause)

### Cross-cutting enforcement for all of these

* Freeze policy must be checked against the full affected set:

  * current branch
  * any descendant being rebased
  * any ancestor impacted by fold/delete semantics
* Every rewrite must:

  * journal before/after OIDs
  * update metadata only after branch ref changes succeed
  * pause safely on conflicts

### Tests (non-negotiable)

For each command:

* happy path integration test
* non-interactive ambiguity refusal test (where applicable)
* freeze blocking test
* conflict pause + continue/abort test (where conflicts are possible)

### Acceptance gate

* Conflict scenarios are covered for each rewriting command.
* Undo coverage exists for local ref moves and metadata changes (with clear messaging that remote cannot be undone).

---

## Milestone 10: Phase 4 multi-forge scaffolding activation (without destabilizing core)

**Goal:** prove the architecture boundary works: core depends on `Forge`, not GitHub.

### Deliverables

* `GitLabForge` stub behind feature flag:

  * returns NotImplemented for operations
  * compiles and is selectable in config
* Ensure no command code imports GitHub-specific types except inside adapter module
* Add integration tests asserting that:

  * selecting unsupported forge produces stable, actionable errors

### Acceptance gate

* Swapping forge selection does not require touching planner/executor logic.

---

# 3) Engineering execution plan (how to slice work for teams)

Here’s a practical way to parallelize without stepping on invariants.

### Track A: Core architecture spine (highest leverage)

* Git interface
* Scanner + capabilities
* Plan + executor + lock + journal + op-state
* Event ledger + fingerprint + divergence detection

### Track B: Storage and schema

* config loader/writer
* metadata store
* secret stores

### Track C: Local commands

* log/info navigation
* tracking/freeze
* create/restack/continue/abort/undo

### Track D: Doctor

* issue catalog + fix options
* doctor command + handoff

### Track E: GitHub adapter + remote commands

* forge trait + github impl
* submit/sync/get/merge/pr/unlink

Parallelization rule: **no mutating command merges until it uses executor plans**. If a PR introduces a “direct git mutation” path, treat it as a correctness bug.

---

# 4) “Definition of Done” for FULL spec implementation

To declare v1 complete, all of the following must be true:

### Correctness and integrity

* Every mutating command:

  * uses engine lifecycle
  * uses planner + executor
  * uses lock + journal + op-state
  * uses CAS ref updates for all ref mutations including metadata refs
  * fast verify passes at end (or exits in a valid awaiting_user conflict state)

### Out-of-band resilience

* Divergence detection implemented and recorded in event ledger.
* Gating prevents acting on invalid models.
* Doctor offers explicit repair options and never applies repairs without confirmation.

### GitHub v1 completeness

* Auth via SecretStore.
* Submit is idempotent, supports draft toggling via GraphQL, and push semantics match spec.
* Sync/get/merge/pr/unlink work with robust error mapping.

### Tests

* Every command and flag path has tests, including pitfall tests.
* Fault injection tests exist for executor/journal step boundaries.
* Out-of-band fuzz harness exists and runs (at least in nightly).

### Documentation

* `docs/commands/<cmd>.md` exists for every command in the CLI.
* `docs/references.md` exists and is maintained.
* Every module has `//!` docs and doctests where feasible.

---

# 5) Practical implementation checklist (quick “what to code next”)

If you want the tightest critical-path ordering (the shortest route through the maze), it’s this:

1. Strong types + strict schemas (metadata + config)
2. Git interface (single doorway)
3. Metadata store in refs with CAS
4. Lock + journal + op-state
5. Scanner + fingerprint + event ledger
6. Gating + ready contexts
7. Planner + executor
8. restack + continue/abort (prove pausing works)
9. track/untrack/freeze + verify
10. create (including empty + insert)
11. log/info/navigation
12. doctor
13. auth + forge + submit
14. sync/get/merge/pr/unlink
15. advanced rewrite commands one-by-one with conflict tests
