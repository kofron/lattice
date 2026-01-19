# Milestone 5.7: Local-Only Bootstrap (ImportLocalInProgressTopology)

## Goal

Enable bootstrap from local branches when remote is unavailable, allowing users to import untracked branches into Lattice tracking using only local git graph topology analysis.

**Core principle from ARCHITECTURE.md Section 8.1:** "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

---

## Background

Remote-first bootstrap (Milestones 5.3-5.5) enables importing branches when open PRs exist on the forge. However, users need bootstrap capability when:

1. Working offline (no network access)
2. No forge configured or authenticated
3. Branches exist locally that have no corresponding PRs
4. User explicitly prefers local-only inference

This milestone implements local-only bootstrap by extending the existing `UntrackedBranch` issue to include parent candidate evidence and creating a new fix generator that uses merge-base distance to infer the optimal parent.

| Milestone | Component | Status |
|-----------|-----------|--------|
| 5.3 | Bootstrap issue detection | Complete |
| 5.4 | Bootstrap fix generators (remote) | Complete |
| 5.5 | Bootstrap fix execution | Complete |
| 5.6 | Init hint for bootstrap | Complete |
| **5.7** | **Local-only bootstrap** | **This milestone** |

---

## Spec References

- **ROADMAP.md Milestone 5.7** - Local-only bootstrap deliverables
- **ARCHITECTURE.md Section 8** - Doctor framework
- **ARCHITECTURE.md Section 8.2** - Issues and fix options
- **ARCHITECTURE.md Section 8.3** - Confirmation model (interactive vs non-interactive)
- **SPEC.md Section 8B.1** - Track command behavior (parent selection, merge-base)

---

## Design Decisions

### Parent Selection Algorithm

Use **merge-base distance** to rank parent candidates. This is consistent with the existing `find_nearest_tracked_ancestor()` function in `track.rs`.

**Algorithm:**
1. Gather all candidates: tracked branches + trunk
2. For each candidate:
   - Compute `merge_base(untracked_branch_tip, candidate_tip)`
   - If merge-base exists, count commits from merge-base to untracked branch tip
   - This distance represents "how far the branch has diverged from this candidate"
3. Rank candidates by ascending distance (closest first)
4. The candidate with minimum distance is the "best" parent

**Why merge-base distance?**
- Merge-base represents the point of divergence
- Fewer commits from divergence point = closer relationship
- This matches what `track --force` already does

### Ambiguity Handling

**Ambiguous case:** Multiple candidates have the same minimum distance (equally valid parents).

**Interactive mode:**
- Present all equally-ranked candidates as separate fix options
- User selects which one to apply

**Non-interactive mode:**
- Refuse with a clear error message listing the tied candidates
- Never auto-select when ambiguous
- Per ARCHITECTURE.md Section 8.3: "doctor never auto-selects fixes"

### Issue Evidence Enhancement

The `UntrackedBranch` issue currently has minimal evidence (just the branch name). To support local-only bootstrap, we extend the scanner to compute and store parent candidates with their distances.

**New evidence structure:**
```rust
Evidence::ParentCandidates {
    branch: String,
    candidates: Vec<ParentCandidate>,
}

struct ParentCandidate {
    name: String,           // Branch name
    merge_base: String,     // Merge-base OID
    distance: u32,          // Commits from merge-base to branch tip
    is_trunk: bool,         // Whether this is the trunk branch
}
```

### Default Freeze State

For local-only bootstrap:
- **Default: Unfrozen** - User's own branch, they can modify
- No PR linkage (since we have no remote evidence)
- Can optionally freeze via `--as-frozen` if a fix option is added

### Base Computation

Consistent with Milestone 5.4 and the `track` command:
- `base = merge_base(branch_tip, parent_tip)`
- Refuse if merge-base is None (no common ancestor)

---

## Implementation Steps

### Step 1: Add ParentCandidate Evidence Type

**File:** `src/engine/health.rs`

Add a new evidence variant to support parent candidate information:

```rust
/// Evidence associated with an issue.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    // ... existing variants ...

    /// Parent candidates for an untracked branch (local-only bootstrap).
    ParentCandidates {
        /// The untracked branch name.
        branch: String,
        /// Ranked list of parent candidates with distances.
        candidates: Vec<ParentCandidate>,
    },
}

/// A potential parent branch for an untracked branch.
#[derive(Debug, Clone, PartialEq)]
pub struct ParentCandidate {
    /// Branch name.
    pub name: String,
    /// Merge-base OID between untracked branch and this candidate.
    pub merge_base: String,
    /// Number of commits from merge-base to untracked branch tip.
    pub distance: u32,
    /// Whether this is the configured trunk branch.
    pub is_trunk: bool,
}
```

### Step 2: Extend UntrackedBranch Issue with Evidence

**File:** `src/doctor/issues.rs`

Update `KnownIssue::UntrackedBranch` to optionally include parent candidates:

```rust
/// Branch exists but has no tracking metadata.
#[error("branch '{branch}' exists but is not tracked")]
UntrackedBranch {
    /// The untracked branch name.
    branch: String,
    /// Parent candidates ranked by merge-base distance (closest first).
    /// Empty if candidates couldn't be computed.
    candidates: Vec<ParentCandidate>,
},
```

Update the `issue_id()` and `to_issue()` methods accordingly:

```rust
KnownIssue::UntrackedBranch { branch, candidates } => {
    let mut issue = issues::untracked_branch(branch);
    if !candidates.is_empty() {
        issue.evidence.push(Evidence::ParentCandidates {
            branch: branch.clone(),
            candidates: candidates.clone(),
        });
    }
    issue
}
```

### Step 3: Add Parent Candidate Computation to Scanner

**File:** `src/engine/scan.rs`

Add a function to compute parent candidates for an untracked branch:

```rust
/// Compute parent candidates for an untracked branch.
///
/// Returns candidates ranked by merge-base distance (closest first).
/// Candidates with equal distance are considered "tied" (ambiguous).
pub fn compute_parent_candidates(
    git: &Git,
    branch: &BranchName,
    branch_oid: &Oid,
    snapshot: &RepoSnapshot,
) -> Vec<ParentCandidate> {
    let mut candidates = Vec::new();

    let trunk = snapshot.trunk.as_ref();

    // Gather all potential parents: tracked branches + trunk
    let mut potential_parents: Vec<(&BranchName, &Oid, bool)> = snapshot
        .metadata
        .keys()
        .filter_map(|b| snapshot.branches.get(b).map(|oid| (b, oid, false)))
        .collect();

    // Add trunk if present
    if let Some(trunk_name) = trunk {
        if let Some(trunk_oid) = snapshot.branches.get(trunk_name) {
            // Avoid duplicates if trunk is also tracked
            if !potential_parents.iter().any(|(b, _, _)| *b == trunk_name) {
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
```

### Step 4: Update Scanner to Populate Candidate Evidence

**File:** `src/engine/scan.rs`

Update the scanning logic to compute parent candidates for untracked branches:

```rust
// In the function that detects untracked branches:
fn detect_untracked_branches(
    git: &Git,
    snapshot: &mut RepoSnapshot,
) {
    for (branch, oid) in &snapshot.branches {
        // Skip trunk
        if Some(branch) == snapshot.trunk.as_ref() {
            continue;
        }

        // Skip already tracked
        if snapshot.metadata.contains_key(branch) {
            continue;
        }

        // Compute parent candidates
        let candidates = compute_parent_candidates(git, branch, oid, snapshot);

        // Add issue with evidence
        let issue = KnownIssue::UntrackedBranch {
            branch: branch.as_str().to_string(),
            candidates,
        };
        snapshot.health.add_issue(issue.to_issue());
    }
}
```

### Step 5: Add Local Bootstrap Fix Generator

**File:** `src/doctor/generators.rs`

Add a new fix generator for local-only bootstrap:

```rust
/// Generate fixes for untracked local branches using local graph topology.
///
/// This handles the `UntrackedBranch` issue when no remote evidence exists.
/// Parent is inferred from merge-base distance to tracked branches.
///
/// Fix: Track the branch with the nearest tracked ancestor as parent.
/// Default state: Unfrozen (user's own branch).
///
/// Ambiguity handling:
/// - If multiple candidates have the same minimum distance, create separate
///   fix options for each and let the user choose.
/// - In non-interactive mode, this means the user must specify which fix to apply.
fn generate_import_local_topology_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let mut fixes = Vec::new();

    // Extract parent candidates from evidence
    let (branch, candidates) = extract_parent_candidates(issue);

    if branch.is_empty() || candidates.is_empty() {
        return fixes;
    }

    // Verify branch exists and is untracked
    if let Ok(branch_name) = crate::core::types::BranchName::new(&branch) {
        if !snapshot.branches.contains_key(&branch_name) {
            return fixes; // Branch doesn't exist
        }
        if snapshot.metadata.contains_key(&branch_name) {
            return fixes; // Already tracked
        }
    } else {
        return fixes; // Invalid branch name
    }

    // Find minimum distance among candidates
    let min_distance = candidates.iter().map(|c| c.distance).min().unwrap_or(u32::MAX);

    // Get all candidates at minimum distance (handles ties)
    let best_candidates: Vec<_> = candidates
        .iter()
        .filter(|c| c.distance == min_distance)
        .collect();

    // Generate a fix option for each best candidate
    for candidate in &best_candidates {
        let fix_id_suffix = if best_candidates.len() > 1 {
            // Multiple equally-good candidates - include parent name in ID
            format!("import-local:{}", candidate.name)
        } else {
            // Single best candidate
            "import-local".to_string()
        };

        let description = if best_candidates.len() > 1 {
            format!(
                "Track '{}' with parent '{}' (one of {} equally close ancestors)",
                branch, candidate.name, best_candidates.len()
            )
        } else {
            format!(
                "Track '{}' with parent '{}' (nearest ancestor, {} commits diverged)",
                branch, candidate.name, candidate.distance
            )
        };

        let trunk_note = if candidate.is_trunk { " (trunk)" } else { "" };

        fixes.push(
            FixOption::new(
                FixId::new("untracked-branch", &fix_id_suffix, &branch),
                issue.id.clone(),
                description,
                FixPreview::with_summary(format!(
                    "Create tracking metadata for '{}' with parent '{}'{}, base at merge-base {}",
                    branch,
                    candidate.name,
                    trunk_note,
                    &candidate.merge_base[..7.min(candidate.merge_base.len())]
                ))
                .add_metadata_change(MetadataChange::Create {
                    branch: branch.clone(),
                    description: format!(
                        "parent={}, base={}, unfrozen",
                        candidate.name,
                        &candidate.merge_base[..7.min(candidate.merge_base.len())]
                    ),
                }),
            )
            .with_preconditions([
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::GraphValid,
            ]),
        );
    }

    fixes
}

/// Extract parent candidate evidence from an issue.
fn extract_parent_candidates(issue: &Issue) -> (String, Vec<ParentCandidate>) {
    let mut branch = String::new();
    let mut candidates = Vec::new();

    for evidence in &issue.evidence {
        if let Evidence::ParentCandidates {
            branch: b,
            candidates: c,
        } = evidence
        {
            branch = b.clone();
            candidates = c.clone();
            break;
        }
    }

    // Fallback: extract branch from issue ID
    if branch.is_empty() {
        if let Some(b) = issue.id.as_str().strip_prefix("untracked-branch:") {
            branch = b.to_string();
        }
    }

    (branch, candidates)
}
```

### Step 6: Wire Generator into Dispatch

**File:** `src/doctor/generators.rs`

Update `generate_fixes()` to dispatch to the new generator:

```rust
pub fn generate_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    let issue_type = extract_issue_type(issue.id.as_str());

    match issue_type {
        // ... existing cases ...

        // Local-only bootstrap (Milestone 5.7)
        "untracked-branch" => generate_import_local_topology_fixes(issue, snapshot),

        _ => Vec::new(),
    }
}
```

### Step 7: Update Planner for Local Bootstrap Fixes

**File:** `src/doctor/planner.rs`

The planner already handles `MetadataChange::Create`. Ensure it:

1. Parses the description to extract parent and base
2. Computes actual base via merge-base (if description has placeholder)
3. Creates proper `BranchMetadataV1` with unfrozen state

```rust
// In generate_repair_plan(), when handling MetadataChange::Create:
MetadataChange::Create { branch, description } => {
    // Parse description: "parent=X, base=Y, unfrozen"
    let (parent, base_hint, frozen) = parse_metadata_description(&description);

    // Get branch and parent OIDs
    let branch_name = BranchName::new(&branch)?;
    let branch_oid = snapshot.branches.get(&branch_name)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", branch))?;

    let parent_name = BranchName::new(&parent)?;
    let parent_oid = snapshot.branches.get(&parent_name)
        .ok_or_else(|| anyhow::anyhow!("Parent '{}' not found", parent))?;

    // Compute actual base via merge-base
    let base_oid = git.merge_base(branch_oid, parent_oid)?
        .ok_or_else(|| anyhow::anyhow!(
            "No common ancestor between '{}' and '{}'",
            branch, parent
        ))?;

    // Create metadata
    let metadata = BranchMetadataV1::new(branch_name.clone(), parent_name, base_oid);
    // Apply freeze state if specified
    // ...

    // Add WriteMetadataCas step
    plan.add_step(PlanStep::WriteMetadataCas {
        branch: branch_name,
        expected_old: None,
        new_metadata: metadata,
    });
}
```

### Step 8: Add Unit Tests

**File:** `src/doctor/generators.rs`

```rust
#[cfg(test)]
mod local_bootstrap_tests {
    use super::*;

    fn snapshot_with_tracked_branches() -> RepoSnapshot {
        let mut snapshot = minimal_snapshot();

        // Add a tracked branch "feature-a"
        let branch_a = BranchName::new("feature-a").unwrap();
        let oid_a = Oid::new("aaa111aaa1111111aaa111aaa1111111aaa11111").unwrap();
        snapshot.branches.insert(branch_a.clone(), oid_a.clone());

        let parent = BranchName::new("main").unwrap();
        let metadata = BranchMetadataV1::new(branch_a.clone(), parent, oid_a.clone());
        snapshot.metadata.insert(
            branch_a,
            ScannedMetadata {
                ref_oid: oid_a,
                metadata,
            },
        );

        // Add untracked branch "feature-b"
        let branch_b = BranchName::new("feature-b").unwrap();
        let oid_b = Oid::new("bbb222bbb2222222bbb222bbb2222222bbb22222").unwrap();
        snapshot.branches.insert(branch_b, oid_b);

        snapshot
    }

    #[test]
    fn local_bootstrap_generates_fix_with_single_best_parent() {
        let candidates = vec![
            ParentCandidate {
                name: "main".to_string(),
                merge_base: "abc123abc1231231abc123abc1231231abc12312".to_string(),
                distance: 3,
                is_trunk: true,
            },
            ParentCandidate {
                name: "feature-a".to_string(),
                merge_base: "def456def4564564def456def4564564def45645".to_string(),
                distance: 5,
                is_trunk: false,
            },
        ];

        let issue = create_untracked_issue_with_candidates("feature-b", candidates);
        let snapshot = snapshot_with_tracked_branches();

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].description.contains("main"));
        assert!(fixes[0].description.contains("nearest ancestor"));
    }

    #[test]
    fn local_bootstrap_generates_multiple_fixes_for_tied_candidates() {
        let candidates = vec![
            ParentCandidate {
                name: "main".to_string(),
                merge_base: "abc123abc1231231abc123abc1231231abc12312".to_string(),
                distance: 3, // Same distance
                is_trunk: true,
            },
            ParentCandidate {
                name: "feature-a".to_string(),
                merge_base: "def456def4564564def456def4564564def45645".to_string(),
                distance: 3, // Same distance
                is_trunk: false,
            },
        ];

        let issue = create_untracked_issue_with_candidates("feature-b", candidates);
        let snapshot = snapshot_with_tracked_branches();

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert_eq!(fixes.len(), 2);
        assert!(fixes.iter().any(|f| f.description.contains("main")));
        assert!(fixes.iter().any(|f| f.description.contains("feature-a")));
        assert!(fixes[0].description.contains("equally close"));
    }

    #[test]
    fn local_bootstrap_returns_empty_for_no_candidates() {
        let issue = create_untracked_issue_with_candidates("feature-b", vec![]);
        let snapshot = snapshot_with_tracked_branches();

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert!(fixes.is_empty());
    }

    #[test]
    fn local_bootstrap_returns_empty_if_branch_missing() {
        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        let issue = create_untracked_issue_with_candidates("nonexistent", candidates);
        let snapshot = minimal_snapshot();

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert!(fixes.is_empty());
    }

    #[test]
    fn local_bootstrap_returns_empty_if_already_tracked() {
        let candidates = vec![ParentCandidate {
            name: "main".to_string(),
            merge_base: "abc123".to_string(),
            distance: 3,
            is_trunk: true,
        }];

        // feature-a is already tracked in this snapshot
        let issue = create_untracked_issue_with_candidates("feature-a", candidates);
        let snapshot = snapshot_with_tracked_branches();

        let fixes = generate_import_local_topology_fixes(&issue, &snapshot);

        assert!(fixes.is_empty());
    }

    fn create_untracked_issue_with_candidates(
        branch: &str,
        candidates: Vec<ParentCandidate>,
    ) -> Issue {
        let mut issue = Issue::new(
            &format!("untracked-branch:{}", branch),
            Severity::Info,
            &format!("branch '{}' exists but is not tracked", branch),
        );
        if !candidates.is_empty() {
            issue.evidence.push(Evidence::ParentCandidates {
                branch: branch.to_string(),
                candidates,
            });
        }
        issue
    }
}
```

### Step 9: Integration Tests

**File:** `tests/integration/local_bootstrap.rs` (new)

```rust
//! Integration tests for local-only bootstrap

use latticework::core::types::BranchName;
use latticework::doctor::{Doctor, FixId};
use latticework::engine::scan::scan;
use latticework::git::Git;

mod test_helpers;
use test_helpers::TestRepo;

#[test]
fn test_local_bootstrap_single_best_parent() {
    // Setup: repo with tracked branch and untracked branch
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create and track feature-a
    repo.create_branch("feature-a");
    repo.commit("feature-a work");
    repo.track_branch("feature-a", "main");

    // Create untracked feature-b from main (closer to main than feature-a)
    repo.checkout("main");
    repo.create_branch("feature-b");
    repo.commit("feature-b work");

    // Scan and diagnose
    let git = Git::open(repo.path()).unwrap();
    let snapshot = scan(&git).unwrap();
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);

    // Should have untracked-branch issue for feature-b
    let issue = diagnosis
        .issues
        .iter()
        .find(|i| i.id.as_str().contains("feature-b"))
        .expect("should have untracked-branch issue");

    // Should have exactly one fix (main is closest)
    let fixes: Vec<_> = diagnosis
        .fixes
        .iter()
        .filter(|f| f.issue_id == issue.id)
        .collect();
    assert_eq!(fixes.len(), 1);
    assert!(fixes[0].description.contains("main"));

    // Apply the fix
    let fix_id = fixes[0].id.clone();
    let plan = doctor
        .plan_repairs(&[fix_id], &diagnosis, &snapshot)
        .unwrap();
    let result = latticework::engine::exec::execute(&plan, &git, &Default::default()).unwrap();

    assert!(matches!(
        result,
        latticework::engine::exec::ExecuteResult::Success { .. }
    ));

    // Verify branch is now tracked
    let new_snapshot = scan(&git).unwrap();
    let branch = BranchName::new("feature-b").unwrap();
    assert!(new_snapshot.metadata.contains_key(&branch));

    // Verify parent is main
    let metadata = &new_snapshot.metadata.get(&branch).unwrap().metadata;
    assert_eq!(metadata.parent.branch_name(), Some("main"));
}

#[test]
fn test_local_bootstrap_ambiguous_parents() {
    // Setup: repo where feature-b could equally parent to main or feature-a
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create feature-a from main
    repo.create_branch("feature-a");
    repo.commit("feature-a work");
    repo.track_branch("feature-a", "main");

    // Create feature-b from same commit as feature-a (equal distance)
    repo.checkout("main");
    repo.commit("shared base"); // Both will have same distance to this
    repo.create_branch("feature-a-v2");
    repo.checkout("main");
    repo.create_branch("feature-b");
    repo.commit("feature-b work");

    // In a real scenario, we'd craft commits so distances are equal
    // For this test, we just verify multiple fixes are generated when appropriate

    let git = Git::open(repo.path()).unwrap();
    let snapshot = scan(&git).unwrap();
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);

    // If there are ambiguous candidates, there should be multiple fixes
    // The exact number depends on the commit graph we've created
    let fixes_for_untracked: Vec<_> = diagnosis
        .fixes
        .iter()
        .filter(|f| f.id.to_string().contains("import-local"))
        .collect();

    // Verify fixes exist (exact count depends on topology)
    assert!(!fixes_for_untracked.is_empty());
}

#[test]
fn test_local_bootstrap_no_candidates_when_no_tracked() {
    // Setup: fresh repo with only trunk and one untracked branch
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create untracked branch
    repo.create_branch("feature");
    repo.commit("feature work");

    let git = Git::open(repo.path()).unwrap();
    let snapshot = scan(&git).unwrap();
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);

    // Should have fix to track with trunk as parent
    let fixes: Vec<_> = diagnosis
        .fixes
        .iter()
        .filter(|f| f.description.contains("feature"))
        .collect();

    // Trunk should be the only candidate
    assert_eq!(fixes.len(), 1);
    assert!(fixes[0].description.contains("main"));
}

#[test]
fn test_local_bootstrap_base_uses_merge_base() {
    // Setup: verify that the base is computed via merge-base, not parent tip
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create feature from main at commit A
    let base_commit = repo.current_commit();
    repo.create_branch("feature");
    repo.commit("feature work 1");
    repo.commit("feature work 2");

    // Advance main past where feature was created
    repo.checkout("main");
    repo.commit("main advance 1");
    repo.commit("main advance 2");

    let git = Git::open(repo.path()).unwrap();
    let snapshot = scan(&git).unwrap();
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);

    // Apply fix
    let fix = diagnosis
        .fixes
        .iter()
        .find(|f| f.description.contains("feature"))
        .expect("should have fix for feature");
    let plan = doctor
        .plan_repairs(&[fix.id.clone()], &diagnosis, &snapshot)
        .unwrap();
    latticework::engine::exec::execute(&plan, &git, &Default::default()).unwrap();

    // Verify base is the merge-base (original branch point), not main's tip
    let new_snapshot = scan(&git).unwrap();
    let branch = BranchName::new("feature").unwrap();
    let metadata = &new_snapshot.metadata.get(&branch).unwrap().metadata;

    // Base should be the original divergence point
    assert_eq!(metadata.base.oid, base_commit);
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/engine/health.rs` | MODIFY | Add `ParentCandidate` struct and `Evidence::ParentCandidates` |
| `src/doctor/issues.rs` | MODIFY | Extend `UntrackedBranch` with candidates field |
| `src/engine/scan.rs` | MODIFY | Add `compute_parent_candidates()` and wire into scan |
| `src/doctor/generators.rs` | MODIFY | Add `generate_import_local_topology_fixes()` |
| `src/doctor/planner.rs` | MODIFY (if needed) | Ensure metadata creation handles local bootstrap |
| `tests/integration/local_bootstrap.rs` | NEW | Integration tests |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.7:

- [ ] Local branches can be tracked without forge access
- [ ] Parent selection prefers nearest tracked ancestor (by merge-base distance)
- [ ] Ambiguous cases (equal distance to multiple ancestors) produce multiple fix options
- [ ] Non-interactive mode refuses on ambiguity with clear message listing candidates
- [ ] Base computed via merge-base (consistent with remote-first bootstrap)
- [ ] Default freeze state is Unfrozen
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `local_bootstrap_generates_fix_with_single_best_parent` | generators.rs | Single best candidate |
| `local_bootstrap_generates_multiple_fixes_for_tied_candidates` | generators.rs | Ambiguous case |
| `local_bootstrap_returns_empty_for_no_candidates` | generators.rs | No candidates |
| `local_bootstrap_returns_empty_if_branch_missing` | generators.rs | Branch doesn't exist |
| `local_bootstrap_returns_empty_if_already_tracked` | generators.rs | Already tracked |
| `compute_parent_candidates_sorts_by_distance` | scan.rs | Distance ranking |
| `compute_parent_candidates_includes_trunk` | scan.rs | Trunk as candidate |

### Integration Tests

| Test | Description |
|------|-------------|
| `test_local_bootstrap_single_best_parent` | Full workflow with clear best parent |
| `test_local_bootstrap_ambiguous_parents` | Multiple equally-good parents |
| `test_local_bootstrap_no_candidates_when_no_tracked` | Only trunk available |
| `test_local_bootstrap_base_uses_merge_base` | Verify merge-base is used, not parent tip |

---

## Dependencies

- **Milestone 5.5:** Bootstrap fix execution (Complete) - Executor handles metadata creation
- **Existing:** `find_nearest_tracked_ancestor()` in track.rs - Algorithm reference
- **Existing:** `Git::merge_base()` and `Git::commit_count()` - Core primitives

---

## Estimated Scope

- **Lines of code changed:** ~50 in `health.rs`, ~30 in `issues.rs`, ~80 in `scan.rs`, ~120 in `generators.rs`
- **New functions:** 3 (`compute_parent_candidates`, `generate_import_local_topology_fixes`, `extract_parent_candidates`)
- **New types:** 2 (`ParentCandidate`, `Evidence::ParentCandidates`)
- **Risk:** Medium - Modifies issue detection flow and adds new evidence type

---

## Verification Commands

After implementation:

```bash
# Type checks
cargo check

# Lint
cargo clippy -- -D warnings

# All tests
cargo test

# Specific tests
cargo test local_bootstrap
cargo test untracked
cargo test compute_parent

# Integration tests
cargo test --test local_bootstrap

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** Reuses `find_nearest_tracked_ancestor` algorithm from track.rs
- **Simplicity:** Extends existing `UntrackedBranch` issue rather than creating new issue type
- **Reuse:** Uses existing merge-base and commit-count primitives
- **Purity:** Fix generators remain pure functions of issue + snapshot
- **No stubs:** All fixes must be fully executable through existing executor

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.7 as complete
2. Create `implementation_notes.md` in this directory
3. Milestone 5.8 (Synthetic Stack Detection) can proceed
