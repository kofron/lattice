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
