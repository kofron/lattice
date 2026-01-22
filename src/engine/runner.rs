//! engine::runner
//!
//! Engine runner - the single entry point for command execution.
//!
//! # Architecture
//!
//! Per ARCHITECTURE.md Section 12, this module enforces the command lifecycle:
//!
//! ```text
//! Scan -> Gate -> [Repair if needed] -> Plan -> Execute -> Verify -> Return
//! ```
//!
//! **Key principle:** Commands cannot call `scan()` directly. All command
//! execution flows through `run_command()`, which ensures gating is enforced.
//!
//! # Invariants
//!
//! - Commands receive `ReadyContext`, never raw `RepoSnapshot`
//! - Gating is always performed before planning
//! - If gating fails, a `RepairBundle` is returned (not silent failure)
//! - The executor is the only component that mutates the repository
//!
//! # Example
//!
//! ```ignore
//! use latticework::engine::runner::{run_command, run_command_with_scope};
//! use latticework::engine::command::Command;
//!
//! // Simple command without scope
//! let result = run_command(&my_command, &git, &ctx)?;
//!
//! // Command with scope resolution
//! let result = run_command_with_scope(&my_command, &git, &ctx, Some(&target_branch))?;
//! ```

use super::command::{Command, CommandOutput};
use super::exec::{ExecuteResult, Executor};
use super::gate::{gate, gate_with_scope, GateResult, RepairBundle, RequirementSet};
use super::plan::Plan;
use super::scan::{scan, scan_with_remote};
use super::Context;
use crate::core::types::BranchName;
use crate::git::Git;
use thiserror::Error;

#[cfg(any(test, feature = "fault_injection", feature = "test_hooks"))]
use super::engine_hooks;

/// Errors from the engine runner.
#[derive(Debug, Error)]
pub enum RunError {
    /// Scan failed.
    #[error("scan failed: {0}")]
    Scan(#[from] super::scan::ScanError),

    /// Gating failed - needs repair.
    #[error("gating failed: {0}")]
    NeedsRepair(RepairBundle),

    /// Planning failed.
    #[error("planning failed: {0}")]
    Plan(#[from] super::plan::PlanError),

    /// Execution failed.
    #[error("execution failed: {0}")]
    Execute(#[from] super::exec::ExecuteError),

    /// Verification failed.
    #[error("verification failed: {0}")]
    Verify(#[from] super::verify::VerifyError),
}

impl RunError {
    /// Check if this is a repair-needed error.
    pub fn is_needs_repair(&self) -> bool {
        matches!(self, RunError::NeedsRepair(_))
    }

    /// Get the repair bundle if this is a NeedsRepair error.
    pub fn into_repair_bundle(self) -> Option<RepairBundle> {
        match self {
            RunError::NeedsRepair(bundle) => Some(bundle),
            _ => None,
        }
    }
}

/// Run a command through the full lifecycle.
///
/// This is the primary entry point for command execution. It enforces
/// the complete lifecycle: Scan -> Gate -> Plan -> Execute -> Verify.
///
/// # Type Parameters
///
/// * `C` - The command type (must implement `Command`)
///
/// # Arguments
///
/// * `command` - The command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
///
/// # Returns
///
/// `CommandOutput<C::Output>` on success, or `RunError` on failure.
///
/// # Lifecycle
///
/// 1. **Scan**: Read repository state
/// 2. **Gate**: Verify requirements using `C::REQUIREMENTS`
/// 3. **Plan**: Call `command.plan()` with validated context
/// 4. **Execute**: Apply plan through executor
/// 5. **Verify**: Re-scan and verify invariants
/// 6. **Finish**: Call `command.finish()` with result
///
/// If gating fails, returns `RunError::NeedsRepair` with a bundle
/// for the Doctor to handle.
pub fn run_command<C: Command>(
    command: &C,
    git: &Git,
    ctx: &Context,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_command_internal(command, git, ctx, C::REQUIREMENTS, None)
}

/// Run a command with scope resolution.
///
/// Like `run_command`, but also resolves the target scope for commands
/// that operate on a branch and its stack (like restack, submit).
///
/// # Arguments
///
/// * `command` - The command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
/// * `target` - Target branch for scope resolution (None = current branch)
///
/// # Scope Resolution
///
/// The scope includes:
/// - The target branch
/// - All ancestors up to trunk
///
/// This scope is available in `ReadyContext.data` as `ValidatedData::StackScope`.
pub fn run_command_with_scope<C: Command>(
    command: &C,
    git: &Git,
    ctx: &Context,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_command_internal(command, git, ctx, C::REQUIREMENTS, target)
}

/// Run a command with explicit requirements.
///
/// This is useful for commands with mode-dependent requirements (like submit).
/// Instead of using `C::REQUIREMENTS`, the caller provides the requirements.
///
/// # Arguments
///
/// * `command` - The command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
/// * `requirements` - Requirement set to use for gating
///
/// # Example
///
/// ```ignore
/// let mode = SubmitMode::resolve(args.no_restack, is_bare)?;
/// let result = run_command_with_requirements(&cmd, &git, &ctx, mode.requirements())?;
/// ```
pub fn run_command_with_requirements<C: Command>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &'static RequirementSet,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_command_internal(command, git, ctx, requirements, None)
}

/// Run a command with explicit requirements and scope.
///
/// Combines `run_command_with_scope` and `run_command_with_requirements`.
pub fn run_command_with_requirements_and_scope<C: Command>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &'static RequirementSet,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_command_internal(command, git, ctx, requirements, target)
}

/// Internal implementation of command running.
fn run_command_internal<C: Command>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &RequirementSet,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    if ctx.debug {
        eprintln!("[debug] Starting command lifecycle");
        eprintln!("[debug] Requirements: {}", requirements.name);
    }

    // Step 1: Scan
    if ctx.debug {
        eprintln!("[debug] Step 1: Scan");
    }
    let snapshot = scan(git)?;

    // Step 2: Gate
    if ctx.debug {
        eprintln!("[debug] Step 2: Gate");
    }
    let ready = if target.is_some() {
        match gate_with_scope(snapshot, requirements, target) {
            GateResult::Ready(ctx) => *ctx,
            GateResult::NeedsRepair(bundle) => {
                if ctx.debug {
                    eprintln!(
                        "[debug] Gating failed: {} missing capabilities",
                        bundle.missing_capabilities.len()
                    );
                }
                return Err(RunError::NeedsRepair(bundle));
            }
        }
    } else {
        match gate(snapshot, requirements) {
            GateResult::Ready(ctx) => *ctx,
            GateResult::NeedsRepair(bundle) => {
                if ctx.debug {
                    eprintln!(
                        "[debug] Gating failed: {} missing capabilities",
                        bundle.missing_capabilities.len()
                    );
                }
                return Err(RunError::NeedsRepair(bundle));
            }
        }
    };

    // Step 3: Plan
    if ctx.debug {
        eprintln!("[debug] Step 3: Plan");
    }
    let plan = command.plan(&ready)?;

    if ctx.debug {
        eprintln!("[debug] Plan has {} steps", plan.step_count());
    }

    // Early return for empty plans
    if plan.is_empty() {
        if ctx.debug {
            eprintln!("[debug] Empty plan, skipping execution");
        }
        let result = ExecuteResult::Success {
            fingerprint: ready.snapshot.fingerprint.clone(),
        };
        return Ok(command.finish(result));
    }

    // Test hook: Allows drift harness to inject out-of-band mutations between
    // planning (which captures expected OIDs) and execution (which validates
    // them with CAS). Per ROADMAP.md Anti-Drift Mechanisms item 5.
    // No-op in production builds.
    #[cfg(any(test, feature = "fault_injection", feature = "test_hooks"))]
    {
        if let Ok(info) = git.info() {
            engine_hooks::invoke_before_execute(&info);
        }
    }

    // Step 3.5: Pre-execution occupancy check (nice UX before acquiring lock)
    // Per SPEC.md §4.6.8, refuse if any touched branch is checked out elsewhere
    if plan.touches_branch_refs() {
        if ctx.debug {
            eprintln!("[debug] Step 3.5: Pre-execution occupancy check");
        }
        check_occupancy_for_plan(git, &plan)?;
    }

    // Step 4: Execute
    // Note: Executor also revalidates occupancy under lock and runs post-verification
    // per ARCHITECTURE.md §6.2 (self-enforcing)
    if ctx.debug {
        eprintln!("[debug] Step 4: Execute");
    }
    let executor = Executor::new(git);
    let result = executor.execute(&plan, ctx)?;

    // Step 5: Finish
    // Note: Verification is now inside executor (self-enforcing per ARCHITECTURE.md §6.2)
    if ctx.debug {
        eprintln!("[debug] Step 5: Finish");
    }
    Ok(command.finish(result))
}

/// Run a simple function through gating without the full Command trait.
///
/// This is a convenience function for cases where a full Command implementation
/// is overkill. It handles scan and gate, then calls the provided function.
///
/// # Arguments
///
/// * `git` - Git interface
/// * `ctx` - Execution context
/// * `requirements` - Requirements to check
/// * `f` - Function to run with validated context
///
/// # Returns
///
/// The result of `f`, or a `RunError` if gating fails.
///
/// # Example
///
/// ```ignore
/// let result = run_gated(&git, &ctx, &requirements::READ_ONLY, |ready| {
///     // Access validated snapshot
///     println!("Trunk: {:?}", ready.trunk());
///     Ok("done")
/// })?;
/// ```
pub fn run_gated<T, F>(
    git: &Git,
    _ctx: &Context,
    requirements: &RequirementSet,
    f: F,
) -> Result<T, RunError>
where
    F: FnOnce(&super::gate::ReadyContext) -> Result<T, RunError>,
{
    // Scan
    let snapshot = scan(git)?;

    // Gate
    let ready = match gate(snapshot, requirements) {
        GateResult::Ready(ctx) => *ctx,
        GateResult::NeedsRepair(bundle) => {
            return Err(RunError::NeedsRepair(bundle));
        }
    };

    // Run function
    f(&ready)
}

/// Run a read-only command through the engine lifecycle.
///
/// This is the entry point for commands that implement `ReadOnlyCommand`.
/// Unlike `run_command`, this does not involve the executor or plan/journal
/// infrastructure since read-only commands do not mutate repository state.
///
/// # Lifecycle
///
/// 1. **Scan**: Read repository state
/// 2. **Gate**: Verify requirements using `C::REQUIREMENTS`
/// 3. **Execute**: Call `command.execute()` with validated context
///
/// # Type Parameters
///
/// * `C` - The command type (must implement `ReadOnlyCommand`)
///
/// # Arguments
///
/// * `command` - The read-only command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
///
/// # Returns
///
/// `C::Output` on success, or `RunError` on failure.
///
/// # Example
///
/// ```ignore
/// use latticework::engine::runner::run_readonly_command;
/// use latticework::engine::command::ReadOnlyCommand;
///
/// struct LogCommand { all: bool }
///
/// impl ReadOnlyCommand for LogCommand {
///     const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
///     type Output = String;
///
///     fn execute(&self, ctx: &ReadyContext) -> Result<String, PlanError> {
///         Ok(format_log(&ctx.snapshot))
///     }
/// }
///
/// let cmd = LogCommand { all: true };
/// let output = run_readonly_command(&cmd, &git, &ctx)?;
/// ```
pub fn run_readonly_command<C: super::command::ReadOnlyCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
) -> Result<C::Output, RunError> {
    if ctx.debug {
        eprintln!("[debug] Starting read-only command lifecycle");
        eprintln!("[debug] Requirements: {}", C::REQUIREMENTS.name);
    }

    // Step 1: Scan
    if ctx.debug {
        eprintln!("[debug] Step 1: Scan");
    }
    let snapshot = scan(git)?;

    // Step 2: Gate
    if ctx.debug {
        eprintln!("[debug] Step 2: Gate");
    }
    let ready = match gate(snapshot, C::REQUIREMENTS) {
        GateResult::Ready(ctx) => *ctx,
        GateResult::NeedsRepair(bundle) => {
            if ctx.debug {
                eprintln!(
                    "[debug] Gating failed: {} missing capabilities",
                    bundle.missing_capabilities.len()
                );
            }
            return Err(RunError::NeedsRepair(bundle));
        }
    };

    // Step 3: Execute (read-only, no planning or executor needed)
    if ctx.debug {
        eprintln!("[debug] Step 3: Execute read-only command");
    }
    command.execute(&ready).map_err(RunError::Plan)
}

// ============================================================================
// Async Command Runners
// ============================================================================

/// Run an async command through the full lifecycle.
///
/// This is the entry point for commands that implement `AsyncCommand`.
/// It handles the async planning phase and follows the same lifecycle as
/// synchronous commands: Scan -> Gate -> Plan -> Execute -> Verify.
///
/// # Lifecycle
///
/// 1. **Scan**: Read repository state
/// 2. **Gate**: Verify requirements using `C::REQUIREMENTS`
/// 3. **Plan (async)**: Call `command.plan()` with validated context
/// 4. **Execute**: Apply plan through executor
/// 5. **Verify**: Re-scan and verify invariants
/// 6. **Finish**: Call `command.finish()` with result
///
/// # Type Parameters
///
/// * `C` - The command type (must implement `AsyncCommand`)
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
///
/// # Example
///
/// ```ignore
/// use latticework::engine::runner::run_async_command;
/// use latticework::engine::command::AsyncCommand;
///
/// let cmd = MergeCommand { method: MergeMethod::Squash };
/// let rt = tokio::runtime::Runtime::new()?;
/// let result = rt.block_on(run_async_command(&cmd, &git, &ctx))?;
/// ```
pub async fn run_async_command<C: super::command::AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_async_command_internal(command, git, ctx, C::REQUIREMENTS, None).await
}

/// Run an async command with explicit requirements.
///
/// This is useful for commands with mode-dependent requirements (like submit).
/// Instead of using `C::REQUIREMENTS`, the caller provides the requirements.
///
/// # Arguments
///
/// * `command` - The async command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
/// * `requirements` - Requirement set to use for gating
///
/// # Example
///
/// ```ignore
/// let mode = SubmitMode::resolve(args.no_restack, is_bare)?;
/// let rt = tokio::runtime::Runtime::new()?;
/// let result = rt.block_on(run_async_command_with_requirements(
///     &cmd, &git, &ctx, mode.requirements()
/// ))?;
/// ```
pub async fn run_async_command_with_requirements<C: super::command::AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &'static RequirementSet,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_async_command_internal(command, git, ctx, requirements, None).await
}

/// Run an async command with scope resolution.
///
/// Like `run_async_command`, but also resolves the target scope for commands
/// that operate on a branch and its stack (like submit).
///
/// # Arguments
///
/// * `command` - The async command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
/// * `target` - Target branch for scope resolution (None = current branch)
///
/// # Scope Resolution
///
/// The scope includes:
/// - The target branch
/// - All ancestors up to trunk
///
/// This scope is available in `ReadyContext.data` as `ValidatedData::StackScope`.
pub async fn run_async_command_with_scope<C: super::command::AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_async_command_internal(command, git, ctx, C::REQUIREMENTS, target).await
}

/// Run an async command with explicit requirements and scope.
///
/// Combines `run_async_command_with_scope` and `run_async_command_with_requirements`.
pub async fn run_async_command_with_requirements_and_scope<C: super::command::AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &'static RequirementSet,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    run_async_command_internal(command, git, ctx, requirements, target).await
}

/// Internal implementation of async command running.
async fn run_async_command_internal<C: super::command::AsyncCommand>(
    command: &C,
    git: &Git,
    ctx: &Context,
    requirements: &RequirementSet,
    target: Option<&BranchName>,
) -> Result<CommandOutput<C::Output>, RunError> {
    if ctx.debug {
        eprintln!("[debug] Starting async command lifecycle");
        eprintln!("[debug] Requirements: {}", requirements.name);
    }

    // Step 1: Scan (async version for remote capability checks)
    // Use scan_with_remote() since we're in an async context - this enables
    // RepoAuthorized capability checking which requires async API calls.
    if ctx.debug {
        eprintln!("[debug] Step 1: Async Scan (with remote capabilities)");
    }
    let snapshot = scan_with_remote(git).await?;

    // Step 2: Gate
    if ctx.debug {
        eprintln!("[debug] Step 2: Gate");
    }
    let ready = if target.is_some() {
        match gate_with_scope(snapshot, requirements, target) {
            GateResult::Ready(ctx) => *ctx,
            GateResult::NeedsRepair(bundle) => {
                if ctx.debug {
                    eprintln!(
                        "[debug] Gating failed: {} missing capabilities",
                        bundle.missing_capabilities.len()
                    );
                }
                return Err(RunError::NeedsRepair(bundle));
            }
        }
    } else {
        match gate(snapshot, requirements) {
            GateResult::Ready(ctx) => *ctx,
            GateResult::NeedsRepair(bundle) => {
                if ctx.debug {
                    eprintln!(
                        "[debug] Gating failed: {} missing capabilities",
                        bundle.missing_capabilities.len()
                    );
                }
                return Err(RunError::NeedsRepair(bundle));
            }
        }
    };

    // Step 3: Plan (async)
    if ctx.debug {
        eprintln!("[debug] Step 3: Async Plan");
    }
    let plan = command.plan(&ready).await?;

    if ctx.debug {
        eprintln!("[debug] Plan has {} steps", plan.step_count());
    }

    // Early return for empty plans
    if plan.is_empty() {
        if ctx.debug {
            eprintln!("[debug] Empty plan, skipping execution");
        }
        let result = ExecuteResult::Success {
            fingerprint: ready.snapshot.fingerprint.clone(),
        };
        return Ok(command.finish(result));
    }

    // Test hook: Allows drift harness to inject out-of-band mutations between
    // planning (which captures expected OIDs) and execution (which validates
    // them with CAS). Per ROADMAP.md Anti-Drift Mechanisms item 5.
    // No-op in production builds.
    #[cfg(any(test, feature = "fault_injection", feature = "test_hooks"))]
    {
        if let Ok(info) = git.info() {
            engine_hooks::invoke_before_execute(&info);
        }
    }

    // Step 3.5: Pre-execution occupancy check (nice UX before acquiring lock)
    // Per SPEC.md §4.6.8, refuse if any touched branch is checked out elsewhere
    if plan.touches_branch_refs() {
        if ctx.debug {
            eprintln!("[debug] Step 3.5: Pre-execution occupancy check");
        }
        check_occupancy_for_plan(git, &plan)?;
    }

    // Step 4: Execute
    // Note: Executor also revalidates occupancy under lock and runs post-verification
    // per ARCHITECTURE.md §6.2 (self-enforcing)
    if ctx.debug {
        eprintln!("[debug] Step 4: Execute");
    }
    let executor = Executor::new(git);
    let result = executor.execute(&plan, ctx)?;

    // Step 5: Finish
    // Note: Verification is now inside executor (self-enforcing per ARCHITECTURE.md §6.2)
    if ctx.debug {
        eprintln!("[debug] Step 5: Finish");
    }
    Ok(command.finish(result))
}

/// Check worktree occupancy for a plan before execution.
///
/// This is the "nice UX" check before acquiring the lock. It provides
/// actionable guidance to the user (branch X is checked out in worktree Y).
/// The executor also revalidates under lock for correctness.
///
/// Per SPEC.md §4.6.8, commands that would mutate a branch checked out in
/// another worktree must be refused with clear guidance.
///
/// # Arguments
///
/// * `git` - Git interface
/// * `plan` - The plan to check for occupancy conflicts
///
/// # Returns
///
/// `Ok(())` if no conflicts, `Err(RunError::NeedsRepair)` with guidance if conflicts exist.
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
        let bundle = RepairBundle {
            command: plan.command.clone(),
            missing_capabilities: vec![],
            blocking_issues: vec![issue],
        };
        return Err(RunError::NeedsRepair(bundle));
    }

    Ok(())
}

/// Check if a command's requirements would be satisfied.
///
/// This is useful for pre-flight checks before running a command,
/// or for determining which commands are available in the current state.
///
/// # Arguments
///
/// * `git` - Git interface
/// * `requirements` - Requirements to check
///
/// # Returns
///
/// `Ok(())` if requirements are satisfied, or the `RepairBundle` if not.
pub fn check_requirements(git: &Git, requirements: &RequirementSet) -> Result<(), RepairBundle> {
    let snapshot = scan(git).map_err(|e| RepairBundle {
        command: requirements.name.to_string(),
        missing_capabilities: vec![],
        blocking_issues: vec![super::health::Issue::new(
            "scan-failed",
            super::health::Severity::Blocking,
            format!("Failed to scan repository: {}", e),
        )],
    })?;

    match gate(snapshot, requirements) {
        GateResult::Ready(_) => Ok(()),
        GateResult::NeedsRepair(bundle) => Err(bundle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod run_error {
        use super::*;
        use crate::engine::capabilities::Capability;

        #[test]
        fn is_needs_repair() {
            let bundle = RepairBundle {
                command: "test".to_string(),
                missing_capabilities: vec![Capability::RepoOpen],
                blocking_issues: vec![],
            };
            let err = RunError::NeedsRepair(bundle);
            assert!(err.is_needs_repair());
        }

        #[test]
        fn into_repair_bundle() {
            let bundle = RepairBundle {
                command: "test".to_string(),
                missing_capabilities: vec![Capability::RepoOpen],
                blocking_issues: vec![],
            };
            let err = RunError::NeedsRepair(bundle);
            let extracted = err.into_repair_bundle();
            assert!(extracted.is_some());
            assert_eq!(extracted.unwrap().command, "test");
        }

        #[test]
        fn into_repair_bundle_none_for_other_errors() {
            let err = RunError::Plan(crate::engine::PlanError::InvalidState("test".to_string()));
            assert!(err.into_repair_bundle().is_none());
        }

        #[test]
        fn display_formatting() {
            let bundle = RepairBundle {
                command: "test".to_string(),
                missing_capabilities: vec![Capability::RepoOpen],
                blocking_issues: vec![],
            };
            let err = RunError::NeedsRepair(bundle);
            let msg = err.to_string();
            assert!(msg.contains("gating failed"));
        }
    }
}
