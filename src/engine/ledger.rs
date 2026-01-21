//! engine::ledger
//!
//! Event ledger for divergence detection and audit trail.
//!
//! # Architecture
//!
//! The event ledger is an append-only commit chain stored at `refs/lattice/event-log`.
//! Per ARCHITECTURE.md Section 3.4, each commit contains one event record and points
//! to the previous event commit.
//!
//! The ledger provides:
//! - Divergence detection and reporting
//! - Audit trail of Lattice-intended structural changes
//! - Recovery hints when metadata is corrupted
//! - Record of doctor proposals and applied repairs
//!
//! **Important:** The ledger is evidence, not authority. It records what Lattice
//! intended and observed, but does not replace repository state as truth.
//!
//! # Event Categories
//!
//! - `IntentRecorded`: Intent to perform an operation was recorded
//! - `Committed`: Operation completed successfully
//! - `Aborted`: Operation was aborted
//! - `DivergenceObserved`: Out-of-band changes detected
//! - `DoctorProposed`: Doctor proposed a repair
//! - `DoctorApplied`: Doctor applied a repair
//!
//! # Example
//!
//! ```ignore
//! use latticework::engine::ledger::{Event, EventLedger};
//!
//! let ledger = EventLedger::new(&git);
//!
//! // Record an intent
//! ledger.append(Event::IntentRecorded {
//!     op_id: "abc-123".to_string(),
//!     command: "restack".to_string(),
//!     plan_digest: "sha256:...".to_string(),
//!     fingerprint_before: "fp:...".to_string(),
//! })?;
//!
//! // Check for divergence
//! if let Some(fp) = ledger.last_committed_fingerprint()? {
//!     // Compare with current fingerprint
//! }
//! ```

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::types::Oid;
use crate::git::{Git, GitError};

/// The ref name for the event ledger.
pub const LEDGER_REF: &str = "refs/lattice/event-log";

/// Errors from ledger operations.
#[derive(Debug, Error)]
pub enum LedgerError {
    /// Git operation failed.
    #[error("git error: {0}")]
    Git(#[from] GitError),

    /// Failed to serialize event.
    #[error("failed to serialize event: {0}")]
    Serialize(String),

    /// Failed to deserialize event.
    #[error("failed to deserialize event: {0}")]
    Deserialize(String),

    /// CAS precondition failed (concurrent append).
    #[error("concurrent ledger append detected")]
    ConcurrentAppend,

    /// Ledger is corrupted.
    #[error("ledger corrupted: {0}")]
    Corrupted(String),
}

/// An event in the ledger.
///
/// Events are stored as JSON in commit trees. Each event type contains
/// the information needed for its purpose (divergence detection, audit, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Intent to perform an operation was recorded.
    ///
    /// Recorded before the first mutation of an operation.
    IntentRecorded {
        /// Operation ID (matches journal).
        op_id: String,
        /// Command being executed.
        command: String,
        /// Hash of the plan for integrity checking.
        plan_digest: String,
        /// Fingerprint before operation.
        fingerprint_before: String,
        /// Timestamp.
        timestamp: String,
    },

    /// Operation completed successfully.
    ///
    /// Recorded after all mutations are applied and verified.
    Committed {
        /// Operation ID.
        op_id: String,
        /// Fingerprint after operation.
        fingerprint_after: String,
        /// Timestamp.
        timestamp: String,
    },

    /// Operation was aborted.
    ///
    /// Recorded when an operation fails or is explicitly aborted.
    Aborted {
        /// Operation ID.
        op_id: String,
        /// Reason for abort.
        reason: String,
        /// Timestamp.
        timestamp: String,
    },

    /// Out-of-band divergence detected.
    ///
    /// Recorded when the current fingerprint differs from the last committed.
    DivergenceObserved {
        /// Fingerprint from last Committed event.
        prior_fingerprint: String,
        /// Current fingerprint.
        current_fingerprint: String,
        /// Refs that changed.
        changed_refs: Vec<String>,
        /// Timestamp.
        timestamp: String,
    },

    /// Doctor proposed a repair.
    ///
    /// Recorded when doctor presents fix options to the user.
    DoctorProposed {
        /// Issue IDs that triggered the proposal.
        issue_ids: Vec<String>,
        /// Fix IDs that were offered.
        fix_ids: Vec<String>,
        /// Timestamp.
        timestamp: String,
    },

    /// Doctor applied a repair.
    ///
    /// Recorded after doctor successfully applies fixes.
    DoctorApplied {
        /// Fix IDs that were applied.
        fix_ids: Vec<String>,
        /// Fingerprint after repair.
        fingerprint_after: String,
        /// Timestamp.
        timestamp: String,
    },

    /// Undo was applied.
    ///
    /// Recorded after successfully undoing a previous operation.
    UndoApplied {
        /// Operation ID of the operation that was undone.
        undone_op_id: String,
        /// Number of refs that were restored.
        refs_restored: usize,
        /// Timestamp.
        timestamp: String,
    },
}

impl Event {
    /// Create an IntentRecorded event.
    pub fn intent_recorded(
        op_id: impl Into<String>,
        command: impl Into<String>,
        plan_digest: impl Into<String>,
        fingerprint_before: impl Into<String>,
    ) -> Self {
        Event::IntentRecorded {
            op_id: op_id.into(),
            command: command.into(),
            plan_digest: plan_digest.into(),
            fingerprint_before: fingerprint_before.into(),
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Create a Committed event.
    pub fn committed(op_id: impl Into<String>, fingerprint_after: impl Into<String>) -> Self {
        Event::Committed {
            op_id: op_id.into(),
            fingerprint_after: fingerprint_after.into(),
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Create an Aborted event.
    pub fn aborted(op_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Event::Aborted {
            op_id: op_id.into(),
            reason: reason.into(),
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Create a DivergenceObserved event.
    pub fn divergence_observed(
        prior_fingerprint: impl Into<String>,
        current_fingerprint: impl Into<String>,
        changed_refs: Vec<String>,
    ) -> Self {
        Event::DivergenceObserved {
            prior_fingerprint: prior_fingerprint.into(),
            current_fingerprint: current_fingerprint.into(),
            changed_refs,
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Create a DoctorProposed event.
    pub fn doctor_proposed(issue_ids: Vec<String>, fix_ids: Vec<String>) -> Self {
        Event::DoctorProposed {
            issue_ids,
            fix_ids,
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Create a DoctorApplied event.
    pub fn doctor_applied(fix_ids: Vec<String>, fingerprint_after: impl Into<String>) -> Self {
        Event::DoctorApplied {
            fix_ids,
            fingerprint_after: fingerprint_after.into(),
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Create an UndoApplied event.
    pub fn undo_applied(undone_op_id: impl Into<String>, refs_restored: usize) -> Self {
        Event::UndoApplied {
            undone_op_id: undone_op_id.into(),
            refs_restored,
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Get the operation ID if this event has one.
    pub fn op_id(&self) -> Option<&str> {
        match self {
            Event::IntentRecorded { op_id, .. } => Some(op_id),
            Event::Committed { op_id, .. } => Some(op_id),
            Event::Aborted { op_id, .. } => Some(op_id),
            Event::UndoApplied { undone_op_id, .. } => Some(undone_op_id),
            Event::DivergenceObserved { .. } => None,
            Event::DoctorProposed { .. } => None,
            Event::DoctorApplied { .. } => None,
        }
    }

    /// Get the fingerprint after this event, if applicable.
    pub fn fingerprint_after(&self) -> Option<&str> {
        match self {
            Event::Committed {
                fingerprint_after, ..
            } => Some(fingerprint_after),
            Event::DoctorApplied {
                fingerprint_after, ..
            } => Some(fingerprint_after),
            _ => None,
        }
    }

    /// Check if this is a Committed event.
    pub fn is_committed(&self) -> bool {
        matches!(self, Event::Committed { .. })
    }

    /// Check if this is a DivergenceObserved event.
    pub fn is_divergence(&self) -> bool {
        matches!(self, Event::DivergenceObserved { .. })
    }

    /// Serialize to canonical JSON.
    pub fn to_json(&self) -> Result<String, LedgerError> {
        serde_json::to_string_pretty(self).map_err(|e| LedgerError::Serialize(e.to_string()))
    }

    /// Deserialize from JSON.
    pub fn from_json(json: &str) -> Result<Self, LedgerError> {
        serde_json::from_str(json).map_err(|e| LedgerError::Deserialize(e.to_string()))
    }
}

/// A stored event with its commit OID.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    /// The commit OID containing this event.
    pub commit_oid: Oid,
    /// The event data.
    pub event: Event,
}

/// The event ledger.
///
/// Provides append-only storage for events as a commit chain in Git.
/// Each event is stored as a JSON blob in a commit tree, with the
/// commit pointing to the previous event.
pub struct EventLedger<'a> {
    git: &'a Git,
}

impl<'a> EventLedger<'a> {
    /// Create a new event ledger.
    pub fn new(git: &'a Git) -> Self {
        Self { git }
    }

    /// Append an event to the ledger.
    ///
    /// Uses CAS to ensure the ledger hasn't changed since we last read it.
    /// This prevents lost events from concurrent appends.
    ///
    /// # Returns
    ///
    /// The OID of the new commit on success.
    ///
    /// # Errors
    ///
    /// - `LedgerError::ConcurrentAppend` if the ledger was modified concurrently
    /// - `LedgerError::Git` for Git operation failures
    pub fn append(&self, event: Event) -> Result<Oid, LedgerError> {
        // Get current ledger head (if any).
        // Note: We use try_resolve_ref_to_object because the ledger ref points to
        // a blob (event JSON), not a commit. Using try_resolve_ref would fail with
        // "invalid object id" when the ref exists but points to a non-commit object.
        let current_head = self.git.try_resolve_ref_to_object(LEDGER_REF)?;

        // Serialize event to JSON
        let json = event.to_json()?;

        // Create blob with event JSON
        let blob_oid = self.git.write_blob(json.as_bytes())?;

        // Create tree with event.json
        let tree_oid = self.create_tree_with_blob(&blob_oid)?;

        // Create commit pointing to previous (if any)
        let commit_oid = self.create_event_commit(&tree_oid, current_head.as_ref(), &event)?;

        // Update ref with CAS
        self.git
            .update_ref_cas(
                LEDGER_REF,
                &commit_oid,
                current_head.as_ref(),
                "lattice: append event",
            )
            .map_err(|e| match e {
                GitError::CasFailed { .. } => LedgerError::ConcurrentAppend,
                other => LedgerError::Git(other),
            })?;

        Ok(commit_oid)
    }

    /// Read the most recent event.
    ///
    /// Returns `None` if the ledger is empty.
    pub fn latest(&self) -> Result<Option<StoredEvent>, LedgerError> {
        let head_oid = match self.git.try_resolve_ref_to_object(LEDGER_REF)? {
            Some(oid) => oid,
            None => return Ok(None),
        };

        let event = self.read_event_from_commit(&head_oid)?;
        Ok(Some(StoredEvent {
            commit_oid: head_oid,
            event,
        }))
    }

    /// Read the last N events (most recent first).
    ///
    /// Returns fewer than `count` if the ledger has fewer events.
    pub fn recent(&self, count: usize) -> Result<Vec<StoredEvent>, LedgerError> {
        if count == 0 {
            return Ok(vec![]);
        }

        let mut events = Vec::with_capacity(count);
        let mut current_oid = self.git.try_resolve_ref_to_object(LEDGER_REF)?;

        while let Some(oid) = current_oid {
            if events.len() >= count {
                break;
            }

            let event = self.read_event_from_commit(&oid)?;
            events.push(StoredEvent {
                commit_oid: oid.clone(),
                event,
            });

            // Get parent commit
            let parents = self.git.commit_parents(&oid)?;
            current_oid = parents.into_iter().next();
        }

        Ok(events)
    }

    /// Get the fingerprint from the last Committed event.
    ///
    /// This is used for divergence detection. Returns `None` if no
    /// Committed event exists in the ledger.
    pub fn last_committed_fingerprint(&self) -> Result<Option<String>, LedgerError> {
        let mut current_oid = self.git.try_resolve_ref_to_object(LEDGER_REF)?;

        while let Some(oid) = current_oid {
            let event = self.read_event_from_commit(&oid)?;

            if let Some(fp) = event.fingerprint_after() {
                return Ok(Some(fp.to_string()));
            }

            // Get parent commit
            let parents = self.git.commit_parents(&oid)?;
            current_oid = parents.into_iter().next();
        }

        Ok(None)
    }

    /// Check if the ledger is empty.
    pub fn is_empty(&self) -> Result<bool, LedgerError> {
        Ok(self.git.try_resolve_ref_to_object(LEDGER_REF)?.is_none())
    }

    /// Get the count of events in the ledger.
    ///
    /// Note: This walks the entire chain, so may be slow for long histories.
    pub fn count(&self) -> Result<usize, LedgerError> {
        let mut count = 0;
        let mut current_oid = self.git.try_resolve_ref_to_object(LEDGER_REF)?;

        while let Some(oid) = current_oid {
            count += 1;
            let parents = self.git.commit_parents(&oid)?;
            current_oid = parents.into_iter().next();
        }

        Ok(count)
    }

    /// Find events for a specific operation ID.
    ///
    /// Returns events in chronological order (oldest first).
    pub fn events_for_op(&self, op_id: &str) -> Result<Vec<StoredEvent>, LedgerError> {
        let mut events = Vec::new();
        let mut current_oid = self.git.try_resolve_ref_to_object(LEDGER_REF)?;

        while let Some(oid) = current_oid {
            let event = self.read_event_from_commit(&oid)?;

            if event.op_id() == Some(op_id) {
                events.push(StoredEvent {
                    commit_oid: oid.clone(),
                    event,
                });
            }

            let parents = self.git.commit_parents(&oid)?;
            current_oid = parents.into_iter().next();
        }

        events.reverse(); // Chronological order
        Ok(events)
    }

    // =========================================================================
    // Internal helpers
    // =========================================================================

    /// Create a tree containing a single event.json blob.
    fn create_tree_with_blob(&self, blob_oid: &Oid) -> Result<Oid, LedgerError> {
        use crate::git::TreeEntry;

        let entries = [TreeEntry {
            name: "event.json",
            oid: blob_oid,
            mode: 0o100644, // Regular file
        }];

        self.git.write_tree(&entries).map_err(LedgerError::Git)
    }

    /// Create a commit for an event.
    ///
    /// Creates a real git commit with the tree containing the event JSON,
    /// pointing to the parent commit (if any). This forms the commit chain
    /// required by ARCHITECTURE.md Section 3.4.
    fn create_event_commit(
        &self,
        tree_oid: &Oid,
        parent: Option<&Oid>,
        _event: &Event,
    ) -> Result<Oid, LedgerError> {
        let parents: Vec<&Oid> = parent.into_iter().collect();

        self.git
            .write_commit(tree_oid, &parents, "lattice: event")
            .map_err(LedgerError::Git)
    }

    /// Read an event from a commit.
    ///
    /// Extracts the event JSON from the commit's tree by:
    /// 1. Getting the tree OID from the commit
    /// 2. Finding `event.json` in the tree
    /// 3. Reading and parsing the blob
    fn read_event_from_commit(&self, commit_oid: &Oid) -> Result<Event, LedgerError> {
        // Get the tree from the commit
        let tree_oid = self.git.commit_tree(commit_oid)?;

        // Find event.json in the tree
        let blob_oid = self
            .git
            .tree_entry_oid(&tree_oid, "event.json")?
            .ok_or_else(|| LedgerError::Corrupted("commit missing event.json".into()))?;

        // Read and parse the event
        let json = self.git.read_blob_as_string(&blob_oid)?;
        Event::from_json(&json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod event {
        use super::*;

        #[test]
        fn intent_recorded_creation() {
            let event = Event::intent_recorded("op-1", "restack", "digest", "fp-before");

            match event {
                Event::IntentRecorded {
                    op_id,
                    command,
                    plan_digest,
                    fingerprint_before,
                    ..
                } => {
                    assert_eq!(op_id, "op-1");
                    assert_eq!(command, "restack");
                    assert_eq!(plan_digest, "digest");
                    assert_eq!(fingerprint_before, "fp-before");
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn committed_creation() {
            let event = Event::committed("op-1", "fp-after");

            match event {
                Event::Committed {
                    op_id,
                    fingerprint_after,
                    ..
                } => {
                    assert_eq!(op_id, "op-1");
                    assert_eq!(fingerprint_after, "fp-after");
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn aborted_creation() {
            let event = Event::aborted("op-1", "CAS failed");

            match event {
                Event::Aborted { op_id, reason, .. } => {
                    assert_eq!(op_id, "op-1");
                    assert_eq!(reason, "CAS failed");
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn divergence_observed_creation() {
            let event =
                Event::divergence_observed("fp-old", "fp-new", vec!["refs/heads/main".to_string()]);

            match event {
                Event::DivergenceObserved {
                    prior_fingerprint,
                    current_fingerprint,
                    changed_refs,
                    ..
                } => {
                    assert_eq!(prior_fingerprint, "fp-old");
                    assert_eq!(current_fingerprint, "fp-new");
                    assert_eq!(changed_refs, vec!["refs/heads/main"]);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn doctor_proposed_creation() {
            let event =
                Event::doctor_proposed(vec!["issue-1".to_string()], vec!["fix-1".to_string()]);

            match event {
                Event::DoctorProposed {
                    issue_ids, fix_ids, ..
                } => {
                    assert_eq!(issue_ids, vec!["issue-1"]);
                    assert_eq!(fix_ids, vec!["fix-1"]);
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn doctor_applied_creation() {
            let event = Event::doctor_applied(vec!["fix-1".to_string()], "fp-after");

            match event {
                Event::DoctorApplied {
                    fix_ids,
                    fingerprint_after,
                    ..
                } => {
                    assert_eq!(fix_ids, vec!["fix-1"]);
                    assert_eq!(fingerprint_after, "fp-after");
                }
                _ => panic!("wrong event type"),
            }
        }

        #[test]
        fn op_id_accessor() {
            assert_eq!(
                Event::intent_recorded("op-1", "", "", "").op_id(),
                Some("op-1")
            );
            assert_eq!(Event::committed("op-2", "").op_id(), Some("op-2"));
            assert_eq!(Event::aborted("op-3", "").op_id(), Some("op-3"));
            assert_eq!(Event::divergence_observed("", "", vec![]).op_id(), None);
            assert_eq!(Event::doctor_proposed(vec![], vec![]).op_id(), None);
            assert_eq!(Event::doctor_applied(vec![], "").op_id(), None);
        }

        #[test]
        fn fingerprint_after_accessor() {
            assert_eq!(Event::committed("", "fp").fingerprint_after(), Some("fp"));
            assert_eq!(
                Event::doctor_applied(vec![], "fp").fingerprint_after(),
                Some("fp")
            );
            assert_eq!(
                Event::intent_recorded("", "", "", "").fingerprint_after(),
                None
            );
            assert_eq!(Event::aborted("", "").fingerprint_after(), None);
            assert_eq!(
                Event::divergence_observed("", "", vec![]).fingerprint_after(),
                None
            );
        }

        #[test]
        fn is_committed() {
            assert!(Event::committed("", "").is_committed());
            assert!(!Event::aborted("", "").is_committed());
        }

        #[test]
        fn is_divergence() {
            assert!(Event::divergence_observed("", "", vec![]).is_divergence());
            assert!(!Event::committed("", "").is_divergence());
        }

        #[test]
        fn json_roundtrip() {
            let events = vec![
                Event::intent_recorded("op", "cmd", "digest", "fp"),
                Event::committed("op", "fp"),
                Event::aborted("op", "reason"),
                Event::divergence_observed("old", "new", vec!["ref".to_string()]),
                Event::doctor_proposed(vec!["i".to_string()], vec!["f".to_string()]),
                Event::doctor_applied(vec!["f".to_string()], "fp"),
            ];

            for event in events {
                let json = event.to_json().unwrap();
                let parsed = Event::from_json(&json).unwrap();
                assert_eq!(event, parsed);
            }
        }

        #[test]
        fn json_has_type_tag() {
            let event = Event::committed("op", "fp");
            let json = event.to_json().unwrap();
            assert!(json.contains("\"type\""));
            assert!(json.contains("\"committed\""));
        }
    }

    mod ledger_error {
        use super::*;

        #[test]
        fn display_formatting() {
            let err = LedgerError::ConcurrentAppend;
            assert!(err.to_string().contains("concurrent"));

            let err = LedgerError::Serialize("bad json".into());
            assert!(err.to_string().contains("serialize"));

            let err = LedgerError::Deserialize("parse error".into());
            assert!(err.to_string().contains("deserialize"));

            let err = LedgerError::Corrupted("invalid structure".into());
            assert!(err.to_string().contains("corrupted"));
        }
    }

    mod stored_event {
        use super::*;

        #[test]
        fn constructible() {
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            let stored = StoredEvent {
                commit_oid: oid.clone(),
                event: Event::committed("op", "fp"),
            };

            assert_eq!(stored.commit_oid, oid);
            assert!(stored.event.is_committed());
        }
    }

    /// Integration tests that use a real git repository.
    ///
    /// These tests verify the ledger works correctly with actual git operations,
    /// particularly that refs pointing to blobs (not commits) are handled correctly.
    mod integration {
        use super::*;
        use std::path::Path;
        use std::process::Command;
        use tempfile::TempDir;

        /// Test fixture that creates a real git repository.
        struct TestRepo {
            dir: TempDir,
        }

        impl TestRepo {
            /// Create a new test repository with an initial commit.
            fn new() -> Self {
                let dir = TempDir::new().expect("failed to create temp dir");

                // Initialize git repo
                run_git(dir.path(), &["init"]);
                run_git(dir.path(), &["config", "user.email", "test@example.com"]);
                run_git(dir.path(), &["config", "user.name", "Test User"]);

                // Create initial commit (required for a valid repo)
                std::fs::write(dir.path().join("README.md"), "# Test Repo\n").unwrap();
                run_git(dir.path(), &["add", "README.md"]);
                run_git(dir.path(), &["commit", "-m", "Initial commit"]);

                Self { dir }
            }

            /// Get the path to the repository.
            fn path(&self) -> &Path {
                self.dir.path()
            }

            /// Open a Git interface to this repository.
            fn git(&self) -> crate::git::Git {
                crate::git::Git::open(self.path()).expect("failed to open test repo")
            }
        }

        /// Run a git command in the given directory.
        fn run_git(dir: &Path, args: &[&str]) {
            let output = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git command failed");

            if !output.status.success() {
                panic!(
                    "git {:?} failed: {}",
                    args,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }

        /// Test that appending to an empty ledger works.
        ///
        /// This is the core bug: the ledger stores events as blobs, but
        /// try_resolve_ref calls peel_to_commit() which fails for blobs.
        /// When the ledger ref doesn't exist, this should return Ok(None),
        /// not an error about invalid object ids.
        #[test]
        fn append_to_empty_ledger_succeeds() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            // Ledger should be empty initially
            assert!(ledger.is_empty().unwrap(), "ledger should start empty");

            // Appending to an empty ledger should succeed
            let event = Event::committed("test-op", "fingerprint-123");
            let result = ledger.append(event);

            assert!(
                result.is_ok(),
                "append to empty ledger should succeed, got: {:?}",
                result.err()
            );
        }

        /// Test that reading from an empty ledger returns None, not an error.
        ///
        /// The bug manifests as "invalid object id: refs/lattice/event-log"
        /// when the ref doesn't exist, instead of returning None.
        #[test]
        fn latest_on_empty_ledger_returns_none() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            let result = ledger.latest();

            assert!(
                result.is_ok(),
                "latest on empty ledger should succeed, got: {:?}",
                result.err()
            );
            assert!(
                result.unwrap().is_none(),
                "latest on empty ledger should return None"
            );
        }

        /// Test that last_committed_fingerprint on empty ledger returns None.
        ///
        /// This is called during scanning and must not error when the ledger
        /// doesn't exist yet.
        #[test]
        fn last_committed_fingerprint_on_empty_ledger_returns_none() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            let result = ledger.last_committed_fingerprint();

            assert!(
                result.is_ok(),
                "last_committed_fingerprint on empty ledger should succeed, got: {:?}",
                result.err()
            );
            assert!(
                result.unwrap().is_none(),
                "last_committed_fingerprint on empty ledger should return None"
            );
        }

        /// Test that reading after appending works correctly.
        ///
        /// After appending, the ledger ref points to a blob. Reading it back
        /// must work - this verifies the full round-trip.
        #[test]
        fn append_then_read_roundtrip() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            // Append an event
            let event = Event::committed("test-op", "fp-after-test");
            ledger.append(event).expect("append should succeed");

            // Read it back
            let latest = ledger.latest().expect("latest should succeed");
            assert!(latest.is_some(), "should have an event after append");

            let stored = latest.unwrap();
            assert!(stored.event.is_committed());
            assert_eq!(stored.event.fingerprint_after(), Some("fp-after-test"));
        }

        /// Test that multiple appends work correctly.
        ///
        /// Each append creates a commit pointing to the previous, forming a chain
        /// as required by ARCHITECTURE.md Section 3.4.
        #[test]
        fn multiple_appends_work() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            // Append multiple events - each one should succeed
            ledger
                .append(Event::committed("op-1", "fp-1"))
                .expect("first append");
            ledger
                .append(Event::committed("op-2", "fp-2"))
                .expect("second append");
            ledger
                .append(Event::committed("op-3", "fp-3"))
                .expect("third append");

            // Latest should be the last one appended
            let latest = ledger.latest().expect("latest").expect("should exist");
            assert_eq!(latest.event.fingerprint_after(), Some("fp-3"));

            // Ledger should not be empty
            assert!(!ledger.is_empty().expect("is_empty"));
        }

        /// Test that history is preserved and can be walked via recent().
        ///
        /// This is the key test for the commit chain: we should be able to
        /// retrieve all events in reverse chronological order.
        #[test]
        fn history_is_preserved() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            // Append 3 events
            ledger
                .append(Event::committed("op-1", "fp-1"))
                .expect("first append");
            ledger
                .append(Event::committed("op-2", "fp-2"))
                .expect("second append");
            ledger
                .append(Event::committed("op-3", "fp-3"))
                .expect("third append");

            // Should be able to read all 3 via recent()
            let events = ledger.recent(10).expect("recent should succeed");
            assert_eq!(events.len(), 3, "should have 3 events");
            // Most recent first
            assert_eq!(events[0].event.fingerprint_after(), Some("fp-3"));
            assert_eq!(events[1].event.fingerprint_after(), Some("fp-2"));
            assert_eq!(events[2].event.fingerprint_after(), Some("fp-1"));
        }

        /// Test that count() correctly walks the entire history.
        #[test]
        fn count_reflects_all_events() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            assert_eq!(ledger.count().expect("count"), 0);

            ledger
                .append(Event::committed("op-1", "fp-1"))
                .expect("append");
            assert_eq!(ledger.count().expect("count"), 1);

            ledger
                .append(Event::committed("op-2", "fp-2"))
                .expect("append");
            assert_eq!(ledger.count().expect("count"), 2);

            ledger
                .append(Event::committed("op-3", "fp-3"))
                .expect("append");
            assert_eq!(ledger.count().expect("count"), 3);
        }

        /// Test that last_committed_fingerprint() walks history to find it.
        ///
        /// If the most recent event doesn't have a fingerprint_after (e.g.,
        /// DivergenceObserved), it should walk back to find the last one.
        #[test]
        fn last_committed_fingerprint_walks_history() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            // Append a Committed event, then a DivergenceObserved (no fingerprint_after)
            ledger
                .append(Event::committed("op-1", "fp-1"))
                .expect("append");
            ledger
                .append(Event::divergence_observed("fp-1", "fp-changed", vec![]))
                .expect("append");

            // Should find fp-1 by walking past the divergence event
            let fp = ledger
                .last_committed_fingerprint()
                .expect("last_committed_fingerprint");
            assert_eq!(fp, Some("fp-1".to_string()));
        }

        /// Test that events_for_op() finds all events for a given operation.
        #[test]
        fn events_for_op_finds_all_related_events() {
            let repo = TestRepo::new();
            let git = repo.git();
            let ledger = EventLedger::new(&git);

            ledger
                .append(Event::intent_recorded("op-1", "cmd", "digest", "fp"))
                .expect("append");
            ledger
                .append(Event::committed("op-2", "fp-2"))
                .expect("append"); // Different op
            ledger
                .append(Event::committed("op-1", "fp-1"))
                .expect("append");

            let events = ledger.events_for_op("op-1").expect("events_for_op");
            assert_eq!(events.len(), 2);
            // Chronological order (oldest first)
            assert!(matches!(events[0].event, Event::IntentRecorded { .. }));
            assert!(matches!(events[1].event, Event::Committed { .. }));
        }
    }
}
