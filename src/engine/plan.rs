//! engine::plan
//!
//! Deterministic plan generation.
//!
//! # Architecture
//!
//! Per ARCHITECTURE.md Section 6.1, plans are the sole intermediate
//! representation between validated state and repository mutation.
//!
//! Plans are:
//! - **Deterministic**: Same input always produces the same plan
//! - **Previewable**: Can be shown to user before execution
//! - **Serializable**: Can be recorded in journals for recovery
//! - **Typed**: Steps are strongly typed with explicit touched refs
//!
//! # Invariants
//!
//! - Planner does not perform I/O
//! - Planner does not mutate any state
//! - Plans are pure data structures
//! - All ref mutations in steps include expected old OIDs for CAS
//!
//! # Example
//!
//! ```
//! use lattice::engine::plan::{Plan, PlanStep};
//! use lattice::core::ops::journal::OpId;
//!
//! let plan = Plan::new(OpId::new(), "restack")
//!     .with_step(PlanStep::Checkpoint {
//!         name: "start".to_string(),
//!     })
//!     .with_step(PlanStep::UpdateRefCas {
//!         refname: "refs/heads/feature".to_string(),
//!         old_oid: Some("abc123...".to_string()),
//!         new_oid: "def456...".to_string(),
//!         reason: "rebase onto main".to_string(),
//!     });
//!
//! assert!(!plan.is_empty());
//! assert_eq!(plan.steps.len(), 2);
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::gate::ReadyContext;
use crate::core::metadata::schema::BranchMetadataV1;
use crate::core::ops::journal::OpId;

/// A typed plan step.
///
/// Each step represents an atomic operation that the executor will apply.
/// Steps include all information needed for CAS validation and rollback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlanStep {
    /// Update a ref with CAS semantics.
    ///
    /// The update only succeeds if the ref's current value matches `old_oid`.
    UpdateRefCas {
        /// Full ref name (e.g., "refs/heads/feature").
        refname: String,
        /// Expected current OID, or None if creating.
        old_oid: Option<String>,
        /// New OID to set.
        new_oid: String,
        /// Human-readable reason for the update.
        reason: String,
    },

    /// Delete a ref with CAS semantics.
    ///
    /// The delete only succeeds if the ref's current value matches `old_oid`.
    DeleteRefCas {
        /// Full ref name.
        refname: String,
        /// Expected current OID.
        old_oid: String,
        /// Human-readable reason for deletion.
        reason: String,
    },

    /// Write metadata with CAS semantics.
    ///
    /// Creates or updates a branch's metadata ref.
    WriteMetadataCas {
        /// Branch name.
        branch: String,
        /// Expected current metadata ref OID, or None if creating.
        old_ref_oid: Option<String>,
        /// The metadata to write (boxed to reduce enum size).
        metadata: Box<BranchMetadataV1>,
    },

    /// Delete metadata with CAS semantics.
    DeleteMetadataCas {
        /// Branch name.
        branch: String,
        /// Expected current metadata ref OID.
        old_ref_oid: String,
    },

    /// Run a git command.
    ///
    /// For operations that require invoking git (rebase, merge, etc.).
    RunGit {
        /// Git command arguments (excluding "git" itself).
        args: Vec<String>,
        /// Human-readable description of what the command does.
        description: String,
        /// Refs that are expected to change.
        expected_effects: Vec<String>,
    },

    /// Checkpoint marker for recovery.
    ///
    /// Used to mark significant points in multi-step operations.
    /// If execution is interrupted, recovery knows where to resume.
    Checkpoint {
        /// Checkpoint name for identification.
        name: String,
    },

    /// Pause for conflict resolution.
    ///
    /// Indicates that the operation may pause here for user intervention.
    /// Used after RunGit steps that may produce conflicts.
    PotentialConflictPause {
        /// Branch where conflict may occur.
        branch: String,
        /// Git operation that may conflict.
        git_operation: String,
    },
}

impl PlanStep {
    /// Get the refs touched by this step.
    ///
    /// Returns refs that will be read or modified by this step.
    pub fn touched_refs(&self) -> Vec<&str> {
        match self {
            PlanStep::UpdateRefCas { refname, .. } => vec![refname.as_str()],
            PlanStep::DeleteRefCas { refname, .. } => vec![refname.as_str()],
            PlanStep::WriteMetadataCas { branch: _, .. } => {
                // Metadata ref name
                vec![] // Would need to construct; simplified for now
            }
            PlanStep::DeleteMetadataCas { branch: _, .. } => {
                vec![]
            }
            PlanStep::RunGit {
                expected_effects, ..
            } => expected_effects.iter().map(|s| s.as_str()).collect(),
            PlanStep::Checkpoint { .. } => vec![],
            PlanStep::PotentialConflictPause { .. } => vec![],
        }
    }

    /// Check if this step modifies refs (as opposed to being a marker).
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            PlanStep::UpdateRefCas { .. }
                | PlanStep::DeleteRefCas { .. }
                | PlanStep::WriteMetadataCas { .. }
                | PlanStep::DeleteMetadataCas { .. }
                | PlanStep::RunGit { .. }
        )
    }

    /// Get a human-readable description of this step.
    pub fn description(&self) -> String {
        match self {
            PlanStep::UpdateRefCas {
                refname, reason, ..
            } => {
                format!("Update {}: {}", refname, reason)
            }
            PlanStep::DeleteRefCas {
                refname, reason, ..
            } => {
                format!("Delete {}: {}", refname, reason)
            }
            PlanStep::WriteMetadataCas { branch, .. } => {
                format!("Write metadata for {}", branch)
            }
            PlanStep::DeleteMetadataCas { branch, .. } => {
                format!("Delete metadata for {}", branch)
            }
            PlanStep::RunGit { description, .. } => description.clone(),
            PlanStep::Checkpoint { name } => format!("Checkpoint: {}", name),
            PlanStep::PotentialConflictPause {
                branch,
                git_operation,
            } => {
                format!("May pause for {} conflict on {}", git_operation, branch)
            }
        }
    }
}

/// A complete execution plan.
///
/// Contains all information needed for the executor to apply changes
/// to the repository. Plans are immutable once created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Operation ID for journal correlation.
    pub op_id: OpId,
    /// Command that generated this plan.
    pub command: String,
    /// Ordered steps to execute.
    pub steps: Vec<PlanStep>,
    /// All refs that will be touched (for CAS validation).
    touched_refs: Vec<String>,
    /// Pre-computed digest for integrity checking.
    #[serde(skip)]
    digest_cache: Option<String>,
}

impl Plan {
    /// Create a new empty plan.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::engine::plan::Plan;
    /// use lattice::core::ops::journal::OpId;
    ///
    /// let plan = Plan::new(OpId::new(), "restack");
    /// assert!(plan.is_empty());
    /// ```
    pub fn new(op_id: OpId, command: impl Into<String>) -> Self {
        Self {
            op_id,
            command: command.into(),
            steps: vec![],
            touched_refs: vec![],
            digest_cache: None,
        }
    }

    /// Add a step to the plan (builder pattern).
    pub fn with_step(mut self, step: PlanStep) -> Self {
        // Collect touched refs
        for r in step.touched_refs() {
            if !self.touched_refs.contains(&r.to_string()) {
                self.touched_refs.push(r.to_string());
            }
        }
        self.steps.push(step);
        self.digest_cache = None; // Invalidate cache
        self
    }

    /// Add multiple steps.
    pub fn with_steps(mut self, steps: impl IntoIterator<Item = PlanStep>) -> Self {
        for step in steps {
            self = self.with_step(step);
        }
        self
    }

    /// Compute a digest of the plan for integrity checking.
    ///
    /// The digest is a SHA-256 hash of the canonical JSON serialization.
    /// This allows verifying that a plan hasn't been modified.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::engine::plan::{Plan, PlanStep};
    /// use lattice::core::ops::journal::OpId;
    ///
    /// let plan = Plan::new(OpId::new(), "test")
    ///     .with_step(PlanStep::Checkpoint { name: "start".to_string() });
    ///
    /// let digest = plan.digest();
    /// assert!(digest.starts_with("sha256:"));
    /// ```
    pub fn digest(&self) -> String {
        // Use cached value if available
        if let Some(ref cached) = self.digest_cache {
            return cached.clone();
        }

        // Compute digest from canonical JSON
        let json = serde_json::to_string(&self).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        let hash = hasher.finalize();
        format!("sha256:{}", hex::encode(hash))
    }

    /// Check if the plan is empty (no-op).
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Check if the plan has any mutation steps.
    pub fn has_mutations(&self) -> bool {
        self.steps.iter().any(|s| s.is_mutation())
    }

    /// Get the number of steps.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Get the number of mutation steps.
    pub fn mutation_count(&self) -> usize {
        self.steps.iter().filter(|s| s.is_mutation()).count()
    }

    /// Get all touched refs.
    pub fn touched_refs(&self) -> &[String] {
        &self.touched_refs
    }

    /// Generate a preview string for user confirmation.
    ///
    /// Returns a human-readable description of what the plan will do.
    pub fn preview(&self) -> String {
        if self.is_empty() {
            return format!("{}: No changes needed", self.command);
        }

        let mut lines = vec![format!("{}:", self.command)];

        for (i, step) in self.steps.iter().enumerate() {
            lines.push(format!("  {}. {}", i + 1, step.description()));
        }

        lines.join("\n")
    }

    /// Get an iterator over mutation steps only.
    pub fn mutations(&self) -> impl Iterator<Item = &PlanStep> {
        self.steps.iter().filter(|s| s.is_mutation())
    }
}

impl PartialEq for Plan {
    fn eq(&self, other: &Self) -> bool {
        self.op_id == other.op_id && self.command == other.command && self.steps == other.steps
    }
}

/// Errors from plan generation.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// Invalid state for planning.
    #[error("invalid state for planning: {0}")]
    InvalidState(String),

    /// Missing required data.
    #[error("missing required data: {0}")]
    MissingData(String),

    /// Conflict with frozen branch.
    #[error("cannot modify frozen branch: {0}")]
    FrozenBranch(String),
}

/// Plan the hello command.
///
/// The hello command has no actual steps - it's just a lifecycle validation.
/// This is kept for Milestone 0 compatibility.
pub fn plan_hello(ready: &ReadyContext) -> Result<Plan, PlanError> {
    let _ = ready; // Not used for hello
    Ok(Plan::new(OpId::new(), "hello"))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod plan_step {
        use super::*;

        #[test]
        fn update_ref_cas() {
            let step = PlanStep::UpdateRefCas {
                refname: "refs/heads/feature".to_string(),
                old_oid: Some("abc123".to_string()),
                new_oid: "def456".to_string(),
                reason: "rebase".to_string(),
            };

            assert!(step.is_mutation());
            assert!(step.description().contains("refs/heads/feature"));
        }

        #[test]
        fn delete_ref_cas() {
            let step = PlanStep::DeleteRefCas {
                refname: "refs/heads/feature".to_string(),
                old_oid: "abc123".to_string(),
                reason: "untrack".to_string(),
            };

            assert!(step.is_mutation());
            assert!(step.description().contains("Delete"));
        }

        #[test]
        fn checkpoint_not_mutation() {
            let step = PlanStep::Checkpoint {
                name: "test".to_string(),
            };

            assert!(!step.is_mutation());
            assert!(step.description().contains("Checkpoint"));
        }

        #[test]
        fn run_git() {
            let step = PlanStep::RunGit {
                args: vec![
                    "rebase".to_string(),
                    "--onto".to_string(),
                    "main".to_string(),
                ],
                description: "rebase feature onto main".to_string(),
                expected_effects: vec!["refs/heads/feature".to_string()],
            };

            assert!(step.is_mutation());
            let refs = step.touched_refs();
            assert!(refs.contains(&"refs/heads/feature"));
        }

        #[test]
        fn serialization_roundtrip() {
            let steps = vec![
                PlanStep::UpdateRefCas {
                    refname: "refs/heads/a".to_string(),
                    old_oid: None,
                    new_oid: "abc".to_string(),
                    reason: "create".to_string(),
                },
                PlanStep::Checkpoint {
                    name: "mid".to_string(),
                },
                PlanStep::DeleteRefCas {
                    refname: "refs/heads/b".to_string(),
                    old_oid: "def".to_string(),
                    reason: "delete".to_string(),
                },
            ];

            for step in steps {
                let json = serde_json::to_string(&step).unwrap();
                let parsed: PlanStep = serde_json::from_str(&json).unwrap();
                assert_eq!(step, parsed);
            }
        }
    }

    mod plan {
        use super::*;

        #[test]
        fn new_is_empty() {
            let plan = Plan::new(OpId::new(), "test");
            assert!(plan.is_empty());
            assert_eq!(plan.step_count(), 0);
            assert!(!plan.has_mutations());
        }

        #[test]
        fn with_step_builder() {
            let plan = Plan::new(OpId::new(), "test")
                .with_step(PlanStep::Checkpoint {
                    name: "a".to_string(),
                })
                .with_step(PlanStep::Checkpoint {
                    name: "b".to_string(),
                });

            assert_eq!(plan.step_count(), 2);
        }

        #[test]
        fn with_steps_builder() {
            let steps = vec![
                PlanStep::Checkpoint {
                    name: "a".to_string(),
                },
                PlanStep::Checkpoint {
                    name: "b".to_string(),
                },
            ];
            let plan = Plan::new(OpId::new(), "test").with_steps(steps);

            assert_eq!(plan.step_count(), 2);
        }

        #[test]
        fn digest_deterministic() {
            let op_id = OpId::from_string("fixed-id");
            let plan1 = Plan::new(op_id.clone(), "test").with_step(PlanStep::Checkpoint {
                name: "x".to_string(),
            });
            let plan2 = Plan::new(op_id, "test").with_step(PlanStep::Checkpoint {
                name: "x".to_string(),
            });

            assert_eq!(plan1.digest(), plan2.digest());
        }

        #[test]
        fn digest_changes_with_content() {
            let plan1 = Plan::new(OpId::new(), "test").with_step(PlanStep::Checkpoint {
                name: "a".to_string(),
            });
            let plan2 = Plan::new(OpId::new(), "test").with_step(PlanStep::Checkpoint {
                name: "b".to_string(),
            });

            // Different op_ids mean different digests
            assert_ne!(plan1.digest(), plan2.digest());
        }

        #[test]
        fn digest_has_prefix() {
            let plan = Plan::new(OpId::new(), "test");
            assert!(plan.digest().starts_with("sha256:"));
        }

        #[test]
        fn has_mutations() {
            let plan = Plan::new(OpId::new(), "test").with_step(PlanStep::Checkpoint {
                name: "a".to_string(),
            });
            assert!(!plan.has_mutations());

            let plan = Plan::new(OpId::new(), "test").with_step(PlanStep::UpdateRefCas {
                refname: "r".to_string(),
                old_oid: None,
                new_oid: "n".to_string(),
                reason: "r".to_string(),
            });
            assert!(plan.has_mutations());
        }

        #[test]
        fn mutation_count() {
            let plan = Plan::new(OpId::new(), "test")
                .with_step(PlanStep::Checkpoint {
                    name: "a".to_string(),
                })
                .with_step(PlanStep::UpdateRefCas {
                    refname: "r".to_string(),
                    old_oid: None,
                    new_oid: "n".to_string(),
                    reason: "r".to_string(),
                })
                .with_step(PlanStep::Checkpoint {
                    name: "b".to_string(),
                })
                .with_step(PlanStep::DeleteRefCas {
                    refname: "r".to_string(),
                    old_oid: "o".to_string(),
                    reason: "r".to_string(),
                });

            assert_eq!(plan.step_count(), 4);
            assert_eq!(plan.mutation_count(), 2);
        }

        #[test]
        fn preview_empty() {
            let plan = Plan::new(OpId::new(), "test");
            let preview = plan.preview();
            assert!(preview.contains("No changes"));
        }

        #[test]
        fn preview_with_steps() {
            let plan = Plan::new(OpId::new(), "restack")
                .with_step(PlanStep::Checkpoint {
                    name: "start".to_string(),
                })
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/heads/feature".to_string(),
                    old_oid: Some("abc".to_string()),
                    new_oid: "def".to_string(),
                    reason: "rebase onto main".to_string(),
                });

            let preview = plan.preview();
            assert!(preview.contains("restack"));
            assert!(preview.contains("1."));
            assert!(preview.contains("2."));
        }

        #[test]
        fn serialization_roundtrip() {
            let plan = Plan::new(OpId::from_string("test-id"), "cmd")
                .with_step(PlanStep::Checkpoint {
                    name: "a".to_string(),
                })
                .with_step(PlanStep::UpdateRefCas {
                    refname: "r".to_string(),
                    old_oid: None,
                    new_oid: "n".to_string(),
                    reason: "r".to_string(),
                });

            let json = serde_json::to_string(&plan).unwrap();
            let parsed: Plan = serde_json::from_str(&json).unwrap();

            assert_eq!(plan.op_id, parsed.op_id);
            assert_eq!(plan.command, parsed.command);
            assert_eq!(plan.steps, parsed.steps);
        }

        #[test]
        fn equality() {
            let op_id = OpId::from_string("same");
            let plan1 = Plan::new(op_id.clone(), "cmd").with_step(PlanStep::Checkpoint {
                name: "a".to_string(),
            });
            let plan2 = Plan::new(op_id, "cmd").with_step(PlanStep::Checkpoint {
                name: "a".to_string(),
            });

            assert_eq!(plan1, plan2);
        }

        #[test]
        fn touched_refs_tracked() {
            let plan = Plan::new(OpId::new(), "test")
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/heads/a".to_string(),
                    old_oid: None,
                    new_oid: "n".to_string(),
                    reason: "r".to_string(),
                })
                .with_step(PlanStep::DeleteRefCas {
                    refname: "refs/heads/b".to_string(),
                    old_oid: "o".to_string(),
                    reason: "r".to_string(),
                });

            let refs = plan.touched_refs();
            assert!(refs.contains(&"refs/heads/a".to_string()));
            assert!(refs.contains(&"refs/heads/b".to_string()));
        }
    }

    mod plan_error {
        use super::*;

        #[test]
        fn display_formatting() {
            let err = PlanError::InvalidState("test".to_string());
            assert!(err.to_string().contains("invalid state"));

            let err = PlanError::MissingData("trunk".to_string());
            assert!(err.to_string().contains("missing"));

            let err = PlanError::FrozenBranch("feature".to_string());
            assert!(err.to_string().contains("frozen"));
        }
    }

    mod plan_hello {
        use super::*;
        use crate::core::types::Fingerprint;
        use crate::engine::capabilities::Capability;
        use crate::engine::gate::{ReadyContext, ValidatedData};
        use crate::engine::health::RepoHealthReport;
        use crate::engine::scan::RepoSnapshot;
        use crate::git::{GitState, RepoInfo, WorktreeStatus};
        use std::collections::HashMap;
        use std::path::PathBuf;

        fn make_ready_context() -> ReadyContext {
            let mut health = RepoHealthReport::new();
            health.add_capability(Capability::RepoOpen);

            let snapshot = RepoSnapshot {
                info: RepoInfo {
                    git_dir: PathBuf::from("/repo/.git"),
                    work_dir: PathBuf::from("/repo"),
                },
                git_state: GitState::Clean,
                worktree_status: WorktreeStatus::default(),
                current_branch: None,
                branches: HashMap::new(),
                metadata: HashMap::new(),
                repo_config: None,
                trunk: None,
                graph: crate::core::graph::StackGraph::new(),
                fingerprint: Fingerprint::compute(&[]),
                health,
            };

            ReadyContext {
                snapshot,
                data: ValidatedData::None,
            }
        }

        #[test]
        fn produces_empty_plan() {
            let ctx = make_ready_context();
            let plan = plan_hello(&ctx).unwrap();
            assert!(plan.is_empty());
            assert_eq!(plan.command, "hello");
        }
    }
}
