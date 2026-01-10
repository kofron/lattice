# Lattice CLI Engineering Specification

*A Rust-native, single-binary clone of the Graphite Git CLI workflow for stacked branches and PRs (no proprietary UI).*

This document is an engineering-ready spec for implementing **Lattice**, a Rust CLI that mirrors Graphite CLI semantics for stacked development: creating, navigating, restacking, submitting, syncing, and merging stacked branches and pull requests.

It incorporates the directives and feedback provided:

* **Correctness and integrity are the prime invariant.**
* **Branch metadata is structured, self-describing, and always includes base commit tracking** (no “optional soup”).
* **Graphite-like UX and defaults** (including empty `create`, submit idempotency rules, force semantics).
* **Explicit `freeze`/`unfreeze` as first-class features** with enforcement.
* **Global config + repo overrides** (sane defaults plus customization).
* **Token storage starts as a file compromise** but must be architected behind a **swappable secret provider**.
* **GitHub-only in v1** with clean scaffolding for future forges.
* **GraphQL support is required** for draft toggling.
* **Extensive tests for every feature**, including pitfall-focused behavioral tests.
* **Extremely good documentation** at `mod.rs` and file level with doctests.

---

## Table of contents

1. Goals and non-goals
2. Prime invariant: Repository and metadata integrity
3. Conceptual model and glossary
4. Storage model

   * Branch metadata refs
   * Operation journal and crash safety
   * Global and repo configuration
   * Secret storage abstraction
5. Architecture
6. CLI contract

   * Global flags and behavior
   * Output formats and exit codes
   * Interactive vs non-interactive rules
7. Stack graph invariants and verification
8. Command reference (complete)

   * Setup and configuration
   * Tracking and structure
   * Navigation
   * Stack mutation
   * Remote and PR integration
   * Conflict recovery and undo
   * Informational commands
9. Testing strategy (mandatory)
10. Documentation standard (mandatory)
11. Implementation phases and acceptance gates
12. Appendices

* Data schemas
* Parity notes and compatibility policy

---

## 1. Goals and non-goals

### 1.1 Goals

* Provide a **single compiled Rust binary** `lattice` that mimics the Graphite CLI workflow and semantics for stacked branches and PRs.
* Use an **adapter architecture** for remote forges (GitHub v1, others later).
* Maintain **branch relationship metadata in Git itself** (no external DB) using refs under `refs/branch-metadata/*`.
* Guarantee the **integrity/correctness** of:

  * the Git repository state (refs, working tree, index),
  * and the Lattice metadata state,
  * **at the beginning and end of every command** (including “paused due to conflicts” end states).
* Provide **extensive automated tests for every feature**, including edge cases and known pitfalls.
* Provide **excellent docs**, with `//!` module docs and doctests for core types and helper functions.

### 1.2 Non-goals

* No Graphite web UI replication.
* No proprietary SaaS dependencies.
* No GitHub “stack view” page; any `--stack` UX will be implemented locally (print/open multiple PRs).
* No background daemon. Everything is on-demand per command.
* v1 supports **GitHub only** (auth + PR operations), but the codebase must be structured to add GitLab/Bitbucket later with minimal core changes.

---

## 2. Prime invariant: Repository and metadata integrity

**Prime invariant (must hold):**

> At the beginning and end of every `lattice` command, the repository and metadata must be in a **self-consistent state**.

### 2.1 Definition: “self-consistent state”

A state is self-consistent if all of the following are true:

1. **No partially applied multi-ref updates exist without a journal entry** that can complete or roll back safely.
2. The **stack graph derived from metadata** is valid:

   * No cycles
   * All tracked branches exist as local refs
   * Exactly one configured trunk per stack root (v1: single trunk total, architecture allows more)
3. For every tracked non-trunk branch `b` with parent `p`:

   * `b.meta.base` exists as a git object (commit OID)
   * `b.meta.base` is an ancestor of `b.tip` (or equals it for empty branches)
   * `b.meta.base` is reachable from `p.tip` (unless the parent has been force-rewritten; see corruption handling)
4. The metadata itself is **parseable, versioned, and self-describing**.
5. If a command ends in a **paused conflict state**, then:

   * Git’s rebase/merge state is valid (as created by Git),
   * Lattice has a **current operation journal** describing exactly how to resume/abort,
   * and metadata still accurately describes refs that have actually been updated so far.

### 2.2 Integrity enforcement mechanism

Every mutating command MUST:

* Acquire an **exclusive repo lock** (`.git/lattice/lock`) before making changes.
* Run a **preflight verification** (fast, deterministic).
* Create an **operation journal** before any irreversible step.
* Apply changes using a **transaction-like ref update strategy**:

  * All ref changes (branch refs + metadata refs) must be recorded as `(refname, old_oid, new_oid, reason)`.
  * Apply ref updates with `git update-ref` semantics that include expected old OIDs to prevent clobber.
* On success:

  * Run a **postflight verification** (fast).
  * Write a final journal “committed” marker.
* On failure:

  * Attempt rollback using the journal.
  * If rollback cannot be completed automatically, the command MUST:

    * leave the repo in a safe paused state,
    * and instruct the user to run `lattice abort` or `lattice continue` as appropriate.

**Hard rule:** Lattice must never silently leave metadata out of sync with branch refs.

---

## 3. Conceptual model and glossary

* **Trunk**: The mainline branch (usually `main`/`master`/`develop`). Configured per repo. v1 assumes one trunk.
* **Tracked branch**: A local branch with metadata in `refs/branch-metadata/<branch>`.
* **Parent**: The branch a branch is stacked on.
* **Child**: A branch stacked directly on another branch.
* **Stack**: A DAG rooted at trunk. Graphite workflows typically form a chain, but Lattice supports branching DAGs.
* **Base commit** (`base`): For a tracked branch `b`, the commit in `b`’s history that corresponds to the parent’s tip at the time `b` was created or last restacked. This is required for correctness and corruption detection.
* **Frozen branch**: A tracked branch marked immutable to Lattice (no operations that would rewrite or add commits on it).

---

## 4. Storage model

### 4.1 Branch metadata refs

For each tracked local branch `refs/heads/<branch>`, Lattice stores a metadata ref:

* **Ref name**: `refs/branch-metadata/<branch>`
* **Ref target**: A **blob object** containing JSON (UTF-8).

This makes metadata:

* inspectable via git plumbing,
* local by default,
* optionally shareable by pushing those refs (opt-in).

#### 4.1.1 Metadata schema requirements

* **Self-describing**: includes a `kind` string and `schema_version`.
* **No boolean blindness**: use enums/objects instead of scattered optionals.
* **Always includes `parent` and `base`** for tracked branches (including empty branches).
* **Includes freeze state and PR linkage state** as structured variants, not nullable fields.

See Appendix A for the full schema.

---

### 4.2 Operation journal and crash safety

Mutating commands must write an operation journal under:

* `.git/lattice/ops/<op_id>.json`
* `.git/lattice/current-op` (text file containing `<op_id>`) while in progress

The journal is the source of truth for:

* rollback (`abort`)
* resume (`continue`)
* undo last completed op (`undo`)

#### 4.2.1 Journal structure

* `op_id`: UUID v4
* `command`: string (`"restack"`, `"submit"`, etc.)
* `started_at`, `finished_at`
* `state`: `{"phase": "in_progress" | "paused" | "committed" | "rolled_back"}`
* `steps`: append-only list; each step includes:

  * `kind`: `ref_update` | `git_process` | `metadata_write` | `checkpoint`
  * `before`/`after` ref snapshots where applicable
  * stable “expected old” OIDs for safety
* `conflict`: when paused, include:

  * current branch name
  * git state detection (`rebase`, `merge`, etc.)
  * remaining planned branches

#### 4.2.2 Crash consistency contract

* Journals must be written with `fsync` at each appended step boundary.
* A command interrupted mid-flight must be recoverable:

  * next invocation of `lattice` detects `current-op`
  * refuses most commands and instructs:

    * `lattice continue`, `lattice abort`, or `lattice undo` depending on journal state
* No command (except `continue/abort/undo/info/log`) may proceed if a journal is active.

---

### 4.3 Global and repo configuration

Lattice has:

* **Global config** (user scope)
* **Repo config** (per-repository overrides)

#### 4.3.1 Config locations and precedence

Global config search order:

1. `$LATTICE_CONFIG` if set
2. `$XDG_CONFIG_HOME/lattice/config.toml` if present
3. `~/.lattice/config.toml` (canonical write location in v1)

Repo config location:

* `.git/lattice/config.toml` (canonical)

Precedence:

* Repo config overrides global config.
* CLI flags override both.

#### 4.3.2 Config schema highlights

Global config includes:

* default forge (`github`)
* interactive defaults
* hook verification defaults
* branch naming rules
* submit defaults
* secret storage provider selection (see next section)

Repo config includes:

* trunk branch name
* remote name (`origin` default)
* metadata ref sync setting (disabled by default)
* forge repo identification override (rare, but allowed)

---

### 4.4 Secret storage abstraction – GitHub App OAuth tokens

Lattice authenticates to GitHub using **GitHub App OAuth device flow**. This section defines how authentication tokens are stored and managed.

#### 4.4.1 What we store

Lattice stores **GitHub App user auth** for API access:

* `access_token` (short-lived bearer token, starts with `ghu_`)
* `refresh_token` (rotating, single-use on refresh, starts with `ghr_`)
* expiration timestamps for both
* a minimal identity cache for display (durable GitHub user id + login)

Tokens are stored via `SecretStore` and MUST never be written into:

* repo config
* metadata refs
* event ledger
* journal/op-state markers

#### 4.4.2 Secret keys and formats

All GitHub App auth secrets are keyed by host. v1 supports `github.com` by default.

**SecretStore key pattern:** `github_app.oauth.<host>`

**Stored value (JSON, UTF-8, schema-versioned):**

```json
{
  "kind": "lattice.github-app-oauth",
  "schema_version": 1,

  "host": "github.com",
  "client_id": "<GITHUB_APP_CLIENT_ID>",

  "user": {
    "id": 1234567,
    "login": "octocat"
  },

  "tokens": {
    "access_token": "ghu_...",
    "access_token_expires_at": "2026-01-10T12:34:56Z",

    "refresh_token": "ghr_...",
    "refresh_token_expires_at": "2026-07-10T12:34:56Z"
  },

  "timestamps": {
    "created_at": "2026-01-10T12:00:00Z",
    "updated_at": "2026-01-10T12:34:56Z"
  }
}
```

#### 4.4.3 Concurrency and refresh safety

Refresh tokens are **single-use** and rotate on each refresh. To prevent double-refresh races across concurrent `lattice` invocations, Lattice MUST use an auth-scoped lock:

* Lock path: `~/.lattice/auth/lock.<host>` (advisory file lock)

Rules:

* Any code path that may refresh tokens MUST hold this lock.
* Reads may proceed without the lock, but if the access token is expired or near-expiry, the refresher must acquire the lock and re-check before refreshing.
* The lock is held only for the duration of the refresh operation, not the entire command.

#### 4.4.4 Redaction hard rule

Tokens MUST never appear in:

* logs (including `--debug` output)
* JSON outputs
* journal/op-state markers
* doctor explanations
* error messages

All error reporting must redact:

* values that match `ghu_*` and `ghr_*` patterns
* `Authorization` headers
* request/response bodies from OAuth token endpoints

#### 4.4.5 SecretStore trait

Define trait:

```rust
/// Stores and retrieves secrets like GitHub App OAuth tokens.
/// Swappable so we can move from plaintext file to keychain later.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> anyhow::Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> anyhow::Result<()>;
    fn delete(&self, key: &str) -> anyhow::Result<()>;
}
```

v1 MUST include:

1. **FileSecretStore** (default):

   * stores secrets in `~/.lattice/secrets.toml`
   * enforces file permissions `0600` on Unix
   * never prints secrets
2. **KeychainSecretStore** (pluggable, may be feature-gated):

   * uses OS keychain via a crate like `keyring`
   * enabled by config: `secrets.provider = "keychain"`

**Important:** Even if keychain support is feature-gated initially, the core architecture must route through `SecretStore` so swapping providers does not rewrite command code.

---

### 4.5 Repo config additions – GitHub owner/repo context

To manage authorization at the owner/repo level, Lattice stores **repo identity and authorization cache** in repo config.

Repo config file location:

* `.git/lattice/config.toml` (canonical)

Add the following table to repo config:

```toml
[forge.github]
host = "github.com"
owner = "ORG_OR_USER"
repo = "REPO_NAME"

# Optional caches (may be missing/stale; never correctness-critical):
installation_id = 12345
repository_id = 67890
authorized_at = "2026-01-10T12:00:00Z"
```

Notes:

* `owner`/`repo` default to parsing the git remote URL; config acts as an override.
* Cached ids (`installation_id`, `repository_id`) speed up "is the app installed for this repo?" checks but are not required for correctness.
* `authorized_at` is a timestamp for cache invalidation (recommended TTL: 10 minutes).

---

## 5. Architecture

### 5.1 Crate layout (recommended)

```
lattice/
├─ src/
│  ├─ main.rs
│  ├─ lib.rs
│  ├─ cli/
│  │  ├─ mod.rs
│  │  ├─ args.rs
│  │  └─ commands/
│  ├─ core/
│  │  ├─ mod.rs
│  │  ├─ graph.rs
│  │  ├─ verify.rs
│  │  ├─ ops/
│  │  │  ├─ mod.rs
│  │  │  ├─ journal.rs
│  │  │  └─ lock.rs
│  │  ├─ metadata/
│  │  │  ├─ mod.rs
│  │  │  ├─ schema.rs
│  │  │  └─ store.rs
│  │  ├─ config/
│  │  │  ├─ mod.rs
│  │  │  └─ schema.rs
│  │  └─ naming.rs
│  ├─ git/
│  │  ├─ mod.rs
│  │  ├─ git_cli.rs
│  │  ├─ git2.rs
│  │  ├─ refs.rs
│  │  └─ rebase.rs
│  ├─ forge/
│  │  ├─ mod.rs
│  │  ├─ traits.rs
│  │  └─ github.rs
│  ├─ secrets/
│  │  ├─ mod.rs
│  │  ├─ file_store.rs
│  │  └─ keychain_store.rs
│  └─ ui/
│     ├─ mod.rs
│     ├─ prompts.rs
│     └─ output.rs
└─ tests/
   ├─ integration/
   └─ fixtures/
```

### 5.2 Dependency guidance

* CLI: `clap` derive
* Serialization: `serde`, `serde_json`, `toml`
* Error handling: `anyhow`, `thiserror`
* Git:

  * read-only and ref inspection via `git2` (optional but recommended)
  * complex workflows (rebase, add -p, commit) via spawning `git` CLI to respect user config and hooks
* HTTP: `reqwest` or `octocrab` for GitHub
* Testing: `assert_cmd`, `assert_fs`, `tempfile`, `insta`, `proptest`

---

## 6. CLI contract

### 6.1 Global flags (available on all commands)

| Flag                                 | Behavior                                                                        |
| ------------------------------------ | ------------------------------------------------------------------------------- |
| `--help` / `-h`                      | help for command                                                                |
| `--version`                          | version                                                                         |
| `--cwd <path>`                       | run as if executed in that directory                                            |
| `--debug`                            | verbose debug logging                                                           |
| `--interactive` / `--no-interactive` | controls prompts, selectors, editors                                            |
| `--verify` / `--no-verify`           | controls git hooks where applicable                                             |
| `-q, --quiet`                        | minimal output; implies `--no-interactive`                                      |
| `--json`                             | optional v1 feature: machine-readable output for key commands (log/info/submit) |

### 6.2 Interactive rules (Graphite-like)

* If interactive:

  * allow selecting branches in fuzzy selectors
  * allow prompts for confirmation and metadata editing
  * allow opening editor (commit message, PR body)
* If non-interactive:

  * any operation requiring a choice MUST error with a clear message unless user supplied flags to disambiguate
  * any destructive operation MUST require `--force` or equivalent

### 6.3 Exit codes

* `0`: success
* `1`: known failure (validation, conflicts requiring action, missing auth)
* `2`: unexpected/internal error (bug)
* `3`: refused due to active operation journal (must continue/abort/undo)

---

## 7. Stack graph invariants and verification

Lattice maintains a derived in-memory graph:

* Nodes: tracked branches
* Edge: `child -> parent` (stored as parent pointer in metadata)
* Root: trunk (configured)

### 7.1 Verification modes

* **Fast verify** (default at start/end of mutating commands):

  * ensure parseability
  * ensure acyclic
  * ensure refs exist
  * ensure base ancestry constraints
* **Full verify** (optional `lattice verify` future command, or `--verify-graph` debug flag):

  * also verify that metadata matches derived children sets
  * optionally validate PR linkage consistency

### 7.2 Corruption and repair policy

Lattice must detect and handle:

* Missing metadata for a tracked-expected branch
* Metadata referencing nonexistent parent
* Base commit no longer reachable from parent tip (common after manual `git rebase`/force push)
* Cycles introduced by incorrect parent settings

Repair strategy:

* Never silently rewrite history.
* Offer explicit repair actions:

  * `lattice untrack` + `lattice track`
  * `lattice move --onto <parent>`
  * `lattice init --reset` to clear metadata
* `lattice sync` may re-parent orphaned children to the closest tracked ancestor if configured to do so, but must prompt unless `--force`.

---

## 8. Command reference

Each command section includes:

* **Synopsis**
* **Flags**
* **Behavior**
* **Integrity contract** (what must be true at start/end)
* **Behavioral tests** (minimum required)
* **Documentation target** (file that must exist in repo)

> **Documentation requirement:** For every command `X`, create `docs/commands/X.md` and include:
>
> * summary
> * examples
> * gotchas
> * mapping to Graphite’s equivalent command(s)
> * doctestable snippets where possible (Rust doctests for internal APIs; CLI examples in docs are validated via integration tests)

---

# 8A. Setup and configuration

## 8A.1 `lattice auth` (GitHub App device flow)

**Docs:** `docs/commands/auth.md`

### Synopsis

* `lattice auth` (alias for `lattice auth login`)
* `lattice auth login`
* `lattice auth status`
* `lattice auth logout`
* `lattice auth login --host github.com` (future-proofing; v1 default is github.com)
* `lattice auth login --no-browser` (do not attempt to open a browser)

### Flags

* `--host <host>`: GitHub host (default: `github.com`, reserved for future enterprise support)
* `--no-browser`: do not attempt to open the verification URL in a browser

### Behavior: login (device flow)

`lattice auth login`:

1. Resolves GitHub host (default `github.com`).
2. Starts device flow using the canonical client ID (see `GITHUB_APP_CLIENT_ID` in source):
   * requests a device code via `POST https://github.com/login/device/code`
   * prints the verification URL and user code to the terminal
   * optionally opens a browser to the verification URL (unless `--no-browser`)
3. Polls `POST https://github.com/login/oauth/access_token` until success, cancellation, or expiration.
   * handles `authorization_pending` by continuing to poll
   * handles `slow_down` by increasing the polling interval
4. On success:
   * stores tokens (access + refresh + expirations) via `SecretStore` under key `github_app.oauth.<host>`
   * calls `GET /user` to cache durable identity (id + login) for `auth status`
5. Never prints tokens.

### Behavior: status

`lattice auth status` prints (non-secret):

* host(s) logged in
* GitHub user login + id
* access token expiry (timestamp only, not the token)
* whether current repo appears authorized for the GitHub App (best-effort check)

### Behavior: logout

`lattice auth logout`:

* deletes `github_app.oauth.<host>` from `SecretStore`
* clears any cached authorization fields in repo config for that host (best-effort)

### Non-interactive rules

* `lattice auth login` may be run in non-interactive mode.
  * It will print the device URL and code and poll for completion.
  * If additional choices are required (future multi-host), it must error with instructions.

### Integrity contract

* Auth does not mutate repository refs and requires no repo lock.
* Auth writes MUST be atomic with respect to token refresh locking (see Section 4.4.3).

### Errors (stable)

| Condition | Exit Code | Message |
|-----------|-----------|---------|
| Missing auth for host | 1 | "Not authenticated. Run `lattice auth login`." |
| Device flow disabled | 1 | "Device flow is disabled for this GitHub App. Enable 'Device Flow' in the app settings." |
| Expired refresh token | 1 | "Authentication expired. Run `lattice auth login` again." |
| User cancelled | 1 | "Authentication cancelled." |
| Polling timeout | 1 | "Device flow expired. Please try again." |

### Behavioral tests

* Device flow happy path:
  * mock `POST /login/device/code`
  * mock repeated `POST /login/oauth/access_token` returning `authorization_pending` then success
  * verify secret store write occurred and tokens never appear in stdout/stderr
* `slow_down` handling increases polling interval correctly
* Refresh flow:
  * mock refresh response that rotates refresh_token
  * verify single refresh under concurrent calls using lock (spawn two tasks; one refresh must win)
* Logout removes secret key and does not leak token
* Status displays user info without exposing tokens

---

## 8A.2 `lattice init`

**Docs:** `docs/commands/init.md`

### Synopsis

* `lattice init`
* `lattice init --trunk <branch>`
* `lattice init --reset`

### Flags

* `--trunk <branch>`: set trunk
* `--reset`: remove all `refs/branch-metadata/*` and clear repo config

### Behavior

* Creates `.git/lattice/config.toml` if missing.
* If trunk not specified and interactive: prompt to pick from local branches.
* Writes trunk name to repo config.
* On `--reset`:

  * requires confirmation unless `--force` (or `--no-interactive` implies must pass `--force`)
  * deletes all metadata refs
  * clears operation history

### Integrity contract

* After init, repo config must exist and be parseable.
* After reset, metadata namespace must be empty and graph must be “only trunk”.

### Behavioral tests

* Init picks trunk and persists.
* Init with nonexistent trunk errors.
* Reset deletes metadata refs and leaves repo usable.
* Reset refuses in non-interactive without force.

---

## 8A.3 `lattice config`

**Docs:** `docs/commands/config.md`

### Synopsis

* `lattice config`
* `lattice config get <key>`
* `lattice config set <key> <value>`
* `lattice config edit` (optional editor-based)
* `lattice config list`

### Behavior

* Reads global + repo config, applies precedence.
* `set` writes to repo config by default for repo keys, global config for global keys (explicit override flags allowed).
* Must validate schema on write.

### Behavioral tests

* Precedence: repo overrides global.
* `set trunk` updates repo config.
* Invalid values rejected.
* `edit` is skippable in CI via env `LATTICE_TEST_EDITOR`.

---

## 8A.4 `lattice alias`

**Docs:** `docs/commands/alias.md`

### Synopsis

* `lattice alias list`
* `lattice alias add <name> <expansion...>`
* `lattice alias remove <name>`
* `lattice alias reset`
* `lattice alias import-legacy` (optional)

### Behavior

* Aliases expand before Clap parsing or via Clap subcommand wrapper.
* Must prevent alias shadowing a real command unless `--force`.

### Tests

* Alias executes expected command.
* Reset clears alias map.

---

## 8A.5 `lattice completion`

**Docs:** `docs/commands/completion.md`

### Synopsis

* `lattice completion --shell bash|zsh|fish|powershell`

### Behavior

* Uses Clap completion generation.
* Prints script to stdout.

### Tests

* Non-empty output for each shell.

---

## 8A.6 `lattice changelog`

**Docs:** `docs/commands/changelog.md`

### Behavior

* Prints version and release notes summary (from embedded file or `CHANGELOG.md`).
* Not required to be exhaustive.

---

# 8B. Tracking and structure

## 8B.1 `lattice track [branch]`

**Docs:** `docs/commands/track.md`

### Synopsis

* `lattice track`
* `lattice track <branch>`
* `lattice track <branch> --parent <parent>`
* `lattice track <branch> --force`

### Flags

* `--parent <branch>`: explicit parent
* `--force`: choose nearest tracked ancestor automatically (no prompt)
* `--as-frozen` (optional): track as frozen by default (useful when tracking teammate branches)

### Behavior

* Defaults to current branch if `<branch>` omitted.
* Determines parent:

  * if `--parent`, use it
  * else if interactive, prompt among tracked branches and trunk
  * else if `--force`, choose nearest tracked ancestor by commit ancestry
  * else error
* Computes base commit:

  * set `base = parent.tip` at tracking time
* Writes metadata ref for branch with:

  * parent pointer
  * base commit
  * freeze state (default unfrozen unless `--as-frozen`)
  * PR state = none

### Integrity contract

* After track, graph remains acyclic and base is consistent.

### Tests

* Track outside-created branch with `--parent trunk`.
* Track without parent in interactive mode.
* Track with `--force` selects expected ancestor.
* Track already-tracked branch updates parent if explicitly requested (repair use case).

---

## 8B.2 `lattice untrack [branch]`

**Docs:** `docs/commands/untrack.md`

### Synopsis

* `lattice untrack`
* `lattice untrack <branch>`
* `lattice untrack <branch> --force`

### Behavior

* Defaults to current branch.
* Removes metadata refs for branch and all descendants (must prompt unless `--force`).
* Never deletes git branches.

### Tests

* Untrack middle of chain removes descendants.
* Refuses in non-interactive without force when descendants exist.

---

## 8B.3 `lattice trunk`

**Docs:** `docs/commands/trunk.md`

### Synopsis

* `lattice trunk` (print trunk)
* `lattice trunk set <branch>` (alias for init --trunk)
* `lattice trunk --all` (reserved)

v1 supports one trunk; architecture can support more later.

---

## 8B.4 `lattice freeze [branch]` and `lattice unfreeze [branch]`

**Docs:** `docs/commands/freeze.md`, `docs/commands/unfreeze.md`

### Behavior

* Defaults to current branch.
* Freeze must apply to:

  * the target branch
  * and (Graphite-like) its **downstack ancestors** up to trunk (configurable scope, default “downstack inclusive”)
* Unfreeze reverses the same scope.

### Enforcement

Every mutating command that would:

* create commits,
* amend commits,
* rebase,
* reorder,
* fold/pop,
  must check freeze state of affected branches and refuse unless:
* the operation is explicitly permitted (example: `get` may update a frozen branch from remote),
* or user passes an override flag (v1: no override except `get --unfrozen` for fetched branches, see below).

### Tests

* Frozen branch blocks `modify`, `absorb`, `restack`, `move`, `fold`, `squash`, `delete` (if delete implies restack children, still blocked unless it can avoid rewriting frozen refs).
* `get` allowed on frozen branch.
* Freeze scope rules correct.

---

# 8C. Navigation

## 8C.1 `lattice checkout [branch]`

**Docs:** `docs/commands/checkout.md`

### Synopsis

* `lattice checkout`
* `lattice checkout <branch>`
* `lattice checkout --trunk`
* `lattice checkout --stack`
* `lattice checkout --all`

### Behavior

* If no branch:

  * interactive selector
  * non-interactive: error
* `--stack` filters to ancestors/descendants of current branch (tracked graph).
* Must respect git checkout safety (dirty working tree may block). Lattice should surface git’s error clearly.

### Tests

* Checkout by name.
* Selector path (simulated).
* Stack filtering correctness.

---

## 8C.2 `lattice up [steps]` and `lattice down [steps]`

**Docs:** `docs/commands/up.md`, `docs/commands/down.md`

### Behavior

* `up`: move to child, prompt if multiple; `--to <branch>` chooses descendant target.
* `down`: move to parent; supports `--steps`.

### Tests

* Multi-child prompts or errors non-interactive.
* Steps skip correctly.

---

## 8C.3 `lattice top` / `lattice bottom`

**Docs:** `docs/commands/top.md`, `docs/commands/bottom.md`

### Behavior

* `top`: follow children until leaf; prompt if multiple tips exist.
* `bottom`: follow parents until trunk-child; prompt if ambiguous from trunk.

---

# 8D. Stack mutation commands

These commands acquire repo lock, require clean operation state, write journals, and must maintain the prime invariant.

## 8D.1 `lattice create [name]`

**Docs:** `docs/commands/create.md`

### Synopsis

* `lattice create`
* `lattice create <name>`
* `lattice create -m <msg>`
* `lattice create -a|-u|-p`
* `lattice create --insert`

### Flags

* `-m, --message <msg>`
* `-a, --all` stage all (including untracked)
* `-u, --update` stage modified tracked files
* `-p, --patch` interactive add -p
* `-i, --insert` insert between current and a selected child
* `-v, --verbose` show diff template (optional)

### Behavior (Graphite-like)

1. Preflight: verify no op in progress, verify metadata graph.
2. Determine branch name:

   * if provided, use it
   * else derive from commit message subject (or prompt for message)
3. If there are staged or selectable changes:

   * create new branch off current HEAD
   * create a commit on new branch using staged changes
4. **If there are no changes to commit**:

   * create an **empty branch** (no new commit), pointing to current HEAD
   * still write metadata for it
5. Metadata:

   * parent = current branch
   * base = parent.tip at creation time (which equals current HEAD at that moment)
6. Checkout new branch.
7. If `--insert`:

   * if current has one child, re-parent that child under the new branch
   * if multiple children, prompt which child to move
   * implement by restacking the moved child onto the new branch, journaling ref changes

### Integrity contract

* Empty create must still produce consistent metadata (base and parent must match).
* Insert must not create cycles.

### Behavioral tests

* Create with staged changes produces commit and metadata.
* Create with no changes produces empty branch and metadata.
* Auto-name slug rules.
* `--insert` with one child re-parents correctly.
* `--insert` with multiple children prompts or errors non-interactive.

---

## 8D.2 `lattice modify`

**Docs:** `docs/commands/modify.md`

### Synopsis

* `lattice modify`
* `lattice modify -c`
* `lattice modify -a|-u|-p`
* `lattice modify -m <msg>` / `-e`
* `lattice modify --into <branch>` (v1 optional; if implemented must be tested)
* `lattice modify --interactive-rebase` (v1 optional; can be stubbed with explicit “not implemented”)

### Behavior

* Default: amend HEAD commit on current branch with staged changes.
* If branch is an empty branch (no commits unique beyond base), `modify` creates first commit.
* After mutation, automatically restack descendants unless prevented by freeze.
* If conflicts occur during descendant restack:

  * pause operation
  * write journal state
  * instruct `lattice continue` or `lattice abort`

### Integrity contract

* Must never rewrite frozen branches.
* Metadata must be updated only after branch refs have moved successfully.

### Tests

* Amend changes and restack child.
* Create new commit with `-c`.
* Conflict path pauses, then continue completes.
* Frozen current branch blocks modify.

---

## 8D.3 `lattice restack`

**Docs:** `docs/commands/restack.md`

### Synopsis

* `lattice restack`
* `lattice restack --branch <name>`
* `lattice restack --only`
* `lattice restack --downstack`
* `lattice restack --upstack`

### Restack algorithm (base-commit driven)

For branch `b` with parent `p`:

* If `b.base == p.tip`: aligned, no action.
* Else rebase:

  * `git rebase --onto p.tip b.base b`
  * on success:

    * update `b.base = p.tip` in metadata
  * on failure (conflict):

    * pause with journal

Traversal order:

* **Bottom-up** (closest to trunk first), then toward leaves, to preserve stack correctness.

Frozen rules:

* If `b` is frozen, Lattice must not rebase it.
* If `b` is frozen and parent advanced, Lattice reports it as skipped.
* Descendants of a frozen branch may still be restackable onto that frozen branch’s tip if the frozen branch itself did not move; if parent moved and frozen prevents updating, descendants cannot be brought up to date with trunk via that path.

### Integrity contract

* Every successful rebase must be journaled with before/after ref OIDs.
* If paused, repo state must remain consistent and resume-able.

### Tests

* No-op restack.
* Parent moved, child restacks, base updated.
* Conflict pauses and can continue/abort.
* Frozen branch skipping behavior.

---

## 8D.4 `lattice move`

**Docs:** `docs/commands/move.md`

### Synopsis

* `lattice move --onto <branch>`
* `lattice move --source <branch> --onto <branch>`
* `lattice move` (interactive target selection)

### Behavior

* Changes parent of `source` to `onto`.
* Prevent cycles: cannot move onto a descendant.
* Implementation:

  * validate `onto` exists
  * perform rebase of `source` onto `onto.tip` using `source.base` or merge-base as appropriate:

    * if source is tracked, use `source.base` as the “from” point
    * else require `--from <commit>` or error (v1 simplest: require tracked)
  * update `source.parent = onto`, `source.base = onto.tip`
  * descendants remain descendants of source

### Tests

* Move middle branch; children follow.
* Cycle prevention.
* Conflict path.

---

## 8D.5 `lattice reorder`

**Docs:** `docs/commands/reorder.md`

### Behavior

* Opens editor with the list of branches between trunk and current branch (inclusive, excluding trunk line).
* User reorders lines.
* Lattice computes required rebase sequence to realize new ordering.
* Must validate:

  * same set of branches
  * no duplicates
  * no missing entries
* Must journal each rebase step.
* Conflicts pause.

### Tests

* Swap two adjacent branches.
* Invalid edit detected (duplicate/missing).
* Conflict pause and continue.

---

## 8D.6 `lattice split`

**Docs:** `docs/commands/split.md`

### Synopsis

* `lattice split --by-commit`
* `lattice split --by-hunk` (may be v2 if too complex, but spec requires either implement or explicitly defer with tests asserting “not implemented”)
* `lattice split --by-file <paths...>`

### Behavior (required minimum v1)

* Implement **--by-file** and **--by-commit**.
* Ensure resulting branches are tracked and stacked correctly.
* Ensure no changes are lost: combined diff across resulting stack equals original.

### Tests

* Split by commit creates new branches with subsets.
* Split by file extracts file changes into new branch.
* Sum-of-diffs invariant holds.

---

## 8D.7 `lattice squash`

**Docs:** `docs/commands/squash.md`

### Behavior

* Squash all commits unique to current branch into one.
* Preserve parent relation.
* Restack descendants.
* Respect freeze.

### Tests

* Squash multi-commit branch.
* No-op on single commit.
* Restack child after squash.

---

## 8D.8 `lattice fold`

**Docs:** `docs/commands/fold.md`

### Behavior

* Merge current branch’s changes into its parent, then delete current branch.
* Re-parent children to parent.
* `--keep`: keep the current branch name by renaming parent branch to current name after fold (journaled).

### Tests

* Fold simple branch.
* Fold with child re-parenting.
* `--keep` semantics correct.

---

## 8D.9 `lattice pop`

**Docs:** `docs/commands/pop.md`

### Behavior

* Delete current branch but keep its net changes applied to parent as uncommitted changes.
* Requires clean working tree at start (or explicit `--stash` v2).
* Must remove metadata and re-parent children.

### Tests

* Pop single commit branch.
* Pop multi-commit branch preserves net diff.

---

## 8D.10 `lattice delete [branch]`

**Docs:** `docs/commands/delete.md`

### Behavior

* Deletes local branch and metadata.
* Re-parents children to deleted branch’s parent.
* Does not close PRs or delete remote branches.
* `--upstack` deletes descendants too.
* `--downstack` deletes ancestors (never trunk in v1 unless explicit `--delete-trunk-i-really-mean-it`, recommended to not implement).

### Tests

* Delete middle branch re-parents child.
* Force flag bypasses prompts.
* Upstack and downstack semantics.

---

## 8D.11 `lattice rename [name]`

**Docs:** `docs/commands/rename.md`

### Behavior

* Renames current branch.
* Updates:

  * `refs/heads/<old>` -> `<new>`
  * metadata ref name
  * any metadata `parent` references in other branches pointing to old
* Must journal ref renames (copy + delete pattern in git refs).

### Tests

* Rename updates all metadata pointers.
* Rename refuses if would create ambiguity/cycle.

---

## 8D.12 `lattice revert <sha>`

**Docs:** `docs/commands/revert.md`

### Behavior

* Creates new branch off trunk and performs `git revert <sha>`.
* Handles conflicts with pause/continue/abort.

### Tests

* Revert known commit creates expected inverse change.
* Conflict pause.

---

# 8E. Remote and PR integration (GitHub v1)

## 8E.0 Auth gating for GitHub remote commands

Commands that call GitHub APIs (`submit`, `sync`, `get`, `merge`, `pr` resolution when querying, etc.) require the following capabilities to be satisfied:

* `AuthAvailable(host)` – A valid GitHub App user access token exists for the host OR can be refreshed
* `RemoteResolved(owner, repo)` – The git remote can be parsed to identify the GitHub owner and repository
* `RepoAuthorized(owner, repo)` – The GitHub App is installed and authorized for this repository (best-effort preflight)

If `RepoAuthorized` cannot be established, the command MUST refuse with:

* a clear explanation of why authorization failed
* an install link for the GitHub App: `https://github.com/apps/lattice/installations/new`
* exit code 1

The command must not attempt destructive remote operations without verified authorization.

### 8E.0.1 Determining "RepoAuthorized" (owner/repo level)

Given `host`, `owner`, `repo`:

1. Using the stored user access token, query installations accessible to the user token:
   * `GET /user/installations`
2. For each installation, query repositories accessible to the user token for that installation:
   * `GET /user/installations/{installation_id}/repositories`
   * Continue until the repo is found or all installations are exhausted.
3. If found:
   * Cache `installation_id` and `repository_id` in repo config (best-effort)
   * Return `RepoAuthorized` capability
4. If not found:
   * Treat as "app not installed or not authorized for this repo"
   * Output install instructions: `https://github.com/apps/lattice/installations/new`
   * Exit code 1

**Caching:**

* Cache authorization checks for a short TTL (e.g., 10 minutes) in `.git/lattice/cache/github_auth.json`.
* Repo config caches (`installation_id`, `repository_id`) are allowed to be stale and must never be trusted without validation.

---

## 8E.1 Forge abstraction

Define:

```rust
#[async_trait::async_trait]
pub trait Forge: Send + Sync {
    async fn create_pr(&self, req: CreatePr) -> anyhow::Result<PullRequest>;
    async fn update_pr(&self, req: UpdatePr) -> anyhow::Result<PullRequest>;
    async fn get_pr(&self, number: u64) -> anyhow::Result<PullRequest>;
    async fn find_pr_by_head(&self, head: &str) -> anyhow::Result<Option<PullRequest>>;
    async fn set_draft(&self, pr: u64, draft: bool) -> anyhow::Result<()>;
    async fn request_reviewers(&self, pr: u64, reviewers: Reviewers) -> anyhow::Result<()>;
    async fn merge_pr(&self, pr: u64, method: MergeMethod) -> anyhow::Result<()>;
    fn name(&self) -> &'static str;
}
```

v1 implements `GitHubForge`. Other adapters are stubs behind feature flags, but core must depend only on `Forge`.

### TokenProvider integration

Authentication is handled inside the GitHub adapter implementation by attaching a valid bearer token to each request. The adapter depends on a `TokenProvider` that yields a valid access token and refreshes when needed:

```rust
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    /// Returns a valid bearer token, refreshing if necessary.
    async fn bearer_token(&self) -> anyhow::Result<String>;
}
```

The GitHub adapter MUST:

* call `bearer_token()` per request (or per short-lived cached client)
* retry once on 401/403 if the token is refreshable, then surface a stable auth error

---

## 8E.2 `lattice submit`

**Docs:** `docs/commands/submit.md`

### Synopsis

* `lattice submit`
* `lattice submit --stack`
* `lattice submit --draft`
* `lattice submit --publish`
* `lattice submit --confirm`
* `lattice submit --dry-run`
* `lattice submit --force`
* `lattice submit --always`
* `lattice submit --update-only`
* `lattice submit --reviewers <u1,u2>`
* `lattice submit --team-reviewers <t1,t2>` (GitHub supports team reviewers)
* `lattice submit --restack` / `--no-restack`
* `lattice submit --target-trunk <branch>`
* `lattice submit --view`

### Key semantics (Graphite-like)

Branch set:

* Default: all ancestors from trunk to current branch (inclusive).
* With `--stack`: also include descendants of current branch.

Restack:

* Default: `--restack` enabled (unless config disables).
* If restack would conflict, submit pauses and refuses to proceed until resolved.

Push behavior:

* Default: **skip pushing branches whose local tip matches the last submitted remote tip** (or matches remote tracking ref).
* `--always`: push regardless.
* Default push mode uses **force-with-lease** for safety.
* `--force`: overwrite remote even if lease fails (explicitly dangerous, but Graphite-like).

PR creation/update:

* For each branch in submit set (in stack order):

  * determine PR base branch:

    * parent branch if tracked and included
    * otherwise trunk (or `--target-trunk`)
  * if metadata says PR is linked, update
  * else try `find_pr_by_head` on GitHub

    * if found, link it in metadata and update
    * else create new PR
* Draft toggling:

  * create PR as draft if `--draft`
  * if `--publish`, set draft false (requires GraphQL)
* Editing PR title/body:

  * default interactive behavior: prompt for **new PRs only**
  * `--edit`: prompt for all PRs
  * `--no-edit`: never prompt, use defaults:

    * title: first line of commit message
    * body: remainder or empty
* Reviewers:

  * if provided, request reviewers for created PRs (and optionally for updated PRs if `--rerequest-review` is added later)

### Integrity contract

* Must not create PRs if repo is not in a consistent restacked state (unless user explicitly disables restack and accepts risk, recommended to not allow in v1).
* Must journal any metadata changes (PR linking) and any git ref pushes (recorded for undo only locally; cannot undo remote pushes).

### Behavioral tests (minimum)

* New stack creates PRs with correct bases.
* Re-run submit updates existing PRs, no duplicates.
* Skip unchanged push behavior works.
* `--always` forces pushes.
* `--force` overwrites remote divergence (simulate by remote commit changes).
* Dry-run produces no changes.
* Confirm flow cancels safely.
* Draft create and publish toggling calls GraphQL path.
* Submit refuses when auth missing.

---

## 8E.3 `lattice sync`

**Docs:** `docs/commands/sync.md`

### Synopsis

* `lattice sync`
* `lattice sync --force`
* `lattice sync --restack` / `--no-restack`
* `lattice sync --all` (reserved for multi-trunk)

### Behavior

* `git fetch <remote>`
* Update trunk:

  * fast-forward if possible
  * if not possible, prompt to reset trunk to remote trunk unless `--force`
* For each tracked branch:

  * determine PR state:

    * use metadata-linked PR if present
    * else optionally search by head
  * if PR merged/closed, prompt to delete local branch (unless `--force`)
* If `--restack` enabled:

  * restack all restackable branches; skip those that conflict and report

### Tests

* Merged branch deletion prompt.
* Trunk fast-forward update.
* Diverged trunk requires force or prompt.
* Restack happens post-trunk update.

---

## 8E.4 `lattice get [branch-or-pr]`

**Docs:** `docs/commands/get.md`

### Synopsis

* `lattice get <branch>`
* `lattice get <pr_number>`
* `lattice get --downstack`
* `lattice get --force`
* `lattice get --restack` / `--no-restack`
* `lattice get --unfrozen`

### Behavior

* If argument is number:

  * fetch PR details via forge
  * resolve head branch
* Fetch branch ref from remote into local branch.
* Determine parent:

  * if metadata refs are available and enabled, use them
  * else use PR base branch (GitHub API)
  * else fall back to trunk
* Track fetched branch locally (write metadata):

  * default: **frozen** (safe default when pulling others’ work)
  * `--unfrozen`: mark unfrozen
* If branch already exists locally:

  * update it to match remote (force if `--force`)
  * by default sync upstack branches too unless `--downstack`
* Optionally restack after syncing.

### Tests

* Get by PR number resolves and fetches.
* New fetched branch defaults to frozen; `--unfrozen` overrides.
* Force overwrites divergence.

---

## 8E.5 `lattice merge`

**Docs:** `docs/commands/merge.md`

### Synopsis

* `lattice merge`
* `lattice merge --confirm`
* `lattice merge --dry-run`
* `lattice merge --method merge|squash|rebase` (optional config default)

### Behavior

* Merge PRs from trunk to current branch in order.
* Use GitHub merge API.
* Stop on first failure and report.
* Does not delete local branches automatically (suggest `lattice sync` after).

### Tests

* Merge calls happen in correct order.
* Dry-run no API calls.
* Confirm gating works.

---

## 8E.6 `lattice pr [branch-or-pr]`

**Docs:** `docs/commands/pr.md`

### Behavior

* Opens PR URL in browser, or prints in non-interactive/headless environments.
* If `--stack`, open/print URLs for stack branches (ancestors and optionally descendants).
* If metadata lacks PR number, attempt `find_pr_by_head`.

### Tests

* URL building correct for SSH/HTTPS remotes.
* Stack mode yields multiple URLs.

---

## 8E.7 `lattice unlink [branch]`

**Docs:** `docs/commands/unlink.md`

### Behavior

* Removes PR linkage from metadata (sets PR state to `none`).
* Does not alter PR on GitHub.

### Tests

* Unlink removes metadata linkage.

---

# 8F. Conflict recovery and undo

## 8F.1 `lattice continue`

**Docs:** `docs/commands/continue.md`

### Synopsis

* `lattice continue`
* `lattice continue -a, --all`

### Behavior

* Requires an active journal in paused state.
* Detects git operation type (rebase/cherry-pick/revert).
* If `--all`, stages all changes before continuing.
* Completes current git operation, then resumes remaining journal steps.
* On completion, marks journal committed and clears `current-op`.

### Tests

* Continue completes paused restack chain.
* `--all` stages and continues.

---

## 8F.2 `lattice abort`

**Docs:** `docs/commands/abort.md`

### Behavior

* Requires an active journal.
* Aborts current git operation (`git rebase --abort` etc) if present.
* Rolls back any already-applied ref updates using the journal.
* Restores metadata to pre-op state.
* Clears `current-op` and marks journal rolled_back.

### Tests

* Abort restores pre-op refs even after multiple branches were rebased.
* Abort safe when no git rebase is active but journal exists.

---

## 8F.3 `lattice undo`

**Docs:** `docs/commands/undo.md`

### Behavior

* Undoes the most recent **committed** Lattice operation that is undoable locally:

  * ref moves
  * metadata changes
* Cannot undo remote PR creation or pushes; must clearly explain limitations.
* Uses stored journal snapshots.

### Tests

* Undo modifies refs back.
* Undo refuses when last op not undoable.

---

# 8G. Informational commands

## 8G.1 `lattice log`

**Docs:** `docs/commands/log.md`

### Synopsis

* `lattice log`
* `lattice log short|long`
* `lattice log --stack`
* `lattice log --all`
* `lattice log --reverse`
* `lattice log --show-untracked`

### Behavior

* Default: show tracked branches in stack layout with parent arrows and PR state.
* `short`: concise list
* `long`: include commit summaries and optionally PR status
* `--show-untracked`: include untracked local branches in a separate section.

### Tests

* Snapshot tests for formats.
* Stack filtering.

---

## 8G.2 `lattice info [branch]`

**Docs:** `docs/commands/info.md`

### Flags

* `--diff`, `--stat`, `--patch`, `--body`

### Behavior

* Prints:

  * tracking status
  * parent and children
  * base commit
  * freeze state
  * PR linkage state
* Diff options use git CLI.

### Tests

* Output contains expected fields.
* Diff output includes known hunks.

---

## 8G.3 `lattice parent` / `lattice children`

**Docs:** `docs/commands/parent.md`, `docs/commands/children.md`

Read-only relationship queries.

---

# 9. Testing strategy (mandatory)

**Absolute requirement:** Every command and every flag path must have tests. If a feature is deferred, tests must assert that it is explicitly not implemented (and returns a stable error).

### 9.1 Test layers

1. **Unit tests**

   * metadata schema parsing/serialization
   * naming rules
   * graph building and cycle detection
   * verification logic

2. **Integration tests (real git repos)**

   * Use `tempfile` to create repos
   * Use `assert_cmd` to call built binary
   * Validate refs and commit graphs using git plumbing

3. **Adapter tests**

   * Mock `Forge` trait for deterministic PR behavior
   * Simulate API failures: 401, 403, 404, 429, 5xx

4. **Snapshot tests**

   * `log` output formats
   * selected `info` output sections

5. **Property-based tests**

   * generate random DAG shapes (within constraints)
   * verify graph invariants hold and verify is correct

6. **Fault injection tests (highly recommended)**

   * Feature flag `fault_injection`
   * Inject failures at step boundaries in journaling and ref updates
   * Ensure:

     * rollback works
     * or repo is left in a recoverable paused state

### 9.2 Required pitfall tests (minimum set)

* Manual git rebase that breaks base ancestry.
* Force push divergence during submit.
* Conflicts during restack, absorb, move, revert.
* Frozen branches blocking unsafe operations.
* Non-interactive mode refusing ambiguous operations.
* Undo after multi-branch operation.
* Crash simulation mid-restack and recovery on next invocation.

---

# 10. Documentation standard (mandatory)

### 10.1 `mod.rs` documentation

Every module must include:

* Purpose and invariants
* Example usage (doctests where feasible)
* Notes on failure modes

Example:

````rust
//! core::verify
//!
//! Fast repository and metadata verification used at the start/end of mutating commands.
//!
//! # Invariants
//! - Never mutates the repo.
//! - Must be deterministic.
//!
//! # Examples
//! ```
//! # use lattice::core::verify::fast_verify;
//! # let repo = /* test repo */;
//! fast_verify(&repo).unwrap();
//! ```
````

### 10.2 Doctests

* Prefer doctests for pure functions and schema parsing.
* CLI examples are validated via integration tests, not doctests.

### 10.3 Command docs

Each command must have:

* `docs/commands/<cmd>.md`
* sections: Summary, Examples, Flags, Semantics, Pitfalls, Recovery, Parity notes

---

# 11. Implementation phases and acceptance gates

### Phase 1: Core local stack engine (must be shippable)

* config, init, track/untrack, metadata store
* verify + journaling + locking
* create (including empty), checkout, up/down/top/bottom
* log/info/parent/children
* restack, continue, abort, undo
* freeze/unfreeze enforcement

**Gate:**
All Phase 1 commands have integration tests and fault-injection tests for journaling.

### Phase 2: GitHub integration

* auth + secret store
* submit (skip unchanged, force semantics, draft + publish via GraphQL)
* pr/unlink
* sync/get
* merge

**Gate:**
Forge mocked tests plus “live” test harness behind env vars (optional).

### Phase 3: Advanced rewriting features

* modify/absorb/split/squash/fold/pop/reorder/move/rename/delete/revert (if not already)
* expanded UI polish

**Gate:**
Conflict scenarios tested for each rewriting command.

### Phase 4: Multi-forge scaffolding activation

* introduce GitLab adapter behind feature flag
* ensure Forge trait covers needed primitives

---

# 12. Appendices

## Appendix A: Branch metadata schema (v1)

Stored as JSON blob pointed to by `refs/branch-metadata/<branch>`.

Key principle: **no boolean blindness**. Use structured states.

```json
{
  "kind": "lattice.branch-metadata",
  "schema_version": 1,

  "branch": { "name": "feature-b" },

  "parent": {
    "kind": "branch",
    "name": "feature-a"
  },

  "base": {
    "oid": "abc123def4567890..."
  },

  "freeze": {
    "state": "unfrozen"
  },

  "pr": {
    "state": "none"
  },

  "timestamps": {
    "created_at": "2026-01-07T00:00:00Z",
    "updated_at": "2026-01-07T00:00:00Z"
  }
}
```

PR linked example:

```json
"pr": {
  "state": "linked",
  "forge": "github",
  "number": 42,
  "url": "https://github.com/org/repo/pull/42",
  "last_known": {
    "state": "open",
    "is_draft": true
  }
}
```

Freeze example:

```json
"freeze": {
  "state": "frozen",
  "scope": "downstack_inclusive",
  "reason": "teammate_branch",
  "frozen_at": "2026-01-07T00:00:00Z"
}
```

## Appendix B: Required external documentation links

Because this spec is meant to live in-repo, include a `docs/references.md` containing (at minimum) links to:

```text
Git:
- git update-ref documentation
- git rebase documentation
- git push --force-with-lease documentation

GitHub Auth (GitHub App device flow):
- https://docs.github.com/en/apps/creating-github-apps/writing-code-for-a-github-app/building-a-cli-with-a-github-app
- https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app
- https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/refreshing-user-access-tokens

GitHub App Installation:
- https://docs.github.com/en/apps/using-github-apps/installing-a-github-app-from-a-third-party

GitHub App Installation/Repo Access Discovery:
- https://docs.github.com/en/rest/apps/installations

GitHub API:
- REST Pull Requests API docs
- REST Reviewers API docs
- REST Merge API docs
- GraphQL API docs (mutations for draft toggling, auto-merge if supported)

Rust:
- clap derive docs
- serde_json docs
- keyring crate docs (if keychain enabled)
```

(Those links should be maintained as part of the repo documentation set, and validated periodically.)
