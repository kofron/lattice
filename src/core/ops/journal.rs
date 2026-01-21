//! core::ops::journal
//!
//! Operation journaling for crash safety.
//!
//! This module implements the operation journal per SPEC.md §4.2.2:
//!
//! > "Journals must be written with fsync at each appended step boundary."
//!
//! # Crash Safety Contract
//!
//! The journal provides the following guarantees:
//!
//! 1. **Per-step persistence:** Every `append_*` method writes to disk with fsync
//!    before returning. A crash at any point leaves the journal in a consistent state.
//!
//! 2. **Recoverability:** After a crash, `Journal::read()` returns the journal as
//!    it was after the last successful `append_*` call.
//!
//! 3. **Rollback support:** The journal records enough information to reverse all
//!    ref updates via `ref_updates_for_rollback()`.
//!
//! # Architecture
//!
//! The journal is the source of truth for:
//! - Rollback (`abort`) - restore refs to pre-operation state
//! - Resume (`continue`) - complete a paused operation
//! - Undo last completed operation
//!
//! Per SPEC.md Section 4.6.5, journals and op-state are **repo-scoped**
//! (stored in `<common_dir>/lattice/`), which is shared across all worktrees.
//!
//! # Storage
//!
//! - `<common_dir>/lattice/ops/<op_id>.json` - Journal files (append-only with fsync)
//! - `<common_dir>/lattice/op-state.json` - Current operation marker
//!
//! # Invariants
//!
//! - Journals must be written with fsync at each step boundary
//! - Interrupted commands must be recoverable via the journal
//! - All ref changes must be recorded with before/after OIDs
//! - Op-state is repo-scoped (shared across worktrees)
//! - Continue/abort must be run from the originating worktree when paused
//!
//! # Usage
//!
//! ```ignore
//! use latticework::core::ops::journal::{Journal, StepKind};
//! use latticework::core::paths::LatticePaths;
//!
//! // Create a new journal for an operation
//! let mut journal = Journal::new("restack");
//!
//! // Each append_* method persists immediately with fsync
//! journal.append_ref_update(&paths, "refs/heads/feature", None, "abc123...")?;
//! journal.append_metadata_write(&paths, "feature", None, "meta-oid")?;
//!
//! // Phase transitions also persist
//! journal.commit();
//! journal.write(&paths)?;
//! ```
//!
//! # Migration from `record_*` Methods
//!
//! The old `record_*` methods are deprecated. They only modified in-memory state
//! and required a separate `write()` call, which could be forgotten. Use the
//! corresponding `append_*` methods instead:
//!
//! | Deprecated | Replacement |
//! |------------|-------------|
//! | `record_ref_update()` | `append_ref_update()` |
//! | `record_metadata_write()` | `append_metadata_write()` |
//! | `record_metadata_delete()` | `append_metadata_delete()` |
//! | `record_checkpoint()` | `append_checkpoint()` |
//! | `record_git_process()` | `append_git_process()` |
//! | `record_conflict_paused()` | `append_conflict_paused()` |

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::core::paths::LatticePaths;
use crate::core::types::UtcTimestamp;

/// Errors from journal operations.
#[derive(Debug, Error)]
pub enum JournalError {
    /// I/O error reading or writing journal files.
    #[error("journal i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("journal json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Journal file not found.
    #[error("journal not found: {0}")]
    NotFound(String),

    /// Invalid journal state.
    #[error("invalid journal state: {0}")]
    InvalidState(String),
}

/// Unique identifier for an operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpId(String);

impl OpId {
    /// Generate a new unique operation id.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create an OpId from an existing string.
    ///
    /// Used when reading journals from disk.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Get the string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for OpId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for OpId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The current phase of an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpPhase {
    /// Operation is in progress.
    InProgress,
    /// Operation is paused waiting for user (e.g., conflict resolution).
    Paused,
    /// Operation completed successfully.
    Committed,
    /// Operation was rolled back.
    RolledBack,
}

impl OpPhase {
    /// Check if the operation is finished (committed or rolled back).
    pub fn is_finished(&self) -> bool {
        matches!(self, OpPhase::Committed | OpPhase::RolledBack)
    }

    /// Check if the operation can be resumed.
    pub fn is_resumable(&self) -> bool {
        matches!(self, OpPhase::Paused)
    }

    /// Check if the operation is actively running.
    pub fn is_active(&self) -> bool {
        matches!(self, OpPhase::InProgress)
    }
}

/// A single step in an operation journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalStep {
    /// Step kind with operation-specific data.
    pub kind: StepKind,
    /// Timestamp when step was recorded.
    pub timestamp: UtcTimestamp,
}

/// The kind of journal step.
///
/// Each step records enough information to either undo the operation
/// or resume it from where it left off.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepKind {
    /// A ref update with before/after values.
    ///
    /// Used for branch tip updates and other ref changes.
    RefUpdate {
        /// The full ref name (e.g., "refs/heads/feature").
        refname: String,
        /// OID before the update, or None if the ref was created.
        old_oid: Option<String>,
        /// OID after the update.
        new_oid: String,
    },

    /// A metadata write with full snapshot.
    ///
    /// Records metadata ref changes for tracked branches.
    MetadataWrite {
        /// Branch name.
        branch: String,
        /// Metadata ref OID before the write, or None if created.
        old_ref_oid: Option<String>,
        /// Metadata ref OID after the write.
        new_ref_oid: String,
    },

    /// A metadata delete.
    MetadataDelete {
        /// Branch name.
        branch: String,
        /// Metadata ref OID that was deleted.
        old_ref_oid: String,
    },

    /// A checkpoint marker.
    ///
    /// Used to mark significant points in multi-step operations.
    Checkpoint {
        /// Checkpoint name for debugging/logging.
        name: String,
    },

    /// A git process was run.
    ///
    /// Records git commands for auditing and debugging.
    GitProcess {
        /// Git command arguments.
        args: Vec<String>,
        /// Human-readable description of what the command does.
        description: String,
    },

    /// Conflict detected during operation.
    ///
    /// Records the state when the operation was paused for user intervention.
    /// Per Milestone 0.5, this now includes serialized remaining plan steps
    /// so that `continue` can resume multi-step operations.
    ConflictPaused {
        /// The branch where the conflict occurred.
        branch: String,
        /// Type of git operation that conflicted (rebase, merge, etc.).
        git_state: String,
        /// Branches remaining to process after conflict resolution (for display).
        remaining_branches: Vec<String>,
        /// JSON-serialized remaining plan steps for continuation.
        ///
        /// This field stores `Vec<PlanStep>` as JSON to avoid circular dependency
        /// between `core::ops::journal` and `engine::plan`. The steps are
        /// deserialized by `continue` to resume execution.
        ///
        /// None for legacy journals or operations with no remaining steps.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        remaining_steps_json: Option<String>,
    },
}

/// An operation journal.
///
/// Records all state changes during a Lattice operation for crash
/// recovery, undo, and resume support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    /// Unique operation identifier.
    pub op_id: OpId,
    /// Command that started this operation.
    pub command: String,
    /// When the operation started.
    pub started_at: UtcTimestamp,
    /// When the operation finished (if finished).
    pub finished_at: Option<UtcTimestamp>,
    /// Current phase.
    pub phase: OpPhase,
    /// Steps recorded so far.
    pub steps: Vec<JournalStep>,
}

impl Journal {
    /// Create a new journal for an operation.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            op_id: OpId::new(),
            command: command.into(),
            started_at: UtcTimestamp::now(),
            finished_at: None,
            phase: OpPhase::InProgress,
            steps: vec![],
        }
    }

    /// Get the directory where journals are stored.
    ///
    /// Per SPEC.md §4.6.5, journals are repo-scoped and stored in `<common_dir>/lattice/ops/`.
    pub fn ops_dir(paths: &LatticePaths) -> PathBuf {
        paths.repo_ops_dir()
    }

    /// Get the path to this journal's file.
    pub fn file_path(&self, paths: &LatticePaths) -> PathBuf {
        paths.repo_op_journal_path(self.op_id.as_str())
    }

    /// Add a step to the journal (in-memory only).
    ///
    /// This is internal - external code should use `append_*` methods
    /// which persist immediately per SPEC.md §4.2.2.
    ///
    /// # Note
    /// This method is kept for internal use (tests, journal loading).
    /// Production code paths should use the `append_*` variants.
    pub(crate) fn add_step(&mut self, kind: StepKind) {
        self.steps.push(JournalStep {
            kind,
            timestamp: UtcTimestamp::now(),
        });
    }

    // =========================================================================
    // New append_* methods - these persist immediately per SPEC.md §4.2.2
    // =========================================================================

    /// Append a ref update step and persist to disk.
    ///
    /// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
    /// The step is atomically added and persisted before this method returns.
    ///
    /// # Errors
    ///
    /// Returns an error if the journal cannot be written to disk.
    pub fn append_ref_update(
        &mut self,
        paths: &LatticePaths,
        refname: impl Into<String>,
        old_oid: Option<String>,
        new_oid: impl Into<String>,
    ) -> Result<(), JournalError> {
        self.steps.push(JournalStep {
            kind: StepKind::RefUpdate {
                refname: refname.into(),
                old_oid,
                new_oid: new_oid.into(),
            },
            timestamp: UtcTimestamp::now(),
        });
        self.write(paths)
    }

    /// Append a metadata write step and persist to disk.
    ///
    /// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
    /// The step is atomically added and persisted before this method returns.
    ///
    /// # Errors
    ///
    /// Returns an error if the journal cannot be written to disk.
    pub fn append_metadata_write(
        &mut self,
        paths: &LatticePaths,
        branch: impl Into<String>,
        old_ref_oid: Option<String>,
        new_ref_oid: impl Into<String>,
    ) -> Result<(), JournalError> {
        self.steps.push(JournalStep {
            kind: StepKind::MetadataWrite {
                branch: branch.into(),
                old_ref_oid,
                new_ref_oid: new_ref_oid.into(),
            },
            timestamp: UtcTimestamp::now(),
        });
        self.write(paths)
    }

    /// Append a metadata delete step and persist to disk.
    ///
    /// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
    /// The step is atomically added and persisted before this method returns.
    ///
    /// # Errors
    ///
    /// Returns an error if the journal cannot be written to disk.
    pub fn append_metadata_delete(
        &mut self,
        paths: &LatticePaths,
        branch: impl Into<String>,
        old_ref_oid: impl Into<String>,
    ) -> Result<(), JournalError> {
        self.steps.push(JournalStep {
            kind: StepKind::MetadataDelete {
                branch: branch.into(),
                old_ref_oid: old_ref_oid.into(),
            },
            timestamp: UtcTimestamp::now(),
        });
        self.write(paths)
    }

    /// Append a checkpoint step and persist to disk.
    ///
    /// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
    /// The step is atomically added and persisted before this method returns.
    ///
    /// # Errors
    ///
    /// Returns an error if the journal cannot be written to disk.
    pub fn append_checkpoint(
        &mut self,
        paths: &LatticePaths,
        name: impl Into<String>,
    ) -> Result<(), JournalError> {
        self.steps.push(JournalStep {
            kind: StepKind::Checkpoint { name: name.into() },
            timestamp: UtcTimestamp::now(),
        });
        self.write(paths)
    }

    /// Append a git process step and persist to disk.
    ///
    /// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
    /// The step is atomically added and persisted before this method returns.
    ///
    /// # Errors
    ///
    /// Returns an error if the journal cannot be written to disk.
    pub fn append_git_process(
        &mut self,
        paths: &LatticePaths,
        args: Vec<String>,
        description: impl Into<String>,
    ) -> Result<(), JournalError> {
        self.steps.push(JournalStep {
            kind: StepKind::GitProcess {
                args,
                description: description.into(),
            },
            timestamp: UtcTimestamp::now(),
        });
        self.write(paths)
    }

    /// Append a conflict paused step and persist to disk.
    ///
    /// Per SPEC.md §4.2.2, this writes with fsync at the step boundary.
    /// The step is atomically added and persisted before this method returns.
    ///
    /// Per Milestone 0.5, this method now accepts optional serialized remaining
    /// steps so that `continue` can resume multi-step operations.
    ///
    /// # Arguments
    ///
    /// * `paths` - Repository paths for journal storage
    /// * `branch` - The branch where the conflict occurred
    /// * `git_state` - Type of git operation that conflicted (e.g., "rebase")
    /// * `remaining_branches` - Branch names remaining to process (for display)
    /// * `remaining_steps_json` - JSON-serialized `Vec<PlanStep>` for continuation
    ///
    /// # Errors
    ///
    /// Returns an error if the journal cannot be written to disk.
    pub fn append_conflict_paused(
        &mut self,
        paths: &LatticePaths,
        branch: impl Into<String>,
        git_state: impl Into<String>,
        remaining_branches: Vec<String>,
        remaining_steps_json: Option<String>,
    ) -> Result<(), JournalError> {
        self.steps.push(JournalStep {
            kind: StepKind::ConflictPaused {
                branch: branch.into(),
                git_state: git_state.into(),
                remaining_branches,
                remaining_steps_json,
            },
            timestamp: UtcTimestamp::now(),
        });
        self.write(paths)
    }

    // =========================================================================
    // Deprecated record_* methods - use append_* instead
    // =========================================================================

    /// Record a ref update step (in-memory only).
    ///
    /// # Deprecated
    ///
    /// Use [`append_ref_update()`](Self::append_ref_update) instead, which
    /// persists immediately per SPEC.md §4.2.2.
    #[deprecated(
        since = "0.9.0",
        note = "Use append_ref_update() which persists immediately"
    )]
    pub fn record_ref_update(
        &mut self,
        refname: impl Into<String>,
        old_oid: Option<String>,
        new_oid: impl Into<String>,
    ) {
        self.add_step(StepKind::RefUpdate {
            refname: refname.into(),
            old_oid,
            new_oid: new_oid.into(),
        });
    }

    /// Record a metadata write step (in-memory only).
    ///
    /// # Deprecated
    ///
    /// Use [`append_metadata_write()`](Self::append_metadata_write) instead,
    /// which persists immediately per SPEC.md §4.2.2.
    #[deprecated(
        since = "0.9.0",
        note = "Use append_metadata_write() which persists immediately"
    )]
    pub fn record_metadata_write(
        &mut self,
        branch: impl Into<String>,
        old_ref_oid: Option<String>,
        new_ref_oid: impl Into<String>,
    ) {
        self.add_step(StepKind::MetadataWrite {
            branch: branch.into(),
            old_ref_oid,
            new_ref_oid: new_ref_oid.into(),
        });
    }

    /// Record a metadata delete step (in-memory only).
    ///
    /// # Deprecated
    ///
    /// Use [`append_metadata_delete()`](Self::append_metadata_delete) instead,
    /// which persists immediately per SPEC.md §4.2.2.
    #[deprecated(
        since = "0.9.0",
        note = "Use append_metadata_delete() which persists immediately"
    )]
    pub fn record_metadata_delete(
        &mut self,
        branch: impl Into<String>,
        old_ref_oid: impl Into<String>,
    ) {
        self.add_step(StepKind::MetadataDelete {
            branch: branch.into(),
            old_ref_oid: old_ref_oid.into(),
        });
    }

    /// Record a checkpoint (in-memory only).
    ///
    /// # Deprecated
    ///
    /// Use [`append_checkpoint()`](Self::append_checkpoint) instead,
    /// which persists immediately per SPEC.md §4.2.2.
    #[deprecated(
        since = "0.9.0",
        note = "Use append_checkpoint() which persists immediately"
    )]
    pub fn record_checkpoint(&mut self, name: impl Into<String>) {
        self.add_step(StepKind::Checkpoint { name: name.into() });
    }

    /// Record a git process execution (in-memory only).
    ///
    /// # Deprecated
    ///
    /// Use [`append_git_process()`](Self::append_git_process) instead,
    /// which persists immediately per SPEC.md §4.2.2.
    #[deprecated(
        since = "0.9.0",
        note = "Use append_git_process() which persists immediately"
    )]
    pub fn record_git_process(&mut self, args: Vec<String>, description: impl Into<String>) {
        self.add_step(StepKind::GitProcess {
            args,
            description: description.into(),
        });
    }

    /// Record that a conflict paused the operation (in-memory only).
    ///
    /// # Deprecated
    ///
    /// Use [`append_conflict_paused()`](Self::append_conflict_paused) instead,
    /// which persists immediately per SPEC.md §4.2.2.
    #[deprecated(
        since = "0.9.0",
        note = "Use append_conflict_paused() which persists immediately"
    )]
    pub fn record_conflict_paused(
        &mut self,
        branch: impl Into<String>,
        git_state: impl Into<String>,
        remaining_branches: Vec<String>,
    ) {
        self.add_step(StepKind::ConflictPaused {
            branch: branch.into(),
            git_state: git_state.into(),
            remaining_branches,
            remaining_steps_json: None,
        });
    }

    /// Record a conflict pause with remaining steps JSON (for testing).
    ///
    /// This is the in-memory variant that includes remaining steps.
    /// For production use, prefer `append_conflict_paused()` which persists immediately.
    #[cfg(test)]
    pub fn record_conflict_paused_with_remaining_steps(
        &mut self,
        branch: impl Into<String>,
        git_state: impl Into<String>,
        remaining_branches: Vec<String>,
        remaining_steps_json: Option<String>,
    ) {
        self.add_step(StepKind::ConflictPaused {
            branch: branch.into(),
            git_state: git_state.into(),
            remaining_branches,
            remaining_steps_json,
        });
    }

    /// Mark the operation as committed (successful completion).
    pub fn commit(&mut self) {
        self.phase = OpPhase::Committed;
        self.finished_at = Some(UtcTimestamp::now());
    }

    /// Mark the operation as paused (waiting for user).
    pub fn pause(&mut self) {
        self.phase = OpPhase::Paused;
    }

    /// Mark the operation as rolled back.
    pub fn rollback(&mut self) {
        self.phase = OpPhase::RolledBack;
        self.finished_at = Some(UtcTimestamp::now());
    }

    /// Write the journal to disk with fsync.
    ///
    /// This should be called after each step to ensure crash safety.
    /// Per SPEC.md §4.6.5, journals are written to `<common_dir>/lattice/ops/`.
    ///
    /// # Fault Injection
    ///
    /// When compiled with `cfg(test)` or the `fault_injection` feature,
    /// this method can simulate crashes for testing crash recovery.
    /// Use [`fault_injection::set_crash_after`] to configure.
    pub fn write(&self, paths: &LatticePaths) -> Result<(), JournalError> {
        // Check for fault injection (test-only)
        #[cfg(any(test, feature = "fault_injection"))]
        if fault_injection::should_crash() {
            return Err(JournalError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "simulated crash for fault injection testing",
            )));
        }

        let dir = Self::ops_dir(paths);
        fs::create_dir_all(&dir)?;

        let path = self.file_path(paths);
        let content = serde_json::to_string_pretty(self)?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        file.write_all(content.as_bytes())?;
        file.sync_all()?;

        Ok(())
    }

    /// Read a journal from disk.
    ///
    /// Per SPEC.md §4.6.5, journals are read from `<common_dir>/lattice/ops/`.
    pub fn read(paths: &LatticePaths, op_id: &OpId) -> Result<Self, JournalError> {
        let path = paths.repo_op_journal_path(op_id.as_str());

        if !path.exists() {
            return Err(JournalError::NotFound(op_id.to_string()));
        }

        let content = fs::read_to_string(&path)?;
        let journal = serde_json::from_str(&content)?;
        Ok(journal)
    }

    /// List all journal files.
    ///
    /// Returns operation IDs sorted by modification time (newest first).
    /// Per SPEC.md §4.6.5, journals are listed from `<common_dir>/lattice/ops/`.
    pub fn list(paths: &LatticePaths) -> Result<Vec<OpId>, JournalError> {
        let dir = Self::ops_dir(paths);
        if !dir.exists() {
            return Ok(vec![]);
        }

        let mut entries: Vec<_> = fs::read_dir(&dir)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().into_string().ok()?;
                let id = name.strip_suffix(".json")?;
                let mtime = entry.metadata().ok()?.modified().ok()?;
                Some((OpId::from_string(id), mtime))
            })
            .collect();

        // Sort by modification time (newest first)
        entries.sort_by(|a, b| b.1.cmp(&a.1));

        Ok(entries.into_iter().map(|(id, _)| id).collect())
    }

    /// Get the most recent journal.
    ///
    /// Per SPEC.md §4.6.5, journals are stored at `<common_dir>/lattice/ops/`.
    pub fn most_recent(paths: &LatticePaths) -> Result<Option<Self>, JournalError> {
        let ids = Self::list(paths)?;
        match ids.first() {
            Some(id) => Ok(Some(Self::read(paths, id)?)),
            None => Ok(None),
        }
    }

    /// Delete this journal from disk.
    pub fn delete(&self, paths: &LatticePaths) -> Result<(), JournalError> {
        let path = self.file_path(paths);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Get all ref update steps for undo/rollback.
    ///
    /// Returns steps in reverse order (most recent first) for rollback.
    pub fn ref_updates_for_rollback(&self) -> Vec<&StepKind> {
        self.steps
            .iter()
            .rev()
            .filter_map(|step| match &step.kind {
                kind @ StepKind::RefUpdate { .. } => Some(kind),
                kind @ StepKind::MetadataWrite { .. } => Some(kind),
                kind @ StepKind::MetadataDelete { .. } => Some(kind),
                _ => None,
            })
            .collect()
    }

    /// Check if this journal can be fully rolled back.
    ///
    /// Returns `true` if all ref updates can be reversed. Returns `false` if
    /// any step cannot be undone (e.g., metadata deletion where content wasn't stored).
    ///
    /// # Known Limitations
    ///
    /// - Metadata updates cannot be fully rolled back (old content not stored)
    /// - Metadata deletes cannot be restored (deleted content not stored)
    pub fn can_fully_rollback(&self) -> bool {
        self.steps.iter().all(|step| match &step.kind {
            StepKind::RefUpdate { .. } => true,
            StepKind::MetadataWrite { old_ref_oid, .. } => {
                // Can only rollback if metadata was created (not modified)
                old_ref_oid.is_none()
            }
            StepKind::MetadataDelete { .. } => {
                // Cannot restore deleted metadata without content
                false
            }
            StepKind::Checkpoint { .. }
            | StepKind::GitProcess { .. }
            | StepKind::ConflictPaused { .. } => true,
        })
    }

    /// Get a summary of what would be rolled back.
    ///
    /// Returns a `RollbackSummary` categorizing the changes that would be
    /// rolled back and whether the rollback would be complete.
    pub fn rollback_summary(&self) -> RollbackSummary {
        let mut summary = RollbackSummary::default();

        for step in &self.steps {
            match &step.kind {
                StepKind::RefUpdate { refname, .. } => {
                    summary.ref_updates.push(refname.clone());
                }
                StepKind::MetadataWrite {
                    branch,
                    old_ref_oid,
                    ..
                } => {
                    if old_ref_oid.is_none() {
                        summary.metadata_creates.push(branch.clone());
                    } else {
                        summary.metadata_updates.push(branch.clone());
                    }
                }
                StepKind::MetadataDelete { branch, .. } => {
                    summary.metadata_deletes.push(branch.clone());
                }
                _ => {}
            }
        }

        summary
    }

    // =========================================================================
    // Multi-step continuation support (Milestone 0.5)
    // =========================================================================

    /// Get the JSON-serialized remaining steps from the last ConflictPaused step.
    ///
    /// Returns `None` if:
    /// - Journal has no steps
    /// - Last step is not ConflictPaused
    /// - ConflictPaused has no remaining steps
    ///
    /// This is used by `continue` to resume multi-step operations.
    /// The caller is responsible for deserializing the JSON into `Vec<PlanStep>`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use latticework::engine::plan::PlanStep;
    ///
    /// if let Some(json) = journal.remaining_steps_json() {
    ///     let steps: Vec<PlanStep> = serde_json::from_str(json)?;
    ///     // Execute remaining steps...
    /// }
    /// ```
    pub fn remaining_steps_json(&self) -> Option<&str> {
        self.steps.last().and_then(|step| {
            if let StepKind::ConflictPaused {
                remaining_steps_json: Some(json),
                ..
            } = &step.kind
            {
                Some(json.as_str())
            } else {
                None
            }
        })
    }

    /// Check if the journal has remaining steps to execute.
    ///
    /// Returns `true` if the last step is a ConflictPaused with non-empty
    /// remaining steps JSON.
    ///
    /// This is a quick check for `continue` to determine if resumption is needed.
    pub fn has_remaining_steps(&self) -> bool {
        self.remaining_steps_json()
            .is_some_and(|json| !json.is_empty() && json != "[]")
    }

    /// Get the branch where the operation is paused.
    ///
    /// Returns `None` if the journal is not in a ConflictPaused state.
    pub fn paused_branch(&self) -> Option<&str> {
        self.steps.last().and_then(|step| {
            if let StepKind::ConflictPaused { branch, .. } = &step.kind {
                Some(branch.as_str())
            } else {
                None
            }
        })
    }

    /// Get the remaining branches for display purposes.
    ///
    /// Returns an empty slice if the journal is not in a ConflictPaused state.
    pub fn remaining_branches(&self) -> &[String] {
        self.steps
            .last()
            .and_then(|step| {
                if let StepKind::ConflictPaused {
                    remaining_branches, ..
                } = &step.kind
                {
                    Some(remaining_branches.as_slice())
                } else {
                    None
                }
            })
            .unwrap_or(&[])
    }
}

/// Summary of what a rollback would do.
///
/// Used to preview rollback operations and identify limitations.
#[derive(Debug, Default)]
pub struct RollbackSummary {
    /// Branch refs that would be restored.
    pub ref_updates: Vec<String>,
    /// Metadata that was created and can be deleted.
    pub metadata_creates: Vec<String>,
    /// Metadata that was updated (cannot fully restore).
    pub metadata_updates: Vec<String>,
    /// Metadata that was deleted (cannot restore).
    pub metadata_deletes: Vec<String>,
}

impl RollbackSummary {
    /// Check if rollback would be complete.
    ///
    /// Returns `true` if there are no metadata updates or deletes that
    /// cannot be fully restored.
    pub fn is_complete(&self) -> bool {
        self.metadata_updates.is_empty() && self.metadata_deletes.is_empty()
    }

    /// Get the total number of items that would be rolled back.
    pub fn total_items(&self) -> usize {
        self.ref_updates.len()
            + self.metadata_creates.len()
            + self.metadata_updates.len()
            + self.metadata_deletes.len()
    }
}

// ============================================================================
// OpState Types (SPEC.md §4.6.5)
// ============================================================================

/// Current plan schema version.
///
/// Increment this when the plan format changes in incompatible ways.
/// Used by `continue` to detect version mismatches.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// Reason why an operation is awaiting user action.
///
/// Per SPEC.md §4.6.5, the op-state records why the operation is paused
/// so the user knows what action to take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AwaitingReason {
    /// Rebase/merge/cherry-pick conflict requires manual resolution.
    RebaseConflict,

    /// Rollback could not complete all refs.
    RollbackIncomplete {
        /// Refs that failed to roll back.
        failed_refs: Vec<String>,
    },

    /// Post-verification failed after mutations were applied.
    VerificationFailed {
        /// Description of what verification failed.
        evidence: String,
    },
}

/// A ref that will be touched by an operation, with its expected old OID.
///
/// Used for CAS (compare-and-swap) verification during rollback.
/// Per SPEC.md §4.6.5, touched_refs records "expected olds" for CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchedRef {
    /// The full ref name (e.g., "refs/heads/feature").
    pub refname: String,
    /// The expected OID before the operation, or None if the ref will be created.
    pub expected_old: Option<String>,
}

impl TouchedRef {
    /// Create a new TouchedRef.
    pub fn new(refname: impl Into<String>, expected_old: Option<String>) -> Self {
        Self {
            refname: refname.into(),
            expected_old,
        }
    }
}

/// The op-state marker indicating an operation is in progress.
///
/// This file exists only while a Lattice operation is executing or
/// paused waiting for user intervention. It prevents other Lattice
/// commands from running until the current operation is resolved.
///
/// Per SPEC.md §4.6.5, op-state is repo-scoped (shared across worktrees),
/// stored at `<common_dir>/lattice/op-state.json`. The `origin_git_dir` and
/// `origin_work_dir` fields track where the operation was started, which is
/// required for `continue` and `abort` to be run from the correct worktree.
///
/// # Required Fields (SPEC.md §4.6.5)
///
/// - `plan_digest`: SHA-256 hash of the canonical plan JSON for integrity checking
/// - `plan_schema_version`: Version number for cross-binary compatibility
/// - `touched_refs`: Refs with expected old OIDs for CAS-based rollback
/// - `awaiting_reason`: Why the operation is paused (when phase is Paused)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpState {
    /// Operation id (matches the journal).
    pub op_id: OpId,
    /// Command name.
    pub command: String,
    /// Current phase.
    pub phase: OpPhase,
    /// When the op-state was last updated.
    pub updated_at: UtcTimestamp,
    /// The git_dir of the worktree that started this operation.
    ///
    /// Per SPEC.md §4.6.5, `continue` and `abort` must be run from
    /// the originating worktree when the operation is paused.
    pub origin_git_dir: PathBuf,
    /// The work_dir of the originating worktree.
    ///
    /// None for bare repositories (no working directory available).
    pub origin_work_dir: Option<PathBuf>,

    // ========================================================================
    // New fields per SPEC.md §4.6.5 and ROADMAP.md Milestone 0.4
    // ========================================================================
    /// SHA-256 digest of the canonical plan JSON.
    ///
    /// Used by `continue` to verify the plan hasn't changed since the operation
    /// was started. Format: "sha256:<hex>"
    pub plan_digest: String,

    /// Plan schema version for cross-binary compatibility.
    ///
    /// If a newer binary tries to continue an operation from an older binary
    /// (or vice versa), this field enables detection and clear error messages.
    pub plan_schema_version: u32,

    /// Refs that will be touched by this operation, with expected old OIDs.
    ///
    /// Used for CAS (compare-and-swap) validation during rollback. Each entry
    /// records what the ref's OID should be before the operation.
    pub touched_refs: Vec<TouchedRef>,

    /// Why the operation is awaiting user action.
    ///
    /// Set when `phase == Paused`. Helps the user understand what action is
    /// needed (resolve conflict, acknowledge rollback failure, etc.).
    pub awaiting_reason: Option<AwaitingReason>,
}

impl OpState {
    /// Create a new op-state marker from a journal and plan information.
    ///
    /// # Arguments
    ///
    /// * `journal` - The operation journal
    /// * `paths` - Identifies the originating worktree (required per SPEC.md §4.6.5)
    /// * `work_dir` - Working directory, None for bare repositories
    /// * `plan_digest` - SHA-256 digest of the plan (from `Plan::digest()`)
    /// * `touched_refs` - Refs with expected old OIDs (from `Plan::touched_refs_with_oids()`)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let op_state = OpState::from_journal(
    ///     &journal,
    ///     &paths,
    ///     Some(work_dir),
    ///     plan.digest(),
    ///     plan.touched_refs_with_oids(),
    /// );
    /// ```
    pub fn from_journal(
        journal: &Journal,
        paths: &LatticePaths,
        work_dir: Option<PathBuf>,
        plan_digest: String,
        touched_refs: Vec<TouchedRef>,
    ) -> Self {
        Self {
            op_id: journal.op_id.clone(),
            command: journal.command.clone(),
            phase: journal.phase.clone(),
            updated_at: UtcTimestamp::now(),
            origin_git_dir: paths.git_dir.clone(),
            origin_work_dir: work_dir,
            plan_digest,
            plan_schema_version: PLAN_SCHEMA_VERSION,
            touched_refs,
            awaiting_reason: None,
        }
    }

    /// Path to the op-state file.
    ///
    /// Per SPEC.md §4.6.5, op-state is stored at `<common_dir>/lattice/op-state.json`.
    pub fn path(paths: &LatticePaths) -> PathBuf {
        paths.repo_op_state_path()
    }

    /// Write the op-state marker to disk.
    ///
    /// Per SPEC.md §4.6.5, op-state is written to `<common_dir>/lattice/op-state.json`.
    pub fn write(&self, paths: &LatticePaths) -> Result<(), JournalError> {
        let dir = paths.repo_lattice_dir();
        fs::create_dir_all(&dir)?;

        let path = Self::path(paths);
        let content = serde_json::to_string_pretty(self)?;

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        file.write_all(content.as_bytes())?;
        file.sync_all()?;

        Ok(())
    }

    /// Read the op-state marker, if it exists.
    ///
    /// Per SPEC.md §4.6.5, op-state is read from `<common_dir>/lattice/op-state.json`.
    pub fn read(paths: &LatticePaths) -> Result<Option<Self>, JournalError> {
        let path = Self::path(paths);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)?;
        let state = serde_json::from_str(&content)?;
        Ok(Some(state))
    }

    /// Remove the op-state marker.
    ///
    /// Per SPEC.md §4.6.5, op-state is stored at `<common_dir>/lattice/op-state.json`.
    pub fn remove(paths: &LatticePaths) -> Result<(), JournalError> {
        let path = Self::path(paths);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Check if an op-state marker exists.
    pub fn exists(paths: &LatticePaths) -> bool {
        Self::path(paths).exists()
    }

    /// Update the phase and write to disk.
    pub fn update_phase(
        &mut self,
        phase: OpPhase,
        paths: &LatticePaths,
    ) -> Result<(), JournalError> {
        self.phase = phase;
        self.updated_at = UtcTimestamp::now();
        self.write(paths)
    }

    /// Pause the operation with a reason and write to disk.
    ///
    /// Sets the phase to `Paused` and records why the operation is awaiting
    /// user action. This is called when:
    /// - A rebase/merge conflict occurs
    /// - Rollback couldn't complete all refs
    /// - Post-verification failed
    pub fn pause_with_reason(
        &mut self,
        reason: AwaitingReason,
        paths: &LatticePaths,
    ) -> Result<(), JournalError> {
        self.phase = OpPhase::Paused;
        self.awaiting_reason = Some(reason);
        self.updated_at = UtcTimestamp::now();
        self.write(paths)
    }

    /// Create an op-state with default plan values for legacy commands.
    ///
    /// This is a transitional constructor for CLI commands that haven't been
    /// migrated to the executor pattern yet. It uses empty defaults for the
    /// plan-related fields.
    ///
    /// **Note:** New code should use `from_journal()` with proper plan info.
    #[deprecated(note = "Use from_journal() with plan info instead")]
    pub fn from_journal_legacy(
        journal: &Journal,
        paths: &LatticePaths,
        work_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            op_id: journal.op_id.clone(),
            command: journal.command.clone(),
            phase: journal.phase.clone(),
            updated_at: UtcTimestamp::now(),
            origin_git_dir: paths.git_dir.clone(),
            origin_work_dir: work_dir,
            // Default values for legacy commands without Plan
            plan_digest: "sha256:legacy-no-plan".to_string(),
            plan_schema_version: PLAN_SCHEMA_VERSION,
            touched_refs: vec![],
            awaiting_reason: None,
        }
    }

    /// Check if continue/abort can be run from the current worktree.
    ///
    /// Per SPEC.md §4.6.5, `continue` and `abort` must be run from the
    /// originating worktree when the operation is paused due to Git conflicts.
    /// Returns `Ok(())` if allowed, or `Err` with a message pointing to the
    /// correct worktree.
    pub fn check_origin_worktree(&self, current_git_dir: &Path) -> Result<(), String> {
        if self.origin_git_dir == current_git_dir {
            Ok(())
        } else {
            Err(format!(
                "This operation was started in a different worktree.\n\
                 Please run continue/abort from: {}",
                self.origin_git_dir.display()
            ))
        }
    }
}

// ============================================================================
// Fault Injection for Testing
// ============================================================================

/// Fault injection support for testing crash recovery.
///
/// This module provides controlled failure injection for testing the journal's
/// crash recovery guarantees per SPEC.md §4.2.2.
///
/// # Usage
///
/// ```ignore
/// use latticework::core::ops::journal::fault_injection;
///
/// // Set up to crash after 2 writes
/// fault_injection::set_crash_after(2);
///
/// // First write succeeds
/// journal.append_ref_update(&paths, "refs/heads/a", None, "oid1")?;
///
/// // Second write "crashes"
/// let result = journal.append_ref_update(&paths, "refs/heads/b", None, "oid2");
/// assert!(result.is_err());
///
/// // Clean up
/// fault_injection::reset();
/// ```
#[cfg(any(test, feature = "fault_injection"))]
pub mod fault_injection {
    use std::cell::Cell;

    // Thread-local storage for fault injection state.
    // This ensures each test thread has isolated state, preventing
    // interference when tests run in parallel.
    thread_local! {
        /// Counter for fault injection - crash after N writes.
        /// 0 means no crash simulation (disabled).
        static CRASH_AFTER_WRITES: Cell<usize> = const { Cell::new(0) };

        /// Current write count.
        static WRITE_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    /// Set the write count after which to simulate a crash.
    ///
    /// After `n` successful writes, the next write will fail with a
    /// simulated I/O error. Set to 0 to disable crash simulation.
    ///
    /// # Thread Safety
    ///
    /// Uses thread-local storage, so each test thread has isolated state.
    /// This prevents interference when tests run in parallel.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Crash on the 3rd write attempt
    /// fault_injection::set_crash_after(3);
    /// ```
    pub fn set_crash_after(n: usize) {
        CRASH_AFTER_WRITES.with(|c| c.set(n));
        WRITE_COUNT.with(|c| c.set(0));
    }

    /// Check if we should simulate a crash.
    ///
    /// This is called by `Journal::write()` before each write operation.
    /// Returns `true` if the crash threshold has been reached.
    ///
    /// # Thread Safety
    ///
    /// Uses thread-local storage for isolation between test threads.
    pub fn should_crash() -> bool {
        CRASH_AFTER_WRITES.with(|threshold_cell| {
            let threshold = threshold_cell.get();
            if threshold == 0 {
                return false;
            }
            WRITE_COUNT.with(|count_cell| {
                let count = count_cell.get() + 1;
                count_cell.set(count);
                count >= threshold
            })
        })
    }

    /// Reset fault injection state.
    ///
    /// Call this in test teardown to ensure clean state for subsequent tests.
    pub fn reset() {
        CRASH_AFTER_WRITES.with(|c| c.set(0));
        WRITE_COUNT.with(|c| c.set(0));
    }

    /// Get the current write count (for debugging).
    pub fn write_count() -> usize {
        WRITE_COUNT.with(|c| c.get())
    }
}

#[cfg(test)]
#[allow(deprecated)] // Tests exercise both old record_* and new append_* APIs
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_dir() -> TempDir {
        TempDir::new().expect("create temp dir")
    }

    /// Create test paths from a temp directory.
    /// For normal repos, git_dir == common_dir.
    fn create_test_paths(temp: &TempDir) -> LatticePaths {
        LatticePaths::new(temp.path().to_path_buf(), temp.path().to_path_buf())
    }

    mod op_id {
        use super::*;

        #[test]
        fn new_generates_unique_ids() {
            let id1 = OpId::new();
            let id2 = OpId::new();
            assert_ne!(id1, id2);
        }

        #[test]
        fn from_string_roundtrip() {
            let original = OpId::new();
            let recreated = OpId::from_string(original.as_str());
            assert_eq!(original, recreated);
        }

        #[test]
        fn display_formatting() {
            let id = OpId::from_string("test-id");
            assert_eq!(format!("{}", id), "test-id");
        }
    }

    mod op_phase {
        use super::*;

        #[test]
        fn is_finished() {
            assert!(!OpPhase::InProgress.is_finished());
            assert!(!OpPhase::Paused.is_finished());
            assert!(OpPhase::Committed.is_finished());
            assert!(OpPhase::RolledBack.is_finished());
        }

        #[test]
        fn is_resumable() {
            assert!(!OpPhase::InProgress.is_resumable());
            assert!(OpPhase::Paused.is_resumable());
            assert!(!OpPhase::Committed.is_resumable());
            assert!(!OpPhase::RolledBack.is_resumable());
        }

        #[test]
        fn is_active() {
            assert!(OpPhase::InProgress.is_active());
            assert!(!OpPhase::Paused.is_active());
            assert!(!OpPhase::Committed.is_active());
            assert!(!OpPhase::RolledBack.is_active());
        }
    }

    mod awaiting_reason {
        use super::*;

        #[test]
        fn rebase_conflict_serializes() {
            let reason = AwaitingReason::RebaseConflict;
            let json = serde_json::to_string(&reason).unwrap();
            assert!(json.contains("rebase_conflict"));

            let parsed: AwaitingReason = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, AwaitingReason::RebaseConflict);
        }

        #[test]
        fn rollback_incomplete_serializes() {
            let reason = AwaitingReason::RollbackIncomplete {
                failed_refs: vec!["refs/heads/feature".to_string()],
            };
            let json = serde_json::to_string(&reason).unwrap();
            assert!(json.contains("rollback_incomplete"));
            assert!(json.contains("refs/heads/feature"));

            let parsed: AwaitingReason = serde_json::from_str(&json).unwrap();
            match parsed {
                AwaitingReason::RollbackIncomplete { failed_refs } => {
                    assert_eq!(failed_refs.len(), 1);
                    assert_eq!(failed_refs[0], "refs/heads/feature");
                }
                _ => panic!("wrong variant"),
            }
        }

        #[test]
        fn verification_failed_serializes() {
            let reason = AwaitingReason::VerificationFailed {
                evidence: "hash mismatch".to_string(),
            };
            let json = serde_json::to_string(&reason).unwrap();
            assert!(json.contains("verification_failed"));
            assert!(json.contains("hash mismatch"));

            let parsed: AwaitingReason = serde_json::from_str(&json).unwrap();
            match parsed {
                AwaitingReason::VerificationFailed { evidence } => {
                    assert_eq!(evidence, "hash mismatch");
                }
                _ => panic!("wrong variant"),
            }
        }
    }

    mod touched_ref {
        use super::*;

        #[test]
        fn new_creates_with_values() {
            let tr = TouchedRef::new("refs/heads/feature", Some("abc123".to_string()));
            assert_eq!(tr.refname, "refs/heads/feature");
            assert_eq!(tr.expected_old, Some("abc123".to_string()));
        }

        #[test]
        fn new_with_none() {
            let tr = TouchedRef::new("refs/heads/new-branch", None);
            assert_eq!(tr.refname, "refs/heads/new-branch");
            assert!(tr.expected_old.is_none());
        }

        #[test]
        fn serializes_roundtrip() {
            let tr = TouchedRef::new("refs/heads/test", Some("old-oid".to_string()));
            let json = serde_json::to_string(&tr).unwrap();
            let parsed: TouchedRef = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, tr);
        }

        #[test]
        fn serializes_with_none() {
            let tr = TouchedRef::new("refs/heads/new", None);
            let json = serde_json::to_string(&tr).unwrap();
            let parsed: TouchedRef = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, tr);
        }
    }

    mod plan_schema_version {
        use super::*;

        #[test]
        fn version_is_one() {
            // Current schema version should be 1
            assert_eq!(PLAN_SCHEMA_VERSION, 1);
        }
    }

    mod journal {
        use super::*;

        #[test]
        fn new_creates_valid_journal() {
            let journal = Journal::new("test-command");

            assert_eq!(journal.command, "test-command");
            assert_eq!(journal.phase, OpPhase::InProgress);
            assert!(journal.steps.is_empty());
            assert!(journal.finished_at.is_none());
        }

        #[test]
        fn record_ref_update() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/feature", Some("abc123".to_string()), "def456");

            assert_eq!(journal.steps.len(), 1);
            match &journal.steps[0].kind {
                StepKind::RefUpdate {
                    refname,
                    old_oid,
                    new_oid,
                } => {
                    assert_eq!(refname, "refs/heads/feature");
                    assert_eq!(old_oid, &Some("abc123".to_string()));
                    assert_eq!(new_oid, "def456");
                }
                _ => panic!("wrong step kind"),
            }
        }

        #[test]
        fn record_metadata_write() {
            let mut journal = Journal::new("test");
            journal.record_metadata_write("feature", Some("old-oid".to_string()), "new-oid");

            assert_eq!(journal.steps.len(), 1);
            match &journal.steps[0].kind {
                StepKind::MetadataWrite {
                    branch,
                    old_ref_oid,
                    new_ref_oid,
                } => {
                    assert_eq!(branch, "feature");
                    assert_eq!(old_ref_oid, &Some("old-oid".to_string()));
                    assert_eq!(new_ref_oid, "new-oid");
                }
                _ => panic!("wrong step kind"),
            }
        }

        #[test]
        fn record_metadata_delete() {
            let mut journal = Journal::new("test");
            journal.record_metadata_delete("feature", "deleted-oid");

            assert_eq!(journal.steps.len(), 1);
            match &journal.steps[0].kind {
                StepKind::MetadataDelete {
                    branch,
                    old_ref_oid,
                } => {
                    assert_eq!(branch, "feature");
                    assert_eq!(old_ref_oid, "deleted-oid");
                }
                _ => panic!("wrong step kind"),
            }
        }

        #[test]
        fn record_checkpoint() {
            let mut journal = Journal::new("test");
            journal.record_checkpoint("before-rebase");

            assert_eq!(journal.steps.len(), 1);
            match &journal.steps[0].kind {
                StepKind::Checkpoint { name } => {
                    assert_eq!(name, "before-rebase");
                }
                _ => panic!("wrong step kind"),
            }
        }

        #[test]
        fn record_git_process() {
            let mut journal = Journal::new("test");
            journal.record_git_process(
                vec!["rebase".to_string(), "--onto".to_string()],
                "rebase feature onto main",
            );

            assert_eq!(journal.steps.len(), 1);
            match &journal.steps[0].kind {
                StepKind::GitProcess { args, description } => {
                    assert_eq!(args, &vec!["rebase".to_string(), "--onto".to_string()]);
                    assert_eq!(description, "rebase feature onto main");
                }
                _ => panic!("wrong step kind"),
            }
        }

        #[test]
        fn record_conflict_paused() {
            let mut journal = Journal::new("test");
            journal.record_conflict_paused(
                "feature",
                "rebase",
                vec!["branch-a".to_string(), "branch-b".to_string()],
            );

            assert_eq!(journal.steps.len(), 1);
            match &journal.steps[0].kind {
                StepKind::ConflictPaused {
                    branch,
                    git_state,
                    remaining_branches,
                    remaining_steps_json,
                } => {
                    assert_eq!(branch, "feature");
                    assert_eq!(git_state, "rebase");
                    assert_eq!(remaining_branches.len(), 2);
                    assert!(remaining_steps_json.is_none());
                }
                _ => panic!("wrong step kind"),
            }
        }

        #[test]
        fn record_conflict_paused_with_remaining_steps_json() {
            let mut journal = Journal::new("test");
            let remaining_json =
                r#"[{"WriteMetadataCas":{"branch":"b","old_oid":"abc","new":"def"}}]"#;
            journal.record_conflict_paused_with_remaining_steps(
                "feature",
                "rebase",
                vec!["branch-b".to_string()],
                Some(remaining_json.to_string()),
            );

            assert_eq!(journal.steps.len(), 1);
            match &journal.steps[0].kind {
                StepKind::ConflictPaused {
                    branch,
                    git_state,
                    remaining_branches,
                    remaining_steps_json,
                } => {
                    assert_eq!(branch, "feature");
                    assert_eq!(git_state, "rebase");
                    assert_eq!(remaining_branches.len(), 1);
                    assert_eq!(remaining_steps_json.as_deref(), Some(remaining_json));
                }
                _ => panic!("wrong step kind"),
            }
        }

        #[test]
        fn remaining_steps_json_returns_none_when_no_conflict() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/feature", None, "abc123");
            journal.record_checkpoint("done");

            assert!(journal.remaining_steps_json().is_none());
        }

        #[test]
        fn remaining_steps_json_returns_json_from_conflict_paused() {
            let mut journal = Journal::new("test");
            let remaining_json =
                r#"[{"WriteMetadataCas":{"branch":"b","old_oid":"abc","new":"def"}}]"#;
            journal.record_conflict_paused_with_remaining_steps(
                "feature",
                "rebase",
                vec!["branch-b".to_string()],
                Some(remaining_json.to_string()),
            );

            assert_eq!(journal.remaining_steps_json(), Some(remaining_json));
        }

        #[test]
        fn has_remaining_steps_false_when_no_conflict() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/feature", None, "abc123");

            assert!(!journal.has_remaining_steps());
        }

        #[test]
        fn has_remaining_steps_false_when_empty_json() {
            let mut journal = Journal::new("test");
            journal.record_conflict_paused_with_remaining_steps(
                "feature",
                "rebase",
                vec![],
                Some("[]".to_string()),
            );

            assert!(!journal.has_remaining_steps());
        }

        #[test]
        fn has_remaining_steps_true_when_steps_present() {
            let mut journal = Journal::new("test");
            let remaining_json =
                r#"[{"WriteMetadataCas":{"branch":"b","old_oid":"abc","new":"def"}}]"#;
            journal.record_conflict_paused_with_remaining_steps(
                "feature",
                "rebase",
                vec!["branch-b".to_string()],
                Some(remaining_json.to_string()),
            );

            assert!(journal.has_remaining_steps());
        }

        #[test]
        fn paused_branch_returns_branch_when_conflict_paused() {
            let mut journal = Journal::new("test");
            journal.record_conflict_paused("feature-xyz", "rebase", vec![]);

            assert_eq!(journal.paused_branch(), Some("feature-xyz"));
        }

        #[test]
        fn paused_branch_returns_none_when_not_paused() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/feature", None, "abc123");

            assert!(journal.paused_branch().is_none());
        }

        #[test]
        fn remaining_branches_returns_list_from_conflict_paused() {
            let mut journal = Journal::new("test");
            journal.record_conflict_paused(
                "feature",
                "rebase",
                vec!["branch-a".to_string(), "branch-b".to_string()],
            );

            let branches = journal.remaining_branches();
            assert_eq!(branches.len(), 2);
            assert_eq!(branches[0], "branch-a");
            assert_eq!(branches[1], "branch-b");
        }

        #[test]
        fn remaining_branches_returns_empty_when_not_paused() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/feature", None, "abc123");

            assert!(journal.remaining_branches().is_empty());
        }

        #[test]
        fn commit_sets_phase_and_timestamp() {
            let mut journal = Journal::new("test");
            assert!(journal.finished_at.is_none());

            journal.commit();

            assert_eq!(journal.phase, OpPhase::Committed);
            assert!(journal.finished_at.is_some());
        }

        #[test]
        fn pause_sets_phase() {
            let mut journal = Journal::new("test");
            journal.pause();
            assert_eq!(journal.phase, OpPhase::Paused);
        }

        #[test]
        fn rollback_sets_phase_and_timestamp() {
            let mut journal = Journal::new("test");
            journal.rollback();

            assert_eq!(journal.phase, OpPhase::RolledBack);
            assert!(journal.finished_at.is_some());
        }

        #[test]
        fn write_and_read_roundtrip() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let mut journal = Journal::new("restack");
            journal.record_ref_update("refs/heads/feature", None, "abc123");
            journal.record_checkpoint("midpoint");
            journal.commit();

            journal.write(&paths).expect("write");

            let loaded = Journal::read(&paths, &journal.op_id).expect("read");

            assert_eq!(loaded.op_id, journal.op_id);
            assert_eq!(loaded.command, journal.command);
            assert_eq!(loaded.phase, journal.phase);
            assert_eq!(loaded.steps.len(), journal.steps.len());
        }

        #[test]
        fn read_nonexistent_returns_error() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let result = Journal::read(&paths, &OpId::from_string("nonexistent"));
            assert!(matches!(result, Err(JournalError::NotFound(_))));
        }

        #[test]
        fn list_returns_journals_by_mtime() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal1 = Journal::new("first");
            journal1.write(&paths).expect("write 1");

            // Small delay to ensure different mtime
            std::thread::sleep(std::time::Duration::from_millis(10));

            let journal2 = Journal::new("second");
            journal2.write(&paths).expect("write 2");

            let ids = Journal::list(&paths).expect("list");

            assert_eq!(ids.len(), 2);
            // Most recent first
            assert_eq!(ids[0], journal2.op_id);
            assert_eq!(ids[1], journal1.op_id);
        }

        #[test]
        fn list_empty_dir() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let ids = Journal::list(&paths).expect("list");
            assert!(ids.is_empty());
        }

        #[test]
        fn most_recent_returns_latest() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal1 = Journal::new("first");
            journal1.write(&paths).expect("write 1");

            std::thread::sleep(std::time::Duration::from_millis(10));

            let journal2 = Journal::new("second");
            journal2.write(&paths).expect("write 2");

            let recent = Journal::most_recent(&paths)
                .expect("most_recent")
                .expect("should have journal");

            assert_eq!(recent.op_id, journal2.op_id);
        }

        #[test]
        fn most_recent_returns_none_when_empty() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let recent = Journal::most_recent(&paths).expect("most_recent");
            assert!(recent.is_none());
        }

        #[test]
        fn delete_removes_file() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal = Journal::new("test");
            journal.write(&paths).expect("write");

            let path = journal.file_path(&paths);
            assert!(path.exists());

            journal.delete(&paths).expect("delete");
            assert!(!path.exists());
        }

        #[test]
        fn ref_updates_for_rollback() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/a", None, "oid1");
            journal.record_checkpoint("checkpoint");
            journal.record_ref_update("refs/heads/b", Some("old".to_string()), "oid2");
            journal.record_metadata_write("branch", None, "meta-oid");

            let updates = journal.ref_updates_for_rollback();

            // Should be in reverse order, excluding checkpoints
            assert_eq!(updates.len(), 3);

            // First should be the last step (metadata write)
            assert!(matches!(updates[0], StepKind::MetadataWrite { .. }));
            // Second should be the second ref update
            assert!(
                matches!(updates[1], StepKind::RefUpdate { refname, .. } if refname == "refs/heads/b")
            );
            // Third should be the first ref update
            assert!(
                matches!(updates[2], StepKind::RefUpdate { refname, .. } if refname == "refs/heads/a")
            );
        }

        #[test]
        fn can_fully_rollback_ref_only() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/a", None, "oid1");
            journal.record_ref_update("refs/heads/b", Some("old".to_string()), "oid2");

            assert!(journal.can_fully_rollback());
        }

        #[test]
        fn can_fully_rollback_with_metadata_create() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/a", None, "oid1");
            // Metadata was created (old_ref_oid is None) - can rollback
            journal.record_metadata_write("branch", None, "new-oid");

            assert!(journal.can_fully_rollback());
        }

        #[test]
        fn cannot_fully_rollback_with_metadata_update() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/a", None, "oid1");
            // Metadata was updated (old_ref_oid is Some) - can't restore old content
            journal.record_metadata_write("branch", Some("old".to_string()), "new");

            assert!(!journal.can_fully_rollback());
        }

        #[test]
        fn cannot_fully_rollback_with_metadata_delete() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/a", None, "oid1");
            // Metadata was deleted - can't restore deleted content
            journal.record_metadata_delete("branch", "deleted-oid");

            assert!(!journal.can_fully_rollback());
        }

        #[test]
        fn rollback_summary_categorizes_correctly() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/feature", None, "oid1");
            journal.record_ref_update("refs/heads/other", Some("old".to_string()), "new");
            journal.record_metadata_write("created", None, "new-oid");
            journal.record_metadata_write("updated", Some("old".to_string()), "new");
            journal.record_metadata_delete("deleted", "old-oid");
            journal.record_checkpoint("midpoint");

            let summary = journal.rollback_summary();

            assert_eq!(summary.ref_updates.len(), 2);
            assert_eq!(summary.metadata_creates.len(), 1);
            assert_eq!(summary.metadata_updates.len(), 1);
            assert_eq!(summary.metadata_deletes.len(), 1);
            assert!(!summary.is_complete()); // Has updates and deletes
            assert_eq!(summary.total_items(), 5);
        }

        #[test]
        fn rollback_summary_complete_when_no_problematic_steps() {
            let mut journal = Journal::new("test");
            journal.record_ref_update("refs/heads/feature", None, "oid1");
            journal.record_metadata_write("created", None, "new-oid");
            journal.record_checkpoint("done");

            let summary = journal.rollback_summary();

            assert!(summary.is_complete()); // No updates or deletes
        }

        #[test]
        fn worktree_shares_journals_via_common_dir() {
            // Simulate a worktree where git_dir != common_dir
            let temp = create_test_dir();
            let common_dir = temp.path().to_path_buf();
            let worktree_git_dir = temp.path().join("worktrees").join("feature");

            // Parent repo paths (uses common_dir for storage)
            let parent_paths = LatticePaths::new(common_dir.clone(), common_dir.clone());

            // Worktree paths (different git_dir, same common_dir)
            let worktree_paths = LatticePaths::new(worktree_git_dir, common_dir);

            // Write journal from parent
            let journal = Journal::new("restack");
            journal.write(&parent_paths).expect("write from parent");

            // Read journal from worktree - should see same journal
            let loaded =
                Journal::read(&worktree_paths, &journal.op_id).expect("read from worktree");
            assert_eq!(loaded.op_id, journal.op_id);

            // List from worktree should show the journal
            let ids = Journal::list(&worktree_paths).expect("list from worktree");
            assert_eq!(ids.len(), 1);
            assert_eq!(ids[0], journal.op_id);
        }
    }

    mod op_state {
        use super::*;

        #[test]
        fn from_journal() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);
            let work_dir = Some(temp.path().join("workdir"));

            let journal = Journal::new("test-cmd");
            let touched = vec![TouchedRef::new(
                "refs/heads/feature",
                Some("abc123".to_string()),
            )];
            let state = OpState::from_journal(
                &journal,
                &paths,
                work_dir.clone(),
                "sha256:test-digest".to_string(),
                touched.clone(),
            );

            assert_eq!(state.op_id, journal.op_id);
            assert_eq!(state.command, "test-cmd");
            assert_eq!(state.phase, OpPhase::InProgress);
            assert_eq!(state.origin_git_dir, paths.git_dir);
            assert_eq!(state.origin_work_dir, work_dir);
            assert_eq!(state.plan_digest, "sha256:test-digest");
            assert_eq!(state.plan_schema_version, PLAN_SCHEMA_VERSION);
            assert_eq!(state.touched_refs.len(), 1);
            assert_eq!(state.touched_refs[0].refname, "refs/heads/feature");
            assert!(state.awaiting_reason.is_none());
        }

        #[test]
        fn from_journal_bare_repo() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal = Journal::new("test-cmd");
            // Bare repo has no work_dir
            let state = OpState::from_journal(
                &journal,
                &paths,
                None,
                "sha256:bare-digest".to_string(),
                vec![],
            );

            assert_eq!(state.origin_git_dir, paths.git_dir);
            assert!(state.origin_work_dir.is_none());
            assert_eq!(state.plan_digest, "sha256:bare-digest");
        }

        #[test]
        fn write_and_read_roundtrip() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);
            let work_dir = Some(temp.path().join("workdir"));

            let journal = Journal::new("test");
            let touched = vec![TouchedRef::new("refs/heads/main", None)];
            let state = OpState::from_journal(
                &journal,
                &paths,
                work_dir,
                "sha256:roundtrip".to_string(),
                touched,
            );
            state.write(&paths).expect("write");

            let loaded = OpState::read(&paths).expect("read").expect("should exist");

            assert_eq!(loaded.op_id, state.op_id);
            assert_eq!(loaded.command, state.command);
            assert_eq!(loaded.phase, state.phase);
            assert_eq!(loaded.origin_git_dir, state.origin_git_dir);
            assert_eq!(loaded.origin_work_dir, state.origin_work_dir);
        }

        #[test]
        fn read_nonexistent_returns_none() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let result = OpState::read(&paths).expect("read");
            assert!(result.is_none());
        }

        /// Helper to create OpState with default plan values for tests.
        fn create_test_op_state(
            journal: &Journal,
            paths: &LatticePaths,
            work_dir: Option<PathBuf>,
        ) -> OpState {
            OpState::from_journal(journal, paths, work_dir, "sha256:test".to_string(), vec![])
        }

        #[test]
        fn exists_check() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            assert!(!OpState::exists(&paths));

            let journal = Journal::new("test");
            let state = create_test_op_state(&journal, &paths, None);
            state.write(&paths).expect("write");

            assert!(OpState::exists(&paths));
        }

        #[test]
        fn remove_clears_file() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal = Journal::new("test");
            let state = create_test_op_state(&journal, &paths, None);
            state.write(&paths).expect("write");

            assert!(OpState::exists(&paths));

            OpState::remove(&paths).expect("remove");

            assert!(!OpState::exists(&paths));
        }

        #[test]
        fn remove_nonexistent_ok() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            // Should not error
            OpState::remove(&paths).expect("remove nonexistent");
        }

        #[test]
        fn update_phase() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal = Journal::new("test");
            let mut state = create_test_op_state(&journal, &paths, None);
            state.write(&paths).expect("write");

            state.update_phase(OpPhase::Paused, &paths).expect("update");

            let loaded = OpState::read(&paths).expect("read").expect("should exist");
            assert_eq!(loaded.phase, OpPhase::Paused);
        }

        #[test]
        fn pause_with_reason_sets_fields() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal = Journal::new("test");
            let mut state = create_test_op_state(&journal, &paths, None);
            state.write(&paths).expect("write");

            state
                .pause_with_reason(AwaitingReason::RebaseConflict, &paths)
                .expect("pause");

            let loaded = OpState::read(&paths).expect("read").expect("should exist");
            assert_eq!(loaded.phase, OpPhase::Paused);
            assert_eq!(loaded.awaiting_reason, Some(AwaitingReason::RebaseConflict));
        }

        #[test]
        fn path_uses_common_dir() {
            // Simulate worktree: git_dir != common_dir
            let common_dir = PathBuf::from("/repo/.git");
            let git_dir = PathBuf::from("/repo/.git/worktrees/feature");
            let paths = LatticePaths::new(git_dir, common_dir);

            let path = OpState::path(&paths);
            // Should use common_dir, not git_dir
            assert_eq!(path, PathBuf::from("/repo/.git/lattice/op-state.json"));
        }

        #[test]
        fn check_origin_worktree_same() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal = Journal::new("test");
            let state = create_test_op_state(&journal, &paths, None);

            // Same worktree should pass
            assert!(state.check_origin_worktree(&paths.git_dir).is_ok());
        }

        #[test]
        fn check_origin_worktree_different() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            let journal = Journal::new("test");
            let state = create_test_op_state(&journal, &paths, None);

            // Different worktree should fail
            let other_git_dir = PathBuf::from("/other/worktree/.git");
            let result = state.check_origin_worktree(&other_git_dir);
            assert!(result.is_err());

            let err_msg = result.unwrap_err();
            assert!(err_msg.contains("different worktree"));
            assert!(err_msg.contains(&paths.git_dir.display().to_string()));
        }

        #[test]
        fn worktree_shares_op_state_via_common_dir() {
            // Simulate a worktree where git_dir != common_dir
            let temp = create_test_dir();
            let common_dir = temp.path().to_path_buf();
            let worktree_git_dir = temp.path().join("worktrees").join("feature");

            // Parent repo paths
            let parent_paths = LatticePaths::new(common_dir.clone(), common_dir.clone());

            // Worktree paths (different git_dir, same common_dir)
            let worktree_paths = LatticePaths::new(worktree_git_dir, common_dir);

            // Write op-state from parent
            let journal = Journal::new("restack");
            let state = create_test_op_state(&journal, &parent_paths, None);
            state.write(&parent_paths).expect("write from parent");

            // Read op-state from worktree - should see same state
            let loaded = OpState::read(&worktree_paths)
                .expect("read from worktree")
                .expect("should exist");
            assert_eq!(loaded.op_id, state.op_id);

            // exists() from worktree should return true
            assert!(OpState::exists(&worktree_paths));
        }
    }

    mod journal_error {
        use super::*;

        #[test]
        fn error_display() {
            let err = JournalError::NotFound("abc123".into());
            assert!(err.to_string().contains("abc123"));
            assert!(err.to_string().contains("not found"));

            let err = JournalError::InvalidState("bad state".into());
            assert!(err.to_string().contains("invalid"));
        }
    }

    /// Fault injection tests for crash recovery per SPEC.md §4.2.2.
    ///
    /// These tests verify that:
    /// 1. Crashes after N writes are simulated correctly
    /// 2. Journal recovery works after simulated crashes
    /// 3. Partial writes are detectable
    mod fault_injection_tests {
        use super::*;

        /// Reset fault injection after each test.
        fn cleanup() {
            fault_injection::reset();
        }

        #[test]
        fn crash_after_first_step_leaves_no_journal() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);
            cleanup();

            // Set up to crash on first write
            fault_injection::set_crash_after(1);

            let mut journal = Journal::new("test-op");

            // This should "crash"
            let result = journal.append_ref_update(&paths, "refs/heads/feature", None, "abc123");
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("simulated crash"));

            // Journal file should not exist (crash before write completed)
            let journal_path = paths.repo_op_journal_path(journal.op_id.as_str());
            assert!(!journal_path.exists());

            cleanup();
        }

        #[test]
        fn crash_after_second_step_recovers_first() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);
            cleanup();

            let mut journal = Journal::new("test-op");
            let op_id = journal.op_id.clone();

            // First step succeeds
            journal
                .append_ref_update(&paths, "refs/heads/feature-a", None, "abc123")
                .expect("first append should succeed");

            // Set up to crash on next write
            fault_injection::set_crash_after(1);

            // Second step "crashes"
            let result = journal.append_ref_update(&paths, "refs/heads/feature-b", None, "def456");
            assert!(result.is_err());

            // Recovery: reload journal
            cleanup();
            let recovered = Journal::read(&paths, &op_id).expect("should recover journal");

            // Should have exactly one step (the successful first one)
            assert_eq!(recovered.steps.len(), 1);
            match &recovered.steps[0].kind {
                StepKind::RefUpdate { refname, .. } => {
                    assert_eq!(refname, "refs/heads/feature-a");
                }
                _ => panic!("Expected RefUpdate step"),
            }
        }

        #[test]
        fn all_steps_persisted_on_success() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);
            cleanup();

            let mut journal = Journal::new("test-op");
            let op_id = journal.op_id.clone();

            // Add multiple steps - all should succeed
            journal
                .append_ref_update(&paths, "refs/heads/a", None, "111")
                .expect("step 1");
            journal
                .append_ref_update(&paths, "refs/heads/b", None, "222")
                .expect("step 2");
            journal
                .append_ref_update(&paths, "refs/heads/c", None, "333")
                .expect("step 3");
            journal
                .append_checkpoint(&paths, "done")
                .expect("checkpoint");

            // Reload and verify all steps were persisted
            let recovered = Journal::read(&paths, &op_id).expect("read");
            assert_eq!(recovered.steps.len(), 4);
        }

        #[test]
        fn partial_write_detected_as_error() {
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);
            cleanup();

            let mut journal = Journal::new("test-op");
            let op_id = journal.op_id.clone();

            // Write a valid journal
            journal
                .append_ref_update(&paths, "refs/heads/a", None, "111")
                .expect("write");

            // Manually corrupt the journal file by truncating it
            let journal_path = paths.repo_op_journal_path(op_id.as_str());
            let content = std::fs::read_to_string(&journal_path).expect("read");
            // Truncate to simulate partial write (corrupt JSON)
            std::fs::write(&journal_path, &content[..content.len() / 2]).expect("truncate");

            // Read should fail with parse error (not silent corruption)
            let result = Journal::read(&paths, &op_id);
            assert!(result.is_err());
            // Should be a JSON parse error
            assert!(result.unwrap_err().to_string().contains("json"));
        }

        #[test]
        fn fault_injection_reset_clears_state() {
            cleanup();

            fault_injection::set_crash_after(5);
            assert_eq!(fault_injection::write_count(), 0);

            // Simulate some writes
            fault_injection::should_crash();
            fault_injection::should_crash();
            assert_eq!(fault_injection::write_count(), 2);

            // Reset should clear everything
            cleanup();
            assert_eq!(fault_injection::write_count(), 0);
        }

        #[test]
        fn disabled_fault_injection_allows_all_writes() {
            cleanup();

            // With threshold 0, should_crash always returns false
            fault_injection::set_crash_after(0);

            for _ in 0..100 {
                assert!(!fault_injection::should_crash());
            }

            cleanup();
        }

        #[test]
        fn crash_threshold_triggers_at_exact_count() {
            cleanup();

            // Should crash on the 3rd write
            fault_injection::set_crash_after(3);

            // First two succeed
            assert!(!fault_injection::should_crash()); // 1
            assert!(!fault_injection::should_crash()); // 2

            // Third crashes
            assert!(fault_injection::should_crash()); // 3

            // Subsequent also "crash" (threshold reached)
            assert!(fault_injection::should_crash()); // 4

            cleanup();
        }

        #[test]
        fn append_methods_all_use_write() {
            // This test verifies that all append_* methods go through write()
            // by checking that fault injection affects them all
            let temp = create_test_dir();
            let paths = create_test_paths(&temp);

            // Test each append method with crash injection
            let methods: Vec<(
                &str,
                Box<dyn Fn(&mut Journal, &LatticePaths) -> Result<(), JournalError>>,
            )> = vec![
                (
                    "append_ref_update",
                    Box::new(|j, p| j.append_ref_update(p, "refs/heads/x", None, "oid")),
                ),
                (
                    "append_metadata_write",
                    Box::new(|j, p| j.append_metadata_write(p, "branch", None, "oid")),
                ),
                (
                    "append_metadata_delete",
                    Box::new(|j, p| j.append_metadata_delete(p, "branch", "oid")),
                ),
                (
                    "append_checkpoint",
                    Box::new(|j, p| j.append_checkpoint(p, "test")),
                ),
                (
                    "append_git_process",
                    Box::new(|j, p| j.append_git_process(p, vec!["git".into()], "desc")),
                ),
                (
                    "append_conflict_paused",
                    Box::new(|j, p| j.append_conflict_paused(p, "branch", "rebase", vec![], None)),
                ),
            ];

            for (name, method) in methods {
                cleanup();
                fault_injection::set_crash_after(1);

                let mut journal = Journal::new("test");
                let result = method(&mut journal, &paths);

                assert!(result.is_err(), "{} should fail with fault injection", name);
                assert!(
                    result.unwrap_err().to_string().contains("simulated crash"),
                    "{} should have simulated crash error",
                    name
                );
            }

            cleanup();
        }
    }
}
