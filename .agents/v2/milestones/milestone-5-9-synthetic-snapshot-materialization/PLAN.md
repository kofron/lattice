# Milestone 5.9: Synthetic Stack Snapshot Materialization (Opt-in)

## Goal

Create frozen snapshot branches for synthetic stack context, enabling users to explore the historical structure of work that was merged into a trunk-targeting PR before it landed.

**Core principle from ARCHITECTURE.md Section 8.1:** "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

---

## Background

A **synthetic remote stack** (detected in Milestone 5.8) exists when:
- An open PR P0 targets trunk with head branch H
- Closed/merged PRs exist whose base branch was H

The **interpretation** is that prior reviewed work was merged into H while P0 remains open. This is useful context but not automatically reconstructable as a Lattice stack - the commits have already been squash-merged or rebased.

Milestone 5.9 provides an **opt-in** mechanism to materialize snapshots of these closed PRs as frozen branches, allowing users to:
- Inspect the historical structure of work
- See which PRs contributed to the current state
- Navigate the synthetic stack for context

| Milestone | Component | Status |
|-----------|-----------|--------|
| 5.2 | `list_open_prs` capability | Complete |
| 5.3 | Bootstrap issue detection | Complete |
| 5.4 | Bootstrap fix generators (remote) | Complete |
| 5.5 | Bootstrap fix execution | Complete |
| 5.6 | Init hint for bootstrap | Complete |
| 5.7 | Local-only bootstrap | Complete |
| 5.8 | Synthetic stack detection | Complete |
| **5.9** | **Synthetic snapshot materialization** | **This milestone** |

---

## Spec References

- **ROADMAP.md Milestone 5.9** - Snapshot materialization deliverables
- **ARCHITECTURE.md Section 8** - Doctor framework (fix options, confirmation model)
- **ARCHITECTURE.md Section 6.2** - Executor contract (CAS semantics)
- **ARCHITECTURE.md Section 3.2** - Branch metadata refs
- **SPEC.md Section 8E.1** - Forge trait definition
- **SPEC.md Section 8B.1** - Track command and freeze behavior

---

## Design Decisions

### Branch Storage: `refs/heads/` with Reserved Prefix

Per ROADMAP.md design decision, snapshot branches are stored as normal branches:

**Naming scheme:**
- Branch name: `lattice/snap/pr-<number>`
- Stored as: `refs/heads/lattice/snap/pr-123`
- Collision avoidance: append `-<k>` suffix if name exists (e.g., `lattice/snap/pr-123-1`)

**Rationale (per ROADMAP.md):**
1. Current model assumes tracked branches = `refs/heads/*`
2. Custom namespace would expand architectural scope significantly
3. Normal branches work with existing checkout/navigation/scanner
4. Users can inspect with normal git tools
5. Reserved prefix `lattice/snap/` clearly identifies synthetic snapshots

### Strict All-or-Nothing Execution

**Safety rule:** If any requested snapshot cannot be fetched or validated, the entire fix MUST fail and rollback. No partial application is allowed.

This protects against:
- Orphan branches without metadata
- Incomplete snapshot sets that could confuse users
- Race conditions where some PR refs are available but others have been garbage collected

### Validation: Commit Reachability

A snapshot commit is valid if it is **reachable from the current head H** via commit ancestry.

**Why this matters:**
- GitHub PR refs (`refs/pull/{number}/head`) may point to commits that were rebased away
- Stale PR refs could create misleading branches
- Validation ensures the snapshot represents work that actually contributed to H

**Validation algorithm:**
```
is_valid(snapshot_commit, head_commit) ->
    git merge-base --is-ancestor snapshot_commit head_commit
```

### Freeze Reason: `remote_synthetic_snapshot`

Add a new freeze reason to distinguish synthetic snapshots from normal frozen branches:

```rust
FreezeState::Frozen {
    scope: FreezeScope::Single,
    reason: Some("remote_synthetic_snapshot".to_string()),
    frozen_at: UtcTimestamp::now(),
}
```

This enables:
- Milestone 5.10: Submit scope exclusion for snapshot branches
- Clear UX messaging about why the branch is frozen
- Future tooling to filter/manage snapshot branches

### PR Linkage State: Closed/Merged

Snapshot branches represent closed/merged PRs. The metadata should reflect this:

```rust
PrState::Linked {
    forge: "github",
    number: pr_number,
    url: pr_url,
    last_known: Some(PrStatusCache {
        state: "merged", // or "closed"
        is_draft: false,
    }),
}
```

### Parent Selection: Synthetic Head Branch

All snapshot branches have the same parent: the synthetic stack head branch H.

```
parent = H (the open PR head branch targeting trunk)
```

This creates a flat structure under H, reflecting that all the merged PRs contributed to H but their original stacking relationship has been lost (due to squash-merge).

### Base Computation: Merge-Base

Per the established pattern from Milestone 5.4:

```
base = merge-base(snapshot_tip, H_tip)
```

This is always valid since we've already verified the snapshot commit is an ancestor of H.

---

## Implementation Steps

### Step 1: Add Fix Option Type for Snapshot Materialization

**File:** `src/doctor/fixes.rs`

Add a new fix option variant for materializing synthetic snapshots:

```rust
/// Fix options for bootstrap issues.
#[derive(Debug, Clone)]
pub enum BootstrapFix {
    // ... existing variants ...

    /// Materialize frozen snapshot branches for synthetic stack children.
    ///
    /// This creates local branches from PR refs of closed PRs that were
    /// merged into a synthetic stack head.
    MaterializeSyntheticSnapshots {
        /// The synthetic head branch (open PR targeting trunk).
        head_branch: String,
        /// The closed PRs to materialize as snapshots.
        closed_prs: Vec<ClosedPrToMaterialize>,
    },
}

/// Information about a closed PR to materialize as a snapshot.
#[derive(Debug, Clone)]
pub struct ClosedPrToMaterialize {
    /// PR number.
    pub number: u64,
    /// Head ref of the closed PR (for fetch).
    pub head_ref: String,
    /// PR URL.
    pub url: String,
    /// Whether the PR was merged (vs just closed).
    pub merged: bool,
}
```

### Step 2: Add Freeze Reason Constant

**File:** `src/core/metadata/schema.rs`

Add a constant for the freeze reason (for documentation and consistency):

```rust
/// Freeze reason for synthetic snapshot branches created from closed PRs.
///
/// These branches are always frozen because they represent historical state
/// that should not be modified.
pub const FREEZE_REASON_SYNTHETIC_SNAPSHOT: &str = "remote_synthetic_snapshot";
```

### Step 3: Add GitHub PR Ref Fetch to Git Interface

**File:** `src/git/interface.rs`

GitHub exposes PR head refs at `refs/pull/{number}/head`. Add a method to fetch these:

```rust
impl Git {
    /// Fetch a GitHub pull request ref.
    ///
    /// This fetches `refs/pull/{number}/head` from the remote and returns
    /// the commit OID it points to.
    ///
    /// # Arguments
    ///
    /// * `remote` - The remote name (usually "origin")
    /// * `pr_number` - The PR number to fetch
    ///
    /// # Returns
    ///
    /// The OID of the PR's head commit.
    ///
    /// # Errors
    ///
    /// - `GitError::RefNotFound` if the PR ref doesn't exist (PR deleted/force-pushed)
    /// - `GitError::NetworkError` if fetch fails
    pub fn fetch_pr_ref(&self, remote: &str, pr_number: u64) -> Result<Oid, GitError> {
        let refspec = format!("refs/pull/{}/head", pr_number);
        
        // Fetch the PR ref to FETCH_HEAD
        let result = self.run_command(&[
            "fetch".to_string(),
            remote.to_string(),
            refspec.clone(),
        ])?;
        
        if !result.success {
            if result.stderr.contains("couldn't find remote ref") {
                return Err(GitError::RefNotFound { refname: refspec });
            }
            return Err(GitError::Internal {
                message: format!("fetch failed: {}", result.stderr),
            });
        }
        
        // Read FETCH_HEAD to get the commit OID
        let fetch_head = self.resolve_ref("FETCH_HEAD")?;
        Ok(fetch_head)
    }

    /// Check if a commit is an ancestor of another commit.
    ///
    /// This uses `git merge-base --is-ancestor` to check if `ancestor`
    /// is reachable from `descendant`.
    ///
    /// # Arguments
    ///
    /// * `ancestor` - The potential ancestor commit
    /// * `descendant` - The potential descendant commit
    ///
    /// # Returns
    ///
    /// `true` if `ancestor` is an ancestor of `descendant` (or they're equal).
    pub fn is_ancestor(&self, ancestor: &Oid, descendant: &Oid) -> Result<bool, GitError> {
        let result = self.run_command(&[
            "merge-base".to_string(),
            "--is-ancestor".to_string(),
            ancestor.to_string(),
            descendant.to_string(),
        ])?;
        
        // Exit code 0 = is ancestor, 1 = not ancestor, other = error
        Ok(result.exit_code == 0)
    }
}
```

### Step 4: Add Snapshot Branch Name Generator

**File:** `src/doctor/generators.rs`

Add a function to generate collision-free snapshot branch names:

```rust
use crate::engine::scan::RepoSnapshot;

/// Reserved prefix for synthetic snapshot branches.
pub const SNAPSHOT_PREFIX: &str = "lattice/snap/pr-";

/// Generate a unique snapshot branch name for a PR.
///
/// Uses the pattern `lattice/snap/pr-{number}` with collision avoidance
/// by appending `-{k}` suffixes if the name already exists.
///
/// # Arguments
///
/// * `pr_number` - The PR number
/// * `snapshot` - Current repo snapshot to check for existing branches
///
/// # Returns
///
/// A unique branch name for the snapshot.
pub fn snapshot_branch_name(pr_number: u64, snapshot: &RepoSnapshot) -> String {
    let base_name = format!("{}{}", SNAPSHOT_PREFIX, pr_number);
    
    // Check if base name is available
    if !branch_exists(&base_name, snapshot) {
        return base_name;
    }
    
    // Find first available suffix
    for k in 1..100 {
        let name = format!("{}-{}", base_name, k);
        if !branch_exists(&name, snapshot) {
            return name;
        }
    }
    
    // Fallback with timestamp (extremely unlikely to reach here)
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("{}-{}", base_name, ts)
}

fn branch_exists(name: &str, snapshot: &RepoSnapshot) -> bool {
    snapshot.branches.keys().any(|b| b.as_str() == name)
}
```

### Step 5: Add Snapshot Materialization Fix Generator

**File:** `src/doctor/generators.rs`

Add the fix generator for materializing snapshots:

```rust
use crate::core::config::schema::DoctorBootstrapConfig;
use crate::engine::health::{ClosedPrInfo, Evidence, Issue};
use crate::doctor::fixes::{BootstrapFix, ClosedPrToMaterialize, FixOption, FixId};
use crate::forge::Forge;

/// Generate fix options for materializing synthetic stack snapshots.
///
/// This is triggered when:
/// 1. A `PotentialSyntheticStackHead` issue exists with Tier 2 evidence
/// 2. The evidence contains `SyntheticStackChildren` with closed PRs
/// 3. User has confirmed they want to materialize snapshots
///
/// # Arguments
///
/// * `issue` - The synthetic stack head issue with Tier 2 evidence
///
/// # Returns
///
/// A fix option for materializing the snapshots, or None if no valid
/// closed PRs are found in the evidence.
pub fn generate_materialize_snapshot_fix(issue: &Issue) -> Option<FixOption> {
    // Extract the synthetic head branch from the issue
    let head_branch = extract_synthetic_head_branch(issue)?;
    
    // Find SyntheticStackChildren evidence
    let closed_prs = issue.evidence.iter().find_map(|ev| {
        if let Evidence::SyntheticStackChildren { closed_prs, .. } = ev {
            Some(closed_prs.clone())
        } else {
            None
        }
    })?;
    
    if closed_prs.is_empty() {
        return None;
    }
    
    // Convert to materialization info
    let prs_to_materialize: Vec<ClosedPrToMaterialize> = closed_prs
        .iter()
        .map(|pr| ClosedPrToMaterialize {
            number: pr.number,
            head_ref: pr.head_ref.clone(),
            url: pr.url.clone(),
            merged: pr.merged,
        })
        .collect();
    
    let fix_id = FixId::new(
        "synthetic-stack-head",
        "materialize-snapshots",
        &head_branch,
    );
    
    let description = format!(
        "Create {} frozen snapshot branch(es) from closed PRs merged into '{}'",
        prs_to_materialize.len(),
        head_branch,
    );
    
    Some(FixOption {
        id: fix_id,
        description,
        fix: BootstrapFix::MaterializeSyntheticSnapshots {
            head_branch,
            closed_prs: prs_to_materialize,
        },
        destructive: false,
        requires_confirmation: true,
    })
}

/// Extract the synthetic head branch name from an issue.
///
/// Parses the branch name from the issue message or ID.
fn extract_synthetic_head_branch(issue: &Issue) -> Option<String> {
    // Issue ID format: "synthetic-stack-head:<pr_number>"
    // Message format: "PR #N targeting trunk may be a synthetic stack head (branch 'X')"
    
    if !issue.id.as_str().starts_with("synthetic-stack-head:") {
        return None;
    }
    
    // Extract from message which contains the branch name
    let msg = &issue.message;
    if let Some(start) = msg.find("(branch '") {
        if let Some(end) = msg[start + 9..].find("')") {
            return Some(msg[start + 9..start + 9 + end].to_string());
        }
    }
    
    None
}
```

### Step 6: Add Planner Support for Snapshot Materialization

**File:** `src/doctor/planner.rs`

Extend the planner to convert `MaterializeSyntheticSnapshots` fixes to executable plans:

```rust
use crate::core::metadata::schema::{
    BranchMetadataV1, FreezeState, FreezeScope, PrState, PrStatusCache,
    FREEZE_REASON_SYNTHETIC_SNAPSHOT,
};
use crate::doctor::fixes::{BootstrapFix, ClosedPrToMaterialize};
use crate::doctor::generators::{snapshot_branch_name, SNAPSHOT_PREFIX};
use crate::engine::plan::{Plan, PlanPhase, PlanStep};

impl DoctorPlanner {
    /// Plan the materialization of synthetic stack snapshots.
    ///
    /// This creates a plan that:
    /// 1. Fetches each PR ref from GitHub
    /// 2. Validates the commit is an ancestor of the head branch
    /// 3. Creates a snapshot branch with metadata
    ///
    /// All steps are wrapped in rollback-capable transactions.
    pub fn plan_materialize_snapshots(
        &self,
        head_branch: &str,
        closed_prs: &[ClosedPrToMaterialize],
        snapshot: &RepoSnapshot,
    ) -> Result<Plan, PlanError> {
        let mut steps = Vec::new();
        
        // Get the head branch's current OID for validation
        let head_oid = snapshot.branches
            .get(&BranchName::new(head_branch)?)
            .ok_or_else(|| PlanError::BranchNotFound(head_branch.to_string()))?
            .oid
            .clone();
        
        // Get trunk name for metadata (parent's parent)
        let trunk = snapshot.trunk.as_ref()
            .ok_or(PlanError::TrunkNotConfigured)?
            .clone();
        
        for pr in closed_prs {
            // Generate collision-free branch name
            let branch_name = snapshot_branch_name(pr.number, snapshot);
            
            // Step 1: Fetch the PR ref
            // This uses a RunGit step that fetches refs/pull/{number}/head
            steps.push(PlanStep::RunGit {
                args: vec![
                    "fetch".to_string(),
                    "origin".to_string(),
                    format!("refs/pull/{}/head", pr.number),
                ],
                description: format!("Fetch PR #{} head ref", pr.number),
                expected_effects: vec![], // FETCH_HEAD updated
            });
            
            // Step 2: Create the branch pointing to FETCH_HEAD
            // We use a special step that creates the branch and validates ancestry
            steps.push(PlanStep::CreateSnapshotBranch {
                branch_name: branch_name.clone(),
                pr_number: pr.number,
                head_branch: head_branch.to_string(),
                head_oid: head_oid.clone(),
            });
            
            // Step 3: Create metadata for the branch
            let parent = BranchName::new(head_branch)?;
            let metadata = BranchMetadataV1::builder(
                BranchName::new(&branch_name)?,
                parent.clone(),
                // Base will be computed at execution time as merge-base
                Oid::zero(), // Placeholder - updated during execution
            )
            .freeze_state(FreezeState::frozen(
                FreezeScope::Single,
                Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT.to_string()),
            ))
            .pr_state(PrState::Linked {
                forge: "github".to_string(),
                number: pr.number,
                url: pr.url.clone(),
                last_known: Some(PrStatusCache {
                    state: if pr.merged { "merged" } else { "closed" }.to_string(),
                    is_draft: false,
                }),
            })
            .build();
            
            steps.push(PlanStep::WriteMetadataCas {
                branch: BranchName::new(&branch_name)?,
                metadata,
                expected_old: None, // Creating new
            });
        }
        
        Ok(Plan {
            command: "doctor".to_string(),
            phases: vec![
                PlanPhase {
                    name: "materialize_snapshots".to_string(),
                    steps,
                },
            ],
            rollback_on_failure: true,
        })
    }
}
```

### Step 7: Add CreateSnapshotBranch Plan Step

**File:** `src/engine/plan.rs`

Add a new plan step type for creating snapshot branches with validation:

```rust
/// A step in an execution plan.
#[derive(Debug, Clone)]
pub enum PlanStep {
    // ... existing variants ...

    /// Create a snapshot branch from FETCH_HEAD with ancestry validation.
    ///
    /// This step:
    /// 1. Reads FETCH_HEAD to get the snapshot commit
    /// 2. Validates it's an ancestor of the head branch
    /// 3. Creates the branch pointing to the snapshot commit
    /// 4. Computes merge-base for metadata base field
    CreateSnapshotBranch {
        /// Name for the new branch (e.g., "lattice/snap/pr-42")
        branch_name: String,
        /// PR number (for error messages)
        pr_number: u64,
        /// The synthetic head branch this snapshot belongs to
        head_branch: String,
        /// Current OID of the head branch (for validation)
        head_oid: Oid,
    },
}
```

### Step 8: Implement CreateSnapshotBranch Execution

**File:** `src/engine/exec.rs`

Add executor support for the `CreateSnapshotBranch` step:

```rust
impl Executor {
    /// Execute a CreateSnapshotBranch step.
    ///
    /// This:
    /// 1. Reads FETCH_HEAD to get the PR commit
    /// 2. Validates it's an ancestor of head_branch
    /// 3. Creates the branch via CAS (expecting no prior ref)
    /// 4. Returns the computed merge-base for metadata
    fn execute_create_snapshot_branch(
        &self,
        branch_name: &str,
        pr_number: u64,
        head_branch: &str,
        head_oid: &Oid,
        journal: &mut Journal,
    ) -> Result<StepResult, ExecError> {
        // Step 1: Read FETCH_HEAD
        let fetch_head = self.git.resolve_ref("FETCH_HEAD")
            .map_err(|e| ExecError::GitError(format!(
                "PR #{} ref not found after fetch: {}",
                pr_number, e
            )))?;
        
        // Step 2: Validate ancestry
        let is_ancestor = self.git.is_ancestor(&fetch_head, head_oid)
            .map_err(|e| ExecError::GitError(format!(
                "Failed to check ancestry for PR #{}: {}",
                pr_number, e
            )))?;
        
        if !is_ancestor {
            return Ok(StepResult::Abort {
                error: format!(
                    "PR #{} commit {} is not an ancestor of '{}' ({}). \
                     The PR may have been rebased or force-pushed after being closed.",
                    pr_number,
                    fetch_head.short(8),
                    head_branch,
                    head_oid.short(8),
                ),
            });
        }
        
        // Step 3: Compute merge-base for metadata
        let merge_base = self.git.merge_base(&fetch_head, head_oid)
            .map_err(|e| ExecError::GitError(format!(
                "Failed to compute merge-base for PR #{}: {}",
                pr_number, e
            )))?;
        
        // Step 4: Create the branch (CAS: expect no prior ref)
        let refname = format!("refs/heads/{}", branch_name);
        self.git.update_ref_cas(&refname, Some(&fetch_head), None)
            .map_err(|e| match e {
                GitError::CasFailed { .. } => ExecError::Precondition(format!(
                    "Branch '{}' already exists",
                    branch_name,
                )),
                _ => ExecError::GitError(e.to_string()),
            })?;
        
        // Record in journal
        journal.record_ref_create(&refname, &fetch_head);
        
        // Store merge-base for metadata step
        // (The metadata step will retrieve this from executor state)
        self.snapshot_bases.insert(branch_name.to_string(), merge_base);
        
        Ok(StepResult::Continue)
    }
}
```

### Step 9: Update WriteMetadataCas for Snapshot Base

**File:** `src/engine/exec.rs`

The metadata step needs to retrieve the computed merge-base:

```rust
impl Executor {
    fn execute_write_metadata_cas(
        &self,
        branch: &BranchName,
        mut metadata: BranchMetadataV1,
        expected_old: Option<&str>,
        journal: &mut Journal,
    ) -> Result<StepResult, ExecError> {
        // Check if this is a snapshot branch that needs base resolution
        let branch_str = branch.as_str();
        if branch_str.starts_with(SNAPSHOT_PREFIX) {
            if let Some(base) = self.snapshot_bases.get(branch_str) {
                // Update the placeholder base OID with the computed merge-base
                metadata.base.oid = base.to_string();
            }
        }
        
        // ... rest of existing implementation ...
    }
}
```

### Step 10: Add Executor State for Snapshot Bases

**File:** `src/engine/exec.rs`

Add state tracking for snapshot merge-bases:

```rust
use std::collections::HashMap;

/// Executor for running plans.
pub struct Executor {
    git: Git,
    /// Computed merge-bases for snapshot branches.
    /// Populated by CreateSnapshotBranch steps, consumed by WriteMetadataCas.
    snapshot_bases: HashMap<String, Oid>,
}

impl Executor {
    pub fn new(git: Git) -> Self {
        Self {
            git,
            snapshot_bases: HashMap::new(),
        }
    }
}
```

### Step 11: Wire Fix Option into Doctor Command

**File:** `src/cli/commands/mod.rs` (doctor handling)

Add support for the new fix type in the doctor command:

```rust
// In the doctor fix handling code:
match &fix.fix {
    // ... existing cases ...
    
    BootstrapFix::MaterializeSyntheticSnapshots { head_branch, closed_prs } => {
        // Generate the plan
        let planner = DoctorPlanner::new();
        let plan = planner.plan_materialize_snapshots(
            head_branch,
            closed_prs,
            &snapshot,
        )?;
        
        // Show preview
        println!("\nPlan preview:");
        println!("  Creating {} snapshot branches under '{}':", closed_prs.len(), head_branch);
        for pr in closed_prs {
            let name = snapshot_branch_name(pr.number, &snapshot);
            println!("    - {} (from PR #{})", name, pr.number);
        }
        println!();
        
        // Confirm and execute
        if confirm_fix(&fix, interactive)? {
            let result = executor.execute(&plan)?;
            match result {
                ExecuteResult::Success { .. } => {
                    println!("Created {} snapshot branches.", closed_prs.len());
                }
                ExecuteResult::Aborted { error } => {
                    eprintln!("Failed to materialize snapshots: {}", error);
                    eprintln!("All changes have been rolled back.");
                }
                _ => {}
            }
        }
    }
}
```

### Step 12: Unit Tests

**File:** `src/doctor/generators.rs`

```rust
#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::engine::scan::RepoSnapshot;
    use crate::core::types::BranchName;
    use std::collections::HashMap;

    fn minimal_snapshot() -> RepoSnapshot {
        RepoSnapshot {
            branches: HashMap::new(),
            metadata: HashMap::new(),
            trunk: Some(BranchName::new("main").unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn snapshot_branch_name_no_collision() {
        let snapshot = minimal_snapshot();
        let name = snapshot_branch_name(42, &snapshot);
        assert_eq!(name, "lattice/snap/pr-42");
    }

    #[test]
    fn snapshot_branch_name_with_collision() {
        let mut snapshot = minimal_snapshot();
        // Add an existing branch with the base name
        snapshot.branches.insert(
            BranchName::new("lattice/snap/pr-42").unwrap(),
            ScannedBranch::default(),
        );
        
        let name = snapshot_branch_name(42, &snapshot);
        assert_eq!(name, "lattice/snap/pr-42-1");
    }

    #[test]
    fn snapshot_branch_name_multiple_collisions() {
        let mut snapshot = minimal_snapshot();
        snapshot.branches.insert(
            BranchName::new("lattice/snap/pr-42").unwrap(),
            ScannedBranch::default(),
        );
        snapshot.branches.insert(
            BranchName::new("lattice/snap/pr-42-1").unwrap(),
            ScannedBranch::default(),
        );
        
        let name = snapshot_branch_name(42, &snapshot);
        assert_eq!(name, "lattice/snap/pr-42-2");
    }

    #[test]
    fn generate_materialize_fix_with_evidence() {
        let issue = Issue::new(
            "synthetic-stack-head:42",
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature')",
        )
        .with_evidence(Evidence::SyntheticStackChildren {
            head_branch: "feature".to_string(),
            closed_prs: vec![
                ClosedPrInfo {
                    number: 10,
                    head_ref: "sub-a".to_string(),
                    merged: true,
                    url: "https://github.com/org/repo/pull/10".to_string(),
                },
            ],
            truncated: false,
        });
        
        let fix = generate_materialize_snapshot_fix(&issue);
        
        assert!(fix.is_some());
        let fix = fix.unwrap();
        assert!(fix.description.contains("1 frozen snapshot"));
        assert!(fix.requires_confirmation);
    }

    #[test]
    fn generate_materialize_fix_no_evidence() {
        let issue = Issue::new(
            "synthetic-stack-head:42",
            Severity::Info,
            "PR #42 targeting trunk may be a synthetic stack head (branch 'feature')",
        );
        
        let fix = generate_materialize_snapshot_fix(&issue);
        
        assert!(fix.is_none());
    }
}
```

**File:** `src/git/interface.rs`

```rust
#[cfg(test)]
mod ancestry_tests {
    use super::*;

    #[test]
    fn is_ancestor_returns_true_for_ancestor() {
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();
        
        // Create a repo with two commits
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "first"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "second"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        
        let git = Git::open(repo_path).unwrap();
        let head = git.resolve_ref("HEAD").unwrap();
        let parent = git.resolve_ref("HEAD~1").unwrap();
        
        assert!(git.is_ancestor(&parent, &head).unwrap());
        assert!(!git.is_ancestor(&head, &parent).unwrap());
    }
}
```

### Step 13: Integration Tests

**File:** `tests/integration/snapshot_materialization.rs`

```rust
//! Integration tests for synthetic stack snapshot materialization.

use latticework::doctor::Doctor;
use latticework::engine::scan::scan;
use latticework::forge::{MockForge, PullRequestSummary, ListClosedPrsOpts};
use latticework::git::Git;

mod test_helpers;
use test_helpers::{TestRepo, setup_test_repo_with_remote};

#[tokio::test]
async fn test_materialize_single_snapshot() {
    // Setup: Create a repo with a synthetic stack scenario
    let (repo, remote) = setup_test_repo_with_remote();
    
    // Create history on remote that simulates:
    // 1. feature branch with PR targeting trunk
    // 2. sub-feature that was merged into feature
    remote.create_branch("feature");
    remote.commit("feature work");
    
    remote.checkout("feature");
    remote.create_branch("sub-feature");
    remote.commit("sub-feature work");
    
    // Simulate the merge of sub-feature into feature
    remote.checkout("feature");
    remote.run_git(&["merge", "sub-feature"]);
    
    // Fetch and set up local
    repo.run_git(&["fetch", "origin"]);
    repo.run_git(&["checkout", "-b", "feature", "origin/feature"]);
    
    // Mock forge with the PR data
    let mut mock = MockForge::new();
    mock.add_open_pr(PullRequestSummary {
        number: 1,
        head_ref: "feature".to_string(),
        base_ref: "main".to_string(),
        ..Default::default()
    });
    mock.add_closed_pr(PullRequestSummary {
        number: 2,
        head_ref: "sub-feature".to_string(),
        base_ref: "feature".to_string(),
        ..Default::default()
    });
    
    // Run doctor with deep analysis
    let git = Git::open(repo.path()).unwrap();
    let snapshot = scan(&git).unwrap();
    
    // Detect the synthetic stack
    let open_prs = mock.list_open_prs(Default::default()).await.unwrap();
    let issues = detect_potential_synthetic_heads(&snapshot, &open_prs.pulls);
    
    assert_eq!(issues.len(), 1);
    
    // Perform tier 2 analysis
    let issue = issues[0].to_issue();
    let config = DoctorBootstrapConfig::default();
    let evidence = analyze_synthetic_stack_deep(&issue, &mock, &config).await;
    
    assert!(evidence.is_some());
    
    // Generate fix option
    let mut issue_with_evidence = issue.clone();
    issue_with_evidence.evidence.push(evidence.unwrap());
    
    let fix = generate_materialize_snapshot_fix(&issue_with_evidence);
    assert!(fix.is_some());
    
    // Execute the fix
    let planner = DoctorPlanner::new();
    if let BootstrapFix::MaterializeSyntheticSnapshots { head_branch, closed_prs } = &fix.unwrap().fix {
        let plan = planner.plan_materialize_snapshots(
            head_branch,
            closed_prs,
            &snapshot,
        ).unwrap();
        
        let executor = Executor::new(git.clone());
        let result = executor.execute(&plan).unwrap();
        
        assert!(matches!(result, ExecuteResult::Success { .. }));
    }
    
    // Verify the snapshot branch was created
    let new_snapshot = scan(&git).unwrap();
    let snap_branch = BranchName::new("lattice/snap/pr-2").unwrap();
    
    assert!(new_snapshot.branches.contains_key(&snap_branch));
    assert!(new_snapshot.metadata.contains_key(&snap_branch));
    
    // Verify metadata
    let metadata = &new_snapshot.metadata.get(&snap_branch).unwrap().metadata;
    assert!(metadata.freeze.is_frozen());
    assert!(metadata.pr.is_linked());
    assert_eq!(metadata.parent.name(), "feature");
}

#[tokio::test]
async fn test_rollback_on_invalid_ancestor() {
    // Setup: Create a repo where the PR commit is NOT an ancestor
    let (repo, _) = setup_test_repo_with_remote();
    
    // ... setup that creates an invalid scenario ...
    
    // Attempt materialization - should fail and rollback
    // Verify no orphan branches were created
}

#[tokio::test]
async fn test_collision_avoidance() {
    let repo = TestRepo::new();
    repo.init_with_trunk("main");
    
    // Create a branch that would collide
    repo.run_git(&["checkout", "-b", "lattice/snap/pr-42"]);
    repo.run_git(&["checkout", "main"]);
    
    // Generate name for PR 42
    let git = Git::open(repo.path()).unwrap();
    let snapshot = scan(&git).unwrap();
    let name = snapshot_branch_name(42, &snapshot);
    
    assert_eq!(name, "lattice/snap/pr-42-1");
}

#[tokio::test]
async fn test_all_or_nothing_on_partial_failure() {
    // Setup: Multiple PRs where one will fail validation
    // Execute materialization
    // Verify ALL branches are rolled back, not just the failed one
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/doctor/fixes.rs` | MODIFY | Add `MaterializeSyntheticSnapshots` fix variant, `ClosedPrToMaterialize` type |
| `src/core/metadata/schema.rs` | MODIFY | Add `FREEZE_REASON_SYNTHETIC_SNAPSHOT` constant |
| `src/git/interface.rs` | MODIFY | Add `fetch_pr_ref`, `is_ancestor` methods |
| `src/doctor/generators.rs` | MODIFY | Add `snapshot_branch_name`, `generate_materialize_snapshot_fix`, `SNAPSHOT_PREFIX` |
| `src/doctor/planner.rs` | MODIFY | Add `plan_materialize_snapshots` method |
| `src/engine/plan.rs` | MODIFY | Add `CreateSnapshotBranch` step type |
| `src/engine/exec.rs` | MODIFY | Add `execute_create_snapshot_branch`, `snapshot_bases` state, update `WriteMetadataCas` |
| `src/cli/commands/mod.rs` | MODIFY | Wire fix option into doctor command |
| `tests/integration/snapshot_materialization.rs` | NEW | Integration tests |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.9:

- [ ] Snapshots created as `refs/heads/lattice/snap/pr-<n>` branches
- [ ] Collision avoidance works (appends suffix if needed)
- [ ] Invalid snapshots (commit not reachable from H) rejected
- [ ] Partial failures roll back entirely (no orphan branches)
- [ ] Snapshot branches frozen with reason `remote_synthetic_snapshot`
- [ ] Metadata includes correct parent (synthetic head), base (merge-base), and PR linkage
- [ ] Fix option only appears when Tier 2 evidence exists
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `snapshot_branch_name_no_collision` | generators.rs | Basic name generation |
| `snapshot_branch_name_with_collision` | generators.rs | Single collision avoidance |
| `snapshot_branch_name_multiple_collisions` | generators.rs | Multiple collision avoidance |
| `generate_materialize_fix_with_evidence` | generators.rs | Fix generation with evidence |
| `generate_materialize_fix_no_evidence` | generators.rs | Fix generation without evidence |
| `is_ancestor_returns_true_for_ancestor` | interface.rs | Ancestry check |
| `is_ancestor_returns_false_for_non_ancestor` | interface.rs | Negative ancestry check |

### Integration Tests

| Test | Description |
|------|-------------|
| `test_materialize_single_snapshot` | Full workflow for one snapshot |
| `test_materialize_multiple_snapshots` | Multiple snapshots in one operation |
| `test_rollback_on_invalid_ancestor` | Rollback when validation fails |
| `test_collision_avoidance` | Branch name collision handling |
| `test_all_or_nothing_on_partial_failure` | Complete rollback on any failure |

---

## Dependencies

- **Milestone 5.8:** Synthetic stack detection with Tier 2 evidence (Complete)
- **Milestone 5.5:** Bootstrap fix execution infrastructure (Complete)
- **Existing:** MockForge, Git interface, Executor

---

## Estimated Scope

- **Lines of code changed:** ~100 in `fixes.rs`, ~5 in `schema.rs`, ~80 in `interface.rs`, ~150 in `generators.rs`, ~100 in `planner.rs`, ~30 in `plan.rs`, ~100 in `exec.rs`, ~50 in `mod.rs`
- **New functions:** 7 (`fetch_pr_ref`, `is_ancestor`, `snapshot_branch_name`, `generate_materialize_snapshot_fix`, `plan_materialize_snapshots`, `execute_create_snapshot_branch`)
- **New types:** 3 (`MaterializeSyntheticSnapshots`, `ClosedPrToMaterialize`, `CreateSnapshotBranch`)
- **Risk:** Medium - Creates branches and metadata; relies on rollback for safety

---

## Verification Commands

```bash
# Type checks
cargo check

# Lint
cargo clippy -- -D warnings

# All tests
cargo test

# Specific tests
cargo test snapshot
cargo test materialize
cargo test ancestry
cargo test synthetic

# Integration tests
cargo test --test snapshot_materialization

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** Uses existing fix/plan/executor infrastructure from Milestones 5.4-5.5
- **Simplicity:** Normal branches with reserved prefix avoid custom namespace complexity
- **Reuse:** Extends existing Forge trait, Git interface, metadata schema
- **Purity:** Fix generators and name computation are pure; execution is the imperative shell
- **No stubs:** All features fully implemented with rollback safety

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.9 as complete
2. Create `implementation_notes.md` in this directory
3. Milestone 5.10 (Submit Scope Exclusion) can proceed to ensure snapshot branches don't appear in submit sets
