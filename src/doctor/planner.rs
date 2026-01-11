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

    // Convert preview changes to plan steps
    for change in &fix.preview.ref_changes {
        plan = plan.with_step(ref_change_to_step(change));
    }

    for change in &fix.preview.metadata_changes {
        plan = plan.with_step(metadata_change_to_step(change, snapshot)?);
    }

    for change in &fix.preview.config_changes {
        plan = plan.with_step(config_change_to_step(change)?);
    }

    Ok(plan)
}

/// Convert a RefChange to a PlanStep.
fn ref_change_to_step(change: &RefChange) -> PlanStep {
    match change {
        RefChange::Create { ref_name, new_oid } => PlanStep::UpdateRefCas {
            refname: ref_name.clone(),
            old_oid: None,
            new_oid: new_oid.clone(),
            reason: "doctor: create ref".to_string(),
        },
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
            description: _,
        } => {
            // Create new metadata for the branch
            // For now, create minimal metadata with trunk as parent
            let parent_name = snapshot
                .trunk
                .as_ref()
                .map(|t| t.as_str())
                .unwrap_or("main");

            let metadata = create_minimal_metadata(branch, parent_name, snapshot)?;

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

/// Create minimal metadata for a new branch tracking.
fn create_minimal_metadata(
    branch: &str,
    parent_name: &str,
    snapshot: &RepoSnapshot,
) -> Result<crate::core::metadata::schema::BranchMetadataV1, RepairPlanError> {
    use crate::core::metadata::schema::BranchMetadataV1;
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
    let base_oid_str = snapshot
        .branches
        .get(&parent_branch)
        .map(|o| o.as_str().to_string())
        .unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());

    let base_oid = Oid::new(&base_oid_str)
        .map_err(|e| RepairPlanError::CannotGeneratePlan(format!("invalid base oid: {}", e)))?;

    // Use the constructor to create properly structured metadata
    Ok(BranchMetadataV1::new(branch_name, parent_branch, base_oid))
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

        let step = ref_change_to_step(&change);

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

        let step = ref_change_to_step(&change);

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
