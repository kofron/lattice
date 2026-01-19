//! doctor::planner
//!
//! Repair plan generation for the Doctor framework.
//!
//! # Architecture
//!
//! The repair planner converts selected `FixOption`s into executable `Plan`s.
//! It uses the same plan infrastructure as regular commands, ensuring that
//! repairs go through the same transactional executor.
//!
//! Per ARCHITECTURE.md Section 8.1, there is no separate "repair mutation path."
//! Doctor uses the standard planner and executor.

use thiserror::Error;

use crate::core::ops::journal::OpId;
use crate::engine::plan::{Plan, PlanStep};
use crate::engine::scan::RepoSnapshot;

use super::fixes::{ConfigChange, FixOption, MetadataChange, RefChange};

/// Context for snapshot branch creation.
///
/// Used to pass information about the synthetic head branch when converting
/// snapshot-related RefChanges to PlanSteps.
#[derive(Debug, Clone, Default)]
struct SnapshotContext {
    /// The synthetic head branch these snapshots belong to.
    head_branch: String,
    /// Current OID of the head branch (for ancestry validation).
    head_oid: String,
}

/// Errors from repair plan generation.
#[derive(Debug, Error)]
pub enum RepairPlanError {
    /// Fix preconditions not met.
    #[error("fix preconditions not met: {0}")]
    PreconditionsNotMet(String),

    /// Fix cannot be converted to a plan.
    #[error("cannot generate plan for fix: {0}")]
    CannotGeneratePlan(String),

    /// Conflicting fixes selected.
    #[error("conflicting fixes: {0}")]
    ConflictingFixes(String),
}

/// Generate a repair plan from selected fix options.
///
/// This converts the abstract fix options into concrete plan steps that
/// can be executed by the standard executor.
///
/// # Arguments
///
/// * `fixes` - The fix options to apply (in order)
/// * `snapshot` - Current repository state for context
///
/// # Returns
///
/// A `Plan` containing all the steps needed to apply the fixes.
///
/// # Errors
///
/// Returns an error if:
/// - Fix preconditions are not met
/// - Fixes conflict with each other
/// - A fix cannot be converted to plan steps
pub fn generate_repair_plan(
    fixes: &[&FixOption],
    snapshot: &RepoSnapshot,
) -> Result<Plan, RepairPlanError> {
    if fixes.is_empty() {
        return Ok(Plan::new(OpId::new(), "doctor"));
    }

    // Verify preconditions for all fixes
    let caps = snapshot.health.capabilities();
    for fix in fixes {
        if !fix.preconditions_satisfied(caps) {
            let missing: Vec<_> = fix
                .preconditions
                .iter()
                .filter(|c| !caps.has(c))
                .map(|c| c.description())
                .collect();
            return Err(RepairPlanError::PreconditionsNotMet(format!(
                "fix '{}' requires: {}",
                fix.id,
                missing.join(", ")
            )));
        }
    }

    // Build the plan
    let mut plan = Plan::new(OpId::new(), "doctor");

    // Add a checkpoint at the start
    plan = plan.with_step(PlanStep::Checkpoint {
        name: "doctor-start".to_string(),
    });

    // Convert each fix to plan steps
    for fix in fixes {
        plan = add_fix_steps(plan, fix, snapshot)?;
    }

    // Add a final checkpoint
    plan = plan.with_step(PlanStep::Checkpoint {
        name: "doctor-complete".to_string(),
    });

    Ok(plan)
}

/// Add steps for a single fix to the plan.
fn add_fix_steps(
    mut plan: Plan,
    fix: &FixOption,
    snapshot: &RepoSnapshot,
) -> Result<Plan, RepairPlanError> {
    // Add a marker for this fix
    plan = plan.with_step(PlanStep::Checkpoint {
        name: format!("fix:{}", fix.id),
    });

    // Build snapshot context if this is a synthetic stack fix
    let context = build_snapshot_context(fix, snapshot);

    // Convert preview changes to plan steps
    // For snapshot branches, we need to add a fetch step before each CreateSnapshotBranch
    for change in &fix.preview.ref_changes {
        let step = ref_change_to_step(change, &context);

        // If this is a CreateSnapshotBranch, we need to add a fetch step first
        if let PlanStep::CreateSnapshotBranch { pr_number, .. } = &step {
            // Add the fetch step to get the PR ref
            plan = plan.with_step(PlanStep::RunGit {
                args: vec![
                    "fetch".to_string(),
                    "origin".to_string(),
                    format!("refs/pull/{}/head", pr_number),
                ],
                description: format!("Fetch PR #{} head ref", pr_number),
                expected_effects: vec![], // FETCH_HEAD is updated
            });
        }

        plan = plan.with_step(step);
    }

    for change in &fix.preview.metadata_changes {
        plan = plan.with_step(metadata_change_to_step(change, snapshot)?);
    }

    for change in &fix.preview.config_changes {
        plan = plan.with_step(config_change_to_step(change)?);
    }

    Ok(plan)
}

/// Build snapshot context from a fix's metadata description.
///
/// For synthetic stack materialization fixes, this extracts the head branch
/// and its OID from the fix preview's metadata changes.
fn build_snapshot_context(fix: &FixOption, snapshot: &RepoSnapshot) -> SnapshotContext {
    // Check if this is a synthetic stack head fix
    if !fix.id.as_str().contains("synthetic-stack-head") {
        return SnapshotContext::default();
    }

    // Extract head branch from the metadata changes description
    // Format: "parent=<head_branch>, frozen (remote_synthetic_snapshot), pr=#<num>"
    for change in &fix.preview.metadata_changes {
        if let MetadataChange::Create { description, .. } = change {
            if let Some(start) = description.find("parent=") {
                let rest = &description[start + 7..];
                if let Some(end) = rest.find(',') {
                    let head_branch = rest[..end].trim().to_string();

                    // Get the head branch's current OID
                    if let Ok(branch_name) = crate::core::types::BranchName::new(&head_branch) {
                        if let Some(oid) = snapshot.branches.get(&branch_name) {
                            return SnapshotContext {
                                head_branch,
                                head_oid: oid.as_str().to_string(),
                            };
                        }
                    }

                    // Even without the OID, return the branch name
                    // The executor will need to look it up
                    return SnapshotContext {
                        head_branch,
                        head_oid: String::new(),
                    };
                }
            }
        }
    }

    SnapshotContext::default()
}

/// Convert a RefChange to a PlanStep.
fn ref_change_to_step(change: &RefChange, context: &SnapshotContext) -> PlanStep {
    match change {
        RefChange::Create { ref_name, new_oid } => {
            // Check for snapshot branch creation (Milestone 5.9)
            // Format: "(fetched from PR #<number>)"
            if let Some(pr_num_str) = new_oid
                .strip_prefix("(fetched from PR #")
                .and_then(|s| s.strip_suffix(')'))
            {
                if let Ok(pr_number) = pr_num_str.parse::<u64>() {
                    // This is a snapshot branch - generate fetch + CreateSnapshotBranch steps
                    // The caller will need to handle this specially since we return one step
                    // but actually need two. We use a marker approach here.
                    let branch_name = ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name);

                    // Return a marker step. The caller (add_fix_steps) will expand this.
                    return PlanStep::CreateSnapshotBranch {
                        branch_name: branch_name.to_string(),
                        pr_number,
                        head_branch: context.head_branch.clone(),
                        head_oid: context.head_oid.clone(),
                    };
                }
            }

            // Check for the special placeholder indicating a fetch is needed
            if new_oid == "(fetched from remote)" {
                // Extract branch name from ref_name (e.g., "refs/heads/feature" -> "feature")
                let branch = ref_name.strip_prefix("refs/heads/").unwrap_or(ref_name);

                // Generate a git fetch command that creates the local branch
                PlanStep::RunGit {
                    args: vec![
                        "fetch".to_string(),
                        "origin".to_string(),
                        format!("{}:{}", branch, ref_name),
                    ],
                    description: format!("Fetch '{}' from origin", branch),
                    expected_effects: vec![ref_name.clone()],
                }
            } else {
                PlanStep::UpdateRefCas {
                    refname: ref_name.clone(),
                    old_oid: None,
                    new_oid: new_oid.clone(),
                    reason: "doctor: create ref".to_string(),
                }
            }
        }
        RefChange::Update {
            ref_name,
            old_oid,
            new_oid,
        } => PlanStep::UpdateRefCas {
            refname: ref_name.clone(),
            old_oid: old_oid.clone(),
            new_oid: new_oid.clone(),
            reason: "doctor: update ref".to_string(),
        },
        RefChange::Delete { ref_name, old_oid } => PlanStep::DeleteRefCas {
            refname: ref_name.clone(),
            old_oid: old_oid.clone(),
            reason: "doctor: delete ref".to_string(),
        },
    }
}

/// Convert a MetadataChange to a PlanStep.
fn metadata_change_to_step(
    change: &MetadataChange,
    snapshot: &RepoSnapshot,
) -> Result<PlanStep, RepairPlanError> {
    match change {
        MetadataChange::Create {
            branch,
            description,
        } => {
            // Parse the description to extract parent, frozen, and PR info.
            // Format: "parent=<name>, pr=#<num>, unfrozen|frozen (reason)"
            let (parent_name, frozen, pr_info) = parse_create_description(description, snapshot);

            let metadata =
                create_minimal_metadata(branch, &parent_name, snapshot, frozen, pr_info)?;

            Ok(PlanStep::WriteMetadataCas {
                branch: branch.clone(),
                old_ref_oid: None,
                metadata: Box::new(metadata),
            })
        }
        MetadataChange::Update {
            branch,
            field,
            new_value,
            ..
        } => {
            // Get existing metadata and update it
            if let Ok(branch_name) = crate::core::types::BranchName::new(branch) {
                if let Some(scanned) = snapshot.metadata.get(&branch_name) {
                    let mut metadata = scanned.metadata.clone();

                    // Update the specified field
                    match field.as_str() {
                        "parent" => {
                            use crate::core::metadata::schema::ParentInfo;
                            metadata.parent = ParentInfo::Branch {
                                name: new_value.clone(),
                            };
                        }
                        "base" => {
                            use crate::core::metadata::schema::BaseInfo;
                            metadata.base = BaseInfo {
                                oid: new_value.clone(),
                            };
                        }
                        "pr" => {
                            // Parse PR linkage from new_value format: "linked(#42)"
                            use crate::core::metadata::schema::PrState;
                            if let Some(num_str) = new_value
                                .strip_prefix("linked(#")
                                .and_then(|s| s.strip_suffix(')'))
                            {
                                if let Ok(number) = num_str.parse::<u64>() {
                                    // Use "github" as default forge for now
                                    // URL can be empty - it's a cached field
                                    metadata.pr = PrState::linked("github", number, "");
                                }
                            }
                        }
                        _ => {
                            return Err(RepairPlanError::CannotGeneratePlan(format!(
                                "unknown metadata field: {}",
                                field
                            )));
                        }
                    }

                    // Update timestamp
                    metadata.touch();

                    return Ok(PlanStep::WriteMetadataCas {
                        branch: branch.clone(),
                        old_ref_oid: Some(scanned.ref_oid.as_str().to_string()),
                        metadata: Box::new(metadata),
                    });
                }
            }

            Err(RepairPlanError::CannotGeneratePlan(format!(
                "cannot update metadata for non-existent branch: {}",
                branch
            )))
        }
        MetadataChange::Delete { branch } => {
            // Get the current metadata ref OID for CAS
            if let Ok(branch_name) = crate::core::types::BranchName::new(branch) {
                if let Some(scanned) = snapshot.metadata.get(&branch_name) {
                    return Ok(PlanStep::DeleteMetadataCas {
                        branch: branch.clone(),
                        old_ref_oid: scanned.ref_oid.as_str().to_string(),
                    });
                }
            }

            // If metadata doesn't exist in snapshot, we still try to delete
            // (it might be corrupted/unparseable)
            Ok(PlanStep::DeleteMetadataCas {
                branch: branch.clone(),
                old_ref_oid: "(unknown)".to_string(),
            })
        }
    }
}

/// Convert a ConfigChange to a PlanStep.
fn config_change_to_step(change: &ConfigChange) -> Result<PlanStep, RepairPlanError> {
    // Config changes are handled specially - they're not ref updates
    // For now, we represent them as RunGit steps that would invoke lattice config
    match change {
        ConfigChange::Set { key, value } => Ok(PlanStep::RunGit {
            args: vec![
                "config".to_string(),
                "set".to_string(),
                key.clone(),
                value.clone(),
            ],
            description: format!("Set config {} = {}", key, value),
            expected_effects: vec![],
        }),
        ConfigChange::Remove { key } => Ok(PlanStep::RunGit {
            args: vec!["config".to_string(), "unset".to_string(), key.clone()],
            description: format!("Remove config {}", key),
            expected_effects: vec![],
        }),
        ConfigChange::Migrate { from, to } => Ok(PlanStep::RunGit {
            args: vec![
                "mv".to_string(), // Would be a file move
                from.clone(),
                to.clone(),
            ],
            description: format!("Migrate config {} -> {}", from, to),
            expected_effects: vec![],
        }),
    }
}

/// Parse a MetadataChange::Create description to extract parent, frozen, and PR info.
///
/// Expected formats:
/// - "parent=main, pr=#42, unfrozen"
/// - "parent=feature-a, pr=#42, frozen (teammate_branch)"
///
/// Returns (parent_name, frozen, pr_info) where pr_info is Option<(&str, u64, &str)>.
fn parse_create_description<'a>(
    description: &str,
    snapshot: &'a RepoSnapshot,
) -> (String, bool, Option<(&'a str, u64, &'a str)>) {
    let mut parent_name = snapshot
        .trunk
        .as_ref()
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|| "main".to_string());
    let mut pr_number: Option<u64> = None;

    // Parse "parent=<name>"
    if let Some(start) = description.find("parent=") {
        let rest = &description[start + 7..];
        if let Some(end) = rest.find(',') {
            parent_name = rest[..end].trim().to_string();
        } else {
            parent_name = rest.trim().to_string();
        }
    }

    // Parse "frozen" or "unfrozen"
    // frozen if contains "frozen (" (with reason) or ends with "frozen", but not "unfrozen"
    let frozen = (description.contains("frozen (") || description.ends_with("frozen"))
        && !description.contains("unfrozen");

    // Parse "pr=#<num>"
    if let Some(start) = description.find("pr=#") {
        let rest = &description[start + 4..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(num) = num_str.parse() {
            pr_number = Some(num);
        }
    }

    // For PR info, we need forge and URL. Use defaults for bootstrap.
    // The URL is empty because we don't have it in the description.
    let pr_info = pr_number.map(|num| ("github", num, ""));

    (parent_name, frozen, pr_info)
}

/// Create minimal metadata for a new branch tracking.
///
/// This creates metadata with the parent's tip as the base. For bootstrap
/// fixes, this is acceptable because:
/// 1. The branch is being tracked for the first time
/// 2. The true merge-base will be computed during sync/rebase operations
/// 3. The stack graph will be valid for dependency tracking
///
/// # Arguments
///
/// * `branch` - Branch name to track
/// * `parent_name` - Parent branch name
/// * `snapshot` - Repository snapshot for looking up branch OIDs
/// * `frozen` - Whether to create as frozen (teammate branch)
/// * `pr_info` - Optional (forge, pr_number, url) for PR linkage
fn create_minimal_metadata(
    branch: &str,
    parent_name: &str,
    snapshot: &RepoSnapshot,
    frozen: bool,
    pr_info: Option<(&str, u64, &str)>,
) -> Result<crate::core::metadata::schema::BranchMetadataV1, RepairPlanError> {
    use crate::core::metadata::schema::{BranchMetadataV1, FreezeState, PrState};
    use crate::core::types::{BranchName, Oid};

    // Validate branch name
    let branch_name = BranchName::new(branch).map_err(|e| {
        RepairPlanError::CannotGeneratePlan(format!("invalid branch name '{}': {}", branch, e))
    })?;

    // Validate parent name
    let parent_branch = BranchName::new(parent_name).map_err(|e| {
        RepairPlanError::CannotGeneratePlan(format!("invalid parent name '{}': {}", parent_name, e))
    })?;

    // Get the parent's tip as the base
    // Note: For strict correctness, we'd compute merge-base here, but that
    // requires Git access which the planner doesn't have. The base will be
    // refined during sync/rebase operations.
    let base_oid_str = snapshot
        .branches
        .get(&parent_branch)
        .map(|o| o.as_str().to_string())
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());

    let base_oid = Oid::new(&base_oid_str)
        .map_err(|e| RepairPlanError::CannotGeneratePlan(format!("invalid base oid: {}", e)))?;

    // Build metadata with appropriate freeze and PR states
    let mut builder = BranchMetadataV1::builder(branch_name, parent_branch, base_oid);

    // Check if parent is trunk
    if let Some(trunk) = &snapshot.trunk {
        if trunk.as_str() == parent_name {
            builder = builder.parent_is_trunk();
        }
    }

    // Set freeze state
    if frozen {
        use crate::core::metadata::schema::FreezeScope;
        builder = builder.freeze_state(FreezeState::frozen(
            FreezeScope::Single,
            Some("teammate_branch".to_string()),
        ));
    }

    // Set PR state if provided
    if let Some((forge, number, url)) = pr_info {
        builder = builder.pr_state(PrState::linked(forge, number, url));
    }

    Ok(builder.build())
}

/// Combine multiple repair plans into one.
///
/// This is useful when multiple fixes need to be applied together.
#[allow(dead_code)] // Will be used when Doctor applies multiple fixes
pub fn combine_plans(plans: Vec<Plan>) -> Plan {
    if plans.is_empty() {
        return Plan::new(OpId::new(), "doctor");
    }

    let mut combined = Plan::new(OpId::new(), "doctor");

    for plan in plans {
        for step in plan.steps {
            combined = combined.with_step(step);
        }
    }

    combined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::graph::StackGraph;
    use crate::core::types::{BranchName, Fingerprint, Oid};
    use crate::doctor::fixes::{FixId, FixPreview};
    use crate::engine::capabilities::Capability;
    use crate::engine::health::{IssueId, RepoHealthReport};
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
                common_dir: PathBuf::from(".git"),
                work_dir: Some(PathBuf::from(".")),
                context: crate::git::RepoContext::Normal,
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
            remote_prs: None,
        }
    }

    #[test]
    fn empty_fixes_produces_empty_plan() {
        let snapshot = minimal_snapshot();
        let fixes: Vec<&FixOption> = vec![];

        let plan = generate_repair_plan(&fixes, &snapshot).unwrap();

        assert!(plan.is_empty());
    }

    #[test]
    fn single_fix_produces_plan() {
        let snapshot = minimal_snapshot();

        let fix = FixOption::new(
            FixId::simple("test", "fix"),
            IssueId::singleton("test"),
            "Test fix",
            FixPreview::with_summary("Test").add_ref_change(RefChange::Delete {
                ref_name: "refs/test".to_string(),
                old_oid: "abc123".to_string(),
            }),
        )
        .with_precondition(Capability::RepoOpen);

        let fixes = vec![&fix];
        let plan = generate_repair_plan(&fixes, &snapshot).unwrap();

        assert!(!plan.is_empty());
        assert!(plan.has_mutations());
    }

    #[test]
    fn preconditions_checked() {
        let snapshot = minimal_snapshot();

        let fix = FixOption::new(
            FixId::simple("test", "fix"),
            IssueId::singleton("test"),
            "Test fix",
            FixPreview::new(),
        )
        .with_precondition(Capability::AuthAvailable); // Not in snapshot

        let fixes = vec![&fix];
        let result = generate_repair_plan(&fixes, &snapshot);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RepairPlanError::PreconditionsNotMet(_)
        ));
    }

    #[test]
    fn ref_change_to_step_create() {
        let change = RefChange::Create {
            ref_name: "refs/heads/new".to_string(),
            new_oid: "abc123".to_string(),
        };

        let context = SnapshotContext::default();
        let step = ref_change_to_step(&change, &context);

        match step {
            PlanStep::UpdateRefCas {
                refname,
                old_oid,
                new_oid,
                ..
            } => {
                assert_eq!(refname, "refs/heads/new");
                assert!(old_oid.is_none());
                assert_eq!(new_oid, "abc123");
            }
            _ => panic!("expected UpdateRefCas"),
        }
    }

    #[test]
    fn ref_change_to_step_delete() {
        let change = RefChange::Delete {
            ref_name: "refs/heads/old".to_string(),
            old_oid: "abc123".to_string(),
        };

        let context = SnapshotContext::default();
        let step = ref_change_to_step(&change, &context);

        match step {
            PlanStep::DeleteRefCas {
                refname, old_oid, ..
            } => {
                assert_eq!(refname, "refs/heads/old");
                assert_eq!(old_oid, "abc123");
            }
            _ => panic!("expected DeleteRefCas"),
        }
    }

    #[test]
    fn ref_change_to_step_snapshot_branch() {
        let change = RefChange::Create {
            ref_name: "refs/heads/lattice/snap/pr-42".to_string(),
            new_oid: "(fetched from PR #42)".to_string(),
        };

        let context = SnapshotContext {
            head_branch: "feature".to_string(),
            head_oid: "abc123def4567890abc123def4567890abc12345".to_string(),
        };
        let step = ref_change_to_step(&change, &context);

        match step {
            PlanStep::CreateSnapshotBranch {
                branch_name,
                pr_number,
                head_branch,
                head_oid,
            } => {
                assert_eq!(branch_name, "lattice/snap/pr-42");
                assert_eq!(pr_number, 42);
                assert_eq!(head_branch, "feature");
                assert_eq!(head_oid, "abc123def4567890abc123def4567890abc12345");
            }
            _ => panic!("expected CreateSnapshotBranch"),
        }
    }

    #[test]
    fn combine_empty_plans() {
        let combined = combine_plans(vec![]);
        assert!(combined.is_empty());
    }

    #[test]
    fn combine_multiple_plans() {
        let plan1 = Plan::new(OpId::new(), "a").with_step(PlanStep::Checkpoint {
            name: "one".to_string(),
        });
        let plan2 = Plan::new(OpId::new(), "b").with_step(PlanStep::Checkpoint {
            name: "two".to_string(),
        });

        let combined = combine_plans(vec![plan1, plan2]);

        assert_eq!(combined.step_count(), 2);
    }
}
