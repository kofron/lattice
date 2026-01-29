//! engine::command
//!
//! Command trait for lifecycle integration.
//!
//! # Architecture
//!
//! Per ARCHITECTURE.md Section 5, every command must implement the `Command`
//! trait to participate in the validated execution model. This ensures that:
//!
//! 1. Commands declare their required capabilities statically
//! 2. Commands receive validated context (not raw snapshots)
//! 3. Commands generate plans deterministically
//! 4. The engine enforces the lifecycle uniformly
//!
//! # Invariants
//!
//! - Commands cannot call `scan()` directly (module visibility enforces this)
//! - Commands only receive `ReadyContext` after gating passes
//! - The `plan()` method must be pure (no I/O, no mutations)
//!
//! # Example
//!
//! ```ignore
//! use latticework::engine::command::{Command, CommandOutput};
//! use latticework::engine::gate::{RequirementSet, ReadyContext, requirements};
//! use latticework::engine::plan::{Plan, PlanError};
//! use latticework::engine::exec::ExecuteResult;
//!
//! struct MyCommand {
//!     target: Option<String>,
//! }
//!
//! impl Command for MyCommand {
//!     const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
//!     type Output = ();
//!
//!     fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
//!         // Generate plan from validated context
//!         // ctx.snapshot is guaranteed to satisfy REQUIREMENTS
//!         todo!()
//!     }
//!
//!     fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
//!         match result {
//!             ExecuteResult::Success { .. } => CommandOutput::Success(()),
//!             ExecuteResult::Paused { branch, .. } => {
//!                 CommandOutput::Paused { message: format!("Paused on {}", branch) }
//!             }
//!             ExecuteResult::Aborted { error, .. } => {
//!                 CommandOutput::Failed { error }
//!             }
//!         }
//!     }
//! }
//! ```

use std::future::Future;
use std::pin::Pin;

use super::exec::ExecuteResult;
use super::gate::{ReadyContext, RequirementSet};
use super::plan::{Plan, PlanError};

/// Type alias for async plan futures.
///
/// This represents the return type of `AsyncCommand::plan()`, which is an
/// async function that produces a `Plan`. The lifetime `'a` ties the future
/// to the command and context references.
pub type PlanFut<'a> = Pin<Box<dyn Future<Output = Result<Plan, PlanError>> + Send + 'a>>;

/// Output from a command after execution.
///
/// This represents the final result that will be shown to the user.
#[derive(Debug)]
pub enum CommandOutput<T> {
    /// Command succeeded with output.
    Success(T),

    /// Command was paused for conflict resolution.
    Paused {
        /// Message explaining what to do next.
        message: String,
    },

    /// Command failed.
    Failed {
        /// Error message.
        error: String,
    },
}

impl<T> CommandOutput<T> {
    /// Check if the command succeeded.
    pub fn is_success(&self) -> bool {
        matches!(self, CommandOutput::Success(_))
    }

    /// Check if the command was paused.
    pub fn is_paused(&self) -> bool {
        matches!(self, CommandOutput::Paused { .. })
    }

    /// Check if the command failed.
    pub fn is_failed(&self) -> bool {
        matches!(self, CommandOutput::Failed { .. })
    }

    /// Unwrap the success value, panicking if not successful.
    pub fn unwrap(self) -> T {
        match self {
            CommandOutput::Success(v) => v,
            CommandOutput::Paused { message } => panic!("called unwrap on Paused: {}", message),
            CommandOutput::Failed { error } => panic!("called unwrap on Failed: {}", error),
        }
    }

    /// Convert to a Result, treating Paused as an error.
    pub fn into_result(self) -> Result<T, String> {
        match self {
            CommandOutput::Success(v) => Ok(v),
            CommandOutput::Paused { message } => Err(message),
            CommandOutput::Failed { error } => Err(error),
        }
    }
}

/// A command that can be executed through the engine lifecycle.
///
/// Commands implement this trait to declare their requirements and
/// participate in the validated execution model.
///
/// # Type Parameters
///
/// The associated `Output` type represents what the command produces
/// on successful completion.
///
/// # Requirements
///
/// The `REQUIREMENTS` constant declares what capabilities must be
/// satisfied before the command can execute. The engine uses this
/// to gate execution and produce repair bundles when requirements
/// are not met.
pub trait Command {
    /// The requirement set for this command.
    ///
    /// This must be a compile-time constant reference to a `RequirementSet`.
    /// The engine checks these requirements during gating.
    const REQUIREMENTS: &'static RequirementSet;

    /// Output type produced by this command.
    type Output;

    /// Generate a plan from validated context.
    ///
    /// This method is called after gating succeeds. The `ctx` parameter
    /// contains a `RepoSnapshot` that is guaranteed to satisfy
    /// `REQUIREMENTS`, plus any command-specific validated data.
    ///
    /// # Purity
    ///
    /// This method MUST be pure:
    /// - No I/O operations
    /// - No repository mutations
    /// - No network calls
    /// - Deterministic output for the same input
    ///
    /// # Arguments
    ///
    /// * `ctx` - Validated context containing snapshot and scope data
    ///
    /// # Returns
    ///
    /// A `Plan` describing the mutations to apply, or an error if
    /// planning fails (e.g., due to business logic constraints).
    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError>;

    /// Process execution result into command output.
    ///
    /// This method is called after the executor finishes (whether
    /// successfully, paused, or aborted). It converts the raw
    /// `ExecuteResult` into the command's output type.
    ///
    /// # Arguments
    ///
    /// * `result` - The result from the executor
    ///
    /// # Returns
    ///
    /// A `CommandOutput` that will be shown to the user.
    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output>;
}

/// Marker trait for commands that don't produce meaningful output.
///
/// Most commands just succeed or fail without returning data.
/// This provides a convenient implementation of `finish()` for them.
pub trait SimpleCommand: Command<Output = ()> {
    /// Default implementation of finish for simple commands.
    fn simple_finish(&self, result: ExecuteResult) -> CommandOutput<()> {
        match result {
            ExecuteResult::Success { .. } => CommandOutput::Success(()),
            ExecuteResult::Paused {
                branch, git_state, ..
            } => CommandOutput::Paused {
                message: format!(
                    "Paused for {} on '{}'. Resolve and run 'lattice continue', or 'lattice abort'.",
                    git_state.description(),
                    branch
                ),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

/// A read-only command that queries repository state without mutations.
///
/// Read-only commands differ from mutating commands in that they:
/// - Do not generate a `Plan` (no mutations to execute)
/// - Directly produce output from the validated `ReadyContext`
/// - Do not require journal, op-state, or executor involvement
///
/// This trait provides a simpler interface for commands like `log`, `info`,
/// `parent`, `children`, and `pr` that only read and display information.
///
/// # Example
///
/// ```ignore
/// use latticework::engine::command::ReadOnlyCommand;
/// use latticework::engine::gate::{RequirementSet, ReadyContext, requirements};
/// use latticework::engine::plan::PlanError;
///
/// struct LogCommand {
///     all: bool,
///     json: bool,
/// }
///
/// impl ReadOnlyCommand for LogCommand {
///     const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
///     type Output = String;
///
///     fn execute(&self, ctx: &ReadyContext) -> Result<Self::Output, PlanError> {
///         // Generate output directly from validated context
///         let output = format_stack_log(&ctx.snapshot, self.all, self.json);
///         Ok(output)
///     }
/// }
/// ```
pub trait ReadOnlyCommand {
    /// The requirement set for this command.
    ///
    /// Read-only commands typically use `requirements::READ_ONLY`, but may
    /// use other sets that don't require mutation capabilities.
    const REQUIREMENTS: &'static RequirementSet;

    /// Output type produced by this command.
    type Output;

    /// Execute the read-only command and produce output.
    ///
    /// Unlike `Command::plan()`, this method directly produces the final
    /// output without going through the executor. The `ctx` parameter
    /// contains a validated `ReadyContext` that satisfies `REQUIREMENTS`.
    ///
    /// # Purity
    ///
    /// This method should be pure with respect to repository state:
    /// - No repository mutations
    /// - No metadata changes
    /// - Read operations only
    ///
    /// Note: Output to stdout/stderr is acceptable for display commands.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Validated context containing snapshot and scope data
    ///
    /// # Returns
    ///
    /// The command output, or an error if execution fails.
    fn execute(&self, ctx: &ReadyContext) -> Result<Self::Output, PlanError>;
}

/// An async command that performs network operations.
///
/// Async commands differ from synchronous commands in that:
/// - The `plan()` method may perform async operations (API queries, token refresh)
/// - The plan may include remote operations (push, PR create/update)
/// - Execution may involve both local and remote phases
///
/// This trait is used for commands that interact with remote forges like GitHub,
/// including `submit`, `sync`, `get`, and `merge`.
///
/// # Lifecycle
///
/// Async commands follow the same lifecycle as sync commands:
/// 1. Scan repository state
/// 2. Gate on requirements
/// 3. Plan (async - may query forge APIs)
/// 4. Execute plan
/// 5. Verify and return
///
/// The key difference is that planning may be async to allow querying existing
/// PR state, checking remote branch status, or refreshing auth tokens.
///
/// # Example
///
/// ```ignore
/// use latticework::engine::command::{AsyncCommand, CommandOutput, PlanFut};
/// use latticework::engine::gate::{RequirementSet, ReadyContext, requirements};
/// use latticework::engine::plan::{Plan, PlanError, PlanStep};
/// use latticework::engine::exec::ExecuteResult;
/// use latticework::core::ops::journal::OpId;
///
/// struct SubmitCommand {
///     draft: bool,
///     force: bool,
/// }
///
/// impl AsyncCommand for SubmitCommand {
///     const REQUIREMENTS: &'static RequirementSet = &requirements::REMOTE;
///     type Output = SubmitResult;
///
///     fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a> {
///         Box::pin(async move {
///             let mut plan = Plan::new(OpId::new(), "submit");
///
///             // Query forge for existing PRs (async operation)
///             // ... add ForgePush and ForgeCreatePr steps ...
///
///             Ok(plan)
///         })
///     }
///
///     fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output> {
///         match result {
///             ExecuteResult::Success { .. } => CommandOutput::Success(SubmitResult { /* ... */ }),
///             ExecuteResult::Paused { branch, .. } => CommandOutput::Paused {
///                 message: format!("Conflict on '{}'. Resolve and run 'lattice continue'.", branch),
///             },
///             ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
///         }
///     }
/// }
/// ```
pub trait AsyncCommand: Send + Sync {
    /// The requirement set for this command.
    ///
    /// Async commands typically use `requirements::REMOTE` or
    /// `requirements::REMOTE_BARE_ALLOWED` for bare-repo compatible operations.
    const REQUIREMENTS: &'static RequirementSet;

    /// Output type produced by this command.
    type Output;

    /// Generate a plan asynchronously from validated context.
    ///
    /// Unlike `Command::plan()`, this may perform async operations:
    /// - Query forge for existing PRs
    /// - Refresh authentication tokens
    /// - Resolve remote branch state
    /// - Check PR merge status
    ///
    /// The returned plan may contain both local steps (rebase, metadata update)
    /// and remote steps (push, PR create/update).
    ///
    /// # Arguments
    ///
    /// * `ctx` - Validated context containing snapshot and scope data
    ///
    /// # Returns
    ///
    /// A `Plan` describing the mutations to apply, or an error if
    /// planning fails.
    fn plan<'a>(&'a self, ctx: &'a ReadyContext) -> PlanFut<'a>;

    /// Process execution result into command output.
    ///
    /// Same semantics as `Command::finish()`. This is called after the
    /// executor finishes (whether successfully, paused, or aborted).
    ///
    /// # Arguments
    ///
    /// * `result` - The result from the executor
    ///
    /// # Returns
    ///
    /// A `CommandOutput` that will be shown to the user.
    fn finish(&self, result: ExecuteResult) -> CommandOutput<Self::Output>;
}

/// Marker trait for async commands that don't produce meaningful output.
///
/// Similar to `SimpleCommand` for sync commands, this provides a convenient
/// default implementation of `finish()` for async commands that just succeed
/// or fail without returning data.
pub trait SimpleAsyncCommand: AsyncCommand<Output = ()> {
    /// Default implementation of finish for simple async commands.
    fn simple_finish(&self, result: ExecuteResult) -> CommandOutput<()> {
        match result {
            ExecuteResult::Success { .. } => CommandOutput::Success(()),
            ExecuteResult::Paused {
                branch, git_state, ..
            } => CommandOutput::Paused {
                message: format!(
                    "Paused for {} on '{}'. Resolve and run 'lattice continue', or 'lattice abort'.",
                    git_state.description(),
                    branch
                ),
            },
            ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Fingerprint;
    use crate::git::GitState;

    mod command_output {
        use super::*;

        #[test]
        fn success_is_success() {
            let output: CommandOutput<i32> = CommandOutput::Success(42);
            assert!(output.is_success());
            assert!(!output.is_paused());
            assert!(!output.is_failed());
        }

        #[test]
        fn paused_is_paused() {
            let output: CommandOutput<i32> = CommandOutput::Paused {
                message: "test".to_string(),
            };
            assert!(!output.is_success());
            assert!(output.is_paused());
            assert!(!output.is_failed());
        }

        #[test]
        fn failed_is_failed() {
            let output: CommandOutput<i32> = CommandOutput::Failed {
                error: "test".to_string(),
            };
            assert!(!output.is_success());
            assert!(!output.is_paused());
            assert!(output.is_failed());
        }

        #[test]
        fn unwrap_success() {
            let output: CommandOutput<i32> = CommandOutput::Success(42);
            assert_eq!(output.unwrap(), 42);
        }

        #[test]
        #[should_panic(expected = "Paused")]
        fn unwrap_paused_panics() {
            let output: CommandOutput<i32> = CommandOutput::Paused {
                message: "test".to_string(),
            };
            output.unwrap();
        }

        #[test]
        #[should_panic(expected = "Failed")]
        fn unwrap_failed_panics() {
            let output: CommandOutput<i32> = CommandOutput::Failed {
                error: "test".to_string(),
            };
            output.unwrap();
        }

        #[test]
        fn into_result_success() {
            let output: CommandOutput<i32> = CommandOutput::Success(42);
            assert_eq!(output.into_result(), Ok(42));
        }

        #[test]
        fn into_result_paused() {
            let output: CommandOutput<i32> = CommandOutput::Paused {
                message: "test".to_string(),
            };
            assert!(output.into_result().is_err());
        }

        #[test]
        fn into_result_failed() {
            let output: CommandOutput<i32> = CommandOutput::Failed {
                error: "test".to_string(),
            };
            assert!(output.into_result().is_err());
        }
    }

    mod execute_result_to_output {
        use super::*;

        #[test]
        fn success_maps_correctly() {
            let result = ExecuteResult::Success {
                fingerprint: Fingerprint::compute(&[]),
            };

            // Simulate what a simple command would do
            let output: CommandOutput<()> = match result {
                ExecuteResult::Success { .. } => CommandOutput::Success(()),
                ExecuteResult::Paused { .. } => unreachable!(),
                ExecuteResult::Aborted { .. } => unreachable!(),
            };

            assert!(output.is_success());
        }

        #[test]
        fn paused_maps_correctly() {
            let result = ExecuteResult::Paused {
                branch: "feature".to_string(),
                git_state: GitState::Rebase {
                    current: Some(1),
                    total: Some(3),
                },
                remaining_steps: vec![],
            };

            let output: CommandOutput<()> = match result {
                ExecuteResult::Success { .. } => unreachable!(),
                ExecuteResult::Paused { branch, .. } => CommandOutput::Paused {
                    message: format!("Paused on {}", branch),
                },
                ExecuteResult::Aborted { .. } => unreachable!(),
            };

            assert!(output.is_paused());
        }

        #[test]
        fn aborted_maps_correctly() {
            let result = ExecuteResult::Aborted {
                error: "CAS failed".to_string(),
                applied_steps: vec![],
            };

            let output: CommandOutput<()> = match result {
                ExecuteResult::Success { .. } => unreachable!(),
                ExecuteResult::Paused { .. } => unreachable!(),
                ExecuteResult::Aborted { error, .. } => CommandOutput::Failed { error },
            };

            assert!(output.is_failed());
        }
    }
}
