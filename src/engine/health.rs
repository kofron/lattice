//! engine::health
//!
//! Health report and issue tracking for repository scanning.
//!
//! # Architecture
//!
//! The scanner produces a `RepoHealthReport` containing issues found
//! and capabilities established. Issues have stable, deterministic IDs
//! that can be referenced across sessions.
//!
//! Per ARCHITECTURE.md Section 8.2, issues contain:
//! - `IssueId` (stable and deterministic from evidence)
//! - Severity (`Blocking`, `Warning`, `Info`)
//! - Evidence (refs, object ids, parse failures, cycle traces)
//! - Which capabilities the issue blocks
//!
//! # Example
//!
//! ```
//! use latticework::engine::health::{Issue, IssueId, Severity, Evidence, RepoHealthReport};
//! use latticework::engine::capabilities::Capability;
//!
//! let issue = Issue::new(
//!     "metadata-parse-error",
//!     Severity::Blocking,
//!     "Failed to parse metadata for branch 'feature'",
//! )
//! .with_evidence(Evidence::ParseError {
//!     ref_name: "refs/branch-metadata/feature".to_string(),
//!     message: "invalid JSON".to_string(),
//! })
//! .blocks(Capability::MetadataReadable);
//!
//! assert!(issue.is_blocking());
//! assert!(issue.blocks_capability(&Capability::MetadataReadable));
//! ```

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use sha2::{Digest, Sha256};

use super::capabilities::{Capability, CapabilitySet};
use super::scan::DivergenceInfo;

/// Severity of an issue.
///
/// Severity determines whether the issue blocks command execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Issue blocks command execution.
    ///
    /// Commands cannot proceed until blocking issues are resolved,
    /// typically through the Doctor.
    Blocking,

    /// Issue is a warning but doesn't block execution.
    ///
    /// The user is informed but the operation can proceed.
    Warning,

    /// Informational issue.
    ///
    /// For observability only; doesn't affect execution.
    Info,
}

impl Severity {
    /// Check if this severity blocks command execution.
    pub fn is_blocking(&self) -> bool {
        matches!(self, Severity::Blocking)
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Blocking => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Info => write!(f, "info"),
        }
    }
}

/// A stable, deterministic issue identifier.
///
/// Issue IDs are computed from the issue type and key evidence,
/// making them stable across scanner runs for the same underlying
/// problem. This enables referencing issues in fix commands.
///
/// # Example
///
/// ```
/// use latticework::engine::health::IssueId;
///
/// let id = IssueId::new("metadata-parse-error", "feature");
/// assert!(id.as_str().starts_with("metadata-parse-error:"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IssueId(String);

impl IssueId {
    /// Create an issue ID from a type and key.
    ///
    /// The ID is formatted as `type:hash(key)` where hash is
    /// a truncated SHA-256 of the key for stability.
    pub fn new(issue_type: &str, key: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let hash = hasher.finalize();
        let short_hash = hex::encode(&hash[..4]); // 8 hex chars
        Self(format!("{}:{}", issue_type, short_hash))
    }

    /// Create an issue ID from just a type (no key).
    ///
    /// Use this for singleton issues like "trunk-not-configured".
    pub fn singleton(issue_type: &str) -> Self {
        Self(issue_type.to_string())
    }

    /// Get the string representation of the ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for IssueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A potential parent branch for an untracked branch.
///
/// Used by local-only bootstrap to rank parent candidates by
/// merge-base distance. The candidate with the smallest distance
/// (closest to the untracked branch) is the preferred parent.
#[derive(Debug, Clone, PartialEq)]
pub struct ParentCandidate {
    /// Branch name.
    pub name: String,
    /// Merge-base OID between the untracked branch and this candidate.
    pub merge_base: String,
    /// Number of commits from merge-base to the untracked branch tip.
    /// Lower distance = closer relationship = better parent candidate.
    pub distance: u32,
    /// Whether this is the configured trunk branch.
    pub is_trunk: bool,
}

/// Information about a closed PR that targeted a synthetic stack head.
///
/// Used in Tier 2 deep analysis (Milestone 5.8) to provide details
/// about closed PRs that were merged into a potential synthetic head branch.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedPrInfo {
    /// PR number.
    pub number: u64,
    /// Head branch of the closed PR.
    pub head_ref: String,
    /// Whether the PR was merged (true) or just closed (false).
    pub merged: bool,
    /// PR URL.
    pub url: String,
}

/// Evidence supporting an issue.
///
/// Evidence provides concrete details about what was found during
/// scanning. It's used for diagnostics and for computing stable
/// issue IDs.
#[derive(Debug, Clone)]
pub enum Evidence {
    /// A ref that's involved in the issue.
    Ref {
        /// Full ref name
        name: String,
        /// OID if available
        oid: Option<String>,
    },

    /// A parse error encountered.
    ParseError {
        /// Ref that failed to parse
        ref_name: String,
        /// Error message
        message: String,
    },

    /// A cycle detected in the stack graph.
    Cycle {
        /// Branches involved in the cycle
        branches: Vec<String>,
    },

    /// A missing branch.
    MissingBranch {
        /// Name of the missing branch
        name: String,
    },

    /// Git state information.
    GitState {
        /// Description of the state
        state: String,
    },

    /// A config issue.
    Config {
        /// Config key
        key: String,
        /// Problem description
        problem: String,
    },

    /// Base ancestry violation.
    BaseAncestry {
        /// Branch with the violation
        branch: String,
        /// Base OID
        base_oid: String,
        /// Tip OID
        tip_oid: String,
    },

    /// Frozen branch violation.
    FrozenViolation {
        /// Frozen branch that would be modified
        branch: String,
    },

    /// Parent candidates for an untracked branch (local-only bootstrap).
    ///
    /// Used when detecting untracked branches to provide evidence
    /// about potential parent branches ranked by merge-base distance.
    ParentCandidates {
        /// The untracked branch name.
        branch: String,
        /// Ranked list of parent candidates (closest first).
        candidates: Vec<ParentCandidate>,
    },

    /// PR reference for context (Milestone 5.8: Synthetic Stack Detection).
    ///
    /// Used to provide PR information as evidence for issues.
    PrReference {
        /// PR number.
        number: u64,
        /// PR URL.
        url: String,
        /// Context description for the reference.
        context: String,
    },

    /// Closed PRs that targeted a synthetic stack head (Tier 2 deep analysis).
    ///
    /// Used when `--deep-remote` is enabled to enumerate closed PRs
    /// that were merged into a potential synthetic stack head.
    SyntheticStackChildren {
        /// The synthetic head branch.
        head_branch: String,
        /// Closed PRs that targeted this head.
        closed_prs: Vec<ClosedPrInfo>,
        /// Whether the result was truncated due to budget limits.
        truncated: bool,
    },
}

impl Evidence {
    /// Get a key string for use in issue ID computation.
    ///
    /// This extracts the primary identifier from the evidence.
    pub fn key(&self) -> String {
        match self {
            Evidence::Ref { name, .. } => name.clone(),
            Evidence::ParseError { ref_name, .. } => ref_name.clone(),
            Evidence::Cycle { branches } => branches.join(","),
            Evidence::MissingBranch { name } => name.clone(),
            Evidence::GitState { state } => state.clone(),
            Evidence::Config { key, .. } => key.clone(),
            Evidence::BaseAncestry { branch, .. } => branch.clone(),
            Evidence::FrozenViolation { branch } => branch.clone(),
            Evidence::ParentCandidates { branch, .. } => branch.clone(),
            Evidence::PrReference { number, .. } => number.to_string(),
            Evidence::SyntheticStackChildren { head_branch, .. } => head_branch.clone(),
        }
    }
}

/// An issue found during repository scanning.
///
/// Issues represent problems or observations about the repository
/// state. They may block command execution (if blocking) and may
/// require Doctor intervention to resolve.
#[derive(Debug, Clone)]
pub struct Issue {
    /// Stable identifier for this issue.
    pub id: IssueId,
    /// Severity level.
    pub severity: Severity,
    /// Human-readable message.
    pub message: String,
    /// Evidence supporting the issue.
    pub evidence: Vec<Evidence>,
    /// Capabilities this issue blocks.
    blocked_capabilities: HashSet<Capability>,
}

impl Issue {
    /// Create a new issue.
    ///
    /// The issue ID is computed from the issue type and first evidence
    /// key (if any). For more control, use `with_id()`.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::health::{Issue, Severity};
    ///
    /// let issue = Issue::new(
    ///     "trunk-not-configured",
    ///     Severity::Blocking,
    ///     "No trunk branch configured",
    /// );
    /// assert_eq!(issue.id.as_str(), "trunk-not-configured");
    /// ```
    pub fn new(issue_type: &str, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            id: IssueId::singleton(issue_type),
            severity,
            message: message.into(),
            evidence: vec![],
            blocked_capabilities: HashSet::new(),
        }
    }

    /// Create an issue with a specific ID.
    pub fn with_id(id: IssueId, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            id,
            severity,
            message: message.into(),
            evidence: vec![],
            blocked_capabilities: HashSet::new(),
        }
    }

    /// Add evidence to the issue.
    ///
    /// If this is the first evidence and the ID is a singleton,
    /// the ID is updated to include a hash of the evidence key.
    pub fn with_evidence(mut self, evidence: Evidence) -> Self {
        // Update ID if this is first evidence and ID is singleton
        if self.evidence.is_empty() && !self.id.0.contains(':') {
            let key = evidence.key();
            if !key.is_empty() {
                self.id = IssueId::new(&self.id.0, &key);
            }
        }
        self.evidence.push(evidence);
        self
    }

    /// Mark this issue as blocking a capability.
    pub fn blocks(mut self, capability: Capability) -> Self {
        self.blocked_capabilities.insert(capability);
        self
    }

    /// Mark this issue as blocking multiple capabilities.
    pub fn blocks_all(mut self, capabilities: impl IntoIterator<Item = Capability>) -> Self {
        self.blocked_capabilities.extend(capabilities);
        self
    }

    /// Check if this is a blocking issue.
    pub fn is_blocking(&self) -> bool {
        self.severity.is_blocking()
    }

    /// Check if this issue blocks a specific capability.
    pub fn blocks_capability(&self, cap: &Capability) -> bool {
        self.blocked_capabilities.contains(cap)
    }

    /// Get all capabilities blocked by this issue.
    pub fn blocked_capabilities(&self) -> impl Iterator<Item = &Capability> {
        self.blocked_capabilities.iter()
    }
}

impl PartialEq for Issue {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Issue {}

impl Hash for Issue {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Repository health report from scanning.
///
/// Contains all issues found, the capabilities that were successfully
/// established, and divergence information if out-of-band changes were
/// detected. Used by gating to determine if a command can proceed.
///
/// Per ARCHITECTURE.md Section 7.2, divergence is not an error but
/// evidence for audit and gating decisions.
///
/// # Example
///
/// ```
/// use latticework::engine::health::{RepoHealthReport, Issue, Severity};
/// use latticework::engine::capabilities::Capability;
///
/// let mut report = RepoHealthReport::new();
/// report.add_capability(Capability::RepoOpen);
/// report.add_issue(Issue::new(
///     "warning-example",
///     Severity::Warning,
///     "This is a warning",
/// ));
///
/// assert!(report.capabilities().has(&Capability::RepoOpen));
/// assert!(!report.has_blocking_issues());
/// assert!(!report.has_divergence()); // No divergence by default
/// ```
#[derive(Debug, Clone, Default)]
pub struct RepoHealthReport {
    issues: Vec<Issue>,
    capabilities: CapabilitySet,
    /// Divergence information if out-of-band changes were detected.
    divergence: Option<DivergenceInfo>,
}

impl RepoHealthReport {
    /// Create an empty health report.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a capability to the report.
    pub fn add_capability(&mut self, cap: Capability) {
        self.capabilities.insert(cap);
    }

    /// Add multiple capabilities to the report.
    pub fn add_capabilities(&mut self, caps: impl IntoIterator<Item = Capability>) {
        for cap in caps {
            self.capabilities.insert(cap);
        }
    }

    /// Remove a capability (e.g., when an issue blocks it).
    pub fn remove_capability(&mut self, cap: &Capability) {
        self.capabilities.remove(cap);
    }

    /// Add an issue to the report.
    ///
    /// If the issue blocks any capabilities, those capabilities
    /// are removed from the capability set.
    pub fn add_issue(&mut self, issue: Issue) {
        // Remove blocked capabilities
        for cap in issue.blocked_capabilities() {
            self.capabilities.remove(cap);
        }
        self.issues.push(issue);
    }

    /// Get the capability set.
    pub fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    /// Get all issues.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// Get only blocking issues.
    pub fn blocking_issues(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.is_blocking())
    }

    /// Check if there are any blocking issues.
    pub fn has_blocking_issues(&self) -> bool {
        self.issues.iter().any(|i| i.is_blocking())
    }

    /// Get issues by severity.
    pub fn issues_by_severity(&self, severity: Severity) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.severity == severity)
    }

    /// Find an issue by ID.
    pub fn find_issue(&self, id: &IssueId) -> Option<&Issue> {
        self.issues.iter().find(|i| &i.id == id)
    }

    /// Get the number of issues.
    pub fn issue_count(&self) -> usize {
        self.issues.len()
    }

    /// Check if the report is clean (no issues at all).
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    /// Set divergence information.
    ///
    /// Called by the scanner when out-of-band changes are detected.
    pub fn set_divergence(&mut self, divergence: DivergenceInfo) {
        self.divergence = Some(divergence);
    }

    /// Get divergence information if out-of-band changes were detected.
    ///
    /// Per ARCHITECTURE.md Section 7.2, divergence is evidence that the
    /// repository was modified outside Lattice.
    pub fn divergence(&self) -> Option<&DivergenceInfo> {
        self.divergence.as_ref()
    }

    /// Check if divergence from last committed state was detected.
    pub fn has_divergence(&self) -> bool {
        self.divergence.is_some()
    }
}

/// Common issue types used throughout the codebase.
pub mod issues {
    use super::*;

    /// Create an issue for missing trunk configuration.
    pub fn trunk_not_configured() -> Issue {
        Issue::new(
            "trunk-not-configured",
            Severity::Blocking,
            "No trunk branch configured. Run 'lattice init' to configure.",
        )
        .blocks(Capability::TrunkKnown)
    }

    /// Create an issue for metadata parse failure.
    pub fn metadata_parse_error(branch: &str, error: &str) -> Issue {
        Issue::new(
            "metadata-parse-error",
            Severity::Blocking,
            format!(
                "Failed to parse metadata for branch '{}': {}",
                branch, error
            ),
        )
        .with_evidence(Evidence::ParseError {
            ref_name: format!("refs/branch-metadata/{}", branch),
            message: error.to_string(),
        })
        .blocks(Capability::MetadataReadable)
    }

    /// Create an issue for a cycle in the stack graph.
    pub fn graph_cycle(branches: Vec<String>) -> Issue {
        Issue::new(
            "graph-cycle",
            Severity::Blocking,
            format!("Cycle detected in stack graph: {:?}", branches),
        )
        .with_evidence(Evidence::Cycle {
            branches: branches.clone(),
        })
        .blocks(Capability::GraphValid)
    }

    /// Create an issue for a missing tracked branch.
    pub fn missing_branch(name: &str) -> Issue {
        Issue::new(
            "missing-branch",
            Severity::Blocking,
            format!("Tracked branch '{}' does not exist", name),
        )
        .with_evidence(Evidence::MissingBranch {
            name: name.to_string(),
        })
        .blocks(Capability::GraphValid)
    }

    /// Create an issue for Git operation in progress.
    pub fn git_operation_in_progress(state: &str) -> Issue {
        Issue::new(
            "git-op-in-progress",
            Severity::Blocking,
            format!("Git {} in progress. Complete or abort it first.", state),
        )
        .with_evidence(Evidence::GitState {
            state: state.to_string(),
        })
        .blocks(Capability::NoExternalGitOpInProgress)
    }

    /// Create an issue for Lattice operation in progress.
    pub fn lattice_operation_in_progress(command: &str, op_id: &str) -> Issue {
        Issue::new(
            "lattice-op-in-progress",
            Severity::Blocking,
            format!(
                "Lattice '{}' operation in progress ({}). Run 'lattice continue' or 'lattice abort'.",
                command, op_id
            ),
        )
        .blocks(Capability::NoLatticeOpInProgress)
    }

    /// Create an issue for frozen branch violation.
    pub fn frozen_branch_violation(branch: &str) -> Issue {
        Issue::new(
            "frozen-branch-violation",
            Severity::Blocking,
            format!("Cannot modify frozen branch '{}'", branch),
        )
        .with_evidence(Evidence::FrozenViolation {
            branch: branch.to_string(),
        })
        .blocks(Capability::FrozenPolicySatisfied)
    }

    /// Create an issue for base not being ancestor of tip.
    pub fn base_not_ancestor(branch: &str, base_oid: &str, tip_oid: &str) -> Issue {
        Issue::new(
            "base-not-ancestor",
            Severity::Blocking,
            format!(
                "Base commit {} is not an ancestor of tip {} for branch '{}'",
                &base_oid[..8.min(base_oid.len())],
                &tip_oid[..8.min(tip_oid.len())],
                branch
            ),
        )
        .with_evidence(Evidence::BaseAncestry {
            branch: branch.to_string(),
            base_oid: base_oid.to_string(),
            tip_oid: tip_oid.to_string(),
        })
        .blocks(Capability::GraphValid)
    }

    /// Create an issue for orphaned metadata (metadata ref exists but branch doesn't).
    pub fn orphaned_metadata(branch: &str) -> Issue {
        Issue::new(
            "orphaned-metadata",
            Severity::Warning,
            format!(
                "Metadata exists for branch '{}' but the branch does not exist",
                branch
            ),
        )
        .with_evidence(Evidence::Ref {
            name: format!("refs/branch-metadata/{}", branch),
            oid: None,
        })
    }

    /// Create an issue for a missing parent branch.
    pub fn parent_missing(child: &str, parent: &str) -> Issue {
        Issue::new(
            "parent-missing",
            Severity::Blocking,
            format!(
                "Parent branch '{}' referenced by '{}' does not exist",
                parent, child
            ),
        )
        .with_evidence(Evidence::MissingBranch {
            name: parent.to_string(),
        })
        .blocks(Capability::GraphValid)
    }

    /// Create an issue for config file needing migration.
    pub fn config_migration_needed(old_path: &str, new_path: &str) -> Issue {
        Issue::new(
            "config-migration",
            Severity::Warning,
            format!(
                "Config file at '{}' should be migrated to '{}'",
                old_path, new_path
            ),
        )
        .with_evidence(Evidence::Config {
            key: old_path.to_string(),
            problem: format!("legacy location, should be at {}", new_path),
        })
    }

    /// Create an issue for no remote configured.
    pub fn no_remote_configured() -> Issue {
        Issue::new(
            "no-remote-configured",
            Severity::Warning,
            "No 'origin' remote configured. Remote commands will not be available.",
        )
        .with_evidence(Evidence::Config {
            key: "remote.origin.url".to_string(),
            problem: "not configured".to_string(),
        })
    }

    /// Create an issue for remote not being a GitHub URL.
    pub fn remote_not_github(url: &str) -> Issue {
        Issue::new(
            "remote-not-github",
            Severity::Warning,
            format!(
                "Remote 'origin' ({}) is not a GitHub URL. GitHub commands will not be available.",
                url
            ),
        )
        .with_evidence(Evidence::Config {
            key: "remote.origin.url".to_string(),
            problem: format!("not a GitHub URL: {}", url),
        })
    }

    /// Create an issue for GitHub App not installed or not authorized.
    ///
    /// Per ARCHITECTURE.md Section 8.2, this is a blocking issue requiring
    /// user action - the user must install the GitHub App for the repository.
    pub fn app_not_installed(host: &str, owner: &str, repo: &str) -> Issue {
        Issue::new(
            "app-not-installed",
            Severity::Blocking,
            format!(
                "GitHub App not installed for {}/{}. Install at: https://github.com/apps/lattice/installations/new",
                owner, repo
            ),
        )
        .with_evidence(Evidence::Config {
            key: format!("forge.github.{}/{}/{}", host, owner, repo),
            problem: "GitHub App not installed or not authorized".to_string(),
        })
        .blocks(Capability::RepoAuthorized)
    }

    /// Create an issue for failed repository authorization check.
    ///
    /// This is a warning, not blocking - the user can retry or the check
    /// may have failed due to transient network issues.
    pub fn repo_authorization_check_failed(owner: &str, repo: &str, error: &str) -> Issue {
        Issue::new(
            "repo-auth-check-failed",
            Severity::Warning,
            format!(
                "Could not verify repository authorization for {}/{}: {}",
                owner, repo, error
            ),
        )
        .with_evidence(Evidence::Config {
            key: format!("github.authorization-check.{}/{}", owner, repo),
            problem: error.to_string(),
        })
    }

    /// Create an issue for no working directory available (bare repository).
    ///
    /// Per SPEC.md §4.6.6, bare repositories lack a working directory,
    /// which blocks commands that require checkout, staging, or working
    /// tree operations.
    ///
    /// Per SPEC.md §4.6.9, the message must be high-signal and include:
    /// - Why it failed (bare repo has no working directory)
    /// - How to proceed (create a worktree or use appropriate flags)
    pub fn no_working_directory() -> Issue {
        Issue::new(
            "no-working-directory",
            Severity::Blocking,
            "This command requires a working directory, but this is a bare repository.\n\
             \n\
             To proceed, either:\n\
             • Create a worktree: git worktree add <path> <branch>\n\
             • Run from an existing worktree linked to this repository\n\
             • Use --no-checkout or --no-restack flags if available for this command",
        )
        .blocks(Capability::WorkingDirectoryAvailable)
    }

    /// Create an issue for branches checked out in other worktrees.
    ///
    /// Per SPEC.md §4.6.8, operations that would rewrite a branch checked out
    /// in another worktree must be refused with a clear error message.
    ///
    /// # Arguments
    ///
    /// * `conflicts` - List of (branch name, worktree path) pairs for branches
    ///   that are checked out elsewhere
    pub fn branches_checked_out_elsewhere(
        conflicts: Vec<(crate::core::types::BranchName, std::path::PathBuf)>,
    ) -> Issue {
        let branch_list: Vec<String> = conflicts
            .iter()
            .map(|(b, p)| format!("'{}' (in {})", b, p.display()))
            .collect();

        let message = if conflicts.len() == 1 {
            format!(
                "Branch {} is checked out in another worktree",
                branch_list[0]
            )
        } else {
            format!(
                "Branches checked out in other worktrees: {}",
                branch_list.join(", ")
            )
        };

        let mut issue = Issue::new("branch-checked-out-elsewhere", Severity::Blocking, message);

        // Add evidence for each conflict
        for (branch, path) in &conflicts {
            issue = issue.with_evidence(Evidence::Ref {
                name: format!("refs/heads/{}", branch),
                oid: None,
            });
            // We could add a new Evidence variant for worktree path, but Ref works
            issue = issue.with_evidence(Evidence::Config {
                key: format!("worktree:{}", branch),
                problem: format!("checked out at {}", path.display()),
            });
        }

        issue
    }

    // --- Bootstrap Issues (Remote Evidence) ---

    /// Create an issue for detecting open PRs on the remote.
    ///
    /// This is an informational issue indicating bootstrap opportunity.
    pub fn remote_open_prs_detected(count: usize, truncated: bool) -> Issue {
        let truncation_note = if truncated {
            " (results truncated, more may exist)"
        } else {
            ""
        };

        Issue::new(
            "remote-open-prs-detected",
            Severity::Info,
            format!(
                "Remote has {} open pull request(s){}. Run `lattice doctor --fix` to import.",
                count, truncation_note
            ),
        )
    }

    /// Create an issue for an open PR whose head branch doesn't exist locally.
    ///
    /// The user should fetch the branch to import the PR into Lattice tracking.
    pub fn remote_pr_branch_missing(
        number: u64,
        head_ref: &str,
        base_ref: &str,
        url: &str,
    ) -> Issue {
        Issue::new(
            "remote-pr-branch-missing",
            Severity::Warning,
            format!(
                "Open PR #{} targets '{}' from '{}' but branch '{}' doesn't exist locally",
                number, base_ref, head_ref, head_ref
            ),
        )
        .with_evidence(Evidence::Ref {
            name: format!("refs/heads/{}", head_ref),
            oid: None,
        })
        .with_evidence(Evidence::Config {
            key: format!("pr.{}", number),
            problem: format!("branch missing, base:{} PR URL: {}", base_ref, url),
        })
    }

    /// Create an issue for a local branch matching an open PR but not tracked.
    ///
    /// The user should track the branch to link it with the PR.
    pub fn remote_pr_branch_untracked(
        branch: &str,
        number: u64,
        base_ref: &str,
        url: &str,
    ) -> Issue {
        Issue::new(
            "remote-pr-branch-untracked",
            Severity::Warning,
            format!(
                "Branch '{}' matches open PR #{} but is not tracked by Lattice",
                branch, number
            ),
        )
        .with_evidence(Evidence::Ref {
            name: format!("refs/heads/{}", branch),
            oid: None,
        })
        .with_evidence(Evidence::Config {
            key: format!("pr.{}", number),
            problem: format!("untracked, base:{} PR URL: {}", base_ref, url),
        })
    }

    /// Create an issue for a tracked branch with an open PR but no linkage.
    ///
    /// The user should link the PR to the tracked branch in metadata.
    pub fn remote_pr_not_linked(branch: &str, number: u64, url: &str) -> Issue {
        Issue::new(
            "remote-pr-not-linked",
            Severity::Info,
            format!(
                "Tracked branch '{}' has open PR #{} but PR is not linked in metadata",
                branch, number
            ),
        )
        .with_evidence(Evidence::Ref {
            name: format!("refs/branch-metadata/{}", branch),
            oid: None,
        })
        .with_evidence(Evidence::Config {
            key: format!("pr.{}", number),
            problem: format!("not linked, PR URL: {}", url),
        })
    }

    // --- Synthetic Stack Detection (Milestone 5.8) ---

    /// Create an issue for a potential synthetic stack head.
    ///
    /// This is an informational issue indicating that a PR targeting trunk
    /// may have accumulated commits from merged sub-PRs. The branch could
    /// be a "synthetic stack head" where prior work was merged in.
    ///
    /// # Arguments
    ///
    /// * `branch` - The branch that may be a synthetic stack head
    /// * `pr_number` - The PR number targeting trunk
    /// * `pr_url` - URL of the PR
    pub fn potential_synthetic_stack_head(branch: &str, pr_number: u64, pr_url: &str) -> Issue {
        Issue::new(
            "synthetic-stack-head",
            Severity::Info,
            format!(
                "PR #{} targeting trunk may be a synthetic stack head (branch '{}')",
                pr_number, branch
            ),
        )
        .with_evidence(Evidence::PrReference {
            number: pr_number,
            url: pr_url.to_string(),
            context: format!("Open PR targeting trunk with head branch '{}'", branch),
        })
    }

    // --- Rollback Issues ---

    /// Create an issue for partial rollback failure.
    ///
    /// Per SPEC.md Section 4.2.2, when an abort cannot fully restore refs,
    /// this issue surfaces the problem to the doctor for guided resolution.
    ///
    /// # Arguments
    ///
    /// * `op_id` - The operation ID that was being aborted
    /// * `command` - The command that was being aborted
    /// * `failed_refs` - List of (refname, error_message) pairs for refs that
    ///   could not be rolled back
    pub fn partial_rollback_failure(
        op_id: &str,
        command: &str,
        failed_refs: Vec<(String, String)>,
    ) -> Issue {
        let failed_list: Vec<String> = failed_refs
            .iter()
            .map(|(refname, err)| format!("  - {}: {}", refname, err))
            .collect();

        Issue::new(
            "partial-rollback-failure",
            Severity::Blocking,
            format!(
                "Operation '{}' (id: {}) was partially rolled back.\n\
                 The following refs could not be restored:\n{}\n\n\
                 The repository may be in an inconsistent state.",
                command,
                &op_id[..8.min(op_id.len())],
                failed_list.join("\n")
            ),
        )
        .with_evidence(Evidence::Config {
            key: format!("op.{}", op_id),
            problem: format!("{} refs failed to roll back", failed_refs.len()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod severity {
        use super::*;

        #[test]
        fn blocking_is_blocking() {
            assert!(Severity::Blocking.is_blocking());
        }

        #[test]
        fn warning_not_blocking() {
            assert!(!Severity::Warning.is_blocking());
        }

        #[test]
        fn info_not_blocking() {
            assert!(!Severity::Info.is_blocking());
        }

        #[test]
        fn display_formatting() {
            assert_eq!(format!("{}", Severity::Blocking), "error");
            assert_eq!(format!("{}", Severity::Warning), "warning");
            assert_eq!(format!("{}", Severity::Info), "info");
        }
    }

    mod issue_id {
        use super::*;

        #[test]
        fn new_includes_hash() {
            let id = IssueId::new("test-type", "some-key");
            assert!(id.as_str().starts_with("test-type:"));
            assert!(id.as_str().len() > "test-type:".len());
        }

        #[test]
        fn deterministic_for_same_input() {
            let id1 = IssueId::new("test", "key");
            let id2 = IssueId::new("test", "key");
            assert_eq!(id1, id2);
        }

        #[test]
        fn different_for_different_keys() {
            let id1 = IssueId::new("test", "key1");
            let id2 = IssueId::new("test", "key2");
            assert_ne!(id1, id2);
        }

        #[test]
        fn singleton_has_no_hash() {
            let id = IssueId::singleton("simple-issue");
            assert_eq!(id.as_str(), "simple-issue");
        }

        #[test]
        fn display() {
            let id = IssueId::singleton("test");
            assert_eq!(format!("{}", id), "test");
        }
    }

    mod evidence {
        use super::*;

        #[test]
        fn ref_key() {
            let e = Evidence::Ref {
                name: "refs/heads/main".to_string(),
                oid: Some("abc123".to_string()),
            };
            assert_eq!(e.key(), "refs/heads/main");
        }

        #[test]
        fn parse_error_key() {
            let e = Evidence::ParseError {
                ref_name: "refs/branch-metadata/feature".to_string(),
                message: "invalid".to_string(),
            };
            assert_eq!(e.key(), "refs/branch-metadata/feature");
        }

        #[test]
        fn cycle_key() {
            let e = Evidence::Cycle {
                branches: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            };
            assert_eq!(e.key(), "a,b,c");
        }

        #[test]
        fn missing_branch_key() {
            let e = Evidence::MissingBranch {
                name: "feature".to_string(),
            };
            assert_eq!(e.key(), "feature");
        }

        #[test]
        fn parent_candidates_key() {
            let e = Evidence::ParentCandidates {
                branch: "feature".to_string(),
                candidates: vec![ParentCandidate {
                    name: "main".to_string(),
                    merge_base: "abc123".to_string(),
                    distance: 3,
                    is_trunk: true,
                }],
            };
            assert_eq!(e.key(), "feature");
        }
    }

    mod issue {
        use super::*;

        #[test]
        fn new_creates_singleton_id() {
            let issue = Issue::new("test-issue", Severity::Warning, "Test message");
            assert_eq!(issue.id.as_str(), "test-issue");
        }

        #[test]
        fn with_evidence_updates_id() {
            let issue = Issue::new("test-issue", Severity::Warning, "Test").with_evidence(
                Evidence::MissingBranch {
                    name: "feature".to_string(),
                },
            );
            assert!(issue.id.as_str().starts_with("test-issue:"));
        }

        #[test]
        fn with_evidence_preserves_hashed_id() {
            let issue = Issue::new("test-issue", Severity::Warning, "Test")
                .with_evidence(Evidence::MissingBranch {
                    name: "first".to_string(),
                })
                .with_evidence(Evidence::MissingBranch {
                    name: "second".to_string(),
                });
            // ID should be based on first evidence
            assert!(issue.evidence.len() == 2);
        }

        #[test]
        fn blocks_capability() {
            let issue =
                Issue::new("test", Severity::Blocking, "Test").blocks(Capability::MetadataReadable);
            assert!(issue.blocks_capability(&Capability::MetadataReadable));
            assert!(!issue.blocks_capability(&Capability::RepoOpen));
        }

        #[test]
        fn blocks_all_capabilities() {
            let issue = Issue::new("test", Severity::Blocking, "Test")
                .blocks_all([Capability::MetadataReadable, Capability::GraphValid]);
            assert!(issue.blocks_capability(&Capability::MetadataReadable));
            assert!(issue.blocks_capability(&Capability::GraphValid));
        }

        #[test]
        fn is_blocking() {
            assert!(Issue::new("t", Severity::Blocking, "").is_blocking());
            assert!(!Issue::new("t", Severity::Warning, "").is_blocking());
            assert!(!Issue::new("t", Severity::Info, "").is_blocking());
        }

        #[test]
        fn equality_based_on_id() {
            let issue1 = Issue::new("same", Severity::Blocking, "Message 1");
            let issue2 = Issue::new("same", Severity::Warning, "Message 2");
            assert_eq!(issue1, issue2);
        }

        #[test]
        fn hash_based_on_id() {
            let mut set = HashSet::new();
            set.insert(Issue::new("test", Severity::Blocking, "One"));
            set.insert(Issue::new("test", Severity::Warning, "Two"));
            assert_eq!(set.len(), 1);
        }
    }

    mod repo_health_report {
        use super::*;

        #[test]
        fn new_is_empty() {
            let report = RepoHealthReport::new();
            assert!(report.is_clean());
            assert!(!report.has_blocking_issues());
            assert_eq!(report.issue_count(), 0);
        }

        #[test]
        fn add_capability() {
            let mut report = RepoHealthReport::new();
            report.add_capability(Capability::RepoOpen);
            assert!(report.capabilities().has(&Capability::RepoOpen));
        }

        #[test]
        fn add_capabilities() {
            let mut report = RepoHealthReport::new();
            report.add_capabilities([Capability::RepoOpen, Capability::TrunkKnown]);
            assert!(report.capabilities().has(&Capability::RepoOpen));
            assert!(report.capabilities().has(&Capability::TrunkKnown));
        }

        #[test]
        fn add_issue_removes_blocked_capabilities() {
            let mut report = RepoHealthReport::new();
            report.add_capability(Capability::MetadataReadable);

            let issue =
                Issue::new("test", Severity::Blocking, "Test").blocks(Capability::MetadataReadable);
            report.add_issue(issue);

            assert!(!report.capabilities().has(&Capability::MetadataReadable));
        }

        #[test]
        fn has_blocking_issues() {
            let mut report = RepoHealthReport::new();
            assert!(!report.has_blocking_issues());

            report.add_issue(Issue::new("warn", Severity::Warning, "Warning"));
            assert!(!report.has_blocking_issues());

            report.add_issue(Issue::new("block", Severity::Blocking, "Blocking"));
            assert!(report.has_blocking_issues());
        }

        #[test]
        fn blocking_issues_filter() {
            let mut report = RepoHealthReport::new();
            report.add_issue(Issue::new("warn", Severity::Warning, "Warning"));
            report.add_issue(Issue::new("block", Severity::Blocking, "Blocking"));
            report.add_issue(Issue::new("info", Severity::Info, "Info"));

            let blocking: Vec<_> = report.blocking_issues().collect();
            assert_eq!(blocking.len(), 1);
            assert_eq!(blocking[0].id.as_str(), "block");
        }

        #[test]
        fn issues_by_severity() {
            let mut report = RepoHealthReport::new();
            report.add_issue(Issue::new("warn1", Severity::Warning, ""));
            report.add_issue(Issue::new("warn2", Severity::Warning, ""));
            report.add_issue(Issue::new("block", Severity::Blocking, ""));

            let warnings: Vec<_> = report.issues_by_severity(Severity::Warning).collect();
            assert_eq!(warnings.len(), 2);
        }

        #[test]
        fn find_issue() {
            let mut report = RepoHealthReport::new();
            let issue = Issue::new("find-me", Severity::Info, "Found it");
            report.add_issue(issue);

            let id = IssueId::singleton("find-me");
            assert!(report.find_issue(&id).is_some());

            let missing = IssueId::singleton("not-found");
            assert!(report.find_issue(&missing).is_none());
        }

        #[test]
        fn is_clean() {
            let mut report = RepoHealthReport::new();
            assert!(report.is_clean());

            report.add_issue(Issue::new("test", Severity::Info, ""));
            assert!(!report.is_clean());
        }
    }

    mod common_issues {
        use super::*;

        #[test]
        fn trunk_not_configured() {
            let issue = issues::trunk_not_configured();
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::TrunkKnown));
        }

        #[test]
        fn metadata_parse_error() {
            let issue = issues::metadata_parse_error("feature", "invalid json");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::MetadataReadable));
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn graph_cycle() {
            let issue = issues::graph_cycle(vec!["a".to_string(), "b".to_string()]);
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::GraphValid));
        }

        #[test]
        fn missing_branch() {
            let issue = issues::missing_branch("feature");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::GraphValid));
        }

        #[test]
        fn git_operation_in_progress() {
            let issue = issues::git_operation_in_progress("rebase");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::NoExternalGitOpInProgress));
        }

        #[test]
        fn lattice_operation_in_progress() {
            let issue = issues::lattice_operation_in_progress("restack", "abc-123");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::NoLatticeOpInProgress));
        }

        #[test]
        fn frozen_branch_violation() {
            let issue = issues::frozen_branch_violation("feature");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::FrozenPolicySatisfied));
        }

        #[test]
        fn base_not_ancestor() {
            let issue = issues::base_not_ancestor("feature", "abc123def", "fed321cba");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::GraphValid));
        }

        #[test]
        fn orphaned_metadata() {
            let issue = issues::orphaned_metadata("old-feature");
            assert!(!issue.is_blocking()); // Warning severity
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn parent_missing() {
            let issue = issues::parent_missing("child-branch", "parent-branch");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::GraphValid));
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn config_migration_needed() {
            let issue = issues::config_migration_needed(
                ".git/lattice/repo.toml",
                ".git/lattice/config.toml",
            );
            assert!(!issue.is_blocking()); // Warning severity
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn no_remote_configured() {
            let issue = issues::no_remote_configured();
            assert!(!issue.is_blocking()); // Warning severity
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn remote_not_github() {
            let issue = issues::remote_not_github("git@gitlab.com:user/repo.git");
            assert!(!issue.is_blocking()); // Warning severity
            assert!(issue.message.contains("gitlab.com"));
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn app_not_installed() {
            let issue = issues::app_not_installed("github.com", "myorg", "myrepo");
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::RepoAuthorized));
            assert!(issue.message.contains("myorg/myrepo"));
            assert!(issue
                .message
                .contains("https://github.com/apps/lattice/installations/new"));
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn repo_authorization_check_failed() {
            let issue = issues::repo_authorization_check_failed("owner", "repo", "network timeout");
            assert!(!issue.is_blocking()); // Warning severity
            assert!(issue.message.contains("owner/repo"));
            assert!(issue.message.contains("network timeout"));
            assert_eq!(issue.evidence.len(), 1);
        }

        #[test]
        fn no_working_directory() {
            let issue = issues::no_working_directory();
            assert!(issue.is_blocking());
            assert!(issue.blocks_capability(&Capability::WorkingDirectoryAvailable));
            assert!(issue.message.contains("bare repository"));
        }

        #[test]
        fn branches_checked_out_elsewhere_single() {
            use crate::core::types::BranchName;
            use std::path::PathBuf;

            let conflicts = vec![(
                BranchName::new("feature").unwrap(),
                PathBuf::from("/worktrees/feature"),
            )];

            let issue = issues::branches_checked_out_elsewhere(conflicts);
            assert!(issue.is_blocking());
            assert!(issue.message.contains("feature"));
            assert!(issue.message.contains("another worktree"));
            assert!(!issue.evidence.is_empty());
        }

        #[test]
        fn branches_checked_out_elsewhere_multiple() {
            use crate::core::types::BranchName;
            use std::path::PathBuf;

            let conflicts = vec![
                (
                    BranchName::new("feature-a").unwrap(),
                    PathBuf::from("/worktrees/a"),
                ),
                (
                    BranchName::new("feature-b").unwrap(),
                    PathBuf::from("/worktrees/b"),
                ),
            ];

            let issue = issues::branches_checked_out_elsewhere(conflicts);
            assert!(issue.is_blocking());
            assert!(issue.message.contains("feature-a"));
            assert!(issue.message.contains("feature-b"));
            assert!(issue.message.contains("other worktrees"));
        }

        // --- Bootstrap Issues ---

        #[test]
        fn remote_open_prs_detected() {
            let issue = issues::remote_open_prs_detected(5, false);
            assert!(!issue.is_blocking()); // Info severity
            assert!(issue.message.contains("5 open pull request"));
            assert!(!issue.message.contains("truncated"));
        }

        #[test]
        fn remote_open_prs_detected_truncated() {
            let issue = issues::remote_open_prs_detected(200, true);
            assert!(!issue.is_blocking());
            assert!(issue.message.contains("200"));
            assert!(issue.message.contains("truncated"));
        }

        #[test]
        fn remote_pr_branch_missing() {
            let issue = issues::remote_pr_branch_missing(
                42,
                "feature-branch",
                "main",
                "https://github.com/org/repo/pull/42",
            );
            assert!(!issue.is_blocking()); // Warning severity
            assert!(issue.message.contains("42"));
            assert!(issue.message.contains("feature-branch"));
            assert_eq!(issue.evidence.len(), 2);
        }

        #[test]
        fn remote_pr_branch_untracked() {
            let issue = issues::remote_pr_branch_untracked(
                "feature",
                42,
                "main",
                "https://github.com/org/repo/pull/42",
            );
            assert!(!issue.is_blocking()); // Warning severity
            assert!(issue.message.contains("feature"));
            assert!(issue.message.contains("42"));
            assert!(issue.message.contains("not tracked"));
            assert_eq!(issue.evidence.len(), 2);
        }

        #[test]
        fn remote_pr_not_linked() {
            let issue =
                issues::remote_pr_not_linked("feature", 42, "https://github.com/org/repo/pull/42");
            assert!(!issue.is_blocking()); // Info severity
            assert!(issue.message.contains("feature"));
            assert!(issue.message.contains("42"));
            assert!(issue.message.contains("not linked"));
            assert_eq!(issue.evidence.len(), 2);
        }
    }
}
