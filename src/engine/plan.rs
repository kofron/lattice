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
//! use latticework::engine::plan::{Plan, PlanStep};
//! use latticework::core::ops::journal::OpId;
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

use crate::core::metadata::schema::BranchMetadataV1;
use crate::core::ops::journal::{OpId, TouchedRef};
use crate::core::types::BranchName;

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

    /// Create a snapshot branch from a fetched PR ref with validation.
    ///
    /// This step:
    /// 1. Reads FETCH_HEAD to get the snapshot commit after a fetch
    /// 2. Validates the commit is an ancestor of the head branch (reachability check)
    /// 3. Creates the branch pointing to the snapshot commit
    /// 4. Computes merge-base for metadata base field
    ///
    /// Used by Milestone 5.9 to materialize synthetic stack snapshots.
    CreateSnapshotBranch {
        /// Name for the new branch (e.g., "lattice/snap/pr-42").
        branch_name: String,
        /// PR number (for error messages and logging).
        pr_number: u64,
        /// The synthetic head branch this snapshot belongs to.
        head_branch: String,
        /// Current OID of the head branch (for ancestry validation).
        head_oid: String,
    },

    /// Switch to a different branch (checkout).
    ///
    /// This step changes the working directory's HEAD to point to the specified
    /// branch. Unlike ref updates, this modifies the working directory state
    /// rather than refs directly.
    ///
    /// Used by navigation commands (checkout, up, down, top, bottom).
    Checkout {
        /// Branch to check out.
        branch: String,
        /// Human-readable reason for the checkout.
        reason: String,
    },

    // ========================================================================
    // Forge/Remote Steps (Phase 6)
    // ========================================================================
    /// Fetch from a remote repository.
    ///
    /// This step fetches refs from the specified remote. It may fetch
    /// all branches or a specific refspec.
    ForgeFetch {
        /// Remote name (e.g., "origin").
        remote: String,
        /// Specific refspec to fetch (optional, defaults to all).
        refspec: Option<String>,
    },

    /// Push a branch to a remote.
    ///
    /// This step pushes a local branch to the remote. It uses
    /// `--force-with-lease` for safety when force is enabled.
    ForgePush {
        /// Branch to push.
        branch: String,
        /// Use force push (with lease for safety).
        force: bool,
        /// Remote name (e.g., "origin").
        remote: String,
        /// Human-readable reason for the push.
        reason: String,
    },

    /// Create a pull request on the forge.
    ///
    /// This step creates a new PR via the forge API (e.g., GitHub).
    /// The created PR's number and URL are captured for subsequent steps.
    ForgeCreatePr {
        /// Head branch (the branch being merged).
        head: String,
        /// Base branch (the target branch, e.g., "main").
        base: String,
        /// PR title.
        title: String,
        /// PR body (optional).
        body: Option<String>,
        /// Create as draft PR.
        draft: bool,
    },

    /// Update an existing pull request.
    ///
    /// This step updates PR metadata via the forge API. All fields
    /// are optional - only specified fields are updated.
    ForgeUpdatePr {
        /// PR number to update.
        number: u64,
        /// New base branch (optional).
        base: Option<String>,
        /// New title (optional).
        title: Option<String>,
        /// New body (optional).
        body: Option<String>,
    },

    /// Toggle PR draft status.
    ///
    /// This step changes a PR between draft and ready-for-review states.
    /// Requires GraphQL API for GitHub.
    ForgeDraftToggle {
        /// PR number.
        number: u64,
        /// Set to draft (true) or ready for review (false).
        draft: bool,
    },

    /// Request reviewers on a PR.
    ///
    /// This step requests review from users and/or teams.
    ForgeRequestReviewers {
        /// PR number.
        number: u64,
        /// User logins to request review from.
        users: Vec<String>,
        /// Team slugs to request review from.
        teams: Vec<String>,
    },

    /// Merge a PR via the forge API.
    ///
    /// This step merges a PR using the specified merge method.
    ForgeMergePr {
        /// PR number to merge.
        number: u64,
        /// Merge method: "merge", "squash", or "rebase".
        method: String,
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
            PlanStep::CreateSnapshotBranch { branch_name, .. } => {
                // Will create refs/heads/{branch_name}
                vec![branch_name.as_str()]
            }
            PlanStep::Checkout { .. } => {
                // Checkout modifies HEAD but doesn't touch branch refs
                vec![]
            }
            // Forge steps don't touch local refs directly (except ForgePush which
            // pushes existing refs). The effects are remote-side.
            PlanStep::ForgeFetch { .. } => vec![],
            PlanStep::ForgePush { branch, .. } => {
                // Push reads the local branch ref
                vec![branch.as_str()]
            }
            PlanStep::ForgeCreatePr { .. } => vec![],
            PlanStep::ForgeUpdatePr { .. } => vec![],
            PlanStep::ForgeDraftToggle { .. } => vec![],
            PlanStep::ForgeRequestReviewers { .. } => vec![],
            PlanStep::ForgeMergePr { .. } => vec![],
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
                | PlanStep::CreateSnapshotBranch { .. }
                | PlanStep::Checkout { .. }
                // Forge steps that perform mutations (remote-side effects)
                | PlanStep::ForgeFetch { .. }
                | PlanStep::ForgePush { .. }
                | PlanStep::ForgeCreatePr { .. }
                | PlanStep::ForgeUpdatePr { .. }
                | PlanStep::ForgeDraftToggle { .. }
                | PlanStep::ForgeRequestReviewers { .. }
                | PlanStep::ForgeMergePr { .. }
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
            PlanStep::CreateSnapshotBranch {
                branch_name,
                pr_number,
                ..
            } => {
                format!(
                    "Create snapshot branch '{}' from PR #{}",
                    branch_name, pr_number
                )
            }
            PlanStep::Checkout { branch, reason } => {
                format!("Checkout '{}': {}", branch, reason)
            }
            // Forge step descriptions
            PlanStep::ForgeFetch { remote, refspec } => {
                if let Some(spec) = refspec {
                    format!("Fetch '{}' from {}", spec, remote)
                } else {
                    format!("Fetch from {}", remote)
                }
            }
            PlanStep::ForgePush {
                branch,
                force,
                remote,
                reason,
            } => {
                if *force {
                    format!("Force push '{}' to {}: {}", branch, remote, reason)
                } else {
                    format!("Push '{}' to {}: {}", branch, remote, reason)
                }
            }
            PlanStep::ForgeCreatePr {
                head, base, draft, ..
            } => {
                if *draft {
                    format!("Create draft PR: {} -> {}", head, base)
                } else {
                    format!("Create PR: {} -> {}", head, base)
                }
            }
            PlanStep::ForgeUpdatePr { number, .. } => {
                format!("Update PR #{}", number)
            }
            PlanStep::ForgeDraftToggle { number, draft } => {
                if *draft {
                    format!("Convert PR #{} to draft", number)
                } else {
                    format!("Mark PR #{} ready for review", number)
                }
            }
            PlanStep::ForgeRequestReviewers {
                number,
                users,
                teams,
            } => {
                let mut reviewers = Vec::new();
                reviewers.extend(users.iter().cloned());
                reviewers.extend(teams.iter().map(|t| format!("@{}", t)));
                format!("Request review on PR #{}: {}", number, reviewers.join(", "))
            }
            PlanStep::ForgeMergePr { number, method } => {
                format!("Merge PR #{} ({})", number, method)
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
    /// use latticework::engine::plan::Plan;
    /// use latticework::core::ops::journal::OpId;
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
    /// use latticework::engine::plan::{Plan, PlanStep};
    /// use latticework::core::ops::journal::OpId;
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

    /// Get touched refs with their expected old OIDs for CAS validation.
    ///
    /// This extracts the CAS preconditions from all plan steps, returning
    /// a list of `TouchedRef` entries suitable for storing in `OpState`.
    ///
    /// Per SPEC.md §4.6.5, this information is needed for rollback CAS.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::plan::{Plan, PlanStep};
    /// use latticework::core::ops::journal::OpId;
    ///
    /// let plan = Plan::new(OpId::new(), "test")
    ///     .with_step(PlanStep::UpdateRefCas {
    ///         refname: "refs/heads/feature".to_string(),
    ///         old_oid: Some("abc123".to_string()),
    ///         new_oid: "def456".to_string(),
    ///         reason: "test".to_string(),
    ///     });
    ///
    /// let touched = plan.touched_refs_with_oids();
    /// assert_eq!(touched.len(), 1);
    /// assert_eq!(touched[0].refname, "refs/heads/feature");
    /// assert_eq!(touched[0].expected_old, Some("abc123".to_string()));
    /// ```
    pub fn touched_refs_with_oids(&self) -> Vec<TouchedRef> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for step in &self.steps {
            match step {
                PlanStep::UpdateRefCas {
                    refname, old_oid, ..
                } => {
                    if seen.insert(refname.clone()) {
                        result.push(TouchedRef::new(refname.clone(), old_oid.clone()));
                    }
                }
                PlanStep::DeleteRefCas {
                    refname, old_oid, ..
                } => {
                    if seen.insert(refname.clone()) {
                        result.push(TouchedRef::new(refname.clone(), Some(old_oid.clone())));
                    }
                }
                PlanStep::WriteMetadataCas {
                    branch,
                    old_ref_oid,
                    ..
                } => {
                    let refname = format!("refs/branch-metadata/{}", branch);
                    if seen.insert(refname.clone()) {
                        result.push(TouchedRef::new(refname, old_ref_oid.clone()));
                    }
                }
                PlanStep::DeleteMetadataCas {
                    branch,
                    old_ref_oid,
                    ..
                } => {
                    let refname = format!("refs/branch-metadata/{}", branch);
                    if seen.insert(refname.clone()) {
                        result.push(TouchedRef::new(refname, Some(old_ref_oid.clone())));
                    }
                }
                // RunGit, Checkpoint, PotentialConflictPause, CreateSnapshotBranch
                // don't have explicit old OIDs - they're derived from expected_effects
                // or don't touch refs directly
                _ => {}
            }
        }

        result
    }

    /// Get all branch refs that will be touched by this plan.
    ///
    /// Returns branch names (not full refs) for branches under `refs/heads/`.
    /// This is used for worktree occupancy checking - branches checked out
    /// in other worktrees cannot be mutated.
    ///
    /// # Note
    ///
    /// Metadata refs (`refs/branch-metadata/`) are NOT included because
    /// metadata-only operations don't require occupancy checks per SPEC.md §4.6.8.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::plan::{Plan, PlanStep};
    /// use latticework::core::ops::journal::OpId;
    ///
    /// let plan = Plan::new(OpId::new(), "test")
    ///     .with_step(PlanStep::UpdateRefCas {
    ///         refname: "refs/heads/feature".to_string(),
    ///         old_oid: Some("abc".to_string()),
    ///         new_oid: "def".to_string(),
    ///         reason: "test".to_string(),
    ///     });
    ///
    /// let branches = plan.touched_branches();
    /// assert_eq!(branches.len(), 1);
    /// assert_eq!(branches[0].as_str(), "feature");
    /// ```
    pub fn touched_branches(&self) -> Vec<BranchName> {
        self.touched_refs
            .iter()
            .filter_map(|r| r.strip_prefix("refs/heads/"))
            .filter_map(|name| BranchName::new(name).ok())
            .collect()
    }

    /// Check if this plan touches any branch refs.
    ///
    /// Plans that only touch metadata refs don't need occupancy checks.
    /// Per SPEC.md §4.6.8, metadata-only commands (track, freeze, etc.)
    /// are NOT blocked by worktree occupancy.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::plan::{Plan, PlanStep};
    /// use latticework::core::ops::journal::OpId;
    ///
    /// // Plan with branch ref - needs occupancy check
    /// let plan = Plan::new(OpId::new(), "test")
    ///     .with_step(PlanStep::UpdateRefCas {
    ///         refname: "refs/heads/feature".to_string(),
    ///         old_oid: Some("abc".to_string()),
    ///         new_oid: "def".to_string(),
    ///         reason: "test".to_string(),
    ///     });
    /// assert!(plan.touches_branch_refs());
    ///
    /// // Plan with only checkpoint - no occupancy check needed
    /// let plan = Plan::new(OpId::new(), "test")
    ///     .with_step(PlanStep::Checkpoint { name: "start".to_string() });
    /// assert!(!plan.touches_branch_refs());
    /// ```
    pub fn touches_branch_refs(&self) -> bool {
        self.touched_refs
            .iter()
            .any(|r| r.starts_with("refs/heads/"))
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

        #[test]
        fn touched_branches_extracts_branch_names() {
            let plan = Plan::new(OpId::new(), "test")
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/heads/feature".to_string(),
                    old_oid: Some("abc".to_string()),
                    new_oid: "def".to_string(),
                    reason: "test".to_string(),
                })
                .with_step(PlanStep::DeleteRefCas {
                    refname: "refs/heads/other".to_string(),
                    old_oid: "ghi".to_string(),
                    reason: "test".to_string(),
                });

            let branches = plan.touched_branches();
            assert_eq!(branches.len(), 2);
            let names: Vec<&str> = branches.iter().map(|b| b.as_str()).collect();
            assert!(names.contains(&"feature"));
            assert!(names.contains(&"other"));
        }

        #[test]
        fn touched_branches_ignores_non_branch_refs() {
            let plan = Plan::new(OpId::new(), "test")
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/heads/feature".to_string(),
                    old_oid: None,
                    new_oid: "abc".to_string(),
                    reason: "test".to_string(),
                })
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/tags/v1.0".to_string(),
                    old_oid: None,
                    new_oid: "def".to_string(),
                    reason: "test".to_string(),
                });

            let branches = plan.touched_branches();
            assert_eq!(branches.len(), 1);
            assert_eq!(branches[0].as_str(), "feature");
        }

        #[test]
        fn touches_branch_refs_true_when_has_branch() {
            let plan = Plan::new(OpId::new(), "test").with_step(PlanStep::UpdateRefCas {
                refname: "refs/heads/feature".to_string(),
                old_oid: Some("abc".to_string()),
                new_oid: "def".to_string(),
                reason: "test".to_string(),
            });

            assert!(plan.touches_branch_refs());
        }

        #[test]
        fn touches_branch_refs_false_for_checkpoint_only() {
            let plan = Plan::new(OpId::new(), "test").with_step(PlanStep::Checkpoint {
                name: "start".to_string(),
            });

            assert!(!plan.touches_branch_refs());
        }

        #[test]
        fn touches_branch_refs_false_for_empty_plan() {
            let plan = Plan::new(OpId::new(), "test");
            assert!(!plan.touches_branch_refs());
        }

        #[test]
        fn touched_refs_with_oids_extracts_cas_preconditions() {
            let plan = Plan::new(OpId::new(), "test")
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/heads/feature".to_string(),
                    old_oid: Some("abc123".to_string()),
                    new_oid: "def456".to_string(),
                    reason: "rebase".to_string(),
                })
                .with_step(PlanStep::DeleteRefCas {
                    refname: "refs/heads/old-branch".to_string(),
                    old_oid: "ghi789".to_string(),
                    reason: "cleanup".to_string(),
                })
                .with_step(PlanStep::Checkpoint {
                    name: "mid".to_string(),
                });

            let touched = plan.touched_refs_with_oids();
            assert_eq!(touched.len(), 2);

            // First: UpdateRefCas
            assert_eq!(touched[0].refname, "refs/heads/feature");
            assert_eq!(touched[0].expected_old, Some("abc123".to_string()));

            // Second: DeleteRefCas
            assert_eq!(touched[1].refname, "refs/heads/old-branch");
            assert_eq!(touched[1].expected_old, Some("ghi789".to_string()));
        }

        #[test]
        fn touched_refs_with_oids_deduplicates() {
            let plan = Plan::new(OpId::new(), "test")
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/heads/feature".to_string(),
                    old_oid: Some("first".to_string()),
                    new_oid: "second".to_string(),
                    reason: "first update".to_string(),
                })
                .with_step(PlanStep::UpdateRefCas {
                    refname: "refs/heads/feature".to_string(),
                    old_oid: Some("second".to_string()),
                    new_oid: "third".to_string(),
                    reason: "second update".to_string(),
                });

            let touched = plan.touched_refs_with_oids();
            // Should only have one entry (first occurrence wins)
            assert_eq!(touched.len(), 1);
            assert_eq!(touched[0].refname, "refs/heads/feature");
            assert_eq!(touched[0].expected_old, Some("first".to_string()));
        }

        #[test]
        fn touched_refs_with_oids_includes_metadata_refs() {
            use crate::core::metadata::schema::BranchMetadataV1;
            use crate::core::types::{BranchName, Oid};

            let meta = BranchMetadataV1::new(
                BranchName::new("feature").unwrap(),
                BranchName::new("main").unwrap(),
                Oid::new("abc123abc123abc123abc123abc123abc123abc1").unwrap(),
            );
            let plan = Plan::new(OpId::new(), "track").with_step(PlanStep::WriteMetadataCas {
                branch: "feature".to_string(),
                old_ref_oid: None, // Creating new metadata
                metadata: Box::new(meta),
            });

            let touched = plan.touched_refs_with_oids();
            assert_eq!(touched.len(), 1);
            assert_eq!(touched[0].refname, "refs/branch-metadata/feature");
            assert!(touched[0].expected_old.is_none());
        }

        #[test]
        fn touched_refs_with_oids_handles_new_refs() {
            let plan = Plan::new(OpId::new(), "create").with_step(PlanStep::UpdateRefCas {
                refname: "refs/heads/new-branch".to_string(),
                old_oid: None, // Creating new ref
                new_oid: "abc123".to_string(),
                reason: "create branch".to_string(),
            });

            let touched = plan.touched_refs_with_oids();
            assert_eq!(touched.len(), 1);
            assert_eq!(touched[0].refname, "refs/heads/new-branch");
            assert!(touched[0].expected_old.is_none());
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
}
