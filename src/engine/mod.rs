//! engine
//!
//! Orchestrates the command lifecycle: Scan -> Gate -> Plan -> Execute -> Verify.
//!
//! # Architecture
//!
//! The engine is the central coordinator for all Lattice commands. It enforces
//! the validated execution model defined in ARCHITECTURE.md:
//!
//! 1. **Scan**: Read repository state, detect issues, compute capabilities
//! 2. **Gate**: Verify command requirements are satisfied
//! 3. **Plan**: Generate a deterministic, previewable plan
//! 4. **Execute**: Apply the plan through the single transactional executor
//! 5. **Verify**: Confirm invariants hold after execution
//!
//! # Command Lifecycle
//!
//! Every command follows a uniform lifecycle enforced by the engine:
//!
//! ```text
//! Scan -> Gate -> [Repair if needed] -> Plan -> Execute -> Verify
//! ```
//!
//! If gating fails, control transfers to the Doctor for explicit repair.
//! The engine never guesses or auto-repairs.
//!
//! # Invariants
//!
//! - Commands execute only against validated execution models
//! - The engine never performs mutations directly; all flow through the Executor
//! - If gating fails, a RepairBundle is produced for Doctor
//! - Verification failure after execution indicates a bug
//!
//! # Example
//!
//! ```ignore
//! use latticework::engine::{scan, gate, execute_command};
//! use latticework::engine::gate::requirements;
//!
//! let snapshot = scan::scan(&git)?;
//!
//! match gate::gate(snapshot, &requirements::MUTATING) {
//!     GateResult::Ready(ctx) => {
//!         let plan = my_command.plan(&ctx)?;
//!         let result = executor.execute(&plan, &context)?;
//!         verify::fast_verify(&git, &ctx.snapshot)?;
//!     }
//!     GateResult::NeedsRepair(bundle) => {
//!         // Hand off to Doctor
//!     }
//! }
//! ```

pub mod capabilities;
pub mod command;
pub mod exec;
pub mod gate;
pub mod health;
pub mod ledger;
pub mod modes;
pub mod plan;
pub mod rollback;
pub mod runner;
pub mod scan;
pub mod verify;

// Test-only hooks for fault injection and drift testing.
// Per ROADMAP.md Anti-Drift Mechanisms item 5: "Test-only pause hook in Engine"
// Available under: cfg(test) for unit tests, or feature = "test_hooks"/"fault_injection" for integration tests
#[cfg(any(test, feature = "fault_injection", feature = "test_hooks"))]
pub mod engine_hooks;

// Re-exports for convenience
pub use capabilities::{Capability, CapabilitySet};
pub use command::{Command, CommandOutput, ReadOnlyCommand, SimpleCommand};
pub use exec::{ExecuteError, ExecuteResult, Executor};
pub use gate::{
    check_frozen_policy, compute_freeze_scope, compute_stack_scope, gate, GateResult, ReadyContext,
    RepairBundle, RequirementSet, ValidatedData,
};
pub use health::{Issue, IssueId, RepoHealthReport, Severity};
pub use ledger::{Event, EventLedger, LedgerError};
pub use modes::{GetMode, ModeError, SubmitMode, SyncMode};
pub use plan::{Plan, PlanError, PlanStep};
pub use rollback::{rollback_journal, RollbackError, RollbackResult};
pub use runner::{
    check_requirements, run_command, run_command_with_requirements,
    run_command_with_requirements_and_scope, run_command_with_scope, run_gated,
    run_readonly_command, RunError,
};
pub use scan::{scan, DivergenceInfo, RepoSnapshot, ScanError, ScannedMetadata};
pub use verify::{fast_verify, VerifyError};

use std::path::PathBuf;

use anyhow::Result;

use crate::git::Git;

/// Execution context for commands.
///
/// Contains global settings derived from CLI flags that affect command behavior.
#[derive(Debug, Clone)]
pub struct Context {
    /// Working directory override.
    pub cwd: Option<PathBuf>,
    /// Debug logging enabled.
    pub debug: bool,
    /// Quiet mode (minimal output).
    pub quiet: bool,
    /// Interactive mode enabled.
    pub interactive: bool,
    /// Git hook verification enabled.
    /// When false, git commands are invoked with --no-verify.
    pub verify: bool,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            cwd: None,
            debug: false,
            quiet: false,
            interactive: true,
            verify: true,
        }
    }
}

/// Errors from engine operations.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Scan failed.
    #[error("scan failed: {0}")]
    Scan(#[from] ScanError),

    /// Gating failed (needs repair).
    #[error("gating failed: {0}")]
    GateFailed(String),

    /// Planning failed.
    #[error("planning failed: {0}")]
    Plan(#[from] PlanError),

    /// Execution failed.
    #[error("execution failed: {0}")]
    Execute(#[from] ExecuteError),

    /// Verification failed.
    #[error("verification failed: {0}")]
    Verify(#[from] VerifyError),

    /// Git error.
    #[error("git error: {0}")]
    Git(#[from] crate::git::GitError),
}

/// Run a full lifecycle for a command.
///
/// This is the generic entry point for executing commands through
/// the engine lifecycle. It handles scan, gate, plan, execute, and verify.
///
/// # Type Parameters
///
/// * `C` - The command type (must implement Command trait)
///
/// # Arguments
///
/// * `command` - The command to execute
/// * `git` - Git interface
/// * `ctx` - Execution context
///
/// # Returns
///
/// The command's output on success.
pub fn run_lifecycle<F, T>(
    git: &Git,
    ctx: &Context,
    requirements: &RequirementSet,
    plan_fn: F,
) -> Result<T, EngineError>
where
    F: FnOnce(&ReadyContext) -> Result<(Plan, T), PlanError>,
{
    // 1. Scan
    let snapshot = scan::scan(git)?;

    // 2. Gate
    let ready = match gate::gate(snapshot, requirements) {
        GateResult::Ready(ctx) => ctx,
        GateResult::NeedsRepair(bundle) => {
            return Err(EngineError::GateFailed(bundle.summary()));
        }
    };

    // 3. Plan
    let (plan, output) = plan_fn(&ready)?;

    // 4. Execute
    let executor = Executor::new(git);
    let result = executor.execute(&plan, ctx)?;

    match result {
        ExecuteResult::Success { .. } => {}
        ExecuteResult::Paused { branch, .. } => {
            return Err(EngineError::Execute(ExecuteError::Internal(format!(
                "paused for conflict on {}",
                branch
            ))));
        }
        ExecuteResult::Aborted { error, .. } => {
            return Err(EngineError::Execute(ExecuteError::Internal(error)));
        }
    }

    // 5. Verify (re-scan and verify)
    let post_snapshot = scan::scan(git)?;
    verify::fast_verify(git, &post_snapshot)?;

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod context {
        use super::*;

        #[test]
        fn default_values() {
            let ctx = Context::default();
            assert!(ctx.cwd.is_none());
            assert!(!ctx.debug);
            assert!(!ctx.quiet);
            assert!(ctx.interactive);
            assert!(ctx.verify);
        }

        #[test]
        fn custom_values() {
            let ctx = Context {
                cwd: Some(PathBuf::from("/custom")),
                debug: true,
                quiet: true,
                interactive: false,
                verify: false,
            };
            assert_eq!(ctx.cwd, Some(PathBuf::from("/custom")));
            assert!(ctx.debug);
            assert!(ctx.quiet);
            assert!(!ctx.interactive);
            assert!(!ctx.verify);
        }
    }

    mod engine_error {
        use super::*;

        #[test]
        fn display_formatting() {
            let err = EngineError::GateFailed("missing capabilities".to_string());
            assert!(err.to_string().contains("gating failed"));

            let err = EngineError::Plan(PlanError::InvalidState("bad".to_string()));
            assert!(err.to_string().contains("planning failed"));
        }
    }

    mod re_exports {
        use super::*;

        #[test]
        fn capability_accessible() {
            let _ = Capability::RepoOpen;
        }

        #[test]
        fn capability_set_accessible() {
            let _ = CapabilitySet::new();
        }

        #[test]
        fn issue_accessible() {
            let _ = Issue::new("test", Severity::Info, "msg");
        }

        #[test]
        fn plan_accessible() {
            use crate::core::ops::journal::OpId;
            let _ = Plan::new(OpId::new(), "test");
        }

        #[test]
        fn plan_step_accessible() {
            let _ = PlanStep::Checkpoint {
                name: "test".to_string(),
            };
        }
    }
}
