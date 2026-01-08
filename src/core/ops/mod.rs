//! core::ops
//!
//! Operation journaling and locking.
//!
//! # Modules
//!
//! - [`journal`] - Operation journal for crash safety and undo
//! - [`lock`] - Exclusive repository lock
//!
//! # Architecture
//!
//! Every mutating command:
//! 1. Acquires the exclusive repo lock
//! 2. Creates an operation journal before any irreversible step
//! 3. Records each step with before/after snapshots
//! 4. On success: marks journal committed
//! 5. On failure: uses journal for rollback
//!
//! # Example
//!
//! ```ignore
//! use lattice::core::ops::lock::RepoLock;
//! use lattice::core::ops::journal::{Journal, OpState};
//!
//! // Acquire lock
//! let lock = RepoLock::acquire(git_dir)?;
//!
//! // Create journal
//! let mut journal = Journal::new("my-command");
//!
//! // Write op-state marker
//! let op_state = OpState::from_journal(&journal);
//! op_state.write(git_dir)?;
//!
//! // Record steps as you go
//! journal.record_ref_update("refs/heads/feature", old_oid, new_oid);
//! journal.write(git_dir)?;
//!
//! // Commit on success
//! journal.commit();
//! journal.write(git_dir)?;
//! OpState::remove(git_dir)?;
//! ```

pub mod journal;
pub mod lock;

// Re-export main types for convenience
pub use journal::{Journal, JournalError, OpId, OpPhase, OpState, StepKind};
pub use lock::{LockError, RepoLock};
