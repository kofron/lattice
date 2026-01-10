# Lattice Architecture Reference

## Correctness-by-design under out-of-band Git changes, with explicit repair

This document is the authoritative architectural reference for the Lattice codebase. It defines the system’s invariants, module boundaries, execution model, and the only acceptable paths for mutating repository state. It also defines the repair (“doctor”) framework and the event ledger that enables divergence detection and guided self-healing in the presence of out-of-band changes (direct `git` CLI use, GitHub UI actions, and other tools).

The guiding promise is:

> Lattice never acts on an invalid model, never silently guesses repairs, and never claims success unless the repository and its metadata satisfy the invariants required for the invoked command.

---

## 1. Architectural goals and non-negotiable constraints

### 1.1 Primary goal: correctness-by-design

Correctness-by-design means:

1. **Validity gating:** A non-doctor command executes only when the repository can be represented as a **validated execution model** for that command.
2. **Single write path:** All mutations to Git refs and Lattice metadata occur through a single transactional executor.
3. **Explicit repair:** When state is not valid for a command, Lattice produces a set of **repair choices** and requires explicit user confirmation of the selected repair plan.
4. **Crash and interruption safety:** If Lattice is interrupted, the repository is left either:

   * unchanged, or
   * in a Git-native in-progress state (rebase/merge/cherry-pick), accompanied by a Lattice operation state marker that makes `continue` and `abort` unambiguous.

### 1.2 Out-of-band changes are normal, not exceptional

Git repositories are routinely modified outside Lattice. This architecture treats out-of-band changes as a first-class reality:

* Lattice detects divergence from its last known post-operation snapshot.
* Lattice records divergence as an event in its ledger.
* Lattice refuses to proceed when divergence prevents building the required validated model for the invoked command.
* Lattice never attempts speculative repair without confirmation.

### 1.3 Determinism of decisions

All decisions that affect repository state are deterministic and reproducible from:

* repository snapshots
* configuration
* metadata refs
* event ledger evidence

Non-deterministic systems (including LLMs) are permitted only in the **explanation layer** and are prohibited from influencing plans, repairs, or gating outcomes.

---

## 2. Glossary

* **Tracked branch:** A local branch that has a corresponding metadata ref under `refs/branch-metadata/<branch>`.
* **Parent pointer:** The `parent` field in a branch’s metadata, naming another branch (or a configured trunk) as its downstack parent.
* **Stack graph:** The directed graph induced by parent pointers. Edges point from child to parent.
* **Trunk:** The configured base branch (for example `main`). Trunk configuration is per-repository.
* **Validated execution model:** A command-specific representation of repo state that has passed all invariants required for that command.
* **Repair plan:** A concrete, previewable sequence of repository mutations that resolves one or more blocking issues, requiring user confirmation.
* **Out-of-band divergence:** Any repository change that occurs outside the transactional executor of Lattice.
* **CAS ref update:** Compare-and-swap update of a ref, updating only if the ref currently equals an expected old value.

---

## 3. Repository-resident data model

Lattice stores all persistent, repository-scoped state inside the repository. The storage format is designed to be:

* inspectable with standard Git tooling
* robust under concurrent access
* verifiable for correctness

### 3.1 Repository configuration

Repository configuration is stored under:

* `.git/lattice/config.toml`

This config MUST contain:

* `trunk.branch`: the trunk branch name

Config writes MUST be atomic (write to a temporary file and rename).

### 3.2 Branch metadata refs

Branch metadata is stored as Git refs:

* `refs/branch-metadata/<branch>`

Each ref points to a Git blob object containing a JSON document.

#### 3.2.1 Structural vs cached metadata

Branch metadata is divided into:

**Structural fields (correctness-critical):**

* `version`
* `parent`
* `frozen`

**Cached fields (non-blocking, may be missing or stale):**

* PR linkage information (host, repo, number, URL)
* PR status cache (if present)

The structural fields define the stack graph. Cached fields MUST NOT be used to justify structural changes.

#### 3.2.2 Metadata schema requirements

Metadata parsing is strict:

* unknown fields are rejected
* schema version is required
* invalid values are rejected

This ensures Lattice never “accidentally accepts” malformed metadata and then acts on it.

Illustrative interface (architectural shape):

```rust
/// Structural metadata only. Cached fields are defined separately.
pub struct BranchMetadataV1 {
    pub version: u32,          // always 1 for this schema
    pub parent: BranchName,    // validated refname-compatible branch name
    pub frozen: bool,
}
```

#### 3.2.3 Metadata ref updates

Metadata refs MUST be updated with compare-and-swap semantics. A metadata update is applied only if the current ref value matches the expected old value observed during planning.

This prevents applying a plan to a changed reality.

### 3.3 Lattice operation state marker

Lattice maintains an operation state marker to make in-progress operations explicit:

* `.git/lattice/op-state.json`

This file exists only when Lattice is executing a multi-step operation or when that operation is waiting for user conflict resolution.

The op-state marker MUST include:

* operation id
* command identity
* touched refs and their expected old values
* plan digest
* phase (`executing` or `awaiting_user`)

While the op-state marker exists, Lattice MUST:

* allow only `continue`, `abort`, and read-only commands that do not assume structural validity
* refuse all other mutating commands

### 3.4 Event ledger

Lattice maintains an append-only event ledger stored in Git:

* `refs/lattice/event-log`

The event log is an internal commit chain. Each commit contains one event record and points to the previous event commit. This yields a total order without relying on ref listing order.

#### 3.4.1 Purpose of the event ledger

The event ledger is evidence, not authority. It provides:

* divergence detection and reporting
* an audit trail of Lattice-intended structural changes
* recovery hints when metadata is missing or corrupted
* a record of doctor proposals and applied repairs

The ledger MUST NOT be replayed blindly to overwrite repository state.

#### 3.4.2 Event categories

The ledger contains the following event categories:

* `IntentRecorded`
* `Committed`
* `Aborted`
* `DivergenceObserved`
* `DoctorProposed`
* `DoctorApplied`

Each event record is a strict JSON document with a schema version and required fields sufficient to:

* identify the operation
* identify touched refs and their expected old values
* record pre and post fingerprints (see Section 7)

---

## 4. High-level component architecture

Lattice is structured into components with strict responsibilities. Violating these boundaries is an architectural defect.

### 4.1 Component overview

1. **CLI layer**

   * Parses arguments and global flags.
   * Delegates to the command dispatcher.
   * Does not perform repository mutations.

2. **Command dispatcher**

   * Maps a CLI command to an Engine entrypoint.
   * Supplies UI mode (interactive vs non-interactive), verbosity, and verification flags.

3. **Engine**

   * Orchestrates scanning, gating, planning, doctor handoff, execution, verification, and event recording.
   * Owns the only repository mutation pathway via the Executor.

4. **Scanner**

   * Reads repository state, configuration, metadata refs, Git in-progress state.
   * Produces a `RepoHealthReport` (issues, evidence, capabilities, divergence info).

5. **Gating**

   * For a given command, determines whether required capabilities are satisfied.
   * If satisfied, produces a command-specific validated context.
   * If not satisfied, produces a repair bundle.

6. **Planner**

   * Pure, deterministic logic.
   * Converts a validated context into a concrete `Plan`.
   * Does not perform I/O.

7. **Doctor**

   * Generates repair issues and fix options.
   * Each fix option contains a concrete repair plan.
   * Presents choices and requires explicit confirmation.
   * Executes selected plan through the Executor.

8. **Executor**

   * The only component allowed to mutate the repository.
   * Applies plans transactionally with CAS ref updates.
   * Writes and clears op-state marker.
   * Records intent and commit events around execution.

9. **Git interface**

   * A single, centralized interface to run Git commands and parse their output.
   * All repository reads and writes go through this interface.

10. **Host adapter**

    * Implements Git-host-specific behavior (initially GitHub).
    * Provides typed operations: open PR, update PR, fetch PR status, merge PR, etc.
    * Host adapter results are written only to cached metadata.

11. **Explanation system**

    * Produces human-readable explanations for issues and fix options.
    * Can call network services, including LLMs.
    * Cannot influence decisions or plans.

---

## 5. The validated execution model and capability gating

### 5.1 Core idea

A command executes only against a validated representation that is sufficient for that command.

There is no global “repo is valid” boolean. There is a command-specific validation contract.

### 5.2 Capabilities

The Scanner produces capabilities that describe what is known to be true. Capabilities are composable proofs.

Representative capabilities:

* `RepoOpen`
* `TrunkKnown`
* `NoLatticeOpInProgress`
* `NoExternalGitOpInProgress`
* `MetadataReadable`
* `GraphValid`
* `ScopeResolved`
* `FrozenPolicySatisfied`
* `WorkingCopyStateKnown`
* `AuthAvailable(host)` – A valid GitHub App user access token exists for the specified host OR can be refreshed
* `RemoteResolved(owner, repo)` – The git remote can be parsed to identify the owner and repository
* `RepoAuthorized(owner, repo)` – The authenticated user's token has access to the specified repository via an installed GitHub App

A capability either exists or does not. “Partial” capability is represented as absence plus an issue in the health report.

**Capability derivation for GitHub auth:**

* `AuthAvailable(host)`: Check SecretStore for a valid token bundle. If access token is expired but refresh token is valid, the capability is satisfied (refresh will occur on demand).
* `RemoteResolved(owner, repo)`: Parse the git remote URL (typically `origin`) to extract the GitHub owner and repository name.
* `RepoAuthorized(owner, repo)`: Query the GitHub installations API (`GET /user/installations` and `GET /user/installations/{id}/repositories`) to verify the GitHub App is installed and authorized for the repository. Cache the result for a short TTL (e.g., 10 minutes).

### 5.3 Command requirement sets

Every non-doctor command declares its required capabilities.

Examples (architectural intent):

* `lattice log` requires: `RepoOpen`
  It MAY run without `MetadataReadable` by presenting a degraded view and explicitly indicating missing metadata.

* `lattice restack` requires:
  `RepoOpen + TrunkKnown + NoLatticeOpInProgress + NoExternalGitOpInProgress + MetadataReadable + GraphValid + FrozenPolicySatisfied`

* `lattice submit` requires:
  all of `restack` requirements plus `RemoteResolved + AuthAvailable`

If required capabilities are not satisfied, the command MUST NOT mutate the repository. The engine routes to repair.

### 5.4 Gating output

Gating produces exactly one of:

* `ReadyContext<C>`: validated context for command `C`
* `RepairBundle`: blocking issues and fix options relevant to satisfying `C`’s requirement set

This is the architectural mechanism that makes “states that require healing” equivalent to “states that prevent building a valid execution model for the command.”

---

## 6. Plans and the single transactional write path

### 6.1 Plans

A `Plan` is the sole intermediate representation between validated state and repository mutation.

A plan is:

* deterministic
* previewable
* serializable (for op-state recording and testing)
* composed of typed steps with explicit touched refs

Plan steps are partitioned into phases:

1. **Local structural phase**

   * Git history operations that affect branch tips
   * Structural metadata updates (parent pointers, frozen flags)

2. **Local verification phase**

   * Post-apply re-scan and invariant validation

3. **Remote interaction phase**

   * Push/fetch
   * Host API calls
   * Cached metadata updates based on remote results

Remote interactions MUST NOT be required to restore local structural invariants.

### 6.2 The Executor contract

The Executor is the only place where repository mutations occur.

The Executor MUST:

* acquire the Lattice repository lock for the duration of execution
* write op-state marker before the first mutation
* record `IntentRecorded` event before the first mutation
* apply all ref updates with CAS semantics
* if a CAS precondition fails, abort without continuing execution and record `Aborted`
* if a Git conflict pauses execution, transition op-state to `awaiting_user` and stop
* after successful completion, re-scan and validate required invariants
* record `Committed` event and remove op-state marker

### 6.3 Prohibited mutation paths

Architecturally prohibited:

* updating any refs by writing `.git/refs/*` files directly
* writing metadata refs without CAS preconditions
* mutating repository state from the Planner, Doctor issue generation, Explanation layer, or CLI layer

Reviewers MUST treat violations as correctness bugs.

**Scope clarification for SecretStore writes:**

The single transactional write path applies to **repository state** (refs, metadata, config). SecretStore writes (token storage, token refresh) are outside repository invariants but must be guarded by:

* Auth-scoped file locking (one refresh at a time per host)
* Strict redaction policies (tokens never in logs, errors, or outputs)
* Atomic write semantics (temp file + rename)

---

## 7. Out-of-band divergence detection

### 7.1 Fingerprints

The scanner computes a repository fingerprint over a stable set of ref values:

* trunk ref value
* all tracked branch ref values
* all structural metadata ref values
* repository config version

The fingerprint is a stable hash of sorted `(refname, oid)` entries.

### 7.2 DivergenceObserved event

On each command invocation, the engine compares:

* the current fingerprint
* the last recorded `Committed` event fingerprint

If they differ, the engine records a `DivergenceObserved` event including:

* prior fingerprint
* current fingerprint
* a diff summary of changed refs

Divergence itself is not an error. It becomes evidence surfaced in doctor and in gated command failures.

### 7.3 Divergence and gating

Divergence affects gating only insofar as it prevents required capabilities from being established.

Examples:

* If metadata is corrupt, `MetadataReadable` is absent and commands requiring it are gated.
* If a branch tip changed, structural invariants may still hold, and commands may proceed if their requirements are satisfied.
* If ref CAS preconditions fail during execution, the operation aborts and requires a re-scan and re-plan.

---

## 8. Doctor: explicit repair with user confirmation

### 8.1 Doctor is a framework, not a special-case command

Doctor is the unified repair broker used in two contexts:

1. `lattice doctor` invoked explicitly
2. any non-doctor command that fails gating and requires repair

Doctor shares the same:

* scanner
* planner model (repair plans are plans)
* executor
* event recording

There is no separate “repair mutation path.”

### 8.2 Issues and fix options

A `RepoHealthReport` contains issues with:

* `IssueId` (stable and deterministic from evidence)
* severity (`Blocking`, `Warning`, `Info`)
* evidence (refs, object ids, parse failures, cycle traces)
* one or more `FixOption`s

A `FixOption` contains:

* `FixId`
* preconditions (capabilities that must remain true at apply time)
* a plan preview (ref changes and operations)
* a concrete repair plan

Doctor MUST always present fix options as choices. Doctor MUST never apply a fix without explicit confirmation.

**Authentication-related issues:**

Some blocking issues require **user actions** rather than repository mutations. These issues produce fix options that direct the user to perform an external action rather than generating an Executor plan:

* `AuthenticationRequired` (Blocking)
  * Condition: No valid token exists for the required host
  * Fix: User action – run `lattice auth login`
  * Message: "Not authenticated. Run `lattice auth login`."

* `AppNotInstalled` (Blocking)
  * Condition: GitHub App is not installed or not authorized for the repository
  * Fix: User action – install the GitHub App
  * Message: "GitHub App not installed for {owner}/{repo}. Install at: https://github.com/apps/lattice/installations/new"

* `TokenExpired` (Blocking)
  * Condition: Both access token and refresh token have expired
  * Fix: User action – re-authenticate
  * Message: "Authentication expired. Run `lattice auth login` again."

### 8.3 Confirmation model

The confirmation model is consistent across interactive and non-interactive use:

* Interactive mode:

  * doctor presents issues and fix options
  * user selects fix options
  * doctor presents a combined plan preview
  * user confirms “apply” explicitly

* Non-interactive mode:

  * doctor emits issues and fix options with ids
  * doctor applies fixes only when fix ids are provided explicitly
  * doctor never auto-selects fixes

This ensures “we don’t guess” is enforceable in CI and automation.

### 8.4 Repair outcomes

After applying a repair plan, doctor performs a full post-verify and records `DoctorApplied` in the event ledger.

If repair requires manual conflict resolution, doctor transitions to `awaiting_user` op-state and exits with instructions to run `lattice continue` or `lattice abort`.

---

## 9. Explanation system

### 9.1 Purpose and strict boundary

Explanations exist to improve user understanding. They are not used for decision-making.

The explanation system MAY:

* use network services
* use LLMs
* produce rich narrative explanations

The explanation system MUST NOT:

* change a plan
* select fix options
* influence gating outcomes
* modify repository state

### 9.2 Explain interface

Explanations are generated through an asynchronous interface that returns a future. This supports network-backed explainers without blocking architectural layering.

Architectural shape:

```rust
use core::future::Future;
use core::pin::Pin;

pub type ExplainFut<'a> =
    Pin<Box<dyn Future<Output = Result<Explanation, ExplainError>> + Send + 'a>>;

pub trait Explain<T>: Send + Sync {
    fn explain<'a>(&'a self, item: &'a T, ctx: &'a ExplainCtx) -> ExplainFut<'a>;
}
```

### 9.3 Redaction and privacy

Explanation context includes an explicit redaction policy. By default:

* secrets are never included (tokens, credentials)
* file contents and diffs are excluded unless explicitly enabled
* branch names and commit ids are allowed
* remote URLs are allowed only when necessary for user-facing actions

This prevents accidental leakage when LLM-backed explainers are enabled.

### 9.4 Deterministic fallback

A deterministic, offline explainer is always available and is the baseline for correctness and testability. Network-backed explainers may augment or restyle output but do not replace the baseline explanation content.

---

## 10. Git interface and hooks policy

### 10.1 Single Git interface

All Git interactions are performed through a single Git interface component that:

* invokes the system Git executable
* provides structured results
* normalizes errors into typed failure categories

Direct parsing of `.git` internal files outside this interface is prohibited.

### 10.2 Hooks and verification

Git hooks are honored by default. When `--no-verify` is set, the Git interface invokes Git commands in a way that disables hook execution for operations that support it.

The executor is responsible for carrying the verification policy into plan execution.

---

## 11. Host adapter architecture

### 11.1 Adapter boundary

Host adapters implement operations required by commands such as `submit`, `sync`, `get`, `merge`, and `pr`.

Adapters:

* are invoked only after local structural invariants are satisfied
* may fail without compromising local correctness
* write results only to cached metadata fields

### 11.2 Cached metadata handling

PR linkage and status are cached fields:

* absence is not a structural error
* staleness is not a structural error
* commands that require PR linkage (like `pr`) gate on availability of cached linkage or on the ability to resolve it via adapter query

### 11.3 Authentication Manager

The AuthManager is responsible for providing valid authentication credentials to host adapters.

**Responsibilities:**

* Load token bundle from SecretStore
* Refresh tokens when expired (with auth-scoped locking)
* Redact secrets in all logs, errors, and outputs
* Never participate in repository mutation plans

**Interface shape:**

```rust
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    /// Returns a valid bearer token, refreshing if necessary.
    /// Acquires auth lock during refresh to prevent race conditions.
    async fn bearer_token(&self) -> Result<String, AuthError>;
    
    /// Check if authentication is available without refreshing.
    fn is_authenticated(&self) -> bool;
}
```

**Locking contract:**

* Auth refresh lock at `~/.lattice/auth/lock.<host>`
* Must be acquired before any token refresh operation
* Read operations may proceed without lock but must re-check after acquiring lock if refresh needed

**Error handling:**

* `AuthError::NotAuthenticated` – No token exists for the host
* `AuthError::RefreshFailed` – Refresh token is invalid or expired
* `AuthError::LockContention` – Could not acquire auth lock (retry with backoff)

The AuthManager is invoked by host adapters (e.g., GitHubForge) on each API request. It is responsible for ensuring that the bearer token is valid at the time of the request, refreshing transparently if needed.

---

## 12. Command lifecycle

Every command follows a uniform lifecycle, enforced by the engine.

1. **Scan**

   * compute repo health report
   * detect in-progress ops
   * compute fingerprint and record divergence if needed

2. **Gate**

   * evaluate command requirement set
   * produce `ReadyContext` or `RepairBundle`

3. **Repair (if gated)**

   * present issues and fix options
   * require explicit user selection and confirmation
   * execute repair plan via executor
   * restart lifecycle from Scan

4. **Plan**

   * planner produces command plan from validated context

5. **Execute**

   * executor applies plan with CAS and journaling
   * if conflict, transition to awaiting_user and stop

6. **Verify**

   * post-scan and invariant verification
   * record committed event

7. **Return**

   * produce user-visible output and exit status

This lifecycle is mandatory. Implementations that bypass it are architecturally invalid.

---

## 13. Testing and verification strategy

Correctness is enforced by tests that match the architecture’s contracts.

### 13.1 Invariant tests

Unit tests MUST validate:

* metadata schema strictness
* graph validity checks (cycle detection, parent existence)
* capability derivation correctness
* plan determinism for fixed snapshots

### 13.2 Executor tests

Executor tests MUST validate:

* CAS precondition enforcement
* rollback behavior for early failures
* op-state marker creation and clearing
* correct behavior under simulated interruptions
* refusal of mutations when op-state indicates in-progress

### 13.3 Out-of-band fuzz testing

An automated harness MUST:

* interleave lattice operations with direct Git operations (renames, deletes, rebases, resets, metadata ref edits)
* assert that:

  * gating never constructs a validated context when requirements are not met
  * doctor produces repair choices rather than guessing
  * executor never applies a plan when CAS preconditions fail
  * post-success invariants always hold

### 13.4 CLI integration tests

Integration tests MUST verify:

* command lifecycle adherence
* correct gating and doctor handoff behavior
* stable non-interactive behavior (no hidden prompts)
* stable, parseable output modes for automation (for example JSON reports for doctor)

---

## 14. Extension and review rules

### 14.1 Adding a new command

A new command MUST:

* define its requirement set
* define the validated context it consumes
* implement planning in the planner (pure, deterministic)
* execute only via executor plans
* define which issues gate it and what repair options satisfy it

### 14.2 Adding a new repair

A new repair MUST:

* be represented as one or more fix options
* include explicit evidence and a previewable plan
* require user confirmation
* be executed via the executor
* record doctor proposal and application events

### 14.3 Adding new metadata or event fields

Schema evolution MUST:

* bump schema version
* provide an explicit migration strategy
* maintain strict parsing (unknown fields rejected for each version)

No silent “best effort” parsing is permitted for structural fields.

---

## 15. Summary: the correctness contract made tangible

Lattice’s architecture is built around a simple, disciplined loop:

* **Scan** reality.
* **Refuse** to act without a valid model.
* **Offer** explicit repairs with clear explanations.
* **Apply** changes only through a single transactional executor with CAS ref updates.
* **Verify** invariants before claiming success.
* **Record** evidence in an append-only event ledger.

This is how Lattice stays correct even when the repository is modified out of band: it never pretends, never guesses silently, and never acts without proof.
