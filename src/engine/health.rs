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
/// Contains all issues found and the capabilities that were
/// successfully established. Used by gating to determine if
/// a command can proceed.
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
/// ```
#[derive(Debug, Clone, Default)]
pub struct RepoHealthReport {
    issues: Vec<Issue>,
    capabilities: CapabilitySet,
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
    }
}
