# Phase 6: Async/Remote Commands Migration

## Status: PLANNING

**Started:** 2026-01-21  
**Branch:** `jared-fix-ledger-bug`  
**Prerequisite:** Phase 5 complete (restack reference implementation done)

---

## Executive Summary

Phase 6 migrates the async/remote commands (`submit`, `sync`, `get`, `merge`, `auth`) to implement the `AsyncCommand` trait and flow through the unified `run_async_command()` lifecycle. These commands are unique because they:

1. Perform network I/O (GitHub API calls)
2. May combine local mutations with remote operations
3. Have mode-dependent requirements (bare repo vs worktree)
4. Require authentication gating

**Goal:** All async/remote commands implement `AsyncCommand` or a suitable variant, use mode dispatch for flag-dependent requirements, and flow through the engine lifecycle for consistent gating, journaling, and hook firing.

---

## Architecture Overview

### Current State Analysis

| Command | File | Current Pattern | Issues |
|---------|------|-----------------|--------|
| `submit` | `submit.rs` | Manual `scan()`, `check_requirements()`, tokio runtime | Bypasses engine hooks, manual mode logic |
| `sync` | `sync.rs` | Manual `scan()`, `check_requirements()`, tokio runtime | Bypasses engine hooks, manual mode logic |
| `get` | `get.rs` | Manual `scan()`, `check_requirements()`, tokio runtime | Bypasses engine hooks, manual mode logic |
| `merge` | `merge.rs` | Manual `scan()`, `check_requirements()`, tokio runtime | Bypasses engine hooks |
| `auth` | `auth.rs` | No repo requirements, pure OAuth | Special case - no migration needed |

### Target Architecture

Per ARCHITECTURE.md Section 5 and HANDOFF.md Phase 6:

```
Scan → Gate → [Repair if needed] → Async Plan → Execute → Remote Phase → Verify → Return
```

**Key insight:** Async commands have a **two-phase execution model**:
1. **Local phase:** Repository mutations (restack, metadata updates)
2. **Remote phase:** Forge API operations (push, PR create/update)

---

## New Infrastructure Required

### Task 6.0: AsyncCommand Trait and Runner

**Files to modify:**
- `src/engine/command.rs` - Add `AsyncCommand` trait
- `src/engine/runner.rs` - Add `run_async_command()` function
- `src/engine/plan.rs` - Add Forge-related `PlanStep` variants

#### 6.0.1: AsyncCommand Trait

Add to `src/engine/command.rs`:

```rust
use std::future::Future;
use std::pin::Pin;

/// Type alias for async plan futures.
pub type PlanFut<'a> = Pin<Box<dyn Future<Output = Result<Plan, PlanError>> + Send + 'a>>;

/// An async command that performs network operations.
///
/// Async commands differ from synchronous commands in that:
/// - The `plan()` method may perform async operations (API queries, token refresh)
/// - The plan may include remote operations (push, PR create/update)
/// - Execution splits into local and remote phases
///
/// # Example
///
/// ```ignore
/// use latticework::engine::command::AsyncCommand;
///
/// struct SubmitCommand { /* ... */ }
///
/// impl AsyncCommand for SubmitCommand {
///     const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE;
///     type Output = SubmitResult;
///
///     fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a> {
///         Box::pin(async move {
///             // Async planning (may query forge for existing PRs)
///             let plan = self.build_plan(ctx).await?;
///             Ok(plan)
///         })
///     }
///
///     fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
///         // Same pattern as sync Command
///     }
/// }
/// ```
pub trait AsyncCommand: Send + Sync {
    /// The requirement set for this command.
    const REQUIREMENTS: &'static RequirementSet;

    /// Output type produced by this command.
    type Output;

    /// Generate a plan asynchronously from validated context.
    ///
    /// Unlike `Command::plan()`, this may perform async operations:
    /// - Query forge for existing PRs
    /// - Refresh authentication tokens
    /// - Resolve remote branch state
    ///
    /// The returned plan may contain both local and remote steps.
    fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a>;

    /// Process execution result into command output.
    ///
    /// Same semantics as `Command::finish()`.
    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output>;
}
```

#### 6.0.2: Async Runner Function

Add to `src/engine/runner.rs`:

```rust
/// Run an async command through the full lifecycle.
///
/// This is the entry point for commands that implement `AsyncCommand`.
/// It handles the async planning phase and splits execution into
/// local and remote phases.
///
/// # Lifecycle
///
/// 1. **Scan**: Read repository state
/// 2. **Gate**: Verify requirements using `C::REQUIREMENTS`
/// 3. **Plan (async)**: Call `command.plan()` with validated context
/// 4. **Execute Local**: Apply local plan steps through executor
/// 5. **Execute Remote**: Apply remote plan steps (push, PR ops)
/// 6. **Verify**: Re-scan and verify invariants
/// 7. **Finish**: Call `command.finish()` with result
///
/// # Arguments
///
/// * `command` - The async command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
///
/// # Returns
///
/// `CommandOutput<C::Output>` on success, or `RunError` on failure.
pub async fn run_async_command<C: AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
) -> Result<CommandOutput<C::Output>, RunError> {
    if ctx.debug {
        eprintln!("[debug] Starting async command lifecycle");
        eprintln!("[debug] Requirements: {}", C::REQUIREMENTS.name);
    }

    // Step 1: Scan
    let snapshot = scan(git)?;

    // Step 2: Gate
    let ready = match gate(snapshot, C::REQUIREMENTS) {
        GateResult::Ready(ctx) => *ctx,
        GateResult::NeedsRepair(bundle) => {
            return Err(RunError::NeedsRepair(bundle));
        }
    };

    // Step 3: Plan (async)
    if ctx.debug {
        eprintln!("[debug] Step 3: Async Plan");
    }
    let plan = command.plan(&ready).await?;

    if plan.is_empty() {
        let result = ExecuteResult::Success {
            fingerprint: ready.snapshot.fingerprint.clone(),
        };
        return Ok(command.finish(result));
    }

    // Engine hooks (for test harness)
    #[cfg(any(test, feature = "fault_injection", feature = "test_hooks"))]
    {
        if let Ok(info) = git.info() {
            engine_hooks::invoke_before_execute(&info);
        }
    }

    // Step 4: Execute (handles both local and remote steps)
    let executor = Executor::new(git);
    let result = executor.execute(&plan, ctx)?;

    // Step 5: Finish
    Ok(command.finish(result))
}

/// Run an async command with explicit requirements.
///
/// Used for commands with mode-dependent requirements (submit, sync, get).
pub async fn run_async_command_with_requirements<C: AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &'static RequirementSet,
) -> Result<CommandOutput<C::Output>, RunError> {
    // Same as run_async_command but uses provided requirements instead of C::REQUIREMENTS
    // ... (implementation similar to run_command_with_requirements)
}

/// Run an async command with scope resolution.
///
/// Used for commands that operate on a branch stack (submit).
pub async fn run_async_command_with_scope<C: AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    // Same as run_command_with_scope but async
    // ... (implementation)
}
```

#### 6.0.3: Forge-Related PlanStep Variants

Add to `src/engine/plan.rs`:

```rust
/// Push a branch to the remote.
///
/// This step handles git push operations with force-with-lease
/// for safety and proper remote tracking.
ForgePush {
    /// Branch to push.
    branch: String,
    /// Use force-with-lease for push.
    force: bool,
    /// Remote name (default: "origin").
    remote: String,
    /// Human-readable reason for the push.
    reason: String,
},

/// Create a pull request on the forge.
///
/// This step creates a new PR via the forge API.
ForgeCreatePr {
    /// Head branch (the branch being merged).
    head: String,
    /// Base branch (the target branch).
    base: String,
    /// PR title.
    title: String,
    /// PR body (optional).
    body: Option<String>,
    /// Create as draft.
    draft: bool,
},

/// Update an existing pull request.
///
/// This step updates PR metadata via the forge API.
ForgeUpdatePr {
    /// PR number.
    number: u64,
    /// New base branch (optional).
    base: Option<String>,
    /// New title (optional).
    title: Option<String>,
    /// New body (optional).
    body: Option<String>,
},

/// Set PR draft status.
ForgeDraftToggle {
    /// PR number.
    number: u64,
    /// Set to draft (true) or ready (false).
    draft: bool,
},

/// Request reviewers on a PR.
ForgeRequestReviewers {
    /// PR number.
    number: u64,
    /// User logins to request.
    users: Vec<String>,
    /// Team slugs to request.
    teams: Vec<String>,
},

/// Merge a PR via the forge API.
ForgeMergePr {
    /// PR number.
    number: u64,
    /// Merge method.
    method: String, // "merge", "squash", "rebase"
},

/// Fetch from remote.
ForgeFetch {
    /// Remote name.
    remote: String,
    /// Specific refspec (optional, defaults to all).
    refspec: Option<String>,
},
```

#### 6.0.4: Executor Updates for Forge Steps

The executor needs to handle the new forge-related steps. This requires:

1. Access to a `Forge` instance during execution
2. Async execution for forge steps
3. Proper error handling for network failures

**Option A: Forge passed to executor**
```rust
pub struct Executor<'a> {
    git: &'a Git,
    forge: Option<Box<dyn Forge>>,
}

impl<'a> Executor<'a> {
    pub fn with_forge(git: &'a Git, forge: Box<dyn Forge>) -> Self {
        Self { git, forge: Some(forge) }
    }
}
```

**Option B: Forge steps return deferred actions**
```rust
pub enum StepResult {
    Continue,
    Pause { branch: String, git_state: GitState },
    Abort { error: String },
    // New: deferred forge operation
    Deferred { operation: ForgeOperation },
}
```

**Recommendation:** Option A is simpler and matches the architecture where forge operations are part of plan execution. The forge is created during planning and passed to the executor.

---

## Mode Dispatch Pattern

Per SPEC.md §4.6.7, commands like `submit`, `sync`, and `get` have different behavior in bare repos. This requires mode dispatch at the entry point.

### Mode Types

Add to `src/engine/modes.rs` (new file):

```rust
//! engine::modes
//!
//! Mode types for commands with flag-dependent requirements.
//!
//! Per SPEC.md §4.6.7, commands like submit/sync/get behave differently
//! in bare repos vs normal repos. Mode dispatch ensures the correct
//! requirements are used based on flags and repo context.

use super::gate::RequirementSet;
use super::gate::requirements;

/// Submit command mode.
///
/// Per SPEC.md §8E.2:
/// - Default: restack before submit (requires working directory)
/// - --no-restack: bare-repo compatible, requires alignment check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitMode {
    /// Default mode: restack then submit.
    WithRestack,
    /// Bare-repo compatible: no restack, alignment required.
    NoRestack,
}

impl SubmitMode {
    /// Resolve mode from flags and repo context.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Bare repo without --no-restack (requires working directory)
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

    /// Get the requirement set for this mode.
    pub fn requirements(&self) -> &'static RequirementSet {
        match self {
            Self::WithRestack => &requirements::REMOTE,
            Self::NoRestack => &requirements::REMOTE_BARE_ALLOWED,
        }
    }
}

/// Sync command mode.
///
/// Per SPEC.md §8E.3:
/// - Default: may restack after sync (requires working directory)
/// - --no-restack: bare-repo compatible
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Default mode: may restack after sync.
    WithRestack,
    /// Bare-repo compatible: fetch and PR checks only.
    NoRestack,
}

impl SyncMode {
    /// Resolve mode from flags and repo context.
    pub fn resolve(restack: bool, is_bare: bool) -> Result<Self, ModeError> {
        match (restack, is_bare) {
            (false, _) => Ok(Self::NoRestack),
            (true, false) => Ok(Self::WithRestack),
            (true, true) => Err(ModeError::BareRepoRequiresFlag {
                command: "sync",
                required_flag: "--no-restack",
            }),
        }
    }

    pub fn requirements(&self) -> &'static RequirementSet {
        match self {
            Self::WithRestack => &requirements::REMOTE,
            Self::NoRestack => &requirements::REMOTE_BARE_ALLOWED,
        }
    }
}

/// Get command mode.
///
/// Per SPEC.md §8E.4:
/// - Default: fetch and checkout (requires working directory)
/// - --no-checkout: bare-repo compatible
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetMode {
    /// Default mode: fetch and checkout.
    WithCheckout,
    /// Bare-repo compatible: fetch and track only.
    NoCheckout,
}

impl GetMode {
    /// Resolve mode from flags and repo context.
    pub fn resolve(no_checkout: bool, is_bare: bool) -> Result<Self, ModeError> {
        match (no_checkout, is_bare) {
            (true, _) => Ok(Self::NoCheckout),
            (false, false) => Ok(Self::WithCheckout),
            (false, true) => Err(ModeError::BareRepoRequiresFlag {
                command: "get",
                required_flag: "--no-checkout",
            }),
        }
    }

    pub fn requirements(&self) -> &'static RequirementSet {
        match self {
            Self::WithCheckout => &requirements::REMOTE,
            Self::NoCheckout => &requirements::REMOTE_BARE_ALLOWED,
        }
    }
}

/// Errors from mode resolution.
#[derive(Debug, thiserror::Error)]
pub enum ModeError {
    #[error("{command} requires {required_flag} in bare repositories")]
    BareRepoRequiresFlag {
        command: &'static str,
        required_flag: &'static str,
    },
}
```

---

## Command Migration Plans

### Task 6.1: Migrate `submit` Command

**File:** `src/cli/commands/submit.rs`  
**Complexity:** VERY HIGH  
**Current:** ~450 lines, manual scan/gating, inline async logic

#### Current Analysis

The submit command currently:
1. Manually checks requirements with `check_requirements()`
2. Creates tokio runtime and runs `submit_async()`
3. Inside `submit_async()`:
   - Calls `scan()` directly
   - Checks bare repo constraints
   - Gets authentication
   - Iterates branches, pushing and creating/updating PRs

#### Target Architecture

Split into mode-specific command structs:

```rust
/// Submit with default restack behavior.
pub struct SubmitWithRestackCommand<'a> {
    opts: &'a SubmitOptions<'a>,
    forge: Box<dyn Forge>,
}

impl AsyncCommand for SubmitWithRestackCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE;
    type Output = SubmitResult;

    fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a> {
        Box::pin(async move {
            let current = ctx.snapshot.current_branch()
                .ok_or(PlanError::MissingData("current branch".into()))?;
            
            // Determine submit scope
            let branches = compute_submit_scope(&self.opts, &ctx.snapshot, &current)?;
            
            let mut plan = Plan::new(OpId::new(), "submit");
            
            // Phase 1: Restack (if needed)
            for branch in branches.needing_restack() {
                plan = add_restack_steps(plan, branch, &ctx.snapshot)?;
            }
            
            // Phase 2: Push and PR operations
            for branch in &branches {
                // Push
                plan = plan.with_step(PlanStep::ForgePush {
                    branch: branch.to_string(),
                    force: self.opts.force,
                    remote: "origin".to_string(),
                    reason: "submit to remote".to_string(),
                });
                
                // Create or update PR
                let pr_state = ctx.snapshot.pr_state(branch);
                match pr_state {
                    Some(linked) => {
                        plan = plan.with_step(PlanStep::ForgeUpdatePr {
                            number: linked.number,
                            base: Some(determine_pr_base(branch, &ctx.snapshot)),
                            title: None,
                            body: Some(generate_stack_comment(&ctx.snapshot, branch)),
                        });
                    }
                    None => {
                        plan = plan.with_step(PlanStep::ForgeCreatePr {
                            head: branch.to_string(),
                            base: determine_pr_base(branch, &ctx.snapshot),
                            title: branch.to_string(), // Would be commit subject
                            body: Some(generate_stack_comment(&ctx.snapshot, branch)),
                            draft: self.opts.draft,
                        });
                    }
                }
                
                // Link PR in metadata (after creation)
                plan = plan.with_step(PlanStep::WriteMetadataCas {
                    branch: branch.to_string(),
                    old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(branch)?),
                    metadata: Box::new(/* updated with PR linkage */),
                });
            }
            
            Ok(plan)
        })
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<SubmitResult> {
        match result {
            ExecuteResult::Success { .. } => CommandOutput::Success(SubmitResult { /* ... */ }),
            ExecuteResult::Paused { branch, .. } => CommandOutput::Paused {
                message: format!("Conflict while restacking '{}'. Resolve and run 'lattice continue'.", branch),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

/// Submit without restack (bare-repo compatible).
pub struct SubmitNoRestackCommand<'a> {
    opts: &'a SubmitOptions<'a>,
    forge: Box<dyn Forge>,
}

impl AsyncCommand for SubmitNoRestackCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE_BARE_ALLOWED;
    type Output = SubmitResult;

    fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a> {
        Box::pin(async move {
            // Per SPEC.md §4.6.7: check alignment, normalize metadata if needed
            let branches = compute_submit_scope(&self.opts, &ctx.snapshot, &current)?;
            check_alignment(&ctx.snapshot, &branches)?;
            
            let mut plan = Plan::new(OpId::new(), "submit");
            
            // Normalize base metadata if needed (no history rewrite)
            for branch in branches.needing_normalization() {
                plan = plan.with_step(PlanStep::WriteMetadataCas {
                    // Update base to match parent tip
                });
            }
            
            // Push and PR operations only (no restack)
            // ... same as above minus restack steps
            
            Ok(plan)
        })
    }
}

// Entry point with mode dispatch
pub fn submit(ctx: &Context, /* ... args */) -> Result<()> {
    let cwd = ctx.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;
    let is_bare = git.info()?.work_dir.is_none();
    
    // Resolve mode
    let mode = SubmitMode::resolve(opts.no_restack, is_bare)?;
    
    // Get forge (requires auth check first)
    let token = get_github_token()?;
    let remote_url = git.remote_url("origin")?.ok_or_else(|| anyhow!("No origin"))?;
    let forge = create_forge(&remote_url, &token, None)?;
    
    // Create and run appropriate command
    let rt = tokio::runtime::Runtime::new()?;
    match mode {
        SubmitMode::WithRestack => {
            let cmd = SubmitWithRestackCommand { opts: &opts, forge };
            rt.block_on(run_async_command_with_requirements(&cmd, &git, ctx, mode.requirements()))
        }
        SubmitMode::NoRestack => {
            let cmd = SubmitNoRestackCommand { opts: &opts, forge };
            rt.block_on(run_async_command_with_requirements(&cmd, &git, ctx, mode.requirements()))
        }
    }
}
```

#### Acceptance Criteria

- [ ] `SubmitWithRestackCommand` and `SubmitNoRestackCommand` implement `AsyncCommand`
- [ ] Entry point uses mode dispatch
- [ ] Bare repo without `--no-restack` fails with clear error
- [ ] Alignment check for `--no-restack` mode
- [ ] Base metadata normalization for stale but aligned branches
- [ ] PR creation/update via forge steps
- [ ] Stack comments generated and updated
- [ ] Reviewer assignment works
- [ ] Draft toggle works
- [ ] All existing tests pass

---

### Task 6.2: Migrate `sync` Command

**File:** `src/cli/commands/sync.rs`  
**Complexity:** HIGH

#### Target Architecture

```rust
pub struct SyncWithRestackCommand<'a> {
    force: bool,
    forge: Box<dyn Forge>,
}

impl AsyncCommand for SyncWithRestackCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE;
    type Output = SyncResult;

    fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a> {
        Box::pin(async move {
            let mut plan = Plan::new(OpId::new(), "sync");
            
            // Step 1: Fetch
            plan = plan.with_step(PlanStep::ForgeFetch {
                remote: "origin".to_string(),
                refspec: None,
            });
            
            // Step 2: Update trunk (fast-forward or force)
            let trunk = ctx.snapshot.trunk()?;
            // ... trunk update logic
            
            // Step 3: Check PR states
            // ... query forge for merged/closed PRs
            
            // Step 4: Restack
            for branch in branches_needing_restack {
                plan = add_restack_steps(plan, branch, &ctx.snapshot)?;
            }
            
            Ok(plan)
        })
    }
}

pub struct SyncNoRestackCommand<'a> {
    force: bool,
    forge: Box<dyn Forge>,
}

impl AsyncCommand for SyncNoRestackCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE_BARE_ALLOWED;
    // ... fetch, trunk update, PR checks only
}
```

#### Acceptance Criteria

- [ ] `SyncWithRestackCommand` and `SyncNoRestackCommand` implement `AsyncCommand`
- [ ] Mode dispatch at entry point
- [ ] Bare repo refuses without `--no-restack`
- [ ] Fetch works
- [ ] Trunk fast-forward works
- [ ] Force reset works with `--force`
- [ ] PR state checking and reporting works
- [ ] Restack after sync (when enabled)
- [ ] All existing tests pass

---

### Task 6.3: Migrate `get` Command

**File:** `src/cli/commands/get.rs`  
**Complexity:** MEDIUM-HIGH

#### Target Architecture

```rust
pub struct GetWithCheckoutCommand<'a> {
    target: &'a str,
    unfrozen: bool,
    force: bool,
    forge: Box<dyn Forge>,
}

impl AsyncCommand for GetWithCheckoutCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE;
    // ... fetch, track, checkout
}

pub struct GetNoCheckoutCommand<'a> {
    target: &'a str,
    unfrozen: bool,
    force: bool,
    forge: Box<dyn Forge>,
}

impl AsyncCommand for GetNoCheckoutCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE_BARE_ALLOWED;
    // ... fetch, track only, print worktree guidance
}
```

#### Acceptance Criteria

- [ ] `GetWithCheckoutCommand` and `GetNoCheckoutCommand` implement `AsyncCommand`
- [ ] Mode dispatch at entry point
- [ ] Bare repo refuses without `--no-checkout`
- [ ] PR number resolution via forge
- [ ] Branch fetch works
- [ ] Tracking with parent inference
- [ ] Default frozen (--unfrozen to override)
- [ ] Worktree guidance in bare repos
- [ ] All existing tests pass

---

### Task 6.4: Migrate `merge` Command

**File:** `src/cli/commands/merge.rs`  
**Complexity:** MEDIUM

The merge command is simpler because it's API-only (no local mutations beyond metadata).

```rust
pub struct MergeCommand<'a> {
    method: MergeMethod,
    dry_run: bool,
    forge: Box<dyn Forge>,
}

impl AsyncCommand for MergeCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE_BARE_ALLOWED;
    type Output = MergeResult;

    fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a> {
        Box::pin(async move {
            let mut plan = Plan::new(OpId::new(), "merge");
            
            // Get mergeable PRs in stack order
            for branch in mergeable_branches {
                if let Some(pr_number) = ctx.snapshot.pr_number(&branch) {
                    plan = plan.with_step(PlanStep::ForgeMergePr {
                        number: pr_number,
                        method: self.method.to_string(),
                    });
                }
            }
            
            Ok(plan)
        })
    }
}
```

#### Acceptance Criteria

- [ ] `MergeCommand` implements `AsyncCommand`
- [ ] Works in bare repos (API-only)
- [ ] Merge method selection
- [ ] Dry run mode
- [ ] Stack order merging
- [ ] Error handling on first failure
- [ ] All existing tests pass

---

### Task 6.5: Auth Command (No Migration Needed)

**File:** `src/cli/commands/auth.rs`  
**Decision:** Keep as-is

The `auth` command is special:
- No repository requirements
- Pure OAuth flow
- No need for `AsyncCommand` trait

**Rationale:** Auth doesn't interact with repository state at all. It only manages token storage via `SecretStore`. The current implementation is correct and doesn't need to flow through `run_async_command()`.

---

## Executor Updates for Forge Steps

### Forge Step Handling

The executor needs async capabilities to handle forge steps:

```rust
impl<'a> Executor<'a> {
    /// Execute a plan, handling both sync and async steps.
    pub async fn execute_async(
        &self,
        plan: &Plan,
        ctx: &Context,
        forge: Option<&dyn Forge>,
    ) -> Result<ExecuteResult, ExecuteError> {
        for step in &plan.steps {
            match step {
                // Existing sync steps
                PlanStep::RunGit { .. } => { /* existing logic */ }
                PlanStep::WriteMetadataCas { .. } => { /* existing logic */ }
                
                // New forge steps (require async + forge)
                PlanStep::ForgePush { branch, force, remote, .. } => {
                    self.handle_push(branch, *force, remote)?;
                }
                PlanStep::ForgeCreatePr { head, base, title, body, draft } => {
                    let forge = forge.ok_or(ExecuteError::ForgeRequired)?;
                    let pr = forge.create_pr(CreatePrRequest {
                        head: head.clone(),
                        base: base.clone(),
                        title: title.clone(),
                        body: body.clone(),
                        draft: *draft,
                    }).await.map_err(|e| ExecuteError::Forge(e.to_string()))?;
                    // Store PR info for subsequent steps
                }
                PlanStep::ForgeUpdatePr { number, base, title, body } => {
                    let forge = forge.ok_or(ExecuteError::ForgeRequired)?;
                    forge.update_pr(UpdatePrRequest { /* ... */ }).await?;
                }
                PlanStep::ForgeMergePr { number, method } => {
                    let forge = forge.ok_or(ExecuteError::ForgeRequired)?;
                    forge.merge_pr(*number, parse_method(method)).await?;
                }
                // ... other forge steps
            }
        }
    }
    
    fn handle_push(&self, branch: &str, force: bool, remote: &str) -> Result<(), ExecuteError> {
        let mut args = vec!["push"];
        if force {
            args.push("--force-with-lease");
        }
        args.push(remote);
        args.push(branch);
        
        let result = self.git.run_command(args)?;
        if !result.success {
            return Err(ExecuteError::Push {
                branch: branch.to_string(),
                error: result.stderr,
            });
        }
        Ok(())
    }
}
```

---

## Testing Strategy

### Unit Tests

1. **Mode resolution tests**
   - `SubmitMode::resolve(no_restack=false, is_bare=true)` → Error
   - `SubmitMode::resolve(no_restack=true, is_bare=true)` → NoRestack
   - Same for SyncMode, GetMode

2. **Plan generation tests**
   - Verify forge steps are generated correctly
   - Verify stack comments are included
   - Verify metadata updates include PR linkage

3. **AsyncCommand trait tests**
   - Mock forge for deterministic testing
   - Test plan generation with various scenarios

### Integration Tests

1. **Submit flow**
   - Create stack, submit, verify PRs created
   - Re-submit, verify PRs updated
   - --dry-run produces no changes

2. **Sync flow**
   - Fetch updates trunk
   - Merged PRs detected
   - Restack after sync

3. **Get flow**
   - Fetch by PR number
   - Fetch by branch name
   - --no-checkout in bare repo

4. **Bare repo tests**
   - Submit refuses without --no-restack
   - Submit with --no-restack checks alignment
   - Sync refuses without --no-restack
   - Get refuses without --no-checkout

### Mock Forge Tests

```rust
#[tokio::test]
async fn submit_creates_prs() {
    let mock = MockForge::new()
        .expect_create_pr("feature", "main")
        .returning(PullRequest { number: 1, .. });
    
    let cmd = SubmitNoRestackCommand { forge: Box::new(mock), .. };
    let result = run_async_command(&cmd, &git, &ctx).await;
    
    assert!(result.is_ok());
}
```

---

## Implementation Order

| Order | Task | Description | Depends On |
|-------|------|-------------|------------|
| 1 | 6.0.1 | `AsyncCommand` trait | - |
| 2 | 6.0.2 | `run_async_command()` | 6.0.1 |
| 3 | 6.0.3 | Forge `PlanStep` variants | 6.0.1 |
| 4 | 6.0.4 | Executor forge step handling | 6.0.3 |
| 5 | Mode types | `SubmitMode`, `SyncMode`, `GetMode` | - |
| 6 | 6.4 | Migrate `merge` (simplest) | 6.0.1-6.0.4 |
| 7 | 6.3 | Migrate `get` | 6.0.1-6.0.4, Mode types |
| 8 | 6.2 | Migrate `sync` | 6.0.1-6.0.4, Mode types |
| 9 | 6.1 | Migrate `submit` (most complex) | All above |
| 10 | Tests | Integration and mock forge tests | All above |

---

## Verification Checklist

Before marking Phase 6 complete:

- [ ] `AsyncCommand` trait defined and documented
- [ ] `run_async_command()` and variants implemented
- [ ] Forge `PlanStep` variants added
- [ ] Executor handles forge steps
- [ ] `SubmitMode`, `SyncMode`, `GetMode` implemented
- [ ] `submit` implements `AsyncCommand` (both modes)
- [ ] `sync` implements `AsyncCommand` (both modes)
- [ ] `get` implements `AsyncCommand` (both modes)
- [ ] `merge` implements `AsyncCommand`
- [ ] No direct `scan()` calls in async commands
- [ ] Mode dispatch at all entry points
- [ ] Bare repo errors are clear and actionable
- [ ] All existing tests pass
- [ ] New integration tests for async flows
- [ ] Mock forge tests for deterministic testing
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] Engine hooks fire for all async commands

---

## Risk Areas

1. **Async executor complexity** - The executor becomes async, affecting all callers
2. **Forge error handling** - Network failures during execution need careful handling
3. **Plan mutation after forge calls** - PR numbers from creation need to be captured for metadata
4. **Token refresh during execution** - Long-running submits may need token refresh mid-operation

**Mitigations:**
- Start with `merge` (simplest async command)
- Use mock forge for initial development
- Consider two-phase execution: local first, then remote
- Token refresh should be transparent via `TokenProvider`

---

## References

- **ARCHITECTURE.md Section 5-6, 11, 12** - Command lifecycle, Forge adapter
- **SPEC.md Section 4.6.7** - Bare repo policy for submit/sync/get
- **SPEC.md Section 8E** - Remote and PR integration
- **HANDOFF.md Phase 6** - Original migration spec
- **Phase 5 PLAN.md** - Reference implementation patterns
