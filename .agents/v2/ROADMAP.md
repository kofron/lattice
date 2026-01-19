# Lattice v2 Roadmap

This roadmap covers v2 features for Lattice, building on the foundation established in v1.

## Overview

v2 focuses on completing stubbed implementations, bringing code into spec compliance, and adding new features.

**Governing documents:**
- `SPEC.md` - Engineering specification (command behavior, schemas, tests)
- `ARCHITECTURE.md` - Architectural constraints (execution model, doctor framework, event ledger)

All items in this roadmap MUST comply with these documents. Discrepancies are noted where found.

---

## Implementation Status Summary

| Category | Milestone | Status |
|----------|-----------|--------|
| **Compliance** | Doctor Fix Execution | Complete |
| **Compliance** | Sync Restack | Complete |
| **Compliance** | OAuth RepoAuthorized | Complete |
| **Compliance** | Bare Repo Command Flags | Complete |
| **Compliance** | Event Ledger Completion | Complete |
| **Compliance** | Documentation Alignment | Complete |
| **Foundation** | TTY Detection | Stubbed |
| **Foundation** | Upstack Scope Walking | Incomplete |
| **High Impact** | Alias Command | Not started |
| **Feature** | Split By-Hunk | Deferred |

---

# Category 1: Compliance - Completing Existing Work

These items must be done first to bring the codebase into compliance with SPEC.md and ARCHITECTURE.md.

---

### Milestone 1.1: Doctor Fix Execution

**Status:** Complete

**Priority:** Critical - Core repair functionality is broken

**Spec reference:** ARCHITECTURE.md Section 8 "Doctor: explicit repair with user confirmation"

**Goal:** Complete the `lattice doctor --fix` command to actually execute repair plans.

**Background:** Currently, `lattice doctor --fix <issue-id>` shows the repair plan but prints "Execution not yet implemented" and does not apply fixes.

**Evidence:** `src/cli/commands/mod.rs` line 355:
```rust
println!(
    "Would apply {} fix(es). Execution not yet implemented.",
    parsed_fix_ids.len()
);
```

**ARCHITECTURE.md requirements (Section 8):**

Per Section 8.1: "Doctor shares the same: scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

Per Section 8.3: "Non-interactive mode: doctor applies fixes only when fix ids are provided explicitly; doctor never auto-selects fixes."

Per Section 8.4: "After applying a repair plan, doctor performs a full post-verify and records `DoctorApplied` in the event ledger."

**Key deliverables:**

- Wire doctor fix execution through the existing Executor (same as other commands)
- Apply repair plans using the same transactional model (lock, journal, CAS)
- Record `DoctorApplied` event in event ledger after successful fix
- Record `DoctorProposed` event when fix options are presented
- Handle partial failures: transition to `awaiting_user` op-state if conflicts occur
- Post-verify after repair completes

**Critical files:**

- `src/cli/commands/mod.rs` (lines 340-360) - Replace stub with executor call
- `src/doctor/planner.rs` - Ensure plans are executor-compatible
- `src/doctor/mod.rs` - Add `DoctorProposed` event recording
- `src/engine/exec.rs` - Execute doctor plans (no special mode needed per ARCHITECTURE.md)
- `src/engine/ledger.rs` - Event types already defined, wire recording

**Acceptance gates (per ARCHITECTURE.md Section 8):**

- [x] Doctor uses the same executor as other commands (no separate repair path)
- [x] `lattice doctor --fix trunk-not-configured` actually sets trunk
- [x] `lattice doctor --fix metadata-parse-error` repairs or removes corrupt metadata
- [x] `DoctorApplied` event recorded in ledger after successful fix
- [x] `DoctorProposed` event recorded when fix is planned (per Section 3.4.2)
- [x] Non-interactive mode requires explicit `--fix <id>` (never auto-selects)
- [x] If repair requires conflict resolution, transitions to `awaiting_user` op-state
- [x] Post-verify runs after repair completes
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

### Milestone 1.2: Sync Restack Implementation

**Status:** Complete

**Priority:** High - Flag exists but does nothing

**Spec reference:** SPEC.md Section 8E.3 "lattice sync"

**Goal:** Implement the `--restack` flag for `lattice sync`.

**Implementation:** Replaced stub with call to existing `restack::restack()` function, reusing the fully-tested restack infrastructure (lock acquisition, journal management, conflict handling, frozen branch skipping, topological ordering).

**SPEC.md requirements (Section 8E.3):**

- "If `--restack` enabled: restack all restackable branches; skip those that conflict and report"
- Default behavior is NOT specified as restack-by-default (unlike submit)

**Key deliverables:**

- Call existing restack logic from sync when `--restack` flag is set
- Skip branches that would conflict and report them
- Handle restack conflicts with pause/continue model

**Critical files:**

- `src/cli/commands/sync.rs` (line 218-230) - Wired restack call
- `src/cli/commands/restack.rs` - Reused existing restack logic

**Acceptance gates (per SPEC.md Section 8E.3 Tests):**

- [x] `lattice sync --restack` restacks all restackable branches
- [x] Frozen branches are skipped and reported (via restack logic)
- [x] Restack happens post-trunk update
- [x] Conflicts pause correctly with op-state marker (via restack logic)
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

### Milestone 1.3: OAuth RepoAuthorized Capability

**Status:** Complete

**Priority:** Medium - Auth works but repo authorization check missing

**Spec reference:** SPEC.md Section 8E.0.1 "Determining RepoAuthorized"

**Goal:** Implement the `RepoAuthorized(owner, repo)` capability per SPEC.md.

**Implementation:** Added GitHub installations API client, authorization cache with 10-minute TTL, and integrated into the scanner/gating system. The scanner now checks `RepoAuthorized` after verifying `AuthAvailable` and `RemoteResolved`, using the cache to avoid repeated API calls.

**SPEC.md requirements (Section 8E.0.1):**

1. Query `GET /user/installations` to list user's app installations
2. For each installation, query `GET /user/installations/{installation_id}/repositories`
3. If repo found: cache `installation_id` and `repository_id`, return `RepoAuthorized` capability
4. If not found: output install instructions `https://github.com/apps/lattice/installations/new`, exit code 1

**Caching (per SPEC.md):**
- Cache in `<common_dir>/lattice/cache/github_auth.json`
- TTL: 10 minutes
- Repo config caches must never be trusted without validation

**Key files implemented:**

- `src/auth/installations.rs` (NEW) - GitHub installations API client with pagination
- `src/auth/cache.rs` (NEW) - Authorization cache with 10-minute TTL
- `src/auth/mod.rs` - Export new modules
- `src/engine/capabilities.rs` - Add `RepoAuthorized` capability
- `src/engine/health.rs` - Add `app_not_installed` and `repo_authorization_check_failed` issues
- `src/engine/scan.rs` - Derive `RepoAuthorized` capability during scan
- `src/engine/gate.rs` - Add `RepoAuthorized` to `REMOTE` and `REMOTE_BARE_ALLOWED` requirement sets

**Acceptance gates (per SPEC.md Section 8E.0.1):**

- [x] `GET /user/installations` fetches user's app installations
- [x] `GET /user/installations/{id}/repositories` checks repo access (with pagination)
- [x] `RepoAuthorized(owner, repo)` capability derived by scanner
- [x] Authorization cached in `<common_dir>/lattice/cache/github_auth.json`
- [x] Cache TTL is 10 minutes
- [x] Cache invalidated on 403/404 from API
- [x] App-not-installed message: `https://github.com/apps/lattice/installations/new`
- [x] `AppNotInstalled` is a blocking issue with user-action fix (per ARCHITECTURE.md 8.2)
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

### Milestone 1.4: Bare Repo Command Flags

**Status:** Complete

**Priority:** Medium - SPEC.md requires explicit flag behavior

**Spec reference:** SPEC.md Section 4.6.7 "Bare repo policy for submit/sync/get"

**Goal:** Wire the `--no-restack` and `--no-checkout` flags per SPEC.md requirements.

**Implementation:** Added bare repo detection and flag-based gating to submit, sync, and get commands. Submit includes ancestry alignment checks with automatic base metadata normalization. Get supports no-checkout mode with full tracking metadata creation.

**SPEC.md requirements (Section 4.6.7):**

**submit in bare repos:**
- MUST refuse unless `--no-restack` is provided
- Even with `--no-restack`, MUST refuse if submit set is not aligned
- Alignment is ancestry-based: `p.tip` must be ancestor of `b.tip`
- If ancestry holds but `b.base != p.tip`: normalize base to `p.tip` (metadata-only)

**sync in bare repos:**
- MUST refuse unless `--no-restack` is provided
- With `--no-restack`: may fetch, trunk FF, PR checks, branch deletion prompts

**get in bare repos:**
- MUST refuse unless `--no-checkout` is provided
- With `--no-checkout`: fetch, track branch with parent inference, compute base, default frozen

**Key files implemented:**

- `src/cli/args.rs` - Added `--no-checkout` flag to get command
- `src/cli/commands/get.rs` - Added bare repo check and no-checkout mode with full tracking
- `src/cli/commands/submit.rs` - Added bare repo check and ancestry alignment with metadata normalization
- `src/cli/commands/sync.rs` - Added bare repo check for restack mode

**Acceptance gates (per SPEC.md Section 4.6.7):**

- [x] `lattice submit` refuses in bare repo without `--no-restack`
- [x] `lattice submit --no-restack` checks ancestry alignment
- [x] `lattice submit --no-restack` normalizes base metadata if ancestry holds
- [x] `lattice submit --no-restack` refuses if ancestry violated with message
- [x] `lattice sync` refuses in bare repo without `--no-restack`
- [x] `lattice sync --no-restack` performs fetch, trunk FF, PR checks only
- [x] `lattice get` refuses in bare repo without `--no-checkout`
- [x] `lattice get --no-checkout` fetches, tracks, computes base, defaults frozen
- [x] `lattice get --no-checkout` prints worktree creation guidance
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

### Milestone 1.5: Event Ledger Completion

**Status:** Complete

**Priority:** Medium - Required for ARCHITECTURE.md compliance

**Spec reference:** ARCHITECTURE.md Section 3.4 "Event ledger" and Section 7 "Out-of-band divergence detection"

**Goal:** Complete event recording per ARCHITECTURE.md requirements.

**Full detailed plan:** `.agents/v2/milestones/milestone-1-5-event-ledger-completion/PLAN.md`

**Implementation:** Integrated `detect_divergence()` into the scan flow. The scanner now:
1. Computes the repository fingerprint
2. Compares against the last `Committed` event in the ledger
3. Records a `DivergenceObserved` event when fingerprints differ
4. Surfaces divergence info in `RepoHealthReport` for commands to access

**What's implemented:**
- Storage at `refs/lattice/event-log` (constant in `src/engine/ledger.rs`)
- All 6 event types defined with proper schemas
- Fingerprint computation (SHA-256 based, order-independent)
- `IntentRecorded` recording before mutations in executor
- `Committed` recording after successful execution
- `Aborted` recording on operation failure
- `DoctorProposed` recording when fix options are presented
- `DoctorApplied` recording when fix is executed
- `DivergenceObserved` recording when fingerprint changes between operations
- `detect_and_record_divergence()` integrated into scan flow
- `RepoHealthReport.divergence()` accessor for command visibility
- `surface_divergence_if_debug()` helper for debug output

**Key files modified:**

- `src/engine/health.rs` - Added `divergence` field to `RepoHealthReport`
- `src/engine/scan.rs` - Added `detect_and_record_divergence()` call in scan flow
- `src/cli/commands/mod.rs` - Added `surface_divergence_if_debug()` helper

**Acceptance gates (per ARCHITECTURE.md Section 3.4 and 7):**

- [x] `DivergenceObserved` recorded when fingerprint changes between operations
- [x] Divergence info available in `RepoHealthReport`
- [x] `DoctorProposed` recorded when fix options are presented
- [x] `DoctorApplied` recorded when fix is executed
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

### Milestone 1.6: Documentation Alignment

**Status:** Complete

**Priority:** Low - Docs don't match reality

**Goal:** Update documentation to match implemented code.

**Full detailed plan:** `.agents/v2/milestones/milestone-1-6-documentation-alignment/PLAN.md`

**Implementation:** Created `implementation_notes.md` files for all completed milestones (1.1-1.5, OAuth). Updated v1 ROADMAP.md with v2 reference. All documentation now aligns with implemented code.

**Issues resolved:**

1. **Missing implementation_notes.md files** - Created for milestones 1.1, 1.2, 1.3, 1.4, 1.5, and OAuth
2. **v1 ROADMAP.md OAuth reference** - Added v2 continuation section with clear handoff
3. **Documentation consistency** - All completed milestones now have implementation notes per CLAUDE.md requirements

**Acceptance gates:**

- [x] OAuth `implementation_notes.md` exists
- [x] v1 ROADMAP.md correctly references v2 for OAuth
- [x] All PLAN.md acceptance gates reflect actual status
- [x] All completed milestones (1.1-1.5, OAuth, bare repo) have implementation_notes.md
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

# Category 2: Foundation - Ground-laying for Feature Work

---

### Milestone 2.1: TTY Detection Fix

**Status:** Stubbed

**Priority:** Low - Trivial fix

**Spec reference:** SPEC.md Section 6.2 "Interactive rules"

**Goal:** Replace TTY detection stub with proper implementation.

**SPEC.md requirements (Section 6.2):**
- "If stdin is not a TTY, treat as `--no-interactive`."

**Evidence:** `src/cli/args.rs` lines 74-76:
```rust
fn atty_check() -> bool {
    // TODO: Use atty crate or std::io::IsTerminal when stabilized
    true
}
```

**Fix:** Use `std::io::IsTerminal` (stable since Rust 1.70).

**Critical files:**

- `src/cli/args.rs` - Replace stub with `std::io::stdin().is_terminal()`

**Acceptance gates:**

- [ ] `atty_check()` returns `false` when stdin is not a TTY
- [ ] `atty_check()` returns `true` when stdin is a TTY
- [ ] Non-interactive mode triggers correctly per SPEC.md 6.2
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

### Milestone 2.2: Upstack Scope Walking

**Status:** Incomplete

**Priority:** Low

**Goal:** Implement graph walking for scope validation.

**Evidence:** `src/engine/gate.rs` line 410:
```rust
let branches = vec![branch.clone()];
// TODO: Walk graph to find all upstack branches
```

**Critical files:**

- `src/engine/gate.rs` - Implement graph walking

**Acceptance gates:**

- [ ] `valid_scope` with target branch returns all descendants
- [ ] Freeze policy checks consider entire upstack scope
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

# Category 3: High Impact Features

---

### Milestone 3.1: Alias Command

**Status:** Not started

**Priority:** Medium - Missing SPEC feature

**Spec reference:** SPEC.md Section 8A.4 "lattice alias"

**Goal:** Implement the alias command per SPEC.md.

**SPEC.md requirements (Section 8A.4):**

Synopsis:
- `lattice alias list`
- `lattice alias add <name> <expansion...>`
- `lattice alias remove <name>`
- `lattice alias reset`
- `lattice alias import-legacy` (optional)

Behavior:
- "Aliases expand before Clap parsing or via Clap subcommand wrapper."
- "Must prevent alias shadowing a real command unless `--force`."

**Note:** SPEC.md uses `add`/`remove`, not `set`/`unset`. Must follow spec.

**Key deliverables:**

- `lattice alias list` - List all aliases
- `lattice alias add <name> <expansion...>` - Create alias
- `lattice alias remove <name>` - Remove alias
- `lattice alias reset` - Clear all aliases
- Store aliases in global config
- Expand aliases before Clap parsing
- Prevent shadowing unless `--force`

**Critical files:**

- `src/cli/args.rs` - Add `Alias` command variant, add alias expansion
- `src/cli/commands/mod.rs` - Add dispatch
- `src/cli/commands/alias.rs` - NEW: Implement handlers
- `src/core/config/schema.rs` - Add `[aliases]` table

**Acceptance gates (per SPEC.md Section 8A.4 Tests):**

- [ ] `lattice alias add ss "submit --stack"` creates alias
- [ ] `lattice ss` expands and executes correctly
- [ ] `lattice alias list` shows all aliases
- [ ] `lattice alias remove ss` removes alias
- [ ] `lattice alias reset` clears alias map
- [ ] Cannot shadow real command without `--force`
- [ ] Aliases expand before Clap parsing
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

# Category 4: Additional Features

---

### Milestone 4.1: Split By-Hunk Mode

**Status:** Not started (explicitly deferred from v1)

**Priority:** Low

**Spec reference:** SPEC.md Section 8D.6 "lattice split"

**Goal:** Implement `--by-hunk` mode.

**SPEC.md requirements (Section 8D.6):**
- "required minimum v1" lists `--by-commit` and `--by-file`
- `--by-hunk` is implicitly v2

**Evidence:** `src/cli/commands/split.rs` line 7:
```rust
//! - `--by-hunk`: deferred to v2 (returns "not implemented")
```

**Key deliverables:**

- Interactive hunk selection UI (similar to `git add -p`)
- Maintain sum-of-diffs invariant

**Acceptance gates:**

- [ ] `lattice split --by-hunk` opens interactive hunk selector
- [ ] Combined diff equals original diff (invariant)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

# Category 5: Bootstrap from Existing Work (Proposed)

**Status:** Provisional - Pending architect review

**Proposal source:** Doctor-driven bootstrap feature proposal (January 2026)

This category implements "initialize from history" as a Doctor-driven bootstrap feature, enabling Lattice to become useful immediately on repositories with existing branches and/or open PRs.

**Governing principles:**
- Never act on an invalid model
- Never silently guess repairs
- Mutate state only via the executor with CAS semantics
- Remote-first when authoritative, local inference only when explicitly selected
- `log` becomes useful even before full initialization

---

### Milestone 5.1: Degraded Log Mode

**Status:** Not started

**Priority:** High - Improves immediate usability

**Spec reference:** Proposed SPEC.md Section 8G.1 amendment

**Goal:** Make `lt log` useful before metadata exists, with clear degraded mode indication.

**Deliverables:**
1. Modify `log_cmd.rs` to detect degraded conditions (no metadata refs exist)
2. Add degraded mode banner: "Degraded view - metadata incomplete. Run `lattice doctor` to bootstrap."
3. Show local branches grouped (tracked vs untracked)
4. Show trunk if configured, "trunk: unknown" otherwise
5. Never write metadata or attempt repair in degraded mode

**Critical files:**
- `src/cli/commands/log_cmd.rs` - Add degraded mode detection and rendering

**Acceptance criteria:**
- [ ] `lt log` in fresh repo (post-init, no tracks) shows degraded banner
- [ ] `lt log` with partial metadata shows mixed view (tracked normal, untracked grouped)
- [ ] Banner message includes actionable doctor suggestion
- [ ] No metadata writes occur in degraded mode
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Unit test: log rendering with empty metadata snapshot
- Integration test: fresh repo after `git init` + `lt init`
- Integration test: repo with some tracked, some untracked branches

**Dependencies:** None

**Estimated complexity:** Low

---

### Milestone 5.2: Forge `list_open_prs` Capability

**Status:** Not started

**Priority:** High - Required for remote-first bootstrap

**Spec reference:** Proposed SPEC.md Section 8H.1 "Remote evidence collection"

**Goal:** Enable bulk PR queries for bootstrap evidence collection.

**Design decision (resolved):** Add bulk enumeration to avoid N+1 queries per branch.

**Deliverables:**
1. Add `ListPullsOpts` struct with pagination/limit options:
   - `max_results: Option<usize>` (default 200)
2. Add `PullRequestSummary` struct (lightweight):
   - `number`, `head_ref`, `head_repo_owner` (for forks), `base_ref`, `is_draft`, `url`, `updated_at`
3. Add `list_open_prs(opts: ListPullsOpts) -> Result<Vec<PullRequestSummary>, ForgeError>` to `Forge` trait
4. Implement for GitHub using REST API: `GET /repos/{owner}/{repo}/pulls?state=open`
5. Handle pagination internally (GitHub returns max 100 per page)
6. Add to MockForge for testing
7. Report truncation as Info when budget exceeded

**Critical files:**
- `src/forge/traits.rs` - Add `list_open_prs`, `ListPullsOpts`, `PullRequestSummary`
- `src/forge/github.rs` - Implement with pagination
- `src/forge/mock.rs` - Add mock implementation

**Acceptance criteria:**
- [ ] Can retrieve open PRs up to configured limit (default 200)
- [ ] Pagination handled transparently (follows `Link` header)
- [ ] Rate limit errors surfaced as `ForgeError::RateLimited`
- [ ] Truncation clearly indicated when limit exceeded
- [ ] MockForge supports test scenarios with configurable PR lists
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Unit test: MockForge list behavior
- Unit test: pagination simulation (mock multiple pages)
- Unit test: truncation behavior when exceeding limit
- Integration test: real GitHub API (needs test repo, optional)

**Dependencies:** None

**Estimated complexity:** Medium

---

### Milestone 5.3: Bootstrap Issue Detection (Scanner Extension)

**Status:** Not started

**Priority:** High - Core bootstrap diagnosis

**Spec reference:** Proposed SPEC.md Section 8H.1 "Bootstrap issue family"

**Goal:** Detect bootstrap-related conditions and surface as Doctor issues.

**Deliverables:**
1. Add new issue types to `KnownIssue` enum in `src/doctor/issues.rs`:
   - `RemoteOpenPullRequestsDetected` (Info) - forge reports ≥1 open PR
   - `RemoteOpenPrBranchMissingLocally` (Warning) - open PR head_ref not local
   - `RemoteOpenPrBranchUntracked` (Warning) - local branch exists but no metadata
   - `RemoteOpenPrNotLinkedInMetadata` (Info) - tracked but PR linkage missing
2. Extend scanner to optionally query forge when capabilities allow:
   - Requires: `TrunkKnown`, `RemoteResolved`, `AuthAvailable`, `RepoAuthorized`
3. Store remote evidence in `RepoSnapshot` (new field: `remote_prs: Vec<PullRequest>`)
4. Issues include PR numbers and branch names as evidence
5. Scanner gracefully handles missing auth (skip remote issues)
6. Scanner gracefully handles API failures (log warning, continue)

**Critical files:**
- `src/doctor/issues.rs` - Add new `KnownIssue` variants
- `src/engine/scan.rs` - Add optional forge query and evidence storage
- `src/engine/health.rs` - Handle new issue types

**Acceptance criteria:**
- [ ] Doctor detects open PRs when no local branch exists for head
- [ ] Doctor detects local branches matching open PR heads but untracked
- [ ] Doctor detects tracked branches with missing PR linkage
- [ ] Issues have correct severity levels (Info/Warning per spec)
- [ ] Scanner works offline (no remote issues, no errors)
- [ ] Scanner survives API failures gracefully
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Unit test: issue generation with mock forge returning various PR states
- Unit test: scanner fallback when forge unavailable (auth missing)
- Unit test: scanner fallback when API call fails
- Integration test: real repo with open PRs (optional)

**Dependencies:** Milestone 5.2

**Estimated complexity:** Medium

---

### Milestone 5.4: Bootstrap Fix Generators (Remote-First)

**Status:** Not started

**Priority:** High - Core bootstrap functionality

**Spec reference:** Proposed SPEC.md Section 8H.1 "Bootstrap fix options"

**Goal:** Generate fix options for remote-first bootstrap.

**Design decision (resolved):** Use merge-base for base computation. This aligns with the `track` command update (see prerequisite).

**Prerequisite:** Update `track` command base computation first:
- File: `src/cli/commands/track.rs` lines 113-117
- Change to: `base = merge-base(branch_tip, parent_tip)`
- Add refusal logic when merge-base is None
- Rules:
  - Initial tracking: compute merge-base, refuse if None
  - Retargeting parent: recompute merge-base
  - No-op re-run: MUST NOT rewrite base

**Deliverables:**
1. Add fix generators in `src/doctor/generators.rs`:
   - `generate_track_existing_from_pr_fixes()` - TrackExistingBranchesFromOpenPRs
   - `generate_fetch_and_track_pr_fixes()` - FetchAndTrackOpenPRBranchesFrozen
   - `generate_link_pr_fixes()` - LinkPRsToTrackedBranchesCachedOnly
2. **Use merge-base for base computation** (consistent with updated `track`):
   - `base = merge-base(branch_tip, parent_tip)`
   - Refuse if merge-base is None
3. Add Git interface method: `fetch_ref(remote: &str, refspec: &str) -> Result<Oid, GitError>`
4. Parent selection from PR base relationship:
   - If base_ref is trunk: parent = trunk
   - Else if base_ref exists locally or is an open PR head: parent = base_ref
   - Else parent = trunk (safe fallback)
5. Default freeze states:
   - TrackExisting: Unfrozen
   - FetchAndTrack: Frozen (reason: `teammate_branch`)
6. LinkPR updates ONLY cached PR state, never structural fields

**Critical files:**
- `src/cli/commands/track.rs` - Update base computation (prerequisite)
- `src/doctor/generators.rs` - Add new fix generators
- `src/git/interface.rs` - Add `fetch_ref` method
- `src/doctor/fixes.rs` - Ensure fix previews support new change types

**Acceptance criteria:**
- [ ] `track` command uses merge-base for initial tracking
- [ ] `track` refuses when merge-base is None with actionable error
- [ ] TrackExisting fix creates valid metadata with merge-base
- [ ] FetchAndTrack fix fetches remote ref and creates frozen branch
- [ ] LinkPR fix updates only cached PR state, not structural fields
- [ ] Previews accurately show all ref and metadata changes
- [ ] Fixes are composable (can select and apply multiple)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Unit test: `track` with parent advanced past divergence (must use merge-base)
- Unit test: fix generation logic for each fix type
- Unit test: merge-base computation and refusal on None
- Unit test: parent selection from PR base relationship
- Integration test: apply fixes to test repo with mock forge
- Property test: fixes never create invalid graph (cycle detection)

**Dependencies:** Milestone 5.3, Milestone 5.2

**Estimated complexity:** High

---

### Milestone 5.5: Bootstrap Fix Execution

**Status:** Complete

**Priority:** High - Completes bootstrap workflow

**Spec reference:** Proposed ARCHITECTURE.md Section 8.X "Bootstrap repairs"

**Goal:** Execute bootstrap fixes through the standard Doctor/Executor path.

**Deliverables:**
1. ✓ Extend `src/doctor/planner.rs` to handle bootstrap fix previews
2. ✓ Handle branch ref creation for FetchAndTrack via `PlanStep::RunGit`
3. ✓ Verify post-conditions after execution (graph still valid via `fast_verify`)
4. ✓ Record events in ledger (`DoctorProposed`, `DoctorApplied` - already in place from Milestone 1.1)

**Critical files:**
- `src/doctor/planner.rs` - Handle new preview/change types
- `src/engine/exec.rs` - RunGit execution with command result handling
- `src/git/interface.rs` - Added `run_command` method for arbitrary git execution
- `tests/bootstrap_fixes_integration.rs` - 17 integration tests

**Acceptance criteria:**
- [x] Fixes execute via Executor with CAS semantics
- [x] Failures roll back completely (no partial state)
- [x] Undo works for bootstrap fixes (existing undo mechanism)
- [x] Events recorded with bootstrap evidence (PR numbers, branches affected)
- [x] Post-verify confirms graph validity after execution
- [x] `cargo test` passes
- [x] `cargo clippy` passes

**Test strategy:**
- ✓ Integration test: full bootstrap workflow (detect → generate → preview → apply)
- ✓ Integration test: fix generator correctness for TrackExisting, FetchAndTrack, LinkPR
- ✓ Integration test: precondition checking
- ✓ Integration test: parent inference from PR base_ref

**Implementation notes:** See `.agents/v2/milestones/milestone-5-5-bootstrap-fix-execution/implementation_notes.md`

**Dependencies:** Milestone 5.4, Milestone 1.1 (Doctor Fix Execution - already complete)

**Estimated complexity:** Medium

---

### Milestone 5.6: Init Hint for Bootstrap

**Status:** Complete

**Priority:** Low - UX improvement

**Spec reference:** Proposed SPEC.md Section 8A.2 amendment "Post-init hint"

**Goal:** Show helpful hint after `lt init` when bootstrap opportunities exist.

**Implementation:** Added `show_bootstrap_hint_sync()` and `try_show_bootstrap_hint()` functions to `init.rs`. After successful init (not reset, not quiet), performs a lightweight check for open PRs using `list_open_prs` with limit of 10. If PRs are found and auth is available, prints hint message. All errors are silently swallowed - the hint never blocks init.

**Deliverables:**
1. After init succeeds, perform lightweight forge check (if auth available)
2. If open PRs exist, print hint: "Found N open PRs on remote. Run `lattice doctor` to import them."
3. Hint is non-fatal, does not block init success
4. No metadata mutations during hint check

**Critical files:**
- `src/cli/commands/init.rs` - Added post-init hint logic
- `tests/init_hint_integration.rs` - Integration tests

**Acceptance criteria:**
- [x] Hint shown when open PRs detected and auth available
- [x] No hint when offline or no auth (silent success)
- [x] No metadata mutations during hint check
- [x] Init succeeds regardless of hint check result
- [x] `--quiet` mode suppresses hint
- [x] `--reset` mode skips hint
- [x] `cargo test` passes
- [x] `cargo clippy` passes

**Test strategy:**
- Integration test: init with open PRs shows hint (requires mock/real auth)
- Integration test: init offline succeeds silently (no hint, no error)
- Integration test: init with non-GitHub remote succeeds
- Integration test: init without origin remote succeeds
- Integration test: quiet mode skips hint
- Integration test: reset mode skips hint

**Implementation notes:** See `.agents/v2/milestones/milestone-5-6-init-hint/implementation_notes.md`

**Dependencies:** Milestone 5.2

**Estimated complexity:** Low

---

### Milestone 5.7: Local-Only Bootstrap (ImportLocalInProgressTopology)

**Status:** Not started

**Priority:** Medium - Enables offline bootstrap

**Spec reference:** Proposed SPEC.md Section 8H.1 "Fix Option: ImportLocalInProgressTopology"

**Goal:** Bootstrap from local branches when remote is unavailable.

**Deliverables:**
1. Extend `UntrackedBranch` issue with ancestry evidence (merge-base distances to tracked branches)
2. Add `ImportLocalInProgressTopology` fix generator
3. Deterministic parent selection using merge-base distances (nearest tracked ancestor)
4. Interactive disambiguation when multiple valid parents at equal distance
5. Non-interactive mode refuses on ambiguity (no auto-selection)

**Critical files:**
- `src/doctor/issues.rs` - Extend `UntrackedBranch` evidence
- `src/doctor/generators.rs` - Add `generate_import_local_topology_fixes()`

**Acceptance criteria:**
- [ ] Local branches can be tracked without forge access
- [ ] Parent selection prefers nearest tracked ancestor (by merge-base distance)
- [ ] Ambiguous cases (equal distance to multiple ancestors) require interaction
- [ ] Non-interactive mode fails fast on ambiguity with clear message
- [ ] Base computed via merge-base (consistent with remote-first)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Unit test: parent ranking algorithm (distance-based)
- Integration test: import workflow with clear ancestry (single best parent)
- Integration test: ambiguous ancestry handling (interactive required)
- Integration test: non-interactive refusal on ambiguity

**Dependencies:** Milestone 5.5

**Estimated complexity:** High

---

### Milestone 5.8: Synthetic Stack Detection (Two-Tiered)

**Status:** Not started

**Priority:** Low - Contextual information

**Spec reference:** Proposed SPEC.md Section 8H.1 "Issue: SyntheticRemoteStackDetected"

**Goal:** Detect synthetic remote stack patterns and surface as informational issue.

**Definition:** A synthetic remote stack exists when:
- An open PR P0 targets trunk with head branch H
- Closed PRs exist whose base branch is H

**Interpretation:** Prior reviewed work was merged into H while P0 remains open. Useful context, not automatically reconstructable.

**Design decision (resolved):** Two-tiered approach to balance cost vs. information.

**Tier 1 (Default - Cheap):**
- Fetch open PRs only via `list_open_prs`
- Identify trunk-bound PRs as "potential synthetic stack heads"
- Emit Info issue: "Potential synthetic stack head detected: trunk PR head `H`"
- Do NOT enumerate closed PRs automatically

**Tier 2 (Explicit - Deep Remote):**
- Flag: `lt doctor --deep-remote`
- Config: `doctor.bootstrap.deep_remote = true|false` (default false)
- Budgets (configurable):
  - `max_synthetic_heads`: 3 (default)
  - `max_closed_prs_per_head`: 20 (default)
- Truncation explicitly reported when budgets exceeded
- Closed PR enumeration happens during fix plan construction, not baseline scan

**Deliverables:**
1. Add `SyntheticRemoteStackDetected` issue type (Info severity)
2. Add `--deep-remote` flag to `lt doctor`
3. Add config: `doctor.bootstrap.deep_remote`, `doctor.bootstrap.max_synthetic_heads`, `doctor.bootstrap.max_closed_prs_per_head`
4. Tier 1: Detect trunk-bound open PRs as potential synthetic heads
5. Tier 2: Query closed PRs only when `--deep-remote` enabled and within budgets

**Critical files:**
- `src/doctor/issues.rs` - Add `SyntheticRemoteStackDetected` variant
- `src/cli/args.rs` - Add `--deep-remote` flag to doctor command
- `src/core/config/schema.rs` - Add bootstrap config options
- `src/engine/scan.rs` - Add Tier 1 detection logic
- `src/doctor/generators.rs` - Add Tier 2 closed PR enumeration in fix planning

**Acceptance criteria:**
- [ ] Tier 1: Trunk-bound open PRs flagged as potential synthetic heads
- [ ] Tier 2: `--deep-remote` enables closed PR enumeration
- [ ] Budgets enforced with explicit truncation reporting
- [ ] Config options respected
- [ ] API failures do not block other diagnosis
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Unit test: Tier 1 detection logic with mock data
- Unit test: Tier 2 budget enforcement and truncation
- Unit test: config parsing for bootstrap options
- Integration test: real repo with synthetic stack pattern (optional)

**Dependencies:** Milestone 5.2

**Estimated complexity:** Medium

---

### Milestone 5.9: Synthetic Stack Snapshot Materialization (Opt-in)

**Status:** Not started

**Priority:** Low - Advanced feature

**Spec reference:** Proposed SPEC.md Section 8H.1 "Fix Option: MaterializeSyntheticRemoteStackSnapshots"

**Goal:** Create frozen snapshot branches for synthetic stack context (opt-in, strict).

**Strict safety rules:**
- If any requested snapshot cannot be fetched or validated, the entire fix MUST fail and rollback
- No partial application

**Design decision (resolved):** Use normal branches under `refs/heads/` with reserved prefix.

**Naming scheme:**
- Branch name: `lattice/snap/pr-<number>`
- Stored as: `refs/heads/lattice/snap/pr-123`
- Collision avoidance: append `-<k>` suffix if name exists

**Rationale for `refs/heads/`:**
- Current model assumes tracked branches = `refs/heads/*`
- Custom namespace would expand architectural scope
- Normal branches work with existing checkout/navigation/scanner
- Users can inspect with normal git tools

**Deliverables:**
1. Add `MaterializeSyntheticRemoteStackSnapshots` fix generator
2. Fetch PR head refs (GitHub-specific: `refs/pull/{number}/head`)
3. Validate fetched commit is reachable from current head H
4. Create snapshot branches with collision-safe naming
5. Write metadata:
   - parent = H (the open PR head branch)
   - base = merge-base(snapshot_tip, H_tip)
   - freeze = Frozen (reason: `remote_synthetic_snapshot`)
   - pr = Linked (closed/merged)
6. All-or-nothing execution (rollback on any failure)

**Critical files:**
- `src/doctor/generators.rs` - Add snapshot materialization fix
- `src/core/metadata/schema.rs` - Add `remote_synthetic_snapshot` freeze reason
- `src/git/interface.rs` - Add PR ref fetch capability

**Acceptance criteria:**
- [ ] Snapshots created as `refs/heads/lattice/snap/pr-<n>` branches
- [ ] Collision avoidance works (appends suffix if needed)
- [ ] Invalid snapshots (commit not reachable from H) rejected
- [ ] Partial failures roll back entirely (no orphan branches)
- [ ] Snapshot branches frozen with reason `remote_synthetic_snapshot`
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

**Test strategy:**
- Integration test: successful snapshot creation workflow
- Integration test: collision avoidance when branch name exists
- Integration test: rollback when one PR ref not reachable
- Integration test: rollback when fetch fails

**Dependencies:** Milestone 5.8, Milestone 5.5

**Estimated complexity:** High

---

### Milestone 5.10: Submit Scope Exclusion for Snapshots

**Status:** Complete

**Priority:** Low - Required for 5.9 safety

**Spec reference:** Proposed SPEC.md Section 8E.2 amendment "Excluding snapshot branches"

**Goal:** Ensure synthetic snapshot branches never appear in submit sets.

**Implementation:** Added four helper functions (`is_synthetic_snapshot`, `filter_snapshot_branches`, `report_excluded_snapshots`, `check_current_branch_not_snapshot`) to submit.rs that filter out branches with freeze reason `remote_synthetic_snapshot`. Integrated filtering into `submit_async` after scope computation but before any operations. Added user feedback when branches are excluded and clear error when attempting to submit from a snapshot branch.

**Critical files:**
- `src/cli/commands/submit.rs` - Added exclusion logic and 8 unit tests
- `src/cli/args.rs` - Updated help text

**Acceptance criteria:**
- [x] Snapshot branches excluded from `lt submit` default scope
- [x] Snapshot branches excluded from `lt submit --stack` scope
- [x] Help text documents exclusion behavior
- [x] Clear error when attempting to submit from a snapshot branch
- [x] Clear message when branches are excluded from scope
- [x] Normal branches and other frozen branches not affected
- [x] `cargo test` passes
- [x] `cargo clippy` passes

**Test coverage:**
- 8 unit tests in `submit::tests::snapshot_exclusion` module
- Tests cover: snapshot detection, normal branch pass-through, other freeze reasons, untracked branches, filtering logic, order preservation, current branch validation

**Dependencies:** Milestone 5.9

**Estimated complexity:** Low

---

## Execution Order Summary

| Order | Milestone | Category | Spec Reference | Effort |
|-------|-----------|----------|----------------|--------|
| 1 | 1.1 Doctor Fix Execution | Compliance | ARCH 8 | Medium |
| 2 | 1.2 Sync Restack | Compliance | SPEC 8E.3 | Low |
| 3 | 1.3 OAuth RepoAuthorized | Compliance | SPEC 8E.0.1 | Medium |
| 4 | 1.4 Bare Repo Flags | Compliance | SPEC 4.6.7 | Medium |
| 5 | 1.5 Event Ledger Completion | Compliance | ARCH 3.4, 7 | Medium |
| 6 | 1.6 Documentation | Compliance | - | Low |
| 7 | 2.1 TTY Detection | Foundation | SPEC 6.2 | Trivial |
| 8 | 2.2 Upstack Scope | Foundation | - | Low |
| 9 | 3.1 Alias Command | High Impact | SPEC 8A.4 | Medium |
| 10 | 4.1 Split By-Hunk | Feature | SPEC 8D.6 | High |
| 11 | 5.1 Degraded Log Mode | Bootstrap | Proposed 8G.1 | Low |
| 12 | 5.2 Forge list_open_prs | Bootstrap | Proposed 8H.1 | Medium |
| 13 | 5.3 Bootstrap Issue Detection | Bootstrap | Proposed 8H.1 | Medium |
| 14 | 5.4 Bootstrap Fix Generators | Bootstrap | Proposed 8H.1 | High |
| 15 | 5.5 Bootstrap Fix Execution | Bootstrap | Proposed 8.X | Medium |
| 16 | 5.6 Init Hint | Bootstrap | Proposed 8A.2 | Low |
| 17 | 5.7 Local-Only Bootstrap | Bootstrap | Proposed 8H.1 | High |
| 18 | 5.8 Synthetic Stack Detection | Bootstrap | Proposed 8H.1 | Medium |
| 19 | 5.9 Snapshot Materialization | Bootstrap | Proposed 8H.1 | High |
| 20 | 5.10 Submit Scope Exclusion | Bootstrap | Proposed 8E.2 | Low |

---

## Verified Complete (No Roadmap Entry Needed)

The following SPEC.md commands are **fully implemented**:

| Command | Status | Location |
|---------|--------|----------|
| `lattice completion` | ✓ Complete | `src/cli/commands/completion.rs` |
| `lattice changelog` | ✓ Complete | `src/cli/commands/changelog.rs` |

---

## Conventions

- Each milestone references specific SPEC.md/ARCHITECTURE.md sections
- Acceptance gates are derived from spec requirements and test sections
- All changes must pass `cargo test`, `cargo clippy`, and type checks
- Implementation notes recorded in `implementation_notes.md` after completion
