//! core::ops::journal
//!
//! Operation journal for crash safety and undo.
//!
//! # Architecture
//!
//! The journal is the source of truth for:
//! - Rollback (`abort`) - restore refs to pre-operation state
//! - Resume (`continue`) - complete a paused operation
//! - Undo last completed operation
//!
//! # Storage
//!
//! - `.git/lattice/ops/<op_id>.json` - Journal files (append-only with fsync)
//! - `.git/lattice/op-state.json` - Current operation marker
//!
//! # Invariants
//!
//! - Journals must be written with fsync at each step boundary
//! - Interrupted commands must be recoverable via the journal
//! - All ref changes must be recorded with before/after OIDs
//!
//! # Example
//!
//! ```ignore
//! use latticework::core::ops::journal::{Journal, StepKind};
//!
//! // Create a new journal for an operation
//! let mut journal = Journal::new("restack");
//!
//! // Record steps as they happen
//! journal.record_ref_update(
//!     "refs/heads/feature",
//!     Some("abc123...".to_string()),
//!     "def456...",
//! );
//!
//! // Write to disk with fsync
//! journal.write(git_dir)?;
//!
//! // Mark as committed when done
//! journal.commit();
//! journal.write(git_dir)?;
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

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
    ConflictPaused {
        /// The branch where the conflict occurred.
        branch: String,
        /// Type of git operation that conflicted (rebase, merge, etc.).
        git_state: String,
        /// Branches remaining to process after conflict resolution.
        remaining_branches: Vec<String>,
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
    pub fn ops_dir(git_dir: &Path) -> PathBuf {
        git_dir.join("lattice").join("ops")
    }

    /// Get the path to this journal's file.
    pub fn file_path(&self, git_dir: &Path) -> PathBuf {
        Self::ops_dir(git_dir).join(format!("{}.json", self.op_id))
    }

    /// Add a step to the journal.
    pub fn add_step(&mut self, kind: StepKind) {
        self.steps.push(JournalStep {
            kind,
            timestamp: UtcTimestamp::now(),
        });
    }

    /// Record a ref update step.
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

    /// Record a metadata write step.
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

    /// Record a metadata delete step.
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

    /// Record a checkpoint.
    pub fn record_checkpoint(&mut self, name: impl Into<String>) {
        self.add_step(StepKind::Checkpoint { name: name.into() });
    }

    /// Record a git process execution.
    pub fn record_git_process(&mut self, args: Vec<String>, description: impl Into<String>) {
        self.add_step(StepKind::GitProcess {
            args,
            description: description.into(),
        });
    }

    /// Record that a conflict paused the operation.
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
    pub fn write(&self, git_dir: &Path) -> Result<(), JournalError> {
        let dir = Self::ops_dir(git_dir);
        fs::create_dir_all(&dir)?;

        let path = self.file_path(git_dir);
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
    pub fn read(git_dir: &Path, op_id: &OpId) -> Result<Self, JournalError> {
        let path = Self::ops_dir(git_dir).join(format!("{}.json", op_id));

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
    pub fn list(git_dir: &Path) -> Result<Vec<OpId>, JournalError> {
        let dir = Self::ops_dir(git_dir);
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
    pub fn most_recent(git_dir: &Path) -> Result<Option<Self>, JournalError> {
        let ids = Self::list(git_dir)?;
        match ids.first() {
            Some(id) => Ok(Some(Self::read(git_dir, id)?)),
            None => Ok(None),
        }
    }

    /// Delete this journal from disk.
    pub fn delete(&self, git_dir: &Path) -> Result<(), JournalError> {
        let path = self.file_path(git_dir);
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
}

/// The op-state marker indicating an operation is in progress.
///
/// This file exists only while a Lattice operation is executing or
/// paused waiting for user intervention. It prevents other Lattice
/// commands from running until the current operation is resolved.
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
}

impl OpState {
    /// Create a new op-state marker from a journal.
    pub fn from_journal(journal: &Journal) -> Self {
        Self {
            op_id: journal.op_id.clone(),
            command: journal.command.clone(),
            phase: journal.phase.clone(),
            updated_at: UtcTimestamp::now(),
        }
    }

    /// Path to the op-state file.
    pub fn path(git_dir: &Path) -> PathBuf {
        git_dir.join("lattice").join("op-state.json")
    }

    /// Write the op-state marker to disk.
    pub fn write(&self, git_dir: &Path) -> Result<(), JournalError> {
        let dir = git_dir.join("lattice");
        fs::create_dir_all(&dir)?;

        let path = Self::path(git_dir);
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
    pub fn read(git_dir: &Path) -> Result<Option<Self>, JournalError> {
        let path = Self::path(git_dir);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)?;
        let state = serde_json::from_str(&content)?;
        Ok(Some(state))
    }

    /// Remove the op-state marker.
    pub fn remove(git_dir: &Path) -> Result<(), JournalError> {
        let path = Self::path(git_dir);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    /// Check if an op-state marker exists.
    pub fn exists(git_dir: &Path) -> bool {
        Self::path(git_dir).exists()
    }

    /// Update the phase and write to disk.
    pub fn update_phase(&mut self, phase: OpPhase, git_dir: &Path) -> Result<(), JournalError> {
        self.phase = phase;
        self.updated_at = UtcTimestamp::now();
        self.write(git_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_dir() -> TempDir {
        TempDir::new().expect("create temp dir")
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
                } => {
                    assert_eq!(branch, "feature");
                    assert_eq!(git_state, "rebase");
                    assert_eq!(remaining_branches.len(), 2);
                }
                _ => panic!("wrong step kind"),
            }
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
            let git_dir = temp.path();

            let mut journal = Journal::new("restack");
            journal.record_ref_update("refs/heads/feature", None, "abc123");
            journal.record_checkpoint("midpoint");
            journal.commit();

            journal.write(git_dir).expect("write");

            let loaded = Journal::read(git_dir, &journal.op_id).expect("read");

            assert_eq!(loaded.op_id, journal.op_id);
            assert_eq!(loaded.command, journal.command);
            assert_eq!(loaded.phase, journal.phase);
            assert_eq!(loaded.steps.len(), journal.steps.len());
        }

        #[test]
        fn read_nonexistent_returns_error() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let result = Journal::read(git_dir, &OpId::from_string("nonexistent"));
            assert!(matches!(result, Err(JournalError::NotFound(_))));
        }

        #[test]
        fn list_returns_journals_by_mtime() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let journal1 = Journal::new("first");
            journal1.write(git_dir).expect("write 1");

            // Small delay to ensure different mtime
            std::thread::sleep(std::time::Duration::from_millis(10));

            let journal2 = Journal::new("second");
            journal2.write(git_dir).expect("write 2");

            let ids = Journal::list(git_dir).expect("list");

            assert_eq!(ids.len(), 2);
            // Most recent first
            assert_eq!(ids[0], journal2.op_id);
            assert_eq!(ids[1], journal1.op_id);
        }

        #[test]
        fn list_empty_dir() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let ids = Journal::list(git_dir).expect("list");
            assert!(ids.is_empty());
        }

        #[test]
        fn most_recent_returns_latest() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let journal1 = Journal::new("first");
            journal1.write(git_dir).expect("write 1");

            std::thread::sleep(std::time::Duration::from_millis(10));

            let journal2 = Journal::new("second");
            journal2.write(git_dir).expect("write 2");

            let recent = Journal::most_recent(git_dir)
                .expect("most_recent")
                .expect("should have journal");

            assert_eq!(recent.op_id, journal2.op_id);
        }

        #[test]
        fn most_recent_returns_none_when_empty() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let recent = Journal::most_recent(git_dir).expect("most_recent");
            assert!(recent.is_none());
        }

        #[test]
        fn delete_removes_file() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let journal = Journal::new("test");
            journal.write(git_dir).expect("write");

            let path = journal.file_path(git_dir);
            assert!(path.exists());

            journal.delete(git_dir).expect("delete");
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
    }

    mod op_state {
        use super::*;

        #[test]
        fn from_journal() {
            let journal = Journal::new("test-cmd");
            let state = OpState::from_journal(&journal);

            assert_eq!(state.op_id, journal.op_id);
            assert_eq!(state.command, "test-cmd");
            assert_eq!(state.phase, OpPhase::InProgress);
        }

        #[test]
        fn write_and_read_roundtrip() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let journal = Journal::new("test");
            let state = OpState::from_journal(&journal);
            state.write(git_dir).expect("write");

            let loaded = OpState::read(git_dir).expect("read").expect("should exist");

            assert_eq!(loaded.op_id, state.op_id);
            assert_eq!(loaded.command, state.command);
            assert_eq!(loaded.phase, state.phase);
        }

        #[test]
        fn read_nonexistent_returns_none() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let result = OpState::read(git_dir).expect("read");
            assert!(result.is_none());
        }

        #[test]
        fn exists_check() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            assert!(!OpState::exists(git_dir));

            let journal = Journal::new("test");
            let state = OpState::from_journal(&journal);
            state.write(git_dir).expect("write");

            assert!(OpState::exists(git_dir));
        }

        #[test]
        fn remove_clears_file() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let journal = Journal::new("test");
            let state = OpState::from_journal(&journal);
            state.write(git_dir).expect("write");

            assert!(OpState::exists(git_dir));

            OpState::remove(git_dir).expect("remove");

            assert!(!OpState::exists(git_dir));
        }

        #[test]
        fn remove_nonexistent_ok() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            // Should not error
            OpState::remove(git_dir).expect("remove nonexistent");
        }

        #[test]
        fn update_phase() {
            let temp = create_test_dir();
            let git_dir = temp.path();

            let journal = Journal::new("test");
            let mut state = OpState::from_journal(&journal);
            state.write(git_dir).expect("write");

            state
                .update_phase(OpPhase::Paused, git_dir)
                .expect("update");

            let loaded = OpState::read(git_dir).expect("read").expect("should exist");
            assert_eq!(loaded.phase, OpPhase::Paused);
        }

        #[test]
        fn path_is_correct() {
            let path = OpState::path(Path::new("/repo/.git"));
            assert_eq!(path, PathBuf::from("/repo/.git/lattice/op-state.json"));
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
}
