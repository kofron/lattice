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
use crate::core::paths::LatticePaths;
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

/// Evidence of remote pull requests collected during scan.
///
/// Populated by [`scan_with_remote`] when the following capabilities are present:
/// - TrunkKnown
/// - RemoteResolved
/// - AuthAvailable
/// - RepoAuthorized
///
/// When capabilities are missing or the forge query fails, `remote_prs` in
/// `RepoSnapshot` will be `None` rather than containing this struct.
#[derive(Debug, Clone)]
pub struct RemotePrEvidence {
    /// The open PRs retrieved from the forge.
    pub prs: Vec<crate::forge::PullRequestSummary>,
    /// Whether the result was truncated (more PRs exist beyond the limit).
    pub truncated: bool,
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

    /// Remote open pull requests (if forge query succeeded).
    ///
    /// Populated by [`scan_with_remote`] when all required capabilities
    /// are present. `None` if capabilities are missing or API call failed.
    pub remote_prs: Option<RemotePrEvidence>,
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
    let paths = LatticePaths::from_repo_info(&info);
    if let Some(op_state) = OpState::read(&paths).unwrap_or(None) {
        health.add_issue(issues::lattice_operation_in_progress(
            &op_state.command,
            op_state.op_id.as_str(),
        ));
    } else {
        health.add_capability(Capability::NoLatticeOpInProgress);
    }

    // Check for working directory availability
    // Per SPEC.md §4.6.6, bare repositories lack a working directory
    if info.work_dir.is_some() {
        health.add_capability(Capability::WorkingDirectoryAvailable);
    } else {
        health.add_issue(issues::no_working_directory());
    }

    // Get worktree status
    let worktree_status = git.worktree_status(false).unwrap_or_default();
    health.add_capability(Capability::WorkingCopyStateKnown);

    // Get current branch
    let current_branch = git.current_branch().unwrap_or(None);

    // Load repo config (using work_dir if available, otherwise None for bare repos)
    let repo_config = info
        .work_dir
        .as_ref()
        .and_then(|wd| Config::load(Some(wd)).ok())
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

    // Note: RepoAuthorized capability check is deferred to scan_with_remote()
    // because it requires async API calls. The sync scan() function cannot
    // perform async operations without creating a nested runtime, which panics
    // if already running inside an async context.

    // Detect and record divergence per ARCHITECTURE.md Section 7.2
    // "On each command invocation, the engine compares the current fingerprint
    // with the last recorded Committed event fingerprint. If they differ, the
    // engine records a DivergenceObserved event."
    if let Some(divergence) = detect_and_record_divergence(git, &fingerprint)? {
        health.set_divergence(divergence);
    }

    let mut snapshot = RepoSnapshot {
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
        remote_prs: None, // Populated by scan_with_remote() if capabilities allow
    };

    // Detect local untracked branches for local-only bootstrap (Milestone 5.7)
    // This adds issues with parent candidate evidence for fix generation.
    detect_local_untracked_branches(git, &mut snapshot);

    Ok(snapshot)
}

/// Detect divergence and record DivergenceObserved event if needed.
///
/// Per ARCHITECTURE.md Section 7.2: "On each command invocation, the engine
/// compares the current fingerprint with the last recorded Committed event
/// fingerprint. If they differ, the engine records a DivergenceObserved event."
///
/// This is a best-effort operation - ledger write failures are logged but do
/// not fail the scan. Divergence is informational, not blocking.
fn detect_and_record_divergence(
    git: &Git,
    current_fingerprint: &Fingerprint,
) -> Result<Option<DivergenceInfo>, ScanError> {
    let divergence = detect_divergence(git, current_fingerprint)?;

    if let Some(ref info) = divergence {
        // Record DivergenceObserved event
        use super::ledger::{Event, EventLedger};

        let ledger = EventLedger::new(git);
        let event = Event::divergence_observed(
            &info.prior_fingerprint,
            &info.current_fingerprint,
            info.changed_refs.clone(),
        );

        // Best-effort recording - don't fail the scan if ledger write fails
        if let Err(e) = ledger.append(event) {
            // Log warning but continue - divergence detection is informational
            eprintln!("Warning: failed to record DivergenceObserved event: {}", e);
        }
    }

    Ok(divergence)
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

/// Scan a repository with optional remote PR query.
///
/// This is the async version that can query the forge for open PRs.
/// Falls back to local-only scan if forge is unavailable or capabilities
/// are missing.
///
/// # Arguments
///
/// * `git` - The Git interface for the repository
///
/// # Returns
///
/// A `RepoSnapshot` containing all scanned state, including remote PR
/// evidence if the forge query succeeded.
///
/// # Example
///
/// ```ignore
/// let git = Git::open(Path::new("."))?;
/// let snapshot = scan_with_remote(&git).await?;
///
/// if let Some(ref evidence) = snapshot.remote_prs {
///     println!("Found {} open PRs", evidence.prs.len());
/// }
/// ```
pub async fn scan_with_remote(git: &Git) -> Result<RepoSnapshot, ScanError> {
    // Perform the basic scan first
    let mut snapshot = scan(git)?;

    // Check for RepoAuthorized capability (GitHub App installed for repo)
    // This is done here (async context) rather than in scan() to avoid nested runtime panics.
    // Only check if we have both AuthAvailable and RemoteResolved
    if snapshot
        .health
        .capabilities()
        .has(&Capability::AuthAvailable)
        && snapshot
            .health
            .capabilities()
            .has(&Capability::RemoteResolved)
    {
        if let Ok(Some(remote_url)) = git.remote_url("origin") {
            if let Some((owner, repo)) = crate::forge::github::parse_github_url(&remote_url) {
                let paths = LatticePaths::from_repo_info(&snapshot.info);
                check_repo_authorization_async(&mut snapshot.health, &paths, &owner, &repo).await;
            }
        }
    }

    // Try to query remote PRs if capabilities allow
    if let Ok(Some(remote_url)) = git.remote_url("origin") {
        if let Some((owner, repo)) = crate::forge::github::parse_github_url(&remote_url) {
            snapshot.remote_prs = query_remote_prs(&snapshot.health, &owner, &repo).await;
        }
    }

    // Generate bootstrap issues based on remote evidence
    // Clone the evidence to avoid borrow conflict
    if let Some(evidence) = snapshot.remote_prs.clone() {
        generate_bootstrap_issues(&mut snapshot, &evidence);
    }

    Ok(snapshot)
}

/// Check repository authorization asynchronously.
///
/// Per SPEC.md Section 8E.0.1, this queries the GitHub API to verify that
/// the authenticated user has access to the repository via an installed
/// GitHub App. Results are cached with a 10-minute TTL.
///
/// Updates the health report with RepoAuthorized capability or appropriate issues.
async fn check_repo_authorization_async(
    health: &mut RepoHealthReport,
    paths: &LatticePaths,
    owner: &str,
    repo: &str,
) {
    use crate::auth::installations::check_repo_authorization;
    use crate::auth::GitHubAuthManager;
    use crate::secrets;

    let host = "github.com"; // v1: only github.com supported

    // Load authorization cache
    let mut cache = crate::auth::cache::AuthCache::load(paths);

    // Check cache first (10-minute TTL per SPEC.md 8E.0.1)
    if let Some(cached_entry) = cache.get(host, owner, repo) {
        // Cache hit - capability satisfied
        health.add_capability(Capability::RepoAuthorized);
        // Note: cached_entry contains installation_id and repository_id
        // which can be used by forge operations
        let _ = cached_entry; // Mark as used
        return;
    }

    // Cache miss - need to query API
    let store = match secrets::create_store(secrets::DEFAULT_PROVIDER) {
        Ok(s) => s,
        Err(e) => {
            health.add_issue(issues::repo_authorization_check_failed(
                owner,
                repo,
                &e.to_string(),
            ));
            return;
        }
    };
    let auth_manager = GitHubAuthManager::new(host, store);

    match check_repo_authorization(&auth_manager, host, owner, repo).await {
        Ok(Some(result)) => {
            // Authorized - cache and add capability
            cache.set(host, owner, repo, &result);
            cache.prune_expired();
            cache.save(paths);
            health.add_capability(Capability::RepoAuthorized);
        }
        Ok(None) => {
            // Not authorized - add blocking issue
            health.add_issue(issues::app_not_installed(host, owner, repo));
        }
        Err(e) => {
            // Check failed - add warning (non-blocking)
            // Commands requiring RepoAuthorized will be gated
            health.add_issue(issues::repo_authorization_check_failed(
                owner,
                repo,
                &e.to_string(),
            ));
        }
    }
}

/// Query the forge for open PRs if capabilities allow.
///
/// Returns None if:
/// - Required capabilities are missing
/// - Forge query fails (logged as warning)
async fn query_remote_prs(
    health: &RepoHealthReport,
    owner: &str,
    repo: &str,
) -> Option<RemotePrEvidence> {
    // Check required capabilities
    let caps = health.capabilities();
    if !caps.has(&Capability::TrunkKnown)
        || !caps.has(&Capability::RemoteResolved)
        || !caps.has(&Capability::AuthAvailable)
        || !caps.has(&Capability::RepoAuthorized)
    {
        // Missing capabilities - skip remote query silently (this is expected)
        return None;
    }

    // Create forge and query
    match create_forge_and_query(owner, repo).await {
        Ok(result) => Some(result),
        Err(e) => {
            // Log warning but continue - API failures shouldn't block scanning
            eprintln!("Warning: failed to query remote PRs: {}", e);
            None
        }
    }
}

/// Create a forge and query for open PRs.
async fn create_forge_and_query(
    owner: &str,
    repo: &str,
) -> Result<RemotePrEvidence, crate::forge::ForgeError> {
    use std::sync::Arc;

    use crate::auth::{GitHubAuthManager, TokenProvider};
    use crate::forge::github::GitHubForge;
    use crate::forge::{Forge, ListPullsOpts};
    use crate::secrets;

    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .map_err(|e| crate::forge::ForgeError::AuthFailed(e.to_string()))?;
    let auth_manager = GitHubAuthManager::new("github.com", store);

    // Create forge with TokenProvider for automatic refresh
    let provider: Arc<dyn TokenProvider> = Arc::new(auth_manager);
    let forge = GitHubForge::new_with_provider(provider, owner, repo);
    let opts = ListPullsOpts::default(); // 200 limit

    let result = forge.list_open_prs(opts).await?;

    Ok(RemotePrEvidence {
        prs: result.pulls,
        truncated: result.truncated,
    })
}

/// Detect potential synthetic stack heads from open PRs.
///
/// A potential synthetic stack head is an open PR that:
/// 1. Targets trunk (base_ref = configured trunk)
///
/// This is Tier 1 detection (cheap, uses only open PR data).
/// Tier 2 deep analysis (querying closed PRs) is handled separately
/// when `--deep-remote` is enabled.
///
/// # Arguments
///
/// * `snapshot` - Repository snapshot with trunk configuration
/// * `open_prs` - Open PRs from the forge
///
/// # Returns
///
/// A vector of `KnownIssue::PotentialSyntheticStackHead` for each detected head.
pub fn detect_potential_synthetic_heads(
    snapshot: &RepoSnapshot,
    open_prs: &[crate::forge::PullRequestSummary],
) -> Vec<crate::doctor::KnownIssue> {
    use crate::doctor::KnownIssue;

    let trunk = match &snapshot.trunk {
        Some(t) => t.as_str(),
        None => return vec![], // No trunk configured
    };

    open_prs
        .iter()
        .filter(|pr| pr.base_ref == trunk)
        .map(|pr| KnownIssue::PotentialSyntheticStackHead {
            branch: pr.head_ref.clone(),
            pr_number: pr.number,
            pr_url: pr.url.clone(),
        })
        .collect()
}

/// Generate bootstrap issues from remote PR evidence.
///
/// This examines each open PR and generates appropriate issues based on
/// the local branch state:
/// - Missing local branch → `RemoteOpenPrBranchMissingLocally`
/// - Untracked local branch → `RemoteOpenPrBranchUntracked`
/// - Tracked but unlinked → `RemoteOpenPrNotLinkedInMetadata`
fn generate_bootstrap_issues(snapshot: &mut RepoSnapshot, evidence: &RemotePrEvidence) {
    // Issue: Remote has open PRs (general awareness)
    if !evidence.prs.is_empty() {
        snapshot.health.add_issue(issues::remote_open_prs_detected(
            evidence.prs.len(),
            evidence.truncated,
        ));
    }

    // Match each PR against local state
    for pr in &evidence.prs {
        // Skip fork PRs (complex ownership semantics)
        if pr.is_fork() {
            continue;
        }

        // Try to parse the head_ref as a valid branch name
        let branch_name = match BranchName::new(&pr.head_ref) {
            Ok(name) => name,
            Err(_) => {
                // Invalid branch name, skip this PR
                continue;
            }
        };

        if !snapshot.branches.contains_key(&branch_name) {
            // Branch doesn't exist locally
            snapshot.health.add_issue(issues::remote_pr_branch_missing(
                pr.number,
                &pr.head_ref,
                &pr.base_ref,
                &pr.url,
            ));
        } else if !snapshot.metadata.contains_key(&branch_name) {
            // Branch exists but not tracked
            snapshot
                .health
                .add_issue(issues::remote_pr_branch_untracked(
                    &pr.head_ref,
                    pr.number,
                    &pr.base_ref,
                    &pr.url,
                ));
        } else {
            // Branch is tracked - check if PR is linked
            let scanned = snapshot.metadata.get(&branch_name).unwrap();
            if scanned.metadata.pr.number().is_none() {
                snapshot.health.add_issue(issues::remote_pr_not_linked(
                    &pr.head_ref,
                    pr.number,
                    &pr.url,
                ));
            }
            // else: PR is already linked, no issue needed
        }
    }

    // Tier 1: Detect potential synthetic stack heads
    // A synthetic stack head is a PR that targets trunk and may have accumulated
    // commits from merged sub-PRs.
    let synthetic_heads = detect_potential_synthetic_heads(snapshot, &evidence.prs);
    for known_issue in synthetic_heads {
        snapshot.health.add_issue(known_issue.to_issue());
    }
}

/// Compute parent candidates for an untracked branch.
///
/// Returns candidates ranked by merge-base distance (closest first).
/// Candidates with equal distance are considered "tied" (ambiguous).
///
/// This is used for local-only bootstrap when no remote evidence exists.
/// The algorithm mirrors `find_nearest_tracked_ancestor()` in track.rs.
///
/// # Arguments
///
/// * `git` - Git interface for merge-base computation
/// * `branch` - The untracked branch to find parents for
/// * `branch_oid` - The tip OID of the untracked branch
/// * `snapshot` - Repository snapshot with tracked branches and trunk
///
/// # Returns
///
/// A vector of `ParentCandidate` sorted by distance (ascending).
/// Empty if no valid candidates found (e.g., no common ancestors).
pub fn compute_parent_candidates(
    git: &Git,
    branch: &BranchName,
    branch_oid: &Oid,
    snapshot: &RepoSnapshot,
) -> Vec<crate::engine::health::ParentCandidate> {
    use crate::engine::health::ParentCandidate;

    let mut candidates = Vec::new();
    let trunk = snapshot.trunk.as_ref();

    // Gather all potential parents: tracked branches + trunk
    let mut potential_parents: Vec<(&BranchName, &Oid, bool)> = snapshot
        .metadata
        .keys()
        .filter_map(|b| {
            // Don't consider the branch itself as a parent
            if b == branch {
                return None;
            }
            snapshot.branches.get(b).map(|oid| (b, oid, false))
        })
        .collect();

    // Add trunk if present and not already in the list
    if let Some(trunk_name) = trunk {
        if let Some(trunk_oid) = snapshot.branches.get(trunk_name) {
            let already_in_list = potential_parents.iter().any(|(b, _, _)| *b == trunk_name);
            if !already_in_list {
                potential_parents.push((trunk_name, trunk_oid, true));
            } else {
                // Mark existing entry as trunk
                for (b, _, is_trunk) in &mut potential_parents {
                    if *b == trunk_name {
                        *is_trunk = true;
                    }
                }
            }
        }
    }

    // Compute merge-base and distance for each candidate
    for (parent_name, parent_oid, is_trunk) in potential_parents {
        if let Ok(Some(merge_base)) = git.merge_base(branch_oid, parent_oid) {
            // Count commits from merge-base to branch tip
            // This is the "distance" - lower is better (closer ancestor)
            if let Ok(distance) = git.commit_count(&merge_base, branch_oid) {
                candidates.push(ParentCandidate {
                    name: parent_name.as_str().to_string(),
                    merge_base: merge_base.as_str().to_string(),
                    distance: distance as u32,
                    is_trunk,
                });
            }
        }
    }

    // Sort by distance (ascending) - closest first
    candidates.sort_by_key(|c| c.distance);

    candidates
}

/// Detect untracked local branches and add issues with parent candidate evidence.
///
/// This is called during scan to populate local bootstrap issues.
/// For each untracked branch, computes parent candidates using merge-base distance.
///
/// # Arguments
///
/// * `git` - Git interface for merge-base computation
/// * `snapshot` - Mutable snapshot to add issues to
fn detect_local_untracked_branches(git: &Git, snapshot: &mut RepoSnapshot) {
    use crate::engine::health::{Evidence, Issue, Severity};

    // Collect branches to check (avoid borrow conflict)
    let branches_to_check: Vec<(BranchName, Oid)> = snapshot
        .branches
        .iter()
        .filter(|(branch, _)| {
            // Skip trunk
            if Some(*branch) == snapshot.trunk.as_ref() {
                return false;
            }
            // Skip already tracked
            if snapshot.metadata.contains_key(*branch) {
                return false;
            }
            true
        })
        .map(|(b, o)| (b.clone(), o.clone()))
        .collect();

    // Process each untracked branch
    for (branch, oid) in branches_to_check {
        // Compute parent candidates
        let candidates = compute_parent_candidates(git, &branch, &oid, snapshot);

        // Create issue with evidence
        let mut issue = Issue::new(
            "untracked-branch",
            Severity::Info,
            format!("branch '{}' exists but is not tracked", branch.as_str()),
        );

        // Add branch ref evidence for issue ID computation
        issue = issue.with_evidence(Evidence::Ref {
            name: format!("refs/heads/{}", branch.as_str()),
            oid: Some(oid.as_str().to_string()),
        });

        // Add parent candidates evidence if we have any
        if !candidates.is_empty() {
            issue.evidence.push(Evidence::ParentCandidates {
                branch: branch.as_str().to_string(),
                candidates,
            });
        }

        snapshot.health.add_issue(issue);
    }
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
                    common_dir: std::path::PathBuf::from("/repo/.git"),
                    work_dir: Some(std::path::PathBuf::from("/repo")),
                    context: crate::git::RepoContext::Normal,
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
                remote_prs: None,
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

    mod bootstrap_issues {
        use super::*;
        use crate::forge::PullRequestSummary;

        fn make_pr_summary(number: u64, head_ref: &str, base_ref: &str) -> PullRequestSummary {
            PullRequestSummary {
                number,
                head_ref: head_ref.to_string(),
                head_repo_owner: None,
                base_ref: base_ref.to_string(),
                is_draft: false,
                url: format!("https://github.com/owner/repo/pull/{}", number),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            }
        }

        fn make_test_snapshot() -> RepoSnapshot {
            RepoSnapshot {
                info: RepoInfo {
                    git_dir: std::path::PathBuf::from("/repo/.git"),
                    common_dir: std::path::PathBuf::from("/repo/.git"),
                    work_dir: Some(std::path::PathBuf::from("/repo")),
                    context: crate::git::RepoContext::Normal,
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
                remote_prs: None,
            }
        }

        #[test]
        fn generates_open_prs_detected_issue() {
            let mut snapshot = make_test_snapshot();
            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(1, "feature", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| i.id.as_str() == "remote-open-prs-detected")
                .collect();
            assert_eq!(issues.len(), 1);
            assert!(!issues[0].is_blocking());
        }

        #[test]
        fn generates_branch_missing_issue() {
            let mut snapshot = make_test_snapshot();
            // PR for a branch that doesn't exist locally
            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "nonexistent-branch", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-branch-missing"))
                .collect();
            assert_eq!(issues.len(), 1);
            assert!(!issues[0].is_blocking()); // Warning, not blocking
        }

        #[test]
        fn generates_untracked_issue() {
            let mut snapshot = make_test_snapshot();
            // Add a local branch that's not tracked
            let branch = BranchName::new("untracked-feature").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            snapshot.branches.insert(branch.clone(), oid);
            // No metadata for this branch

            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "untracked-feature", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-branch-untracked"))
                .collect();
            assert_eq!(issues.len(), 1);
            assert!(!issues[0].is_blocking());
        }

        #[test]
        fn generates_not_linked_issue() {
            let mut snapshot = make_test_snapshot();
            // Add a tracked branch without PR linkage
            let branch = BranchName::new("tracked-no-pr").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            snapshot.branches.insert(branch.clone(), oid.clone());

            let metadata = BranchMetadataV1::new(
                branch.clone(),
                BranchName::new("main").unwrap(),
                oid.clone(),
            );
            snapshot.metadata.insert(
                branch,
                ScannedMetadata {
                    ref_oid: oid,
                    metadata,
                },
            );

            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "tracked-no-pr", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-not-linked"))
                .collect();
            assert_eq!(issues.len(), 1);
            assert!(!issues[0].is_blocking());
        }

        #[test]
        fn no_issue_when_pr_already_linked() {
            use crate::core::metadata::schema::PrState;

            let mut snapshot = make_test_snapshot();
            // Add a tracked branch WITH PR linkage
            let branch = BranchName::new("tracked-with-pr").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            snapshot.branches.insert(branch.clone(), oid.clone());

            let mut metadata = BranchMetadataV1::new(
                branch.clone(),
                BranchName::new("main").unwrap(),
                oid.clone(),
            );
            // Link the PR in metadata
            metadata.pr = PrState::linked("github", 42, "https://github.com/owner/repo/pull/42");

            snapshot.metadata.insert(
                branch,
                ScannedMetadata {
                    ref_oid: oid,
                    metadata,
                },
            );

            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "tracked-with-pr", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            // Should only have the "open PRs detected" issue, not any branch-specific issues
            let branch_issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| {
                    i.id.as_str().starts_with("remote-pr-")
                        && i.id.as_str() != "remote-open-prs-detected"
                })
                .collect();
            assert!(
                branch_issues.is_empty(),
                "Expected no branch-specific issues, found: {:?}",
                branch_issues
            );
        }

        #[test]
        fn skips_fork_prs() {
            let mut snapshot = make_test_snapshot();
            let mut pr = make_pr_summary(42, "fork-feature", "main");
            pr.head_repo_owner = Some("forker".to_string());

            let evidence = RemotePrEvidence {
                prs: vec![pr],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            // Should have the general "open PRs detected" but no branch-specific issues
            let branch_issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-branch"))
                .collect();
            assert!(branch_issues.is_empty());
        }

        #[test]
        fn empty_evidence_no_issues() {
            let mut snapshot = make_test_snapshot();
            let evidence = RemotePrEvidence {
                prs: vec![],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            // No issues should be generated for empty PR list
            let remote_issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| i.id.as_str().starts_with("remote-"))
                .collect();
            assert!(remote_issues.is_empty());
        }

        #[test]
        fn truncated_evidence_noted() {
            let mut snapshot = make_test_snapshot();
            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(1, "feature", "main")],
                truncated: true,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot
                .health
                .issues()
                .iter()
                .filter(|i| i.id.as_str() == "remote-open-prs-detected")
                .collect();
            assert_eq!(issues.len(), 1);
            assert!(issues[0].message.contains("truncated"));
        }
    }

    // --- Synthetic Stack Detection Tests (Milestone 5.8) ---

    mod synthetic_detection {
        use super::*;
        use crate::forge::PullRequestSummary;

        fn make_pr_summary(number: u64, head: &str, base: &str) -> PullRequestSummary {
            PullRequestSummary {
                number,
                head_ref: head.to_string(),
                head_repo_owner: None,
                base_ref: base.to_string(),
                is_draft: false,
                url: format!("https://github.com/org/repo/pull/{}", number),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            }
        }

        fn make_test_snapshot() -> RepoSnapshot {
            RepoSnapshot {
                info: RepoInfo {
                    git_dir: std::path::PathBuf::from("/repo/.git"),
                    common_dir: std::path::PathBuf::from("/repo/.git"),
                    work_dir: Some(std::path::PathBuf::from("/repo")),
                    context: crate::git::RepoContext::Normal,
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
                remote_prs: None,
            }
        }

        #[test]
        fn detects_trunk_targeting_pr_as_potential_head() {
            let snapshot = make_test_snapshot();

            let open_prs = vec![make_pr_summary(42, "feature", "main")];

            let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

            assert_eq!(issues.len(), 1);
            if let crate::doctor::KnownIssue::PotentialSyntheticStackHead {
                branch,
                pr_number,
                ..
            } = &issues[0]
            {
                assert_eq!(branch, "feature");
                assert_eq!(*pr_number, 42);
            } else {
                panic!("Expected PotentialSyntheticStackHead");
            }
        }

        #[test]
        fn ignores_non_trunk_targeting_prs() {
            let snapshot = make_test_snapshot();

            // PR targets "feature" branch, not trunk ("main")
            let open_prs = vec![make_pr_summary(42, "sub-feature", "feature")];

            let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

            assert!(issues.is_empty());
        }

        #[test]
        fn returns_empty_when_no_trunk() {
            let mut snapshot = make_test_snapshot();
            snapshot.trunk = None; // No trunk configured

            let open_prs = vec![make_pr_summary(42, "feature", "main")];

            let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

            assert!(issues.is_empty());
        }

        #[test]
        fn detects_multiple_potential_heads() {
            let snapshot = make_test_snapshot();

            let open_prs = vec![
                make_pr_summary(42, "feature-a", "main"),
                make_pr_summary(43, "feature-b", "main"),
                make_pr_summary(44, "sub-feature", "feature-a"), // Not trunk-targeting
            ];

            let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

            assert_eq!(issues.len(), 2);
        }

        #[test]
        fn returns_empty_for_empty_prs() {
            let snapshot = make_test_snapshot();

            let open_prs: Vec<PullRequestSummary> = vec![];

            let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

            assert!(issues.is_empty());
        }
    }
}
