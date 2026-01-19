//! engine::verify
//!
//! Post-execution invariant verification.
//!
//! # Architecture
//!
//! Per ARCHITECTURE.md Section 4.6, verification runs after plan execution
//! to confirm that repository invariants hold. This is the final gate
//! before declaring success.
//!
//! # Invariants Checked
//!
//! From SPEC.md Section 2.1, a self-consistent state requires:
//! 1. All metadata is parseable and valid
//! 2. Stack graph is acyclic
//! 3. All tracked branches exist as local refs
//! 4. For each tracked branch: base is ancestor of tip
//! 5. For each tracked branch: base is reachable from parent tip
//! 6. Freeze state is structurally valid
//!
//! # Invariants
//!
//! - Verify is read-only; it never mutates the repository
//! - Verify is deterministic
//! - If verify fails after execution, this indicates a bug
//!
//! # Example
//!
//! ```ignore
//! use latticework::engine::verify::fast_verify;
//! use latticework::engine::scan::scan;
//!
//! let snapshot = scan(&git)?;
//! fast_verify(&git, &snapshot)?;
//! println!("All invariants verified!");
//! ```

use thiserror::Error;

use super::scan::RepoSnapshot;
use crate::core::graph::StackGraph;
use crate::core::types::{BranchName, Oid};
use crate::git::Git;

/// Errors from verification.
#[derive(Debug, Error)]
pub enum VerifyError {
    /// Metadata is unparseable.
    #[error("metadata unparseable for {branch}: {message}")]
    MetadataUnparseable {
        /// The branch with bad metadata
        branch: String,
        /// The parse error message
        message: String,
    },

    /// Cycle detected in the stack graph.
    #[error("cycle detected in stack graph involving: {branches:?}")]
    CycleDetected {
        /// Branches involved in the cycle
        branches: Vec<String>,
    },

    /// Tracked branch does not exist as a local ref.
    #[error("tracked branch does not exist: {branch}")]
    BranchMissing {
        /// The missing branch
        branch: String,
    },

    /// Base commit is not an ancestor of the branch tip.
    #[error("base is not ancestor of tip for branch {branch}")]
    BaseNotAncestor {
        /// The branch with the violation
        branch: String,
        /// The base OID
        base_oid: String,
        /// The tip OID
        tip_oid: String,
    },

    /// Base commit is not reachable from parent's tip.
    #[error("base is not reachable from parent tip for branch {branch}")]
    BaseNotReachableFromParent {
        /// The branch with the violation
        branch: String,
        /// The parent branch
        parent: String,
        /// The base OID
        base_oid: String,
    },

    /// Invalid freeze state.
    #[error("invalid freeze state for branch {branch}: {message}")]
    InvalidFreezeState {
        /// The branch with the issue
        branch: String,
        /// Description of the problem
        message: String,
    },

    /// Git operation failed during verification.
    #[error("git error during verification: {0}")]
    Git(String),
}

/// Fast verification of core invariants.
///
/// Checks that the repository is in a self-consistent state as defined
/// by SPEC.md Section 2.1. This is called after plan execution to
/// confirm that the operation succeeded correctly.
///
/// # Arguments
///
/// * `git` - Git interface for ancestry checks
/// * `snapshot` - The scanned repository snapshot
///
/// # Returns
///
/// `Ok(())` if all invariants hold, `Err(VerifyError)` otherwise.
///
/// # Example
///
/// ```ignore
/// let snapshot = scan(&git)?;
/// fast_verify(&git, &snapshot)?;
/// ```
pub fn fast_verify(git: &Git, snapshot: &RepoSnapshot) -> Result<(), VerifyError> {
    // 1. Check for cycles in the stack graph
    verify_no_cycles(&snapshot.graph)?;

    // 2. Check all tracked branches exist
    verify_branches_exist(snapshot)?;

    // 3. Check base ancestry constraints
    verify_base_ancestry(git, snapshot)?;

    // 4. Check freeze state validity
    verify_freeze_state(snapshot)?;

    Ok(())
}

/// Verify the stack graph has no cycles.
fn verify_no_cycles(graph: &StackGraph) -> Result<(), VerifyError> {
    if let Some(cycle_branch) = graph.find_cycle() {
        // Collect cycle path
        let mut branches = vec![cycle_branch.as_str().to_string()];
        let mut current = graph.parent(&cycle_branch);
        while let Some(parent) = current {
            if branches.contains(&parent.as_str().to_string()) {
                break;
            }
            branches.push(parent.as_str().to_string());
            current = graph.parent(parent);
        }

        return Err(VerifyError::CycleDetected { branches });
    }

    Ok(())
}

/// Verify all tracked branches exist as local refs.
fn verify_branches_exist(snapshot: &RepoSnapshot) -> Result<(), VerifyError> {
    for branch in snapshot.metadata.keys() {
        if !snapshot.branches.contains_key(branch) {
            return Err(VerifyError::BranchMissing {
                branch: branch.as_str().to_string(),
            });
        }
    }

    Ok(())
}

/// Verify base ancestry constraints for all tracked branches.
///
/// For each tracked branch:
/// - Base must be an ancestor of the branch tip
/// - Base must be reachable from parent's tip
fn verify_base_ancestry(git: &Git, snapshot: &RepoSnapshot) -> Result<(), VerifyError> {
    for (branch, scanned) in &snapshot.metadata {
        let base_oid_str = &scanned.metadata.base.oid;
        let base_oid = match Oid::new(base_oid_str) {
            Ok(oid) => oid,
            Err(_) => continue, // Invalid OID format - would be caught by metadata parsing
        };

        // Get branch tip
        let tip_oid = match snapshot.branches.get(branch) {
            Some(oid) => oid,
            None => continue, // Missing branch - caught by verify_branches_exist
        };

        // Check: base is ancestor of tip
        match git.is_ancestor(&base_oid, tip_oid) {
            Ok(true) => {} // Good
            Ok(false) => {
                return Err(VerifyError::BaseNotAncestor {
                    branch: branch.as_str().to_string(),
                    base_oid: base_oid.to_string(),
                    tip_oid: tip_oid.to_string(),
                });
            }
            Err(e) => {
                return Err(VerifyError::Git(format!(
                    "failed to check ancestry for {}: {}",
                    branch, e
                )));
            }
        }

        // Check: base is reachable from parent tip
        let parent_name = scanned.metadata.parent.name();
        if let Ok(parent_branch) = BranchName::new(parent_name) {
            // Skip if parent is trunk and not tracked (trunk isn't tracked)
            if let Some(trunk) = &snapshot.trunk {
                if &parent_branch == trunk {
                    // Parent is trunk - check base is ancestor of trunk tip
                    if let Some(trunk_tip) = snapshot.branches.get(trunk) {
                        match git.is_ancestor(&base_oid, trunk_tip) {
                            Ok(true) => {} // Good
                            Ok(false) => {
                                return Err(VerifyError::BaseNotReachableFromParent {
                                    branch: branch.as_str().to_string(),
                                    parent: parent_name.to_string(),
                                    base_oid: base_oid.to_string(),
                                });
                            }
                            Err(e) => {
                                return Err(VerifyError::Git(format!(
                                    "failed to check base reachability for {}: {}",
                                    branch, e
                                )));
                            }
                        }
                    }
                    continue;
                }
            }

            // For non-trunk parents, check base is reachable from parent's tip
            if let Some(parent_tip) = snapshot.branches.get(&parent_branch) {
                match git.is_ancestor(&base_oid, parent_tip) {
                    Ok(true) => {} // Good
                    Ok(false) => {
                        return Err(VerifyError::BaseNotReachableFromParent {
                            branch: branch.as_str().to_string(),
                            parent: parent_name.to_string(),
                            base_oid: base_oid.to_string(),
                        });
                    }
                    Err(e) => {
                        return Err(VerifyError::Git(format!(
                            "failed to check base reachability for {}: {}",
                            branch, e
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Verify freeze state is structurally valid.
fn verify_freeze_state(snapshot: &RepoSnapshot) -> Result<(), VerifyError> {
    use crate::core::metadata::schema::FreezeState;

    for scanned in snapshot.metadata.values() {
        let freeze = &scanned.metadata.freeze;

        // Check that frozen branches have valid freeze state
        // The FreezeState enum enforces that Frozen variants always have a scope,
        // so this is just a structural validation that the state makes sense.
        match freeze {
            FreezeState::Frozen { scope, .. } => {
                // Scope is always present when frozen (enforced by enum structure)
                // This match arm just confirms the invariant holds
                let _ = scope;
            }
            FreezeState::Unfrozen => {
                // Unfrozen is always valid
            }
        }
    }

    Ok(())
}

/// Verify a single branch's invariants.
///
/// Useful for targeted verification after modifying a specific branch.
pub fn verify_branch(
    git: &Git,
    snapshot: &RepoSnapshot,
    branch: &BranchName,
) -> Result<(), VerifyError> {
    // Check branch exists
    if !snapshot.branches.contains_key(branch) {
        return Err(VerifyError::BranchMissing {
            branch: branch.as_str().to_string(),
        });
    }

    // Check metadata exists and is valid
    let scanned = match snapshot.metadata.get(branch) {
        Some(s) => s,
        None => return Ok(()), // Not tracked - nothing to verify
    };

    // Check base ancestry
    let base_oid_str = &scanned.metadata.base.oid;
    let base_oid = Oid::new(base_oid_str).map_err(|e| VerifyError::MetadataUnparseable {
        branch: branch.as_str().to_string(),
        message: e.to_string(),
    })?;

    let tip_oid = snapshot.branches.get(branch).unwrap(); // Checked above

    if !git.is_ancestor(&base_oid, tip_oid).unwrap_or(false) {
        return Err(VerifyError::BaseNotAncestor {
            branch: branch.as_str().to_string(),
            base_oid: base_oid.to_string(),
            tip_oid: tip_oid.to_string(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::graph::StackGraph;
    use crate::core::metadata::schema::BranchMetadataV1;
    use crate::core::types::Fingerprint;
    use crate::engine::health::RepoHealthReport;
    use crate::engine::scan::ScannedMetadata;
    use crate::git::{GitState, RepoInfo, WorktreeStatus};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_oid(s: &str) -> Oid {
        // Pad to 40 chars if needed
        let padded = format!("{:0<40}", s);
        Oid::new(&padded).unwrap()
    }

    fn make_empty_snapshot() -> RepoSnapshot {
        RepoSnapshot {
            info: RepoInfo {
                git_dir: PathBuf::from("/repo/.git"),
                common_dir: PathBuf::from("/repo/.git"),
                work_dir: Some(PathBuf::from("/repo")),
                context: crate::git::RepoContext::Normal,
            },
            git_state: GitState::Clean,
            worktree_status: WorktreeStatus::default(),
            current_branch: None,
            branches: HashMap::new(),
            metadata: HashMap::new(),
            repo_config: None,
            trunk: None,
            graph: StackGraph::new(),
            fingerprint: Fingerprint::compute(&[]),
            health: RepoHealthReport::new(),
            remote_prs: None,
        }
    }

    mod verify_no_cycles {
        use super::*;

        #[test]
        fn empty_graph_no_cycle() {
            let graph = StackGraph::new();
            assert!(verify_no_cycles(&graph).is_ok());
        }

        #[test]
        fn linear_chain_no_cycle() {
            let mut graph = StackGraph::new();
            let main = BranchName::new("main").unwrap();
            let a = BranchName::new("a").unwrap();
            let b = BranchName::new("b").unwrap();

            graph.add_edge(a.clone(), main.clone());
            graph.add_edge(b.clone(), a.clone());

            assert!(verify_no_cycles(&graph).is_ok());
        }

        #[test]
        fn cycle_detected() {
            let mut graph = StackGraph::new();
            let a = BranchName::new("a").unwrap();
            let b = BranchName::new("b").unwrap();
            let c = BranchName::new("c").unwrap();

            graph.add_edge(a.clone(), b.clone());
            graph.add_edge(b.clone(), c.clone());
            graph.add_edge(c.clone(), a.clone()); // Creates cycle

            let result = verify_no_cycles(&graph);
            assert!(matches!(result, Err(VerifyError::CycleDetected { .. })));
        }
    }

    mod verify_branches_exist {
        use super::*;

        #[test]
        fn all_exist() {
            let mut snapshot = make_empty_snapshot();

            let branch = BranchName::new("feature").unwrap();
            let oid = make_oid("abc");

            snapshot.branches.insert(branch.clone(), oid.clone());
            snapshot.metadata.insert(
                branch.clone(),
                ScannedMetadata {
                    ref_oid: oid.clone(),
                    metadata: BranchMetadataV1::new(
                        branch.clone(),
                        BranchName::new("main").unwrap(),
                        oid,
                    ),
                },
            );

            assert!(verify_branches_exist(&snapshot).is_ok());
        }

        #[test]
        fn missing_branch() {
            let mut snapshot = make_empty_snapshot();

            let branch = BranchName::new("feature").unwrap();
            let oid = make_oid("abc");

            // Metadata exists but branch doesn't
            snapshot.metadata.insert(
                branch.clone(),
                ScannedMetadata {
                    ref_oid: oid.clone(),
                    metadata: BranchMetadataV1::new(
                        branch.clone(),
                        BranchName::new("main").unwrap(),
                        oid,
                    ),
                },
            );

            let result = verify_branches_exist(&snapshot);
            assert!(matches!(result, Err(VerifyError::BranchMissing { .. })));
        }
    }

    mod verify_freeze_state {
        use super::*;
        use crate::core::metadata::schema::{FreezeScope, FreezeState};

        #[test]
        fn unfrozen_valid() {
            let mut snapshot = make_empty_snapshot();

            let branch = BranchName::new("feature").unwrap();
            let oid = make_oid("abc");

            snapshot.metadata.insert(
                branch.clone(),
                ScannedMetadata {
                    ref_oid: oid.clone(),
                    metadata: BranchMetadataV1::new(
                        branch.clone(),
                        BranchName::new("main").unwrap(),
                        oid,
                    ),
                },
            );

            assert!(verify_freeze_state(&snapshot).is_ok());
        }

        #[test]
        fn frozen_with_scope_valid() {
            let mut snapshot = make_empty_snapshot();

            let branch = BranchName::new("feature").unwrap();
            let oid = make_oid("abc");

            let meta = BranchMetadataV1::builder(
                branch.clone(),
                BranchName::new("main").unwrap(),
                oid.clone(),
            )
            .freeze_state(FreezeState::frozen(
                FreezeScope::DownstackInclusive,
                Some("test".to_string()),
            ))
            .build();

            snapshot.metadata.insert(
                branch.clone(),
                ScannedMetadata {
                    ref_oid: oid,
                    metadata: meta,
                },
            );

            assert!(verify_freeze_state(&snapshot).is_ok());
        }
    }

    mod verify_error {
        use super::*;

        #[test]
        fn display_formatting() {
            let err = VerifyError::MetadataUnparseable {
                branch: "feature".to_string(),
                message: "invalid json".to_string(),
            };
            assert!(err.to_string().contains("feature"));
            assert!(err.to_string().contains("invalid json"));

            let err = VerifyError::CycleDetected {
                branches: vec!["a".to_string(), "b".to_string()],
            };
            assert!(err.to_string().contains("cycle"));

            let err = VerifyError::BranchMissing {
                branch: "feature".to_string(),
            };
            assert!(err.to_string().contains("does not exist"));

            let err = VerifyError::BaseNotAncestor {
                branch: "feature".to_string(),
                base_oid: "abc".to_string(),
                tip_oid: "def".to_string(),
            };
            assert!(err.to_string().contains("not ancestor"));

            let err = VerifyError::BaseNotReachableFromParent {
                branch: "feature".to_string(),
                parent: "main".to_string(),
                base_oid: "abc".to_string(),
            };
            assert!(err.to_string().contains("not reachable"));

            let err = VerifyError::InvalidFreezeState {
                branch: "feature".to_string(),
                message: "no scope".to_string(),
            };
            assert!(err.to_string().contains("freeze state"));
        }
    }
}
