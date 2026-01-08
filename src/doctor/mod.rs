//! doctor
//!
//! Explicit repair framework.
//!
//! # Architecture
//!
//! Doctor is the unified repair broker. It handles:
//! 1. `lattice doctor` invoked explicitly
//! 2. Any command that fails gating and requires repair
//!
//! Doctor shares the same scanner, planner, and executor as regular commands.
//! There is no separate "repair mutation path."
//!
//! # Design Principles
//!
//! - Never guess repairs silently
//! - Always present fix options with clear explanations
//! - Require explicit user confirmation before applying fixes
//! - Record all repairs in the event ledger
//!
//! # Confirmation Model (per ARCHITECTURE.md Section 8.3)
//!
//! Interactive mode:
//! - Doctor presents issues and fix options
//! - User selects fix options
//! - Doctor presents a combined plan preview
//! - User confirms "apply" explicitly
//!
//! Non-interactive mode:
//! - Doctor emits issues and fix options with IDs
//! - Doctor applies fixes only when fix IDs are provided explicitly
//! - Doctor never auto-selects fixes
//!
//! # Example
//!
//! ```ignore
//! use lattice::doctor::{Doctor, DiagnosisReport};
//! use lattice::engine::scan::scan;
//!
//! // Diagnose repository issues
//! let snapshot = scan(&git)?;
//! let doctor = Doctor::new();
//! let diagnosis = doctor.diagnose(&snapshot);
//!
//! // Show issues and available fixes
//! for issue in &diagnosis.issues {
//!     println!("Issue: {}", issue.message);
//!     for fix in &diagnosis.fixes_for_issue(&issue.id) {
//!         println!("  Fix: {} - {}", fix.id, fix.description);
//!     }
//! }
//!
//! // Apply selected fixes (non-interactive)
//! let fix_ids = vec![FixId::parse("trunk-not-configured:set-trunk:main")];
//! let outcome = doctor.apply_fixes(&fix_ids, &snapshot, &git)?;
//! ```

mod fixes;
mod generators;
mod issues;
mod planner;

pub use fixes::*;
pub use generators::generate_fixes;
pub use issues::*;
pub use planner::{generate_repair_plan, RepairPlanError};

use thiserror::Error;

use crate::engine::health::{Issue, IssueId};
use crate::engine::plan::Plan;
use crate::engine::scan::RepoSnapshot;

/// Errors from Doctor operations.
#[derive(Debug, Error)]
pub enum DoctorError {
    /// No fixes selected.
    #[error("no fixes selected")]
    NoFixesSelected,

    /// Fix not found.
    #[error("fix not found: {0}")]
    FixNotFound(String),

    /// Repair plan generation failed.
    #[error("failed to generate repair plan: {0}")]
    PlanError(#[from] RepairPlanError),

    /// Preconditions not satisfied.
    #[error("preconditions not satisfied: {0}")]
    PreconditionsNotMet(String),

    /// Execution failed.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),
}

/// Summary of a diagnosis.
#[derive(Debug, Default)]
pub struct DiagnosisSummary {
    /// Total number of issues.
    pub issue_count: usize,
    /// Number of blocking issues.
    pub blocking_count: usize,
    /// Number of warning issues.
    pub warning_count: usize,
    /// Number of info issues.
    pub info_count: usize,
    /// Total number of available fixes.
    pub fix_count: usize,
}

/// Result of diagnosing a repository.
#[derive(Debug)]
pub struct DiagnosisReport {
    /// All issues found.
    pub issues: Vec<Issue>,
    /// All available fixes.
    pub fixes: Vec<FixOption>,
    /// Summary statistics.
    pub summary: DiagnosisSummary,
}

impl DiagnosisReport {
    /// Check if there are any blocking issues.
    pub fn has_blocking_issues(&self) -> bool {
        self.summary.blocking_count > 0
    }

    /// Check if the repository is healthy (no issues).
    pub fn is_healthy(&self) -> bool {
        self.issues.is_empty()
    }

    /// Get fixes for a specific issue.
    pub fn fixes_for_issue(&self, issue_id: &IssueId) -> Vec<&FixOption> {
        self.fixes
            .iter()
            .filter(|f| &f.issue_id == issue_id)
            .collect()
    }

    /// Find a fix by ID.
    pub fn find_fix(&self, fix_id: &FixId) -> Option<&FixOption> {
        self.fixes.iter().find(|f| &f.id == fix_id)
    }

    /// Get all blocking issues.
    pub fn blocking_issues(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|i| i.is_blocking())
    }

    /// Format the diagnosis for display.
    pub fn format(&self) -> String {
        let mut lines = Vec::new();

        if self.is_healthy() {
            lines.push("Repository is healthy - no issues found.".to_string());
            return lines.join("\n");
        }

        lines.push(format!(
            "Found {} issue(s): {} blocking, {} warnings, {} info",
            self.summary.issue_count,
            self.summary.blocking_count,
            self.summary.warning_count,
            self.summary.info_count
        ));
        lines.push(String::new());

        for issue in &self.issues {
            let severity = if issue.is_blocking() { "ERROR" } else { "WARN" };
            lines.push(format!("[{}] {} ({})", severity, issue.message, issue.id));

            let fixes = self.fixes_for_issue(&issue.id);
            if !fixes.is_empty() {
                lines.push("  Available fixes:".to_string());
                for fix in fixes {
                    lines.push(format!("    {} - {}", fix.id, fix.description));
                }
            }
            lines.push(String::new());
        }

        if self.summary.fix_count > 0 {
            lines.push("Run 'lattice doctor --fix <fix-id>' to apply a fix.".to_string());
        }

        lines.join("\n")
    }
}

/// Outcome of applying repairs.
#[derive(Debug)]
pub struct RepairOutcome {
    /// Fixes that were applied.
    pub applied_fixes: Vec<FixId>,
    /// The plan that was executed.
    pub plan: Plan,
    /// Whether all fixes were successful.
    pub success: bool,
    /// Error message if not successful.
    pub error: Option<String>,
}

impl RepairOutcome {
    /// Create a successful outcome.
    pub fn success(applied_fixes: Vec<FixId>, plan: Plan) -> Self {
        Self {
            applied_fixes,
            plan,
            success: true,
            error: None,
        }
    }

    /// Create a failed outcome.
    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            applied_fixes: Vec::new(),
            plan: Plan::new(crate::core::ops::journal::OpId::new(), "doctor"),
            success: false,
            error: Some(error.into()),
        }
    }
}

/// The Doctor - unified repair broker.
///
/// Per ARCHITECTURE.md Section 8.1, Doctor is a framework, not a special-case
/// command. It shares the same scanner, planner, and executor as regular commands.
///
/// # Confirmation Model
///
/// Doctor never applies fixes without explicit confirmation:
/// - Interactive: user selects from menu
/// - Non-interactive: user provides explicit fix IDs
#[derive(Debug, Default)]
pub struct Doctor {
    /// Whether to run in interactive mode.
    interactive: bool,
}

impl Doctor {
    /// Create a new Doctor instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set interactive mode.
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Diagnose repository issues and generate fix options.
    ///
    /// This examines the repository snapshot and health report to identify
    /// all issues, then generates fix options for each.
    pub fn diagnose(&self, snapshot: &RepoSnapshot) -> DiagnosisReport {
        let health = &snapshot.health;
        let issues: Vec<Issue> = health.issues().to_vec();

        // Generate fixes for each issue
        let mut all_fixes = Vec::new();
        for issue in &issues {
            let fixes = generate_fixes(issue, snapshot);
            all_fixes.extend(fixes);
        }

        // Compute summary
        let blocking_count = issues.iter().filter(|i| i.is_blocking()).count();
        let warning_count = issues
            .iter()
            .filter(|i| i.severity == crate::engine::health::Severity::Warning)
            .count();
        let info_count = issues
            .iter()
            .filter(|i| i.severity == crate::engine::health::Severity::Info)
            .count();

        DiagnosisReport {
            summary: DiagnosisSummary {
                issue_count: issues.len(),
                blocking_count,
                warning_count,
                info_count,
                fix_count: all_fixes.len(),
            },
            issues,
            fixes: all_fixes,
        }
    }

    /// Generate a repair plan for the given fix IDs.
    ///
    /// This validates that all fix IDs exist and their preconditions
    /// are satisfied, then generates a combined plan.
    pub fn plan_repairs(
        &self,
        fix_ids: &[FixId],
        diagnosis: &DiagnosisReport,
        snapshot: &RepoSnapshot,
    ) -> Result<Plan, DoctorError> {
        if fix_ids.is_empty() {
            return Err(DoctorError::NoFixesSelected);
        }

        // Find all fixes
        let mut fixes = Vec::new();
        for fix_id in fix_ids {
            let fix = diagnosis
                .find_fix(fix_id)
                .ok_or_else(|| DoctorError::FixNotFound(fix_id.to_string()))?;
            fixes.push(fix);
        }

        // Generate the repair plan
        let plan = generate_repair_plan(&fixes, snapshot)?;

        Ok(plan)
    }

    /// Preview what fixes would do without applying them.
    ///
    /// Returns a formatted string describing all changes.
    pub fn preview_fixes(
        &self,
        fix_ids: &[FixId],
        diagnosis: &DiagnosisReport,
    ) -> Result<String, DoctorError> {
        if fix_ids.is_empty() {
            return Ok("No fixes selected.".to_string());
        }

        let mut lines = Vec::new();
        lines.push(format!("Preview of {} fix(es):", fix_ids.len()));
        lines.push(String::new());

        for fix_id in fix_ids {
            let fix = diagnosis
                .find_fix(fix_id)
                .ok_or_else(|| DoctorError::FixNotFound(fix_id.to_string()))?;

            lines.push(format!("Fix: {} - {}", fix.id, fix.description));
            lines.push(fix.preview.format());
            lines.push(String::new());
        }

        Ok(lines.join("\n"))
    }

    /// Check if the Doctor is in interactive mode.
    pub fn is_interactive(&self) -> bool {
        self.interactive
    }
}

/// Create a diagnosis from a gate RepairBundle.
///
/// This is used when a command fails gating and needs to be handed off
/// to the Doctor. It generates fix options for the blocking issues.
pub fn diagnose_from_gate_bundle(
    bundle: &crate::engine::gate::RepairBundle,
    snapshot: &RepoSnapshot,
) -> DiagnosisReport {
    // Generate fixes for each blocking issue
    let mut all_fixes = Vec::new();
    for issue in &bundle.blocking_issues {
        let fixes = generate_fixes(issue, snapshot);
        all_fixes.extend(fixes);
    }

    let blocking_count = bundle.blocking_issues.len();

    DiagnosisReport {
        summary: DiagnosisSummary {
            issue_count: blocking_count,
            blocking_count,
            warning_count: 0,
            info_count: 0,
            fix_count: all_fixes.len(),
        },
        issues: bundle.blocking_issues.clone(),
        fixes: all_fixes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::graph::StackGraph;
    use crate::core::types::{BranchName, Fingerprint, Oid};
    use crate::engine::capabilities::Capability;
    use crate::engine::health::{issues, RepoHealthReport};
    use crate::git::{GitState, RepoInfo, WorktreeStatus};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn minimal_snapshot() -> RepoSnapshot {
        let mut branches = HashMap::new();
        branches.insert(
            BranchName::new("main").unwrap(),
            Oid::new("abc123def4567890abc123def4567890abc12345").unwrap(),
        );

        let mut health = RepoHealthReport::new();
        health.add_capability(Capability::RepoOpen);

        RepoSnapshot {
            info: RepoInfo {
                git_dir: PathBuf::from(".git"),
                work_dir: PathBuf::from("."),
            },
            git_state: GitState::Clean,
            worktree_status: WorktreeStatus::default(),
            current_branch: Some(BranchName::new("main").unwrap()),
            branches,
            metadata: HashMap::new(),
            repo_config: None,
            trunk: Some(BranchName::new("main").unwrap()),
            graph: StackGraph::new(),
            fingerprint: Fingerprint::compute(&[]),
            health,
        }
    }

    fn snapshot_with_issues() -> RepoSnapshot {
        let mut snapshot = minimal_snapshot();
        snapshot.health.add_issue(issues::trunk_not_configured());
        snapshot
    }

    mod doctor {
        use super::*;

        #[test]
        fn new_defaults_to_non_interactive() {
            let doctor = Doctor::new();
            assert!(!doctor.is_interactive());
        }

        #[test]
        fn interactive_mode() {
            let doctor = Doctor::new().interactive(true);
            assert!(doctor.is_interactive());
        }

        #[test]
        fn diagnose_healthy_repo() {
            let doctor = Doctor::new();
            let snapshot = minimal_snapshot();

            let diagnosis = doctor.diagnose(&snapshot);

            assert!(diagnosis.is_healthy());
            assert!(!diagnosis.has_blocking_issues());
            assert_eq!(diagnosis.summary.issue_count, 0);
        }

        #[test]
        fn diagnose_with_issues() {
            let doctor = Doctor::new();
            let snapshot = snapshot_with_issues();

            let diagnosis = doctor.diagnose(&snapshot);

            assert!(!diagnosis.is_healthy());
            assert!(diagnosis.has_blocking_issues());
            assert_eq!(diagnosis.summary.blocking_count, 1);
        }

        #[test]
        fn diagnose_generates_fixes() {
            let doctor = Doctor::new();
            let snapshot = snapshot_with_issues();

            let diagnosis = doctor.diagnose(&snapshot);

            // trunk-not-configured should have fix options
            assert!(!diagnosis.fixes.is_empty());
        }

        #[test]
        fn plan_repairs_requires_fixes() {
            let doctor = Doctor::new();
            let snapshot = snapshot_with_issues();
            let diagnosis = doctor.diagnose(&snapshot);

            let result = doctor.plan_repairs(&[], &diagnosis, &snapshot);

            assert!(matches!(result, Err(DoctorError::NoFixesSelected)));
        }

        #[test]
        fn plan_repairs_validates_fix_ids() {
            let doctor = Doctor::new();
            let snapshot = snapshot_with_issues();
            let diagnosis = doctor.diagnose(&snapshot);

            let result =
                doctor.plan_repairs(&[FixId::parse("nonexistent:fix")], &diagnosis, &snapshot);

            assert!(matches!(result, Err(DoctorError::FixNotFound(_))));
        }
    }

    mod diagnosis_report {
        use super::*;

        #[test]
        fn fixes_for_issue() {
            let doctor = Doctor::new();
            let snapshot = snapshot_with_issues();
            let diagnosis = doctor.diagnose(&snapshot);

            let issue_id = IssueId::singleton("trunk-not-configured");
            let fixes = diagnosis.fixes_for_issue(&issue_id);

            assert!(!fixes.is_empty());
        }

        #[test]
        fn format_healthy() {
            let diagnosis = DiagnosisReport {
                issues: vec![],
                fixes: vec![],
                summary: DiagnosisSummary::default(),
            };

            let formatted = diagnosis.format();
            assert!(formatted.contains("healthy"));
        }

        #[test]
        fn format_with_issues() {
            let doctor = Doctor::new();
            let snapshot = snapshot_with_issues();
            let diagnosis = doctor.diagnose(&snapshot);

            let formatted = diagnosis.format();
            assert!(formatted.contains("ERROR"));
            assert!(formatted.contains("trunk"));
        }
    }

    mod repair_outcome {
        use super::*;

        #[test]
        fn success() {
            let plan = Plan::new(crate::core::ops::journal::OpId::new(), "doctor");
            let outcome = RepairOutcome::success(vec![], plan);

            assert!(outcome.success);
            assert!(outcome.error.is_none());
        }

        #[test]
        fn failure() {
            let outcome = RepairOutcome::failure("something went wrong");

            assert!(!outcome.success);
            assert!(outcome.error.is_some());
        }
    }
}
