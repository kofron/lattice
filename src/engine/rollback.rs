//! Rollback logic for restoring refs to pre-operation state.
//!
//! This module provides the core rollback implementation used by:
//! - `abort` command (user-initiated abort)
//! - `Executor` (post-verification failure rollback)
//!
//! Per SPEC.md Section 4.2.2, all ref updates are recorded in the journal
//! and can be reversed using CAS semantics.
//!
//! # Rollback Order
//!
//! Steps are rolled back in reverse journal order. The journal records steps
//! in execution order, so reversing gives the correct undo order.
//!
//! # CAS Semantics
//!
//! All rollback operations use compare-and-swap (CAS) semantics. If a ref
//! has been modified out-of-band since the journal was written, the rollback
//! for that ref will fail. Other refs will still be rolled back.
//!
//! # Known Limitations
//!
//! - Metadata updates cannot be fully rolled back (old content not stored)
//! - Metadata deletes cannot be restored (deleted content not stored)
//!
//! These limitations are acceptable until the journal is extended to store
//! content for full rollback support.

use crate::core::metadata::store::{MetadataStore, StoreError};
use crate::core::ops::journal::{Journal, StepKind};
use crate::core::types::{BranchName, Oid};
use crate::git::{Git, GitError};
use thiserror::Error;

/// Errors from rollback operations.
#[derive(Debug, Error)]
pub enum RollbackError {
    /// CAS precondition failed - ref was modified out-of-band.
    #[error("rollback CAS failed for {refname}: expected {expected}, found {actual}")]
    CasFailed {
        /// The ref that failed CAS.
        refname: String,
        /// The expected OID.
        expected: String,
        /// The actual OID found.
        actual: String,
    },

    /// Git operation failed during rollback.
    #[error("git error during rollback: {0}")]
    GitError(String),

    /// Cannot restore - content not available.
    #[error("cannot restore {refname}: {reason}")]
    CannotRestore {
        /// The ref that cannot be restored.
        refname: String,
        /// Why it cannot be restored.
        reason: String,
    },

    /// Internal error during rollback.
    #[error("rollback internal error: {0}")]
    Internal(String),
}

/// Result of a rollback attempt.
#[derive(Debug)]
pub struct RollbackResult {
    /// Refs that were successfully rolled back.
    pub rolled_back: Vec<String>,
    /// Refs that failed to roll back with their errors.
    pub failed: Vec<(String, RollbackError)>,
    /// Whether all refs were successfully rolled back.
    pub complete: bool,
}

impl Default for RollbackResult {
    fn default() -> Self {
        Self::new()
    }
}

impl RollbackResult {
    /// Create a new empty rollback result.
    pub fn new() -> Self {
        Self {
            rolled_back: vec![],
            failed: vec![],
            complete: true,
        }
    }

    /// Record a successful rollback.
    pub fn record_success(&mut self, refname: String) {
        self.rolled_back.push(refname);
    }

    /// Record a failed rollback.
    pub fn record_failure(&mut self, refname: String, error: RollbackError) {
        self.failed.push((refname, error));
        self.complete = false;
    }

    /// Check if there were any failures.
    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }

    /// Get a summary string for display.
    pub fn summary(&self) -> String {
        if self.complete {
            format!("Rolled back {} refs successfully", self.rolled_back.len())
        } else {
            format!(
                "Partial rollback: {} succeeded, {} failed",
                self.rolled_back.len(),
                self.failed.len()
            )
        }
    }
}

/// Perform rollback of ref changes recorded in journal.
///
/// This function attempts to restore all refs to their pre-operation state
/// using CAS semantics. If any ref has been modified out-of-band, the
/// rollback for that ref will fail but others will still be attempted.
///
/// # Arguments
///
/// * `git` - Git interface
/// * `journal` - Journal containing ref updates to reverse
///
/// # Returns
///
/// `RollbackResult` indicating which refs were rolled back and which failed.
///
/// # Example
///
/// ```ignore
/// use latticework::engine::rollback::rollback_journal;
///
/// let result = rollback_journal(&git, &journal);
/// if result.complete {
///     println!("All refs restored");
/// } else {
///     for (refname, error) in &result.failed {
///         eprintln!("Failed to restore {}: {}", refname, error);
///     }
/// }
/// ```
pub fn rollback_journal(git: &Git, journal: &Journal) -> RollbackResult {
    let mut result = RollbackResult::new();

    let rollback_entries = journal.ref_updates_for_rollback();

    for step in rollback_entries {
        match step {
            StepKind::RefUpdate {
                refname,
                old_oid,
                new_oid,
            } => match rollback_ref_update(git, refname, old_oid.as_deref(), new_oid) {
                Ok(()) => result.record_success(refname.clone()),
                Err(e) => result.record_failure(refname.clone(), e),
            },
            StepKind::MetadataWrite {
                branch,
                old_ref_oid,
                new_ref_oid,
            } => {
                let refname = format!("refs/branch-metadata/{}", branch);
                match rollback_metadata_write(git, branch, old_ref_oid.as_deref(), new_ref_oid) {
                    Ok(()) => result.record_success(refname),
                    Err(e) => result.record_failure(refname, e),
                }
            }
            StepKind::MetadataDelete {
                branch,
                old_ref_oid,
            } => {
                // Metadata was deleted - we can't restore without content
                // Record as a known limitation
                let refname = format!("refs/branch-metadata/{}", branch);
                result.record_failure(
                    refname.clone(),
                    RollbackError::CannotRestore {
                        refname,
                        reason: format!(
                            "deleted metadata content not stored in journal (old_oid: {})",
                            old_ref_oid
                        ),
                    },
                );
            }
            StepKind::Checkpoint { .. }
            | StepKind::GitProcess { .. }
            | StepKind::ConflictPaused { .. } => {
                // Non-reversible or marker steps - skip
            }
        }
    }

    result
}

/// Roll back a single ref update.
fn rollback_ref_update(
    git: &Git,
    refname: &str,
    old_oid: Option<&str>,
    new_oid: &str,
) -> Result<(), RollbackError> {
    if let Some(old_val) = old_oid {
        // Ref existed before - restore it
        let old = Oid::new(old_val).map_err(|e| RollbackError::Internal(e.to_string()))?;
        let expected = Oid::new(new_oid).map_err(|e| RollbackError::Internal(e.to_string()))?;

        git.update_ref_cas(refname, &old, Some(&expected), "lattice rollback")
            .map_err(|e| match e {
                GitError::CasFailed {
                    expected, actual, ..
                } => RollbackError::CasFailed {
                    refname: refname.to_string(),
                    expected,
                    actual,
                },
                other => RollbackError::GitError(other.to_string()),
            })
    } else {
        // Ref was created - delete it
        let expected = Oid::new(new_oid).map_err(|e| RollbackError::Internal(e.to_string()))?;

        git.delete_ref_cas(refname, &expected).map_err(|e| match e {
            GitError::CasFailed {
                expected, actual, ..
            } => RollbackError::CasFailed {
                refname: refname.to_string(),
                expected,
                actual,
            },
            other => RollbackError::GitError(other.to_string()),
        })
    }
}

/// Roll back a metadata write.
fn rollback_metadata_write(
    git: &Git,
    branch: &str,
    old_ref_oid: Option<&str>,
    new_ref_oid: &str,
) -> Result<(), RollbackError> {
    let store = MetadataStore::new(git);
    let branch_name =
        BranchName::new(branch).map_err(|e| RollbackError::Internal(e.to_string()))?;

    if old_ref_oid.is_some() {
        // Metadata existed before - we can't restore content
        // This is a known limitation until we store content in journal
        return Err(RollbackError::CannotRestore {
            refname: format!("refs/branch-metadata/{}", branch),
            reason: "previous metadata content not stored in journal".to_string(),
        });
    }

    // Metadata was created - delete it
    let expected = Oid::new(new_ref_oid).map_err(|e| RollbackError::Internal(e.to_string()))?;

    store
        .delete_cas(&branch_name, &expected)
        .map_err(|e| match e {
            StoreError::CasFailed { expected, actual } => RollbackError::CasFailed {
                refname: format!("refs/branch-metadata/{}", branch),
                expected,
                actual,
            },
            other => RollbackError::GitError(other.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    mod rollback_result {
        use super::*;

        #[test]
        fn new_is_complete() {
            let result = RollbackResult::new();
            assert!(result.complete);
            assert!(result.rolled_back.is_empty());
            assert!(result.failed.is_empty());
        }

        #[test]
        fn record_success_keeps_complete() {
            let mut result = RollbackResult::new();
            result.record_success("refs/heads/feature".to_string());

            assert!(result.complete);
            assert_eq!(result.rolled_back.len(), 1);
            assert!(result.failed.is_empty());
        }

        #[test]
        fn record_failure_clears_complete() {
            let mut result = RollbackResult::new();
            result.record_failure(
                "refs/heads/feature".to_string(),
                RollbackError::CasFailed {
                    refname: "refs/heads/feature".to_string(),
                    expected: "abc".to_string(),
                    actual: "def".to_string(),
                },
            );

            assert!(!result.complete);
            assert!(result.rolled_back.is_empty());
            assert_eq!(result.failed.len(), 1);
        }

        #[test]
        fn has_failures() {
            let mut result = RollbackResult::new();
            assert!(!result.has_failures());

            result.record_failure(
                "ref".to_string(),
                RollbackError::Internal("test".to_string()),
            );
            assert!(result.has_failures());
        }

        #[test]
        fn summary_complete() {
            let mut result = RollbackResult::new();
            result.record_success("refs/heads/a".to_string());
            result.record_success("refs/heads/b".to_string());

            let summary = result.summary();
            assert!(summary.contains("2 refs successfully"));
        }

        #[test]
        fn summary_partial() {
            let mut result = RollbackResult::new();
            result.record_success("refs/heads/a".to_string());
            result.record_failure(
                "refs/heads/b".to_string(),
                RollbackError::Internal("test".to_string()),
            );

            let summary = result.summary();
            assert!(summary.contains("1 succeeded"));
            assert!(summary.contains("1 failed"));
        }
    }

    mod rollback_error {
        use super::*;

        #[test]
        fn cas_failed_display() {
            let err = RollbackError::CasFailed {
                refname: "refs/heads/feature".to_string(),
                expected: "abc123".to_string(),
                actual: "def456".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("refs/heads/feature"));
            assert!(msg.contains("abc123"));
            assert!(msg.contains("def456"));
        }

        #[test]
        fn git_error_display() {
            let err = RollbackError::GitError("command failed".to_string());
            let msg = err.to_string();
            assert!(msg.contains("git error"));
            assert!(msg.contains("command failed"));
        }

        #[test]
        fn cannot_restore_display() {
            let err = RollbackError::CannotRestore {
                refname: "refs/branch-metadata/feature".to_string(),
                reason: "content not stored".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("cannot restore"));
            assert!(msg.contains("content not stored"));
        }

        #[test]
        fn internal_display() {
            let err = RollbackError::Internal("bad state".to_string());
            let msg = err.to_string();
            assert!(msg.contains("internal error"));
            assert!(msg.contains("bad state"));
        }
    }
}
