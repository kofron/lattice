//! engine::scan
//!
//! Repository scanning and capability detection.
//!
//! # Architecture
//!
//! The scanner reads repository state and produces a `RepoSnapshot` containing:
//! - All refs (branches, metadata)
//! - Parsed metadata for tracked branches
//! - Repository and global configuration
//! - Stack graph derived from metadata
//! - Repository fingerprint for divergence detection
//! - Health report with issues and capabilities
//!
//! Per ARCHITECTURE.md Section 4.1, the scanner:
//! - Reads repo config, global config
//! - Reads metadata refs and branch refs
//! - Detects in-progress Git operations
//! - Detects Lattice op in progress
//! - Produces `RepoSnapshot`, `RepoHealthReport`, and `Capabilities`
//!
//! # Invariants
//!
//! - Scan is read-only; it never mutates the repository
//! - Scan is deterministic given the same repository state
//! - Capabilities are binary: present or absent (no partial)

use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

use super::capabilities::Capability;
use super::health::{issues, Evidence, Issue, RepoHealthReport, Severity};
use crate::core::config::schema::RepoConfig;
use crate::core::config::{Config, ConfigError};
use crate::core::graph::StackGraph;
use crate::core::metadata::schema::BranchMetadataV1;
use crate::core::metadata::store::{MetadataStore, StoreError};
use crate::core::ops::journal::OpState;
use crate::core::types::{BranchName, Fingerprint, Oid, RefName};
use crate::git::{Git, GitError, GitState, RepoInfo, WorktreeStatus};

/// Errors from scanning operations.
#[derive(Debug, Error)]
pub enum ScanError {
    /// Failed to open repository.
    #[error("failed to open repository: {0}")]
    RepoOpen(#[from] GitError),

    /// Failed to load config.
    #[error("failed to load config: {0}")]
    Config(#[from] ConfigError),

    /// Failed to read metadata.
    #[error("failed to read metadata: {0}")]
    Metadata(#[from] StoreError),

    /// Internal error.
    #[error("internal scan error: {0}")]
    Internal(String),
}

/// A parsed metadata entry with its ref OID.
#[derive(Debug, Clone)]
pub struct ScannedMetadata {
    /// The metadata ref OID (blob pointer, for CAS).
    pub ref_oid: Oid,
    /// The parsed metadata.
    pub metadata: BranchMetadataV1,
}

/// Complete snapshot of repository state.
///
/// This is the primary output of scanning. It contains all information
/// needed for gating and planning.
#[derive(Debug)]
pub struct RepoSnapshot {
    /// Repository info (git_dir and work_dir paths).
    pub info: RepoInfo,

    /// Current Git state (rebase/merge/etc in progress).
    pub git_state: GitState,

    /// Working tree status (staged/unstaged/conflicts).
    pub worktree_status: WorktreeStatus,

    /// Current branch, if on a branch (None if detached HEAD).
    pub current_branch: Option<BranchName>,

    /// All local branches with their tip OIDs.
    pub branches: HashMap<BranchName, Oid>,

    /// All metadata entries for tracked branches.
    ///
    /// Only includes successfully parsed metadata. Parse failures
    /// are recorded as issues in the health report.
    pub metadata: HashMap<BranchName, ScannedMetadata>,

    /// Repository configuration (from .git/lattice/config.toml).
    pub repo_config: Option<RepoConfig>,

    /// Configured trunk branch name (from repo config).
    pub trunk: Option<BranchName>,

    /// Stack graph derived from metadata parent pointers.
    pub graph: StackGraph,

    /// Repository fingerprint for divergence detection.
    pub fingerprint: Fingerprint,

    /// Health report with issues and capabilities.
    pub health: RepoHealthReport,
}

impl RepoSnapshot {
    /// Check if the repository has a Lattice operation in progress.
    pub fn has_lattice_op_in_progress(&self) -> bool {
        !self
            .health
            .capabilities()
            .has(&Capability::NoLatticeOpInProgress)
    }

    /// Check if the repository has a Git operation in progress.
    pub fn has_git_op_in_progress(&self) -> bool {
        self.git_state.is_in_progress()
    }

    /// Get the configured trunk branch.
    pub fn trunk(&self) -> Option<&BranchName> {
        self.trunk.as_ref()
    }

    /// Check if a branch is tracked (has metadata).
    pub fn is_tracked(&self, branch: &BranchName) -> bool {
        self.metadata.contains_key(branch)
    }

    /// Get metadata for a branch.
    pub fn get_metadata(&self, branch: &BranchName) -> Option<&ScannedMetadata> {
        self.metadata.get(branch)
    }

    /// Get the tip OID for a branch.
    pub fn branch_tip(&self, branch: &BranchName) -> Option<&Oid> {
        self.branches.get(branch)
    }

    /// Get all tracked branch names.
    pub fn tracked_branches(&self) -> impl Iterator<Item = &BranchName> {
        self.metadata.keys()
    }

    /// Get the number of tracked branches.
    pub fn tracked_count(&self) -> usize {
        self.metadata.len()
    }
}

/// Scan a repository and produce a complete snapshot.
///
/// This is the main entry point for scanning. It reads all repository
/// state and produces a `RepoSnapshot` with health report and capabilities.
///
/// # Arguments
///
/// * `git` - The Git interface for the repository
///
/// # Returns
///
/// A `RepoSnapshot` containing all scanned state.
///
/// # Example
///
/// ```ignore
/// let git = Git::open(Path::new("."))?;
/// let snapshot = scan(&git)?;
///
/// if snapshot.health.capabilities().has(&Capability::GraphValid) {
///     println!("Stack graph is valid");
/// }
///
/// for issue in snapshot.health.blocking_issues() {
///     println!("Blocking issue: {}", issue.message);
/// }
/// ```
pub fn scan(git: &Git) -> Result<RepoSnapshot, ScanError> {
    let mut health = RepoHealthReport::new();

    // Get repository info
    let info = git.info()?;

    // RepoOpen capability established
    health.add_capability(Capability::RepoOpen);

    // Check Git state
    let git_state = git.state();
    if git_state.is_in_progress() {
        health.add_issue(issues::git_operation_in_progress(git_state.description()));
    } else {
        health.add_capability(Capability::NoExternalGitOpInProgress);
    }

    // Check for Lattice op in progress
    if let Some(op_state) = OpState::read(&info.git_dir).unwrap_or(None) {
        health.add_issue(issues::lattice_operation_in_progress(
            &op_state.command,
            op_state.op_id.as_str(),
        ));
    } else {
        health.add_capability(Capability::NoLatticeOpInProgress);
    }

    // Get worktree status
    let worktree_status = git.worktree_status(false).unwrap_or_default();
    health.add_capability(Capability::WorkingCopyStateKnown);

    // Get current branch
    let current_branch = git.current_branch().unwrap_or(None);

    // Load repo config
    let repo_config = Config::load(Some(&info.work_dir))
        .ok()
        .and_then(|r| r.config.repo);

    // Get trunk from config
    let trunk = repo_config
        .as_ref()
        .and_then(|c| c.trunk.as_ref())
        .and_then(|t| BranchName::new(t).ok());

    if trunk.is_some() {
        health.add_capability(Capability::TrunkKnown);
    } else {
        health.add_issue(issues::trunk_not_configured());
    }

    // List all local branches
    let branch_list = git.list_branches().unwrap_or_default();
    let mut branches = HashMap::new();
    for branch in branch_list {
        if let Ok(oid) = git.resolve_ref(&format!("refs/heads/{}", branch)) {
            branches.insert(branch, oid);
        }
    }

    // Read all metadata
    let store = MetadataStore::new(git);
    let metadata_refs = store.list_with_oids().unwrap_or_default();

    let mut metadata = HashMap::new();
    let mut all_metadata_readable = true;

    for (branch, ref_oid) in metadata_refs {
        match store.read(&branch) {
            Ok(Some(entry)) => {
                // Check if the branch actually exists
                if !branches.contains_key(&branch) {
                    health.add_issue(issues::missing_branch(branch.as_str()));
                }

                metadata.insert(
                    branch,
                    ScannedMetadata {
                        ref_oid: entry.ref_oid,
                        metadata: entry.metadata,
                    },
                );
            }
            Ok(None) => {
                // Metadata ref exists but couldn't be read (shouldn't happen)
                health.add_issue(
                    Issue::new(
                        "metadata-read-error",
                        Severity::Blocking,
                        format!(
                            "Failed to read metadata for branch '{}' despite ref existing",
                            branch
                        ),
                    )
                    .with_evidence(Evidence::Ref {
                        name: format!("refs/branch-metadata/{}", branch),
                        oid: Some(ref_oid.to_string()),
                    })
                    .blocks(Capability::MetadataReadable),
                );
                all_metadata_readable = false;
            }
            Err(StoreError::ParseError(msg)) => {
                health.add_issue(issues::metadata_parse_error(branch.as_str(), &msg));
                all_metadata_readable = false;
            }
            Err(StoreError::MetadataError(e)) => {
                health.add_issue(issues::metadata_parse_error(
                    branch.as_str(),
                    &e.to_string(),
                ));
                all_metadata_readable = false;
            }
            Err(e) => {
                health.add_issue(
                    Issue::new(
                        "metadata-read-error",
                        Severity::Blocking,
                        format!("Failed to read metadata for branch '{}': {}", branch, e),
                    )
                    .blocks(Capability::MetadataReadable),
                );
                all_metadata_readable = false;
            }
        }
    }

    if all_metadata_readable {
        health.add_capability(Capability::MetadataReadable);
    }

    // Build stack graph from metadata
    let mut graph = StackGraph::new();
    for (branch, scanned) in &metadata {
        let parent_name = scanned.metadata.parent.name();
        if let Ok(parent) = BranchName::new(parent_name) {
            graph.add_edge(branch.clone(), parent);
        }
    }

    // Check for cycles
    if let Some(cycle_branch) = graph.find_cycle() {
        // Collect the cycle path
        let mut cycle_branches = vec![cycle_branch.as_str().to_string()];
        let mut current = graph.parent(&cycle_branch);
        while let Some(parent) = current {
            if cycle_branches.contains(&parent.as_str().to_string()) {
                break;
            }
            cycle_branches.push(parent.as_str().to_string());
            current = graph.parent(parent);
        }
        health.add_issue(issues::graph_cycle(cycle_branches));
    } else if all_metadata_readable {
        health.add_capability(Capability::GraphValid);
    }

    // Compute fingerprint
    let fingerprint = compute_fingerprint(&branches, &metadata, trunk.as_ref());

    // Default to FrozenPolicySatisfied (will be refined by gating for specific operations)
    health.add_capability(Capability::FrozenPolicySatisfied);

    // Check for RemoteResolved capability (GitHub remote configured)
    if let Ok(Some(remote_url)) = git.remote_url("origin") {
        if crate::forge::github::parse_github_url(&remote_url).is_some() {
            health.add_capability(Capability::RemoteResolved);
        } else {
            health.add_issue(issues::remote_not_github(&remote_url));
        }
    } else {
        health.add_issue(issues::no_remote_configured());
    }

    // Check for AuthAvailable capability (GitHub token present)
    if crate::cli::commands::has_github_token() {
        health.add_capability(Capability::AuthAvailable);
    }
    // Note: Missing auth is not an issue - it's just a missing capability.
    // Commands that need auth will gate on AuthAvailable.

    Ok(RepoSnapshot {
        info,
        git_state,
        worktree_status,
        current_branch,
        branches,
        metadata,
        repo_config,
        trunk,
        graph,
        fingerprint,
        health,
    })
}

/// Compute a repository fingerprint for divergence detection.
///
/// The fingerprint is a hash over:
/// - Trunk ref value (if configured)
/// - All tracked branch ref values
/// - All metadata ref values
///
/// # Example
///
/// ```ignore
/// let fp1 = compute_fingerprint(&branches, &metadata, Some(&trunk));
/// // ... something changes ...
/// let fp2 = compute_fingerprint(&branches, &metadata, Some(&trunk));
/// if fp1 != fp2 {
///     println!("Repository changed");
/// }
/// ```
pub fn compute_fingerprint(
    branches: &HashMap<BranchName, Oid>,
    metadata: &HashMap<BranchName, ScannedMetadata>,
    trunk: Option<&BranchName>,
) -> Fingerprint {
    let mut refs: Vec<(RefName, Oid)> = Vec::new();

    // Add trunk if configured
    if let Some(trunk) = trunk {
        if let Some(oid) = branches.get(trunk) {
            refs.push((RefName::for_branch(trunk), oid.clone()));
        }
    }

    // Add all branch tips
    for (name, oid) in branches {
        refs.push((RefName::for_branch(name), oid.clone()));
    }

    // Add all metadata refs
    for (name, scanned) in metadata {
        refs.push((RefName::for_metadata(name), scanned.ref_oid.clone()));
    }

    Fingerprint::compute(&refs)
}

/// Information about divergence from last committed state.
#[derive(Debug, Clone)]
pub struct DivergenceInfo {
    /// Fingerprint from last Committed event.
    pub prior_fingerprint: String,
    /// Current fingerprint.
    pub current_fingerprint: String,
    /// Refs that changed.
    pub changed_refs: Vec<String>,
}

/// Detect divergence from last committed state.
///
/// Compares the current fingerprint with the last `Committed` event in the
/// ledger. If they differ, returns information about the divergence.
///
/// Per ARCHITECTURE.md Section 7.2, divergence is not an error - it's evidence
/// that the repository was modified outside Lattice. This information is used
/// for audit and to inform gating decisions.
///
/// # Arguments
///
/// * `git` - Git interface for reading the ledger
/// * `current_fingerprint` - Current repository fingerprint from scan
///
/// # Returns
///
/// `Some(DivergenceInfo)` if divergence detected, `None` if fingerprints match
/// or if there's no prior committed state.
///
/// # Example
///
/// ```ignore
/// let snapshot = scan(&git)?;
/// if let Some(divergence) = detect_divergence(&git, &snapshot.fingerprint)? {
///     println!("Divergence detected!");
///     for ref_name in &divergence.changed_refs {
///         println!("  Changed: {}", ref_name);
///     }
/// }
/// ```
pub fn detect_divergence(
    git: &Git,
    current_fingerprint: &Fingerprint,
) -> Result<Option<DivergenceInfo>, ScanError> {
    use super::ledger::EventLedger;

    let ledger = EventLedger::new(git);

    // Get the last committed fingerprint
    let prior_fp = match ledger.last_committed_fingerprint() {
        Ok(Some(fp)) => fp,
        Ok(None) => return Ok(None), // No prior state to compare
        Err(e) => {
            // Log but don't fail - ledger issues shouldn't block scanning
            eprintln!("Warning: failed to read ledger: {}", e);
            return Ok(None);
        }
    };

    let current_fp = current_fingerprint.as_str();

    if prior_fp == current_fp {
        return Ok(None); // No divergence
    }

    // Divergence detected - we'd ideally compute changed refs but
    // that requires comparing detailed state which we don't have here.
    // For now, we note that divergence occurred.
    Ok(Some(DivergenceInfo {
        prior_fingerprint: prior_fp,
        current_fingerprint: current_fp.to_string(),
        changed_refs: vec![], // Would require storing ref state in ledger
    }))
}

/// Scan from a path (convenience wrapper).
///
/// Opens the repository at the given path and performs a full scan.
pub fn scan_from_path(path: &Path) -> Result<RepoSnapshot, ScanError> {
    let git = Git::open(path)?;
    scan(&git)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod fingerprint {
        use super::*;

        #[test]
        fn empty_produces_consistent_result() {
            let branches = HashMap::new();
            let metadata = HashMap::new();

            let fp1 = compute_fingerprint(&branches, &metadata, None);
            let fp2 = compute_fingerprint(&branches, &metadata, None);

            assert_eq!(fp1, fp2);
        }

        #[test]
        fn different_branches_different_fingerprint() {
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();

            let mut branches1 = HashMap::new();
            branches1.insert(BranchName::new("main").unwrap(), oid.clone());

            let mut branches2 = HashMap::new();
            branches2.insert(BranchName::new("other").unwrap(), oid.clone());

            let fp1 = compute_fingerprint(&branches1, &HashMap::new(), None);
            let fp2 = compute_fingerprint(&branches2, &HashMap::new(), None);

            assert_ne!(fp1, fp2);
        }

        #[test]
        fn different_oids_different_fingerprint() {
            let oid1 = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            let oid2 = Oid::new("def456abc7890123def456abc7890123def45678").unwrap();

            let mut branches1 = HashMap::new();
            branches1.insert(BranchName::new("main").unwrap(), oid1);

            let mut branches2 = HashMap::new();
            branches2.insert(BranchName::new("main").unwrap(), oid2);

            let fp1 = compute_fingerprint(&branches1, &HashMap::new(), None);
            let fp2 = compute_fingerprint(&branches2, &HashMap::new(), None);

            assert_ne!(fp1, fp2);
        }

        #[test]
        fn order_independent() {
            let oid1 = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            let oid2 = Oid::new("def456abc7890123def456abc7890123def45678").unwrap();

            let mut branches = HashMap::new();
            branches.insert(BranchName::new("branch-a").unwrap(), oid1.clone());
            branches.insert(BranchName::new("branch-b").unwrap(), oid2.clone());

            // Fingerprint should be deterministic regardless of iteration order
            let fp1 = compute_fingerprint(&branches, &HashMap::new(), None);
            let fp2 = compute_fingerprint(&branches, &HashMap::new(), None);

            assert_eq!(fp1, fp2);
        }
    }

    mod repo_snapshot {
        use super::*;

        fn make_snapshot() -> RepoSnapshot {
            RepoSnapshot {
                info: RepoInfo {
                    git_dir: std::path::PathBuf::from("/repo/.git"),
                    work_dir: std::path::PathBuf::from("/repo"),
                },
                git_state: GitState::Clean,
                worktree_status: WorktreeStatus::default(),
                current_branch: Some(BranchName::new("main").unwrap()),
                branches: HashMap::new(),
                metadata: HashMap::new(),
                repo_config: None,
                trunk: Some(BranchName::new("main").unwrap()),
                graph: StackGraph::new(),
                fingerprint: compute_fingerprint(&HashMap::new(), &HashMap::new(), None),
                health: RepoHealthReport::new(),
            }
        }

        #[test]
        fn trunk_accessor() {
            let snapshot = make_snapshot();
            assert_eq!(snapshot.trunk().map(|b| b.as_str()), Some("main"));
        }

        #[test]
        fn is_tracked() {
            let mut snapshot = make_snapshot();
            let branch = BranchName::new("feature").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();

            assert!(!snapshot.is_tracked(&branch));

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

            assert!(snapshot.is_tracked(&branch));
        }

        #[test]
        fn tracked_branches() {
            let mut snapshot = make_snapshot();
            let branch1 = BranchName::new("feature-a").unwrap();
            let branch2 = BranchName::new("feature-b").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();

            snapshot.metadata.insert(
                branch1.clone(),
                ScannedMetadata {
                    ref_oid: oid.clone(),
                    metadata: BranchMetadataV1::new(
                        branch1.clone(),
                        BranchName::new("main").unwrap(),
                        oid.clone(),
                    ),
                },
            );
            snapshot.metadata.insert(
                branch2.clone(),
                ScannedMetadata {
                    ref_oid: oid.clone(),
                    metadata: BranchMetadataV1::new(
                        branch2.clone(),
                        BranchName::new("main").unwrap(),
                        oid,
                    ),
                },
            );

            assert_eq!(snapshot.tracked_count(), 2);
        }
    }

    mod divergence_info {
        use super::*;

        #[test]
        fn constructible() {
            let info = DivergenceInfo {
                prior_fingerprint: "abc".to_string(),
                current_fingerprint: "def".to_string(),
                changed_refs: vec!["refs/heads/main".to_string()],
            };

            assert_eq!(info.prior_fingerprint, "abc");
            assert_eq!(info.current_fingerprint, "def");
            assert_eq!(info.changed_refs.len(), 1);
        }
    }

    mod scan_error {
        use super::*;

        #[test]
        fn display_formatting() {
            let err = ScanError::Internal("something went wrong".to_string());
            assert!(err.to_string().contains("something went wrong"));
        }
    }
}
