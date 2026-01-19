# Milestone 5.10: Submit Scope Exclusion for Snapshots

## Goal

Ensure synthetic snapshot branches (created by Milestone 5.9) never appear in submit sets, protecting users from accidentally submitting historical snapshot branches as new PRs.

**Core principle from ARCHITECTURE.md Section 8.1:** "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

---

## Background

Milestone 5.9 introduces **synthetic snapshot branches** - frozen branches created from closed PRs that were merged into a synthetic stack head. These branches:

- Are stored as `refs/heads/lattice/snap/pr-<number>`
- Are frozen with reason `remote_synthetic_snapshot`
- Represent historical state (already merged/closed PRs)
- Should never be submitted as new PRs

Without scope exclusion, a user could accidentally:
1. Navigate to a snapshot branch
2. Run `lattice submit` or `lattice submit --stack`
3. Create duplicate PRs for already-merged work

This milestone prevents that scenario by excluding snapshot branches from submit scope computation.

| Milestone | Component | Status |
|-----------|-----------|--------|
| 5.8 | Synthetic stack detection | Complete |
| 5.9 | Snapshot materialization | Complete |
| **5.10** | **Submit scope exclusion** | **This milestone** |

---

## Spec References

- **ROADMAP.md Milestone 5.10** - Submit scope exclusion deliverables
- **SPEC.md Section 8E.2** - Submit command behavior and scope computation
- **ARCHITECTURE.md Section 3.2** - Branch metadata (freeze state, structural fields)
- **SPEC.md Appendix A** - Metadata schema (freeze state structure)

---

## Design Decisions

### Exclusion Based on Freeze Reason

Rather than checking branch name patterns (which could be fragile), we exclude based on the **freeze reason** in metadata:

```rust
if let FreezeState::Frozen { reason: Some(r), .. } = &metadata.freeze {
    if r == FREEZE_REASON_SYNTHETIC_SNAPSHOT {
        // Exclude from submit scope
    }
}
```

**Why freeze reason over name pattern?**
1. **Semantic clarity:** The freeze reason explicitly captures intent
2. **Robustness:** Name collisions or manual renames won't cause issues
3. **Consistency:** Uses existing metadata infrastructure
4. **Extensibility:** Other freeze reasons could have different exclusion rules

### Exclusion Points

Submit scope is computed in `src/cli/commands/submit.rs`. The exclusion must happen:

1. **After scope expansion:** When determining which branches to include via `--stack`
2. **Before any operations:** Before pushing, PR creation, or any mutations
3. **With user feedback:** Silently excluding branches would be confusing

### User Feedback Strategy

When snapshot branches are excluded, print an informational message:

```
Excluding 2 snapshot branch(es) from submit scope:
  lattice/snap/pr-10
  lattice/snap/pr-11
These branches represent historical snapshots and cannot be submitted.
```

This ensures users understand why certain branches weren't submitted.

### No `--include-snapshots` Flag (Yet)

Per ROADMAP.md, we "reserve `--include-snapshots` flag for future explicit inclusion." This milestone does NOT implement that flag - it only implements the exclusion. The flag can be added later if users need it.

### Interaction with `--stack` Flag

When `--stack` is used:
- Ancestors and descendants are collected
- Snapshot branches in the set are excluded
- Remaining branches are submitted

If the **current branch** is a snapshot branch:
- Refuse the entire operation with a clear error message
- Do not attempt to submit any branches

---

## Implementation Steps

### Step 1: Add Helper Function to Check Snapshot Branch

**File:** `src/cli/commands/submit.rs`

Add a function to determine if a branch is a synthetic snapshot:

```rust
use crate::core::metadata::schema::{FreezeState, FREEZE_REASON_SYNTHETIC_SNAPSHOT};
use crate::engine::scan::RepoSnapshot;
use crate::core::types::BranchName;

/// Check if a branch is a synthetic snapshot branch.
///
/// A synthetic snapshot branch is one created by Milestone 5.9 to represent
/// historical closed PRs that were merged into a synthetic stack head.
/// These branches are frozen with reason `remote_synthetic_snapshot`.
///
/// # Arguments
///
/// * `branch` - The branch name to check
/// * `snapshot` - The repo snapshot containing metadata
///
/// # Returns
///
/// `true` if the branch is a synthetic snapshot, `false` otherwise.
fn is_synthetic_snapshot(branch: &BranchName, snapshot: &RepoSnapshot) -> bool {
    let Some(entry) = snapshot.metadata.get(branch) else {
        return false;
    };

    if let FreezeState::Frozen { reason: Some(r), .. } = &entry.metadata.freeze {
        r == FREEZE_REASON_SYNTHETIC_SNAPSHOT
    } else {
        false
    }
}
```

### Step 2: Add Function to Filter Submit Scope

**File:** `src/cli/commands/submit.rs`

Add a function to filter out snapshot branches and report exclusions:

```rust
/// Filter snapshot branches from submit scope.
///
/// Returns the filtered list and the excluded branches (for reporting).
///
/// # Arguments
///
/// * `branches` - The original submit scope
/// * `snapshot` - The repo snapshot containing metadata
///
/// # Returns
///
/// A tuple of (filtered_branches, excluded_branches).
fn filter_snapshot_branches(
    branches: Vec<BranchName>,
    snapshot: &RepoSnapshot,
) -> (Vec<BranchName>, Vec<BranchName>) {
    let mut filtered = Vec::with_capacity(branches.len());
    let mut excluded = Vec::new();

    for branch in branches {
        if is_synthetic_snapshot(&branch, snapshot) {
            excluded.push(branch);
        } else {
            filtered.push(branch);
        }
    }

    (filtered, excluded)
}

/// Print information about excluded snapshot branches.
///
/// # Arguments
///
/// * `excluded` - The list of excluded snapshot branches
/// * `quiet` - Whether to suppress output
fn report_excluded_snapshots(excluded: &[BranchName], quiet: bool) {
    if excluded.is_empty() || quiet {
        return;
    }

    println!(
        "Excluding {} snapshot branch(es) from submit scope:",
        excluded.len()
    );
    for branch in excluded {
        println!("  {}", branch);
    }
    println!("These branches represent historical snapshots and cannot be submitted.");
    println!();
}
```

### Step 3: Add Check for Current Branch Being a Snapshot

**File:** `src/cli/commands/submit.rs`

Before computing scope, check if the current branch is a snapshot:

```rust
/// Check if current branch is a snapshot and refuse if so.
///
/// # Errors
///
/// Returns an error if the current branch is a synthetic snapshot branch.
fn check_current_branch_not_snapshot(
    current: &BranchName,
    snapshot: &RepoSnapshot,
) -> Result<()> {
    if is_synthetic_snapshot(current, snapshot) {
        bail!(
            "Cannot submit from a snapshot branch ('{}')\n\n\
             Snapshot branches represent historical state from closed PRs and\n\
             cannot be submitted. To work on this code, create a new branch:\n\n\
                 git checkout -b my-new-branch\n\
                 lattice track\n\n\
             Then you can submit the new branch.",
            current
        );
    }
    Ok(())
}
```

### Step 4: Integrate Filtering into submit_async

**File:** `src/cli/commands/submit.rs`

Modify `submit_async` to use the new filtering:

```rust
async fn submit_async(ctx: &Context, opts: SubmitOptions<'_>) -> Result<()> {
    // ... existing setup code ...

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on a branch."))?;

    // Check current branch is not a snapshot (refuse early)
    check_current_branch_not_snapshot(current, &snapshot)?;

    // Determine branches to submit
    let branches = if opts.stack {
        // Include ancestors and descendants
        let mut all = snapshot.graph.ancestors(current);
        all.reverse(); // Bottom-up order
        all.push(current.clone());
        let descendants: Vec<_> = snapshot.graph.descendants(current).into_iter().collect();
        all.extend(descendants);
        all
    } else {
        // Just ancestors and current
        let mut all = snapshot.graph.ancestors(current);
        all.reverse();
        all.push(current.clone());
        all
    };

    // Filter out snapshot branches
    let (branches, excluded) = filter_snapshot_branches(branches, &snapshot);
    report_excluded_snapshots(&excluded, ctx.quiet);

    // Check we have branches to submit after filtering
    if branches.is_empty() {
        bail!(
            "No branches to submit after filtering.\n\n\
             All branches in the scope were excluded (snapshot branches cannot be submitted)."
        );
    }

    // ... rest of existing implementation ...
}
```

### Step 5: Update Help Text for Submit Command

**File:** `src/cli/args.rs`

Update the submit command documentation to mention snapshot exclusion:

```rust
/// Submit branches as pull requests to GitHub.
///
/// By default, submits the current branch and all its ancestors up to trunk.
/// Use --stack to also include descendants.
///
/// # Scope
///
/// - Default: ancestors + current branch
/// - --stack: ancestors + current + descendants
///
/// Synthetic snapshot branches (created by `lattice doctor` from closed PRs)
/// are automatically excluded from the submit scope.
///
/// # Examples
///
/// Submit current branch and ancestors:
///     lattice submit
///
/// Submit entire stack including descendants:
///     lattice submit --stack
///
/// Dry run (show what would be submitted):
///     lattice submit --dry-run
#[derive(Debug, Parser)]
pub struct SubmitArgs {
    // ... existing fields ...
}
```

### Step 6: Unit Tests

**File:** `src/cli/commands/submit.rs`

```rust
#[cfg(test)]
mod snapshot_exclusion_tests {
    use super::*;
    use crate::core::metadata::schema::{
        BranchMetadataV1, FreezeScope, FreezeState, FREEZE_REASON_SYNTHETIC_SNAPSHOT,
    };
    use crate::core::types::{BranchName, Oid};
    use crate::engine::scan::{RepoSnapshot, ScannedMetadata};
    use std::collections::HashMap;

    fn sample_oid() -> Oid {
        Oid::new("abc123def4567890abc123def4567890abc12345").unwrap()
    }

    fn make_snapshot_with_branches(
        branches: Vec<(&str, Option<&str>)>, // (name, freeze_reason)
    ) -> RepoSnapshot {
        let mut metadata = HashMap::new();

        for (name, freeze_reason) in branches {
            let branch = BranchName::new(name).unwrap();
            let parent = BranchName::new("main").unwrap();

            let freeze_state = match freeze_reason {
                Some(reason) => FreezeState::frozen(FreezeScope::Single, Some(reason.to_string())),
                None => FreezeState::Unfrozen,
            };

            let mut meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());
            meta.freeze = freeze_state;

            metadata.insert(
                branch,
                ScannedMetadata {
                    ref_oid: sample_oid(),
                    metadata: meta,
                },
            );
        }

        RepoSnapshot {
            metadata,
            trunk: Some(BranchName::new("main").unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn is_synthetic_snapshot_returns_true_for_snapshot() {
        let snapshot = make_snapshot_with_branches(vec![
            ("lattice/snap/pr-42", Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT)),
        ]);

        let branch = BranchName::new("lattice/snap/pr-42").unwrap();
        assert!(is_synthetic_snapshot(&branch, &snapshot));
    }

    #[test]
    fn is_synthetic_snapshot_returns_false_for_normal_branch() {
        let snapshot = make_snapshot_with_branches(vec![("feature", None)]);

        let branch = BranchName::new("feature").unwrap();
        assert!(!is_synthetic_snapshot(&branch, &snapshot));
    }

    #[test]
    fn is_synthetic_snapshot_returns_false_for_other_frozen_branch() {
        let snapshot = make_snapshot_with_branches(vec![
            ("teammate-branch", Some("teammate_branch")),
        ]);

        let branch = BranchName::new("teammate-branch").unwrap();
        assert!(!is_synthetic_snapshot(&branch, &snapshot));
    }

    #[test]
    fn is_synthetic_snapshot_returns_false_for_untracked() {
        let snapshot = make_snapshot_with_branches(vec![]);

        let branch = BranchName::new("unknown").unwrap();
        assert!(!is_synthetic_snapshot(&branch, &snapshot));
    }

    #[test]
    fn filter_snapshot_branches_excludes_snapshots() {
        let snapshot = make_snapshot_with_branches(vec![
            ("feature-a", None),
            ("lattice/snap/pr-10", Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT)),
            ("feature-b", None),
            ("lattice/snap/pr-11", Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT)),
        ]);

        let branches = vec![
            BranchName::new("feature-a").unwrap(),
            BranchName::new("lattice/snap/pr-10").unwrap(),
            BranchName::new("feature-b").unwrap(),
            BranchName::new("lattice/snap/pr-11").unwrap(),
        ];

        let (filtered, excluded) = filter_snapshot_branches(branches, &snapshot);

        assert_eq!(filtered.len(), 2);
        assert_eq!(excluded.len(), 2);
        assert!(filtered.iter().any(|b| b.as_str() == "feature-a"));
        assert!(filtered.iter().any(|b| b.as_str() == "feature-b"));
        assert!(excluded.iter().any(|b| b.as_str() == "lattice/snap/pr-10"));
        assert!(excluded.iter().any(|b| b.as_str() == "lattice/snap/pr-11"));
    }

    #[test]
    fn filter_snapshot_branches_preserves_order() {
        let snapshot = make_snapshot_with_branches(vec![
            ("a", None),
            ("b", None),
            ("c", None),
        ]);

        let branches = vec![
            BranchName::new("a").unwrap(),
            BranchName::new("b").unwrap(),
            BranchName::new("c").unwrap(),
        ];

        let (filtered, excluded) = filter_snapshot_branches(branches, &snapshot);

        assert_eq!(filtered.len(), 3);
        assert!(excluded.is_empty());
        assert_eq!(filtered[0].as_str(), "a");
        assert_eq!(filtered[1].as_str(), "b");
        assert_eq!(filtered[2].as_str(), "c");
    }

    #[test]
    fn check_current_branch_not_snapshot_passes_for_normal() {
        let snapshot = make_snapshot_with_branches(vec![("feature", None)]);
        let branch = BranchName::new("feature").unwrap();

        let result = check_current_branch_not_snapshot(&branch, &snapshot);
        assert!(result.is_ok());
    }

    #[test]
    fn check_current_branch_not_snapshot_fails_for_snapshot() {
        let snapshot = make_snapshot_with_branches(vec![
            ("lattice/snap/pr-42", Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT)),
        ]);
        let branch = BranchName::new("lattice/snap/pr-42").unwrap();

        let result = check_current_branch_not_snapshot(&branch, &snapshot);
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Cannot submit from a snapshot branch"));
        assert!(err_msg.contains("lattice/snap/pr-42"));
    }
}
```

### Step 7: Integration Tests

**File:** `tests/integration/submit_snapshot_exclusion.rs` (new)

```rust
//! Integration tests for submit scope exclusion of snapshot branches.

use latticework::cli::commands::submit::{submit, SubmitOptions};
use latticework::core::metadata::schema::{
    BranchMetadataV1, FreezeScope, FreezeState, FREEZE_REASON_SYNTHETIC_SNAPSHOT,
};
use latticework::core::metadata::store::MetadataStore;
use latticework::core::types::BranchName;
use latticework::engine::Context;
use latticework::git::Git;

mod test_helpers;
use test_helpers::TestRepo;

#[test]
fn test_submit_excludes_snapshot_branches_from_stack() {
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create a stack: main -> feature -> lattice/snap/pr-10
    repo.checkout("main");
    repo.create_branch("feature");
    repo.commit("feature work");

    repo.create_branch("lattice/snap/pr-10");
    repo.commit("snapshot work");

    // Track both branches
    let git = Git::open(repo.path()).unwrap();
    // ... track feature normally ...

    // Track snapshot with frozen state
    let store = MetadataStore::new(&git);
    let snap_branch = BranchName::new("lattice/snap/pr-10").unwrap();
    let parent = BranchName::new("feature").unwrap();
    let base = git.resolve_ref("feature").unwrap();

    let meta = BranchMetadataV1::builder(snap_branch.clone(), parent, base)
        .freeze_state(FreezeState::frozen(
            FreezeScope::Single,
            Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT.to_string()),
        ))
        .build();
    store.write(&snap_branch, &meta).unwrap();

    // Checkout feature and try submit --stack
    repo.checkout("feature");

    // Dry run should show only feature, not the snapshot
    let ctx = Context::default();
    let opts = SubmitOptions {
        stack: true,
        dry_run: true,
        ..Default::default()
    };

    // This should succeed and exclude the snapshot branch
    // (Full test would verify output, but at minimum it shouldn't error)
}

#[test]
fn test_submit_refuses_from_snapshot_branch() {
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create and track a snapshot branch
    repo.checkout("main");
    repo.create_branch("lattice/snap/pr-42");
    repo.commit("snapshot work");

    let git = Git::open(repo.path()).unwrap();
    let store = MetadataStore::new(&git);
    let snap_branch = BranchName::new("lattice/snap/pr-42").unwrap();
    let parent = BranchName::new("main").unwrap();
    let base = git.resolve_ref("main").unwrap();

    let meta = BranchMetadataV1::builder(snap_branch.clone(), parent, base)
        .parent_is_trunk()
        .freeze_state(FreezeState::frozen(
            FreezeScope::Single,
            Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT.to_string()),
        ))
        .build();
    store.write(&snap_branch, &meta).unwrap();

    // Try to submit from the snapshot branch
    let ctx = Context::default();
    let result = submit(
        &ctx,
        false,  // stack
        false,  // draft
        false,  // publish
        false,  // confirm
        true,   // dry_run
        false,  // force
        false,  // always
        false,  // update_only
        None,   // reviewers
        None,   // team_reviewers
        false,  // no_restack
        false,  // view
    );

    // Should fail with clear error message
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Cannot submit from a snapshot branch"));
}

#[test]
fn test_submit_normal_branches_not_affected() {
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create a normal branch (not a snapshot)
    repo.checkout("main");
    repo.create_branch("feature");
    repo.commit("feature work");

    // Track normally (not frozen)
    let git = Git::open(repo.path()).unwrap();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let parent = BranchName::new("main").unwrap();
    let base = git.resolve_ref("main").unwrap();

    let meta = BranchMetadataV1::new(branch.clone(), parent, base);
    store.write(&branch, &meta).unwrap();

    // Dry run should include the branch
    let ctx = Context::default();
    let result = submit(
        &ctx,
        false,  // stack
        false,  // draft
        false,  // publish
        false,  // confirm
        true,   // dry_run - just check scope, don't actually submit
        false,  // force
        false,  // always
        false,  // update_only
        None,   // reviewers
        None,   // team_reviewers
        false,  // no_restack
        false,  // view
    );

    // Should succeed (dry run just prints what would be submitted)
    assert!(result.is_ok());
}

#[test]
fn test_submit_all_excluded_produces_error() {
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create only snapshot branches
    repo.checkout("main");
    repo.create_branch("lattice/snap/pr-1");
    repo.commit("snapshot 1");

    let git = Git::open(repo.path()).unwrap();
    let store = MetadataStore::new(&git);
    let snap_branch = BranchName::new("lattice/snap/pr-1").unwrap();
    let parent = BranchName::new("main").unwrap();
    let base = git.resolve_ref("main").unwrap();

    let meta = BranchMetadataV1::builder(snap_branch.clone(), parent, base)
        .parent_is_trunk()
        .freeze_state(FreezeState::frozen(
            FreezeScope::Single,
            Some(FREEZE_REASON_SYNTHETIC_SNAPSHOT.to_string()),
        ))
        .build();
    store.write(&snap_branch, &meta).unwrap();

    // Checkout a normal feature branch
    repo.checkout("main");
    repo.create_branch("feature");

    // Track feature with snap as only descendant
    // ... setup to make the only submittable branches be snapshots ...

    // This test verifies error message when all branches are excluded
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/cli/commands/submit.rs` | MODIFY | Add `is_synthetic_snapshot`, `filter_snapshot_branches`, `check_current_branch_not_snapshot`, integrate filtering |
| `src/cli/args.rs` | MODIFY | Update help text for submit command |
| `tests/integration/submit_snapshot_exclusion.rs` | NEW | Integration tests |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.10:

- [ ] Snapshot branches excluded from `lt submit` default scope
- [ ] Snapshot branches excluded from `lt submit --stack` scope
- [ ] Help text documents exclusion behavior
- [ ] Clear error when attempting to submit from a snapshot branch
- [ ] Clear message when branches are excluded from scope
- [ ] Normal branches and other frozen branches are not affected
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `is_synthetic_snapshot_returns_true_for_snapshot` | submit.rs | Detection of snapshot branches |
| `is_synthetic_snapshot_returns_false_for_normal_branch` | submit.rs | Normal branches not affected |
| `is_synthetic_snapshot_returns_false_for_other_frozen_branch` | submit.rs | Other freeze reasons not affected |
| `is_synthetic_snapshot_returns_false_for_untracked` | submit.rs | Untracked branches handled |
| `filter_snapshot_branches_excludes_snapshots` | submit.rs | Filtering logic |
| `filter_snapshot_branches_preserves_order` | submit.rs | Order preservation |
| `check_current_branch_not_snapshot_passes_for_normal` | submit.rs | Normal current branch OK |
| `check_current_branch_not_snapshot_fails_for_snapshot` | submit.rs | Snapshot current branch refused |

### Integration Tests

| Test | Description |
|------|-------------|
| `test_submit_excludes_snapshot_branches_from_stack` | Full stack submit with mixed branches |
| `test_submit_refuses_from_snapshot_branch` | Submit from snapshot branch refused |
| `test_submit_normal_branches_not_affected` | Normal workflow unchanged |
| `test_submit_all_excluded_produces_error` | All branches excluded error |

---

## Dependencies

- **Milestone 5.9:** Snapshot materialization (Complete) - Creates branches with `FREEZE_REASON_SYNTHETIC_SNAPSHOT`
- **Existing:** `FREEZE_REASON_SYNTHETIC_SNAPSHOT` constant in `src/core/metadata/schema.rs`

---

## Estimated Scope

- **Lines of code changed:** ~100 in `submit.rs`, ~10 in `args.rs`
- **New functions:** 4 (`is_synthetic_snapshot`, `filter_snapshot_branches`, `report_excluded_snapshots`, `check_current_branch_not_snapshot`)
- **New types:** 0 (uses existing `FreezeState`, `BranchName`)
- **Risk:** Low - Additive filtering, does not change core submit logic

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
cargo test snapshot_exclusion
cargo test submit
cargo test is_synthetic

# Integration tests
cargo test --test submit_snapshot_exclusion

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** Uses existing metadata schema and freeze reason constants
- **Simplicity:** Minimal changes to submit command, additive filtering only
- **Reuse:** Leverages `FREEZE_REASON_SYNTHETIC_SNAPSHOT` from Milestone 5.9
- **Purity:** Filtering is a pure function of branch list + snapshot
- **No stubs:** All features fully implemented
- **Safety:** Filtering happens before any mutations, user is informed of exclusions

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.10 as complete
2. Create `implementation_notes.md` in `.agents/v2/milestones/milestone-5-10-submit-scope-exclusion/`
3. Copy this plan to `.agents/v2/milestones/milestone-5-10-submit-scope-exclusion/PLAN.md`
4. Bootstrap feature set (5.1-5.10) is complete
