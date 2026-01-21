//! engine::gate
//!
//! Capability gating for command execution.
//!
//! # Architecture
//!
//! Per ARCHITECTURE.md Section 5, gating determines whether a command's
//! required capabilities are satisfied. Each command declares its requirement
//! set. If requirements are met, gating produces a `ReadyContext`. If not,
//! it produces a `RepairBundle` for the Doctor.
//!
//! **Key insight:** There is no global "repo is valid" boolean. Each command
//! has its own validation contract. Read-only commands may run with fewer
//! capabilities than mutating commands.
//!
//! # Invariants
//!
//! - Gating never produces a ReadyContext when requirements are not met
//! - Gating is deterministic given the same snapshot
//! - A missing capability always maps to one or more blocking issues
//!
//! # Example
//!
//! ```ignore
//! use latticework::engine::gate::{gate, requirements};
//! use latticework::engine::scan::scan;
//!
//! let snapshot = scan(&git)?;
//!
//! match gate(&snapshot, &requirements::MUTATING) {
//!     GateResult::Ready(ctx) => {
//!         // Proceed with planning and execution
//!     }
//!     GateResult::NeedsRepair(bundle) => {
//!         // Hand off to Doctor
//!         for issue in &bundle.blocking_issues {
//!             println!("Blocking: {}", issue.message);
//!         }
//!     }
//! }
//! ```

use super::capabilities::{Capability, CapabilitySet};
use super::health::Issue;
use super::scan::{RepoSnapshot, ScannedMetadata};
use crate::core::graph::StackGraph;
use crate::core::types::BranchName;
use std::collections::HashMap;

/// Requirements for a command to execute.
///
/// Each command declares its required capabilities. Gating checks if
/// the snapshot's capabilities satisfy these requirements.
#[derive(Debug, Clone)]
pub struct RequirementSet {
    /// Required capabilities for this command type.
    pub capabilities: &'static [Capability],
    /// Human-readable name for this requirement set.
    pub name: &'static str,
}

impl RequirementSet {
    /// Create a new requirement set.
    pub const fn new(name: &'static str, capabilities: &'static [Capability]) -> Self {
        Self { capabilities, name }
    }

    /// Check if all requirements are satisfied.
    pub fn satisfied_by(&self, caps: &CapabilitySet) -> bool {
        caps.has_all(self.capabilities)
    }

    /// Get the missing capabilities.
    pub fn missing(&self, caps: &CapabilitySet) -> Vec<Capability> {
        caps.missing(self.capabilities)
    }
}

/// Predefined requirement sets for common command categories.
pub mod requirements {
    use super::*;

    /// Requirements for read-only commands (log, info, parent, children).
    ///
    /// These commands only need the repository to be accessible.
    /// Works in bare repositories.
    pub const READ_ONLY: RequirementSet = RequirementSet::new("read-only", &[Capability::RepoOpen]);

    /// Requirements for commands that read stack structure (checkout, up, down).
    ///
    /// These need metadata and graph to be valid but don't require
    /// absence of in-progress operations.
    ///
    /// Per SPEC.md §4.6.6 Category C, navigation commands require a working
    /// directory and do NOT work in bare repositories.
    pub const NAVIGATION: RequirementSet = RequirementSet::new(
        "navigation",
        &[
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::MetadataReadable,
            Capability::GraphValid,
            Capability::WorkingDirectoryAvailable,
        ],
    );

    /// Requirements for mutating commands (create, restack, modify, fold, etc.).
    ///
    /// These require no in-progress operations, a valid graph, and a working
    /// directory (since they modify the working tree).
    ///
    /// Per SPEC.md §4.6.6 Category C, these commands do NOT work in bare
    /// repositories.
    pub const MUTATING: RequirementSet = RequirementSet::new(
        "mutating",
        &[
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::NoLatticeOpInProgress,
            Capability::NoExternalGitOpInProgress,
            Capability::MetadataReadable,
            Capability::GraphValid,
            Capability::FrozenPolicySatisfied,
            Capability::WorkingDirectoryAvailable,
        ],
    );

    /// Requirements for metadata-only mutating commands (track, untrack, freeze, unfreeze).
    ///
    /// These modify only metadata refs, not the working tree or branch tips.
    ///
    /// Per SPEC.md §4.6.6 Category B, these commands work in bare repositories.
    pub const MUTATING_METADATA_ONLY: RequirementSet = RequirementSet::new(
        "mutating-metadata-only",
        &[
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::NoLatticeOpInProgress,
            Capability::NoExternalGitOpInProgress,
            Capability::MetadataReadable,
            Capability::GraphValid,
            Capability::FrozenPolicySatisfied,
            // Note: WorkingDirectoryAvailable NOT required
        ],
    );

    /// Requirements for remote commands (submit, sync, get).
    ///
    /// All of MUTATING plus remote and auth requirements.
    ///
    /// Per SPEC.md §4.6.6, submit/sync/get may work in bare repos with
    /// restrictions (e.g., `--no-restack`, `--no-checkout`), but by default
    /// they require a working directory.
    ///
    /// Per SPEC.md §8E.0, requires RepoAuthorized to verify GitHub App access.
    pub const REMOTE: RequirementSet = RequirementSet::new(
        "remote",
        &[
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::NoLatticeOpInProgress,
            Capability::NoExternalGitOpInProgress,
            Capability::MetadataReadable,
            Capability::GraphValid,
            Capability::FrozenPolicySatisfied,
            Capability::WorkingDirectoryAvailable,
            Capability::RemoteResolved,
            Capability::AuthAvailable,
            Capability::RepoAuthorized,
        ],
    );

    /// Requirements for remote commands in bare repo mode.
    ///
    /// Per SPEC.md §4.6.6, remote commands can work in bare repos
    /// when used with flags like `--no-restack` or `--no-checkout`.
    /// This requirement set is for those restricted modes.
    ///
    /// Per SPEC.md §8E.0, requires RepoAuthorized to verify GitHub App access.
    pub const REMOTE_BARE_ALLOWED: RequirementSet = RequirementSet::new(
        "remote-bare-allowed",
        &[
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::NoLatticeOpInProgress,
            Capability::NoExternalGitOpInProgress,
            Capability::MetadataReadable,
            Capability::GraphValid,
            Capability::FrozenPolicySatisfied,
            // Note: WorkingDirectoryAvailable NOT required
            Capability::RemoteResolved,
            Capability::AuthAvailable,
            Capability::RepoAuthorized,
        ],
    );

    /// Requirements for continue/abort commands.
    ///
    /// These specifically require a Lattice op to be in progress.
    pub const RECOVERY: RequirementSet = RequirementSet::new(
        "recovery",
        &[
            Capability::RepoOpen,
            // Note: Does NOT require NoLatticeOpInProgress
        ],
    );

    /// Minimal requirements (just repo access).
    pub const MINIMAL: RequirementSet = RequirementSet::new("minimal", &[Capability::RepoOpen]);
}

/// Result of gating check.
#[derive(Debug)]
pub enum GateResult {
    /// Requirements satisfied, ready to execute.
    Ready(Box<ReadyContext>),
    /// Requirements not satisfied, needs repair.
    NeedsRepair(RepairBundle),
}

impl GateResult {
    /// Check if gating passed.
    pub fn is_ready(&self) -> bool {
        matches!(self, GateResult::Ready(_))
    }

    /// Unwrap the ready context, panicking if not ready.
    pub fn unwrap_ready(self) -> ReadyContext {
        match self {
            GateResult::Ready(ctx) => *ctx,
            GateResult::NeedsRepair(_) => panic!("called unwrap_ready on NeedsRepair"),
        }
    }

    /// Unwrap the repair bundle, panicking if ready.
    pub fn unwrap_repair(self) -> RepairBundle {
        match self {
            GateResult::Ready(_) => panic!("called unwrap_repair on Ready"),
            GateResult::NeedsRepair(bundle) => bundle,
        }
    }
}

/// A validated context indicating the command is ready to execute.
///
/// Contains the snapshot and any command-specific validated data
/// extracted during gating.
#[derive(Debug)]
pub struct ReadyContext {
    /// The scanned repository snapshot.
    pub snapshot: RepoSnapshot,
    /// Command-specific validated data.
    pub data: ValidatedData,
}

impl ReadyContext {
    /// Get the trunk branch.
    pub fn trunk(&self) -> Option<&BranchName> {
        self.snapshot.trunk()
    }

    /// Get the capabilities that were verified.
    pub fn capabilities(&self) -> &CapabilitySet {
        self.snapshot.health.capabilities()
    }
}

/// Command-specific validated data.
///
/// Different commands may need different validated data. This enum
/// provides type-safe access to command-specific information that
/// was validated during gating.
#[derive(Debug, Default)]
pub enum ValidatedData {
    /// No specific data needed.
    #[default]
    None,

    /// Stack scope for operations that affect multiple branches.
    StackScope {
        /// The trunk branch.
        trunk: BranchName,
        /// Branches in the scope (in topological order).
        branches: Vec<BranchName>,
    },

    /// Single branch target.
    SingleBranch {
        /// The target branch.
        branch: BranchName,
    },
}

/// Bundle of issues requiring repair.
///
/// Produced when gating fails. Contains the information needed
/// by Doctor to present fix options to the user.
#[derive(Debug)]
pub struct RepairBundle {
    /// The command that failed gating.
    pub command: String,
    /// Capabilities that are missing.
    pub missing_capabilities: Vec<Capability>,
    /// Issues that are blocking execution.
    pub blocking_issues: Vec<Issue>,
}

impl RepairBundle {
    /// Check if the bundle has any blocking issues.
    pub fn has_issues(&self) -> bool {
        !self.blocking_issues.is_empty()
    }

    /// Get a summary message.
    pub fn summary(&self) -> String {
        let n = self.blocking_issues.len();
        if n == 1 {
            format!("1 issue blocking {}", self.command)
        } else {
            format!("{} issues blocking {}", n, self.command)
        }
    }
}

impl std::fmt::Display for RepairBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary())
    }
}

/// Perform gating check for a command.
///
/// Checks if the snapshot's capabilities satisfy the requirement set.
/// If satisfied, returns a `ReadyContext`. If not, returns a `RepairBundle`.
///
/// # Arguments
///
/// * `snapshot` - The scanned repository snapshot
/// * `requirements` - The requirement set to check against
///
/// # Returns
///
/// `GateResult::Ready` if all requirements are satisfied,
/// `GateResult::NeedsRepair` otherwise.
///
/// # Example
///
/// ```ignore
/// let result = gate(snapshot, &requirements::MUTATING);
/// match result {
///     GateResult::Ready(ctx) => {
///         // Plan and execute
///     }
///     GateResult::NeedsRepair(bundle) => {
///         // Show issues to user
///     }
/// }
/// ```
pub fn gate(snapshot: RepoSnapshot, requirements: &RequirementSet) -> GateResult {
    let caps = snapshot.health.capabilities();

    if requirements.satisfied_by(caps) {
        GateResult::Ready(Box::new(ReadyContext {
            snapshot,
            data: ValidatedData::None,
        }))
    } else {
        let missing = requirements.missing(caps);

        // Collect blocking issues that relate to missing capabilities
        let blocking_issues: Vec<Issue> = snapshot.health.blocking_issues().cloned().collect();

        GateResult::NeedsRepair(RepairBundle {
            command: requirements.name.to_string(),
            missing_capabilities: missing,
            blocking_issues,
        })
    }
}

/// Gate with additional scope resolution.
///
/// For commands that operate on a scope of branches (like restack),
/// this resolves the scope and includes it in the validated data.
pub fn gate_with_scope(
    snapshot: RepoSnapshot,
    requirements: &RequirementSet,
    target: Option<&BranchName>,
) -> GateResult {
    // First do basic gating
    let caps = snapshot.health.capabilities();

    if !requirements.satisfied_by(caps) {
        let missing = requirements.missing(caps);
        let blocking_issues: Vec<Issue> = snapshot.health.blocking_issues().cloned().collect();

        return GateResult::NeedsRepair(RepairBundle {
            command: requirements.name.to_string(),
            missing_capabilities: missing,
            blocking_issues,
        });
    }

    // Resolve scope
    let trunk = match snapshot.trunk() {
        Some(t) => t.clone(),
        None => {
            // This shouldn't happen if TrunkKnown capability is satisfied
            return GateResult::NeedsRepair(RepairBundle {
                command: requirements.name.to_string(),
                missing_capabilities: vec![Capability::TrunkKnown],
                blocking_issues: vec![],
            });
        }
    };

    // Determine target branch
    let target_branch = target.cloned().or_else(|| snapshot.current_branch.clone());

    let data = match target_branch {
        Some(branch) => {
            // Get all branches in scope using scope walking
            let branches = compute_stack_scope(&branch, &snapshot.graph, &trunk);
            ValidatedData::StackScope { trunk, branches }
        }
        None => ValidatedData::StackScope {
            trunk,
            branches: vec![],
        },
    };

    GateResult::Ready(Box::new(ReadyContext { snapshot, data }))
}

/// Compute the stack scope for a branch operation.
///
/// Returns all branches that would be in scope for an operation on `target`:
/// - The target branch itself
/// - All ancestors up to (but not including) trunk
///
/// The returned list is in topological order (closest to trunk first),
/// which is the correct order for restack operations.
///
/// # Arguments
///
/// * `target` - The branch being operated on
/// * `graph` - The stack graph
/// * `trunk` - The trunk branch name
///
/// # Example
///
/// ```ignore
/// // Graph: trunk -> A -> B -> C (target)
/// let scope = compute_stack_scope(&c, &graph, &trunk);
/// // Returns: [A, B, C] (in topological order)
/// ```
pub fn compute_stack_scope(
    target: &BranchName,
    graph: &StackGraph,
    trunk: &BranchName,
) -> Vec<BranchName> {
    let mut scope = Vec::new();

    // Walk from target up to trunk, collecting ancestors
    let mut current = Some(target.clone());
    while let Some(branch) = current {
        if &branch == trunk {
            // Don't include trunk in the scope
            break;
        }
        scope.push(branch.clone());
        current = graph.parent(&branch).cloned();
    }

    // Reverse to get topological order (closest to trunk first)
    scope.reverse();
    scope
}

/// Compute the freeze scope for a branch operation.
///
/// Per SPEC.md §8B.4, freeze applies to "the target branch and its
/// downstack ancestors up to trunk."
///
/// This function returns all branches that must be checked for frozen state
/// before an operation that would modify the target branch.
///
/// # Arguments
///
/// * `target` - The branch being operated on
/// * `graph` - The stack graph
/// * `trunk` - The trunk branch name
/// * `include_descendants` - Whether to include descendants (for upstack operations)
///
/// # Returns
///
/// List of branches to check for frozen state.
pub fn compute_freeze_scope(
    target: &BranchName,
    graph: &StackGraph,
    trunk: &BranchName,
    include_descendants: bool,
) -> Vec<BranchName> {
    let mut scope = Vec::new();

    // Always include target
    scope.push(target.clone());

    // Walk downstack (ancestors) to trunk
    let mut current = graph.parent(target).cloned();
    while let Some(branch) = current {
        if &branch == trunk {
            break;
        }
        scope.push(branch.clone());
        current = graph.parent(&branch).cloned();
    }

    // Optionally include descendants (upstack)
    if include_descendants {
        let descendants = graph.descendants(target);
        scope.extend(descendants);
    }

    scope
}

/// Check if frozen policy is satisfied for a scope.
///
/// Returns `Ok(())` if no branches in the scope are frozen,
/// or `Err(frozen_branches)` with the list of frozen branches.
///
/// # Arguments
///
/// * `scope` - Branches to check
/// * `metadata` - Metadata for tracked branches
///
/// # Example
///
/// ```ignore
/// let scope = compute_freeze_scope(&target, &graph, &trunk, true);
/// match check_frozen_policy(&scope, &snapshot.metadata) {
///     Ok(()) => { /* proceed */ }
///     Err(frozen) => {
///         println!("Cannot proceed: branches {:?} are frozen", frozen);
///     }
/// }
/// ```
pub fn check_frozen_policy(
    scope: &[BranchName],
    metadata: &HashMap<BranchName, ScannedMetadata>,
) -> Result<(), Vec<BranchName>> {
    let frozen: Vec<BranchName> = scope
        .iter()
        .filter(|b| {
            metadata
                .get(*b)
                .map(|m| m.metadata.freeze.is_frozen())
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    if frozen.is_empty() {
        Ok(())
    } else {
        Err(frozen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Fingerprint;
    use crate::engine::health::RepoHealthReport;
    use crate::git::{GitState, RepoInfo, WorktreeStatus};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_snapshot_with_caps(caps: &[Capability]) -> RepoSnapshot {
        let mut health = RepoHealthReport::new();
        for cap in caps {
            health.add_capability(*cap);
        }

        RepoSnapshot {
            info: RepoInfo {
                git_dir: PathBuf::from("/repo/.git"),
                common_dir: PathBuf::from("/repo/.git"),
                work_dir: Some(PathBuf::from("/repo")),
                context: crate::git::RepoContext::Normal,
            },
            git_state: GitState::Clean,
            worktree_status: WorktreeStatus::default(),
            current_branch: Some(BranchName::new("main").unwrap()),
            branches: HashMap::new(),
            metadata: HashMap::new(),
            repo_config: None,
            trunk: Some(BranchName::new("main").unwrap()),
            graph: crate::core::graph::StackGraph::new(),
            fingerprint: Fingerprint::compute(&[]),
            health,
            remote_prs: None,
        }
    }

    mod requirement_set {
        use super::*;

        #[test]
        fn satisfied_with_all_caps() {
            let caps = CapabilitySet::with([Capability::RepoOpen, Capability::TrunkKnown]);
            let reqs = RequirementSet::new("test", &[Capability::RepoOpen, Capability::TrunkKnown]);
            assert!(reqs.satisfied_by(&caps));
        }

        #[test]
        fn not_satisfied_with_missing() {
            let caps = CapabilitySet::with([Capability::RepoOpen]);
            let reqs = RequirementSet::new("test", &[Capability::RepoOpen, Capability::TrunkKnown]);
            assert!(!reqs.satisfied_by(&caps));
        }

        #[test]
        fn missing_returns_absent_caps() {
            let caps = CapabilitySet::with([Capability::RepoOpen]);
            let reqs = RequirementSet::new(
                "test",
                &[
                    Capability::RepoOpen,
                    Capability::TrunkKnown,
                    Capability::GraphValid,
                ],
            );
            let missing = reqs.missing(&caps);
            assert_eq!(missing.len(), 2);
            assert!(missing.contains(&Capability::TrunkKnown));
            assert!(missing.contains(&Capability::GraphValid));
        }

        #[test]
        fn empty_requirements_always_satisfied() {
            let caps = CapabilitySet::new();
            let reqs = RequirementSet::new("empty", &[]);
            assert!(reqs.satisfied_by(&caps));
        }
    }

    mod predefined_requirements {
        use super::*;

        #[test]
        fn read_only_requires_repo_open() {
            assert!(requirements::READ_ONLY
                .capabilities
                .contains(&Capability::RepoOpen));
            assert_eq!(requirements::READ_ONLY.capabilities.len(), 1);
            // Read-only does NOT require working directory (works in bare repos)
            assert!(!requirements::READ_ONLY
                .capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }

        #[test]
        fn navigation_requires_graph_and_workdir() {
            assert!(requirements::NAVIGATION
                .capabilities
                .contains(&Capability::GraphValid));
            assert!(requirements::NAVIGATION
                .capabilities
                .contains(&Capability::MetadataReadable));
            // Navigation requires working directory
            assert!(requirements::NAVIGATION
                .capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }

        #[test]
        fn mutating_requires_no_ops_in_progress_and_workdir() {
            assert!(requirements::MUTATING
                .capabilities
                .contains(&Capability::NoLatticeOpInProgress));
            assert!(requirements::MUTATING
                .capabilities
                .contains(&Capability::NoExternalGitOpInProgress));
            // Mutating requires working directory
            assert!(requirements::MUTATING
                .capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }

        #[test]
        fn mutating_metadata_only_does_not_require_workdir() {
            assert!(requirements::MUTATING_METADATA_ONLY
                .capabilities
                .contains(&Capability::NoLatticeOpInProgress));
            assert!(requirements::MUTATING_METADATA_ONLY
                .capabilities
                .contains(&Capability::NoExternalGitOpInProgress));
            // Metadata-only does NOT require working directory (works in bare repos)
            assert!(!requirements::MUTATING_METADATA_ONLY
                .capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }

        #[test]
        fn remote_extends_mutating_with_workdir() {
            assert!(requirements::REMOTE
                .capabilities
                .contains(&Capability::AuthAvailable));
            assert!(requirements::REMOTE
                .capabilities
                .contains(&Capability::RemoteResolved));
            assert!(requirements::REMOTE
                .capabilities
                .contains(&Capability::RepoAuthorized));
            // Also includes mutating requirements
            assert!(requirements::REMOTE
                .capabilities
                .contains(&Capability::NoLatticeOpInProgress));
            // Remote requires working directory by default
            assert!(requirements::REMOTE
                .capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }

        #[test]
        fn remote_bare_allowed_does_not_require_workdir() {
            assert!(requirements::REMOTE_BARE_ALLOWED
                .capabilities
                .contains(&Capability::AuthAvailable));
            assert!(requirements::REMOTE_BARE_ALLOWED
                .capabilities
                .contains(&Capability::RemoteResolved));
            assert!(requirements::REMOTE_BARE_ALLOWED
                .capabilities
                .contains(&Capability::RepoAuthorized));
            // Does NOT require working directory (for bare repo operations)
            assert!(!requirements::REMOTE_BARE_ALLOWED
                .capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }
    }

    mod gate_result {
        use super::*;

        #[test]
        fn is_ready() {
            let snapshot = make_snapshot_with_caps(&[Capability::RepoOpen]);
            let result = gate(snapshot, &requirements::READ_ONLY);
            assert!(result.is_ready());
        }

        #[test]
        fn unwrap_ready() {
            let snapshot = make_snapshot_with_caps(&[Capability::RepoOpen]);
            let result = gate(snapshot, &requirements::READ_ONLY);
            let ctx = result.unwrap_ready();
            assert!(ctx.capabilities().has(&Capability::RepoOpen));
        }

        #[test]
        #[should_panic(expected = "unwrap_ready on NeedsRepair")]
        fn unwrap_ready_panics_on_repair() {
            let snapshot = make_snapshot_with_caps(&[]);
            let result = gate(snapshot, &requirements::READ_ONLY);
            result.unwrap_ready();
        }

        #[test]
        fn unwrap_repair() {
            let snapshot = make_snapshot_with_caps(&[]);
            let result = gate(snapshot, &requirements::READ_ONLY);
            let bundle = result.unwrap_repair();
            assert!(bundle.missing_capabilities.contains(&Capability::RepoOpen));
        }
    }

    mod gate_function {
        use super::*;

        #[test]
        fn passes_when_all_caps_present() {
            let snapshot = make_snapshot_with_caps(&[
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::MetadataReadable,
                Capability::GraphValid,
                Capability::WorkingDirectoryAvailable,
            ]);

            let result = gate(snapshot, &requirements::NAVIGATION);
            assert!(result.is_ready());
        }

        #[test]
        fn fails_when_cap_missing() {
            let snapshot = make_snapshot_with_caps(&[
                Capability::RepoOpen,
                Capability::TrunkKnown,
                // Missing MetadataReadable, GraphValid, and WorkingDirectoryAvailable
            ]);

            let result = gate(snapshot, &requirements::NAVIGATION);
            assert!(!result.is_ready());

            let bundle = result.unwrap_repair();
            assert!(bundle
                .missing_capabilities
                .contains(&Capability::MetadataReadable));
            assert!(bundle
                .missing_capabilities
                .contains(&Capability::GraphValid));
            assert!(bundle
                .missing_capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }

        #[test]
        fn repair_bundle_has_command_name() {
            let snapshot = make_snapshot_with_caps(&[]);
            let result = gate(snapshot, &requirements::MUTATING);
            let bundle = result.unwrap_repair();
            assert_eq!(bundle.command, "mutating");
        }

        #[test]
        fn bare_repo_fails_navigation() {
            // Simulates a bare repo (no WorkingDirectoryAvailable)
            let snapshot = make_snapshot_with_caps(&[
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::MetadataReadable,
                Capability::GraphValid,
                // Missing WorkingDirectoryAvailable
            ]);

            let result = gate(snapshot, &requirements::NAVIGATION);
            assert!(!result.is_ready());

            let bundle = result.unwrap_repair();
            assert!(bundle
                .missing_capabilities
                .contains(&Capability::WorkingDirectoryAvailable));
        }

        #[test]
        fn bare_repo_passes_read_only() {
            // Bare repo should pass read-only requirements
            let snapshot = make_snapshot_with_caps(&[
                Capability::RepoOpen,
                // No WorkingDirectoryAvailable - bare repo
            ]);

            let result = gate(snapshot, &requirements::READ_ONLY);
            assert!(result.is_ready());
        }

        #[test]
        fn bare_repo_passes_metadata_only() {
            // Bare repo should pass metadata-only requirements
            let snapshot = make_snapshot_with_caps(&[
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::NoLatticeOpInProgress,
                Capability::NoExternalGitOpInProgress,
                Capability::MetadataReadable,
                Capability::GraphValid,
                Capability::FrozenPolicySatisfied,
                // No WorkingDirectoryAvailable - bare repo
            ]);

            let result = gate(snapshot, &requirements::MUTATING_METADATA_ONLY);
            assert!(result.is_ready());
        }
    }

    mod repair_bundle {
        use super::*;

        #[test]
        fn summary_singular() {
            use crate::engine::health::{Issue, Severity};

            let bundle = RepairBundle {
                command: "test".to_string(),
                missing_capabilities: vec![],
                blocking_issues: vec![Issue::new("test", Severity::Blocking, "msg")],
            };
            assert!(bundle.summary().contains("1 issue"));
        }

        #[test]
        fn summary_plural() {
            use crate::engine::health::{Issue, Severity};

            let bundle = RepairBundle {
                command: "test".to_string(),
                missing_capabilities: vec![],
                blocking_issues: vec![
                    Issue::new("test1", Severity::Blocking, "msg1"),
                    Issue::new("test2", Severity::Blocking, "msg2"),
                ],
            };
            assert!(bundle.summary().contains("2 issues"));
        }

        #[test]
        fn has_issues() {
            use crate::engine::health::{Issue, Severity};

            let empty = RepairBundle {
                command: "test".to_string(),
                missing_capabilities: vec![],
                blocking_issues: vec![],
            };
            assert!(!empty.has_issues());

            let with_issue = RepairBundle {
                command: "test".to_string(),
                missing_capabilities: vec![],
                blocking_issues: vec![Issue::new("test", Severity::Blocking, "msg")],
            };
            assert!(with_issue.has_issues());
        }
    }

    mod ready_context {
        use super::*;

        #[test]
        fn trunk_accessor() {
            let snapshot = make_snapshot_with_caps(&[Capability::RepoOpen]);
            let ctx = ReadyContext {
                snapshot,
                data: ValidatedData::None,
            };
            assert_eq!(ctx.trunk().map(|b| b.as_str()), Some("main"));
        }

        #[test]
        fn capabilities_accessor() {
            let snapshot = make_snapshot_with_caps(&[Capability::RepoOpen, Capability::TrunkKnown]);
            let ctx = ReadyContext {
                snapshot,
                data: ValidatedData::None,
            };
            assert!(ctx.capabilities().has(&Capability::RepoOpen));
            assert!(ctx.capabilities().has(&Capability::TrunkKnown));
        }
    }

    mod validated_data {
        use super::*;

        #[test]
        fn default_is_none() {
            let data = ValidatedData::default();
            assert!(matches!(data, ValidatedData::None));
        }

        #[test]
        fn stack_scope_constructible() {
            let data = ValidatedData::StackScope {
                trunk: BranchName::new("main").unwrap(),
                branches: vec![
                    BranchName::new("feature-a").unwrap(),
                    BranchName::new("feature-b").unwrap(),
                ],
            };
            match data {
                ValidatedData::StackScope { trunk, branches } => {
                    assert_eq!(trunk.as_str(), "main");
                    assert_eq!(branches.len(), 2);
                }
                _ => panic!("wrong variant"),
            }
        }

        #[test]
        fn single_branch_constructible() {
            let data = ValidatedData::SingleBranch {
                branch: BranchName::new("feature").unwrap(),
            };
            match data {
                ValidatedData::SingleBranch { branch } => {
                    assert_eq!(branch.as_str(), "feature");
                }
                _ => panic!("wrong variant"),
            }
        }
    }

    mod scope_walking {
        use super::*;
        use crate::core::graph::StackGraph;

        fn make_graph() -> (StackGraph, BranchName, BranchName, BranchName, BranchName) {
            // Build graph: main -> a -> b -> c
            let mut graph = StackGraph::new();
            let main = BranchName::new("main").unwrap();
            let a = BranchName::new("a").unwrap();
            let b = BranchName::new("b").unwrap();
            let c = BranchName::new("c").unwrap();

            graph.add_edge(a.clone(), main.clone());
            graph.add_edge(b.clone(), a.clone());
            graph.add_edge(c.clone(), b.clone());

            (graph, main, a, b, c)
        }

        #[test]
        fn compute_stack_scope_includes_target() {
            let (graph, main, a, _, _) = make_graph();
            let scope = compute_stack_scope(&a, &graph, &main);
            assert!(scope.contains(&a));
        }

        #[test]
        fn compute_stack_scope_includes_ancestors() {
            let (graph, main, a, b, c) = make_graph();
            let scope = compute_stack_scope(&c, &graph, &main);

            // Should include a, b, c but not main (trunk)
            assert!(scope.contains(&a));
            assert!(scope.contains(&b));
            assert!(scope.contains(&c));
            assert!(!scope.contains(&main));
        }

        #[test]
        fn compute_stack_scope_topological_order() {
            let (graph, main, a, b, c) = make_graph();
            let scope = compute_stack_scope(&c, &graph, &main);

            // Order should be a, b, c (closest to trunk first)
            let a_pos = scope.iter().position(|x| x == &a).unwrap();
            let b_pos = scope.iter().position(|x| x == &b).unwrap();
            let c_pos = scope.iter().position(|x| x == &c).unwrap();

            assert!(a_pos < b_pos);
            assert!(b_pos < c_pos);
        }

        #[test]
        fn compute_stack_scope_single_branch() {
            let (graph, main, a, _, _) = make_graph();
            let scope = compute_stack_scope(&a, &graph, &main);

            // Only a, since its parent is trunk
            assert_eq!(scope.len(), 1);
            assert_eq!(scope[0], a);
        }

        #[test]
        fn compute_freeze_scope_without_descendants() {
            let (graph, main, a, b, c) = make_graph();
            let scope = compute_freeze_scope(&b, &graph, &main, false);

            // Should include b (target) and a (ancestor), but not c (descendant)
            assert!(scope.contains(&b));
            assert!(scope.contains(&a));
            assert!(!scope.contains(&c));
            assert!(!scope.contains(&main));
        }

        #[test]
        fn compute_freeze_scope_with_descendants() {
            let (graph, main, a, b, c) = make_graph();
            let scope = compute_freeze_scope(&b, &graph, &main, true);

            // Should include b (target), a (ancestor), and c (descendant)
            assert!(scope.contains(&b));
            assert!(scope.contains(&a));
            assert!(scope.contains(&c));
            assert!(!scope.contains(&main));
        }

        #[test]
        fn check_frozen_policy_passes_when_no_frozen() {
            let scope = vec![BranchName::new("a").unwrap(), BranchName::new("b").unwrap()];
            let metadata = HashMap::new(); // No metadata = no frozen branches

            let result = check_frozen_policy(&scope, &metadata);
            assert!(result.is_ok());
        }

        #[test]
        fn check_frozen_policy_fails_when_frozen() {
            use crate::core::metadata::schema::{
                BranchMetadataV1, FreezeScope as FreezeScopeEnum, FreezeState,
            };
            use crate::core::types::Oid;

            let a = BranchName::new("a").unwrap();
            let b = BranchName::new("b").unwrap();
            let scope = vec![a.clone(), b.clone()];

            // Create metadata with 'a' frozen
            let mut metadata = HashMap::new();
            let mut meta_a =
                BranchMetadataV1::new(a.clone(), BranchName::new("main").unwrap(), Oid::zero());
            meta_a.freeze = FreezeState::frozen(FreezeScopeEnum::Single, Some("test".to_string()));
            metadata.insert(
                a.clone(),
                ScannedMetadata {
                    ref_oid: Oid::zero(),
                    metadata: meta_a,
                },
            );

            let result = check_frozen_policy(&scope, &metadata);
            assert!(result.is_err());
            let frozen = result.unwrap_err();
            assert_eq!(frozen.len(), 1);
            assert!(frozen.contains(&a));
        }
    }
}
