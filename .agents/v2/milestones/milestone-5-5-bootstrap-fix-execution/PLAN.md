# Milestone 5.5: Bootstrap Fix Execution

## Goal

Execute bootstrap fixes through the standard Doctor/Executor path, completing the bootstrap workflow from detection through application.

**Core principle from ARCHITECTURE.md Section 8.1:** "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

---

## Background

Milestone 5.4 implemented fix generators that produce `FixOption`s for bootstrap issues. This milestone completes the workflow by ensuring those fixes execute correctly through the standard executor path.

| Milestone | Component | Status |
|-----------|-----------|--------|
| 5.2 | `list_open_prs` capability | Complete |
| 5.3 | Bootstrap issue detection | Complete |
| 5.4 | Bootstrap fix generators | Complete |
| **5.5** | **Bootstrap fix execution** | **This milestone** |

The key insight is that Milestone 5.4 already implemented most of the planner logic needed for bootstrap fixes. This milestone focuses on:
1. Verifying the executor handles bootstrap plan steps correctly
2. Ensuring `RunGit` fetch steps work end-to-end
3. Recording proper events in the ledger
4. Adding integration tests for the complete workflow

---

## Spec References

- **ROADMAP.md Milestone 5.5** - Bootstrap fix execution deliverables
- **ARCHITECTURE.md Section 6.2** - Executor contract
- **ARCHITECTURE.md Section 8** - Doctor framework
- **ARCHITECTURE.md Section 3.4** - Event ledger
- **SPEC.md Section 8B.1** - Track command behavior

---

## Current State Analysis

### What Already Works (from Milestone 5.4)

1. **Fix generators** produce valid `FixOption`s with proper previews
2. **Planner** converts `MetadataChange::Create` to `WriteMetadataCas` steps
3. **Planner** converts `RefChange::Create` with `"(fetched from remote)"` to `RunGit` fetch steps
4. **Planner** parses description format to extract parent, frozen, and PR info
5. **Executor** handles `WriteMetadataCas` and `RunGit` steps

### What Needs Verification/Implementation

1. **RunGit fetch step execution** - The executor's `RunGit` handling is stubbed with a comment "we skip actual execution"
2. **Event recording for doctor fixes** - `DoctorProposed` and `DoctorApplied` events need to be recorded
3. **Post-verify after bootstrap** - Verify graph is still valid after applying bootstrap fixes
4. **Rollback on partial failure** - Ensure no orphan branches/metadata on failure
5. **Integration tests** - Full workflow tests from detection to application

---

## Design Decisions

### RunGit Execution

Per ARCHITECTURE.md Section 10.1, all Git interactions go through the Git interface. The executor's `RunGit` step needs to actually execute the git command.

**Current state in `exec.rs`:**
```rust
PlanStep::RunGit { args, description, .. } => {
    // RunGit would shell out to git - for now we skip actual execution
    // In a real implementation, we'd use std::process::Command
    journal.record_git_process(args.clone(), description);
    // ...
}
```

**Required change:** Implement actual git command execution for fetch operations.

### Event Recording

Per ARCHITECTURE.md Section 8.4:
- `DoctorProposed` - Record when fix options are presented
- `DoctorApplied` - Record after successful fix application with fingerprint

The `doctor` command handler needs to record these events at the appropriate points.

### Undo Support

Bootstrap fixes create metadata and potentially fetch branches. These should be undoable via the existing undo mechanism:
- Metadata creation is tracked in the journal
- Branch ref creation (from fetch) is tracked in journal via `expected_effects`

The existing `undo` command should work without modification, but we should verify.

### CAS Semantics for Fetch

When fetching a branch that doesn't exist locally:
- `old_oid` should be `None` (creating new ref)
- After fetch, the executor should verify the ref was created
- If the ref already exists (race condition), CAS should fail

---

## Implementation Steps

### Step 1: Implement RunGit Fetch Execution

**File:** `src/engine/exec.rs`

The `RunGit` step handler needs to actually execute git commands. For bootstrap, the critical command is:

```
git fetch origin <branch>:<ref>
```

Update the executor to run git commands:

```rust
PlanStep::RunGit {
    args,
    description,
    expected_effects,
} => {
    // Record intent in journal
    journal.record_git_process(args.clone(), description);

    // Execute the git command
    let result = self.git.run_command(args)?;
    
    if !result.success {
        return Ok(StepResult::Abort {
            error: format!("git command failed: {}", result.stderr),
        });
    }

    // Check for conflicts after git command
    let git_state = self.git.state();
    if git_state.is_in_progress() {
        // Conflict occurred - need to pause
        let branch = expected_effects
            .first()
            .and_then(|r| r.strip_prefix("refs/heads/"))
            .unwrap_or("unknown")
            .to_string();
        return Ok(StepResult::Pause {
            branch,
            git_state,
        });
    }

    // Verify expected effects (refs were created/updated)
    for effect in expected_effects {
        if self.git.try_resolve_ref(effect)?.is_none() {
            return Ok(StepResult::Abort {
                error: format!("expected ref '{}' was not created", effect),
            });
        }
    }

    Ok(StepResult::Continue)
}
```

### Step 2: Add `run_command` to Git Interface

**File:** `src/git/interface.rs`

Add a method to run arbitrary git commands:

```rust
/// Result of running a git command.
#[derive(Debug)]
pub struct GitCommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl Git {
    /// Run a git command with the given arguments.
    ///
    /// This is a low-level method for executing arbitrary git commands.
    /// Prefer specific methods (like `fetch_ref`) when available.
    ///
    /// # Arguments
    ///
    /// * `args` - Command arguments (excluding "git" itself)
    ///
    /// # Returns
    ///
    /// A `GitCommandResult` with stdout, stderr, and success status.
    pub fn run_command(&self, args: &[String]) -> Result<GitCommandResult, GitError> {
        use std::process::Command;

        let output = Command::new("git")
            .args(args)
            .current_dir(self.work_dir_or_git_dir())
            .output()
            .map_err(|e| GitError::CommandFailed(format!("failed to run git: {}", e)))?;

        Ok(GitCommandResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }
}
```

### Step 3: Record DoctorProposed Event

**File:** `src/cli/commands/doctor_cmd.rs` (or wherever doctor command is implemented)

When presenting fix options to the user, record a `DoctorProposed` event:

```rust
// After generating diagnosis and before presenting to user
if !diagnosis.fixes.is_empty() {
    let ledger = EventLedger::new(&git);
    let issue_ids: Vec<String> = diagnosis.issues.iter()
        .map(|i| i.id.as_str().to_string())
        .collect();
    let fix_ids: Vec<String> = diagnosis.fixes.iter()
        .map(|f| f.id.to_string())
        .collect();
    let _ = ledger.append(Event::doctor_proposed(issue_ids, fix_ids));
}
```

### Step 4: Record DoctorApplied Event

**File:** `src/cli/commands/doctor_cmd.rs`

After successfully applying fixes, record a `DoctorApplied` event:

```rust
// After executor.execute() succeeds
match result {
    ExecuteResult::Success { fingerprint } => {
        let ledger = EventLedger::new(&git);
        let fix_ids: Vec<String> = applied_fix_ids.iter()
            .map(|f| f.to_string())
            .collect();
        let _ = ledger.append(Event::doctor_applied(fix_ids, fingerprint.as_str()));
        // ... success handling
    }
    // ... other cases
}
```

### Step 5: Post-Verify Graph Validity

**File:** `src/engine/verify.rs` (or add to exec.rs)

After applying bootstrap fixes, verify the graph is still valid:

```rust
/// Verify graph validity after bootstrap fix application.
///
/// Checks:
/// 1. No cycles introduced
/// 2. All tracked branches have valid parents
/// 3. All base commits are ancestors of branch tips
pub fn verify_post_bootstrap(snapshot: &RepoSnapshot) -> Result<(), VerifyError> {
    // Check for cycles
    if let Some(cycle) = snapshot.graph.find_cycle() {
        return Err(VerifyError::CycleDetected(cycle));
    }

    // Check all tracked branches have valid parents
    for (branch, scanned) in &snapshot.metadata {
        if let crate::core::metadata::schema::ParentInfo::Branch { name } = &scanned.metadata.parent {
            let parent = crate::core::types::BranchName::new(name)
                .map_err(|e| VerifyError::InvalidParent(branch.to_string(), e.to_string()))?;
            
            // Parent must exist as a branch or be trunk
            let is_trunk = snapshot.trunk.as_ref().map(|t| t == &parent).unwrap_or(false);
            if !is_trunk && !snapshot.branches.contains_key(&parent) {
                return Err(VerifyError::ParentNotFound(
                    branch.to_string(),
                    name.clone(),
                ));
            }
        }
    }

    Ok(())
}
```

### Step 6: Integration Test - TrackExisting Workflow

**File:** `tests/integration/bootstrap_track_existing.rs` (new)

```rust
//! Integration test: Track existing local branch from open PR

use latticework::doctor::{Doctor, FixId};
use latticework::engine::scan::scan_with_remote;
use latticework::engine::exec::{execute, ExecuteResult};
use latticework::git::Git;

#[tokio::test]
async fn test_track_existing_branch_from_pr() {
    // Setup: Create a test repo with a local branch matching an open PR
    let (repo_dir, git) = setup_test_repo();
    
    // Create a local branch "feature" that isn't tracked
    git.run_command(&["checkout", "-b", "feature"]).unwrap();
    git.run_command(&["commit", "--allow-empty", "-m", "feature commit"]).unwrap();
    git.run_command(&["checkout", "main"]).unwrap();
    
    // Mock: Configure forge to return an open PR for "feature"
    let mock_forge = MockForge::new();
    mock_forge.add_open_pr(PullRequestSummary {
        number: 42,
        head_ref: "feature".to_string(),
        base_ref: "main".to_string(),
        ..Default::default()
    });
    
    // Scan with remote
    let snapshot = scan_with_remote(&git).await.unwrap();
    
    // Diagnose
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);
    
    // Find the track fix
    let fix_id = FixId::new("remote-pr-branch-untracked", "track", "feature");
    let fix = diagnosis.find_fix(&fix_id).expect("fix should exist");
    
    // Generate and execute repair plan
    let plan = doctor.plan_repairs(&[fix_id.clone()], &diagnosis, &snapshot).unwrap();
    let ctx = Context::default();
    let result = execute(&plan, &git, &ctx).unwrap();
    
    // Verify success
    assert!(matches!(result, ExecuteResult::Success { .. }));
    
    // Verify branch is now tracked
    let new_snapshot = scan(&git).unwrap();
    let branch = BranchName::new("feature").unwrap();
    assert!(new_snapshot.metadata.contains_key(&branch));
    
    // Verify metadata has correct parent and PR linkage
    let metadata = &new_snapshot.metadata.get(&branch).unwrap().metadata;
    assert_eq!(metadata.parent.branch_name(), Some("main"));
    assert!(metadata.pr.is_linked());
}
```

### Step 7: Integration Test - FetchAndTrack Workflow

**File:** `tests/integration/bootstrap_fetch_and_track.rs` (new)

```rust
//! Integration test: Fetch and track branch from open PR

#[tokio::test]
async fn test_fetch_and_track_pr_branch() {
    // Setup: Create a test repo with a remote branch not fetched locally
    let (repo_dir, git, remote_repo) = setup_test_repo_with_remote();
    
    // Create a branch on remote only
    remote_repo.run_command(&["checkout", "-b", "teammate-feature"]).unwrap();
    remote_repo.run_command(&["commit", "--allow-empty", "-m", "teammate work"]).unwrap();
    
    // Mock: Configure forge to return an open PR for the remote branch
    let mock_forge = MockForge::new();
    mock_forge.add_open_pr(PullRequestSummary {
        number: 99,
        head_ref: "teammate-feature".to_string(),
        base_ref: "main".to_string(),
        ..Default::default()
    });
    
    // Scan with remote
    let snapshot = scan_with_remote(&git).await.unwrap();
    
    // Verify branch doesn't exist locally
    let branch = BranchName::new("teammate-feature").unwrap();
    assert!(!snapshot.branches.contains_key(&branch));
    
    // Diagnose and find the fetch-and-track fix
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);
    let fix_id = FixId::new("remote-pr-branch-missing", "fetch-and-track", "teammate-feature");
    
    // Execute
    let plan = doctor.plan_repairs(&[fix_id], &diagnosis, &snapshot).unwrap();
    let result = execute(&plan, &git, &Context::default()).unwrap();
    
    // Verify success
    assert!(matches!(result, ExecuteResult::Success { .. }));
    
    // Verify branch now exists and is tracked as frozen
    let new_snapshot = scan(&git).unwrap();
    assert!(new_snapshot.branches.contains_key(&branch));
    assert!(new_snapshot.metadata.contains_key(&branch));
    
    let metadata = &new_snapshot.metadata.get(&branch).unwrap().metadata;
    assert!(metadata.frozen.is_frozen());
    assert!(metadata.pr.is_linked());
}
```

### Step 8: Integration Test - LinkPR Workflow

**File:** `tests/integration/bootstrap_link_pr.rs` (new)

```rust
//! Integration test: Link PR to tracked branch

#[tokio::test]
async fn test_link_pr_to_tracked_branch() {
    // Setup: Create a test repo with a tracked branch without PR linkage
    let (repo_dir, git) = setup_test_repo();
    
    // Create and track a branch
    git.run_command(&["checkout", "-b", "my-feature"]).unwrap();
    git.run_command(&["commit", "--allow-empty", "-m", "my work"]).unwrap();
    // Run: lattice track my-feature
    track_branch(&git, "my-feature", "main").unwrap();
    
    // Mock: Configure forge to return an open PR for this branch
    let mock_forge = MockForge::new();
    mock_forge.add_open_pr(PullRequestSummary {
        number: 123,
        head_ref: "my-feature".to_string(),
        base_ref: "main".to_string(),
        ..Default::default()
    });
    
    // Verify PR is not linked in metadata
    let snapshot = scan(&git).unwrap();
    let branch = BranchName::new("my-feature").unwrap();
    let metadata = &snapshot.metadata.get(&branch).unwrap().metadata;
    assert!(!metadata.pr.is_linked());
    
    // Scan with remote and diagnose
    let snapshot = scan_with_remote(&git).await.unwrap();
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);
    
    // Find and apply the link fix
    let fix_id = FixId::new("remote-pr-not-linked", "link", "my-feature");
    let plan = doctor.plan_repairs(&[fix_id], &diagnosis, &snapshot).unwrap();
    let result = execute(&plan, &git, &Context::default()).unwrap();
    
    // Verify success
    assert!(matches!(result, ExecuteResult::Success { .. }));
    
    // Verify PR is now linked
    let new_snapshot = scan(&git).unwrap();
    let metadata = &new_snapshot.metadata.get(&branch).unwrap().metadata;
    assert!(metadata.pr.is_linked());
    assert_eq!(metadata.pr.number(), Some(123));
}
```

### Step 9: Integration Test - Rollback on Failure

**File:** `tests/integration/bootstrap_rollback.rs` (new)

```rust
//! Integration test: Rollback on partial failure

#[tokio::test]
async fn test_rollback_on_fetch_failure() {
    // Setup: Create a repo where fetch will fail
    let (repo_dir, git) = setup_test_repo();
    
    // Mock: Configure forge to return PR for a branch that doesn't exist on remote
    let mock_forge = MockForge::new();
    mock_forge.add_open_pr(PullRequestSummary {
        number: 404,
        head_ref: "nonexistent-branch".to_string(),
        base_ref: "main".to_string(),
        ..Default::default()
    });
    
    // Scan and diagnose
    let snapshot = scan_with_remote(&git).await.unwrap();
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);
    
    // Find the fetch-and-track fix
    let fix_id = FixId::new("remote-pr-branch-missing", "fetch-and-track", "nonexistent-branch");
    let plan = doctor.plan_repairs(&[fix_id], &diagnosis, &snapshot).unwrap();
    
    // Execute - should fail
    let result = execute(&plan, &git, &Context::default()).unwrap();
    
    // Verify aborted
    assert!(matches!(result, ExecuteResult::Aborted { .. }));
    
    // Verify no orphan metadata was created
    let new_snapshot = scan(&git).unwrap();
    let branch = BranchName::new("nonexistent-branch").unwrap();
    assert!(!new_snapshot.metadata.contains_key(&branch));
    assert!(!new_snapshot.branches.contains_key(&branch));
}
```

### Step 10: Integration Test - Event Recording

**File:** `tests/integration/bootstrap_events.rs` (new)

```rust
//! Integration test: Event ledger recording

#[tokio::test]
async fn test_doctor_events_recorded() {
    let (repo_dir, git) = setup_test_repo_with_untracked_branch();
    
    // Initial ledger state
    let ledger = EventLedger::new(&git);
    let initial_count = ledger.count().unwrap_or(0);
    
    // Diagnose and show fixes (should record DoctorProposed)
    let snapshot = scan_with_remote(&git).await.unwrap();
    let doctor = Doctor::new();
    let diagnosis = doctor.diagnose(&snapshot);
    
    // Simulate presenting to user - this should record DoctorProposed
    record_doctor_proposed(&git, &diagnosis);
    
    // Verify DoctorProposed was recorded
    let events = ledger.recent(1).unwrap();
    assert!(matches!(
        events.first().map(|e| &e.event),
        Some(Event::DoctorProposed { .. })
    ));
    
    // Apply a fix
    let fix_id = FixId::new("remote-pr-branch-untracked", "track", "feature");
    let plan = doctor.plan_repairs(&[fix_id.clone()], &diagnosis, &snapshot).unwrap();
    let result = execute(&plan, &git, &Context::default()).unwrap();
    
    // Record DoctorApplied
    if let ExecuteResult::Success { fingerprint } = result {
        record_doctor_applied(&git, &[fix_id], &fingerprint);
    }
    
    // Verify DoctorApplied was recorded
    let events = ledger.recent(1).unwrap();
    assert!(matches!(
        events.first().map(|e| &e.event),
        Some(Event::DoctorApplied { .. })
    ));
}
```

### Step 11: Unit Tests for RunGit Execution

**File:** `src/engine/exec.rs`

Add unit tests for the `RunGit` step execution:

```rust
#[cfg(test)]
mod run_git_tests {
    use super::*;

    #[test]
    fn run_git_step_executes_command() {
        // This test requires a real git repo
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();
        
        // Initialize a git repo
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        
        let git = Git::open(repo_path).unwrap();
        
        // Test running a simple git command
        let result = git.run_command(&["status".to_string()]).unwrap();
        
        assert!(result.success);
        assert!(result.stdout.contains("On branch") || result.stdout.contains("No commits yet"));
    }

    #[test]
    fn run_git_step_reports_failure() {
        let temp_dir = tempfile::tempdir().unwrap();
        let repo_path = temp_dir.path();
        
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(repo_path)
            .output()
            .unwrap();
        
        let git = Git::open(repo_path).unwrap();
        
        // Run a command that will fail
        let result = git.run_command(&[
            "checkout".to_string(),
            "nonexistent-branch".to_string(),
        ]).unwrap();
        
        assert!(!result.success);
        assert!(!result.stderr.is_empty());
    }
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/engine/exec.rs` | MODIFY | Implement `RunGit` step execution |
| `src/git/interface.rs` | MODIFY | Add `run_command` method |
| `src/cli/commands/doctor_cmd.rs` | MODIFY | Record `DoctorProposed` and `DoctorApplied` events |
| `src/engine/verify.rs` | MODIFY | Add post-bootstrap verification |
| `tests/integration/bootstrap_track_existing.rs` | NEW | Integration test |
| `tests/integration/bootstrap_fetch_and_track.rs` | NEW | Integration test |
| `tests/integration/bootstrap_link_pr.rs` | NEW | Integration test |
| `tests/integration/bootstrap_rollback.rs` | NEW | Integration test |
| `tests/integration/bootstrap_events.rs` | NEW | Integration test |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.5:

- [ ] Fixes execute via Executor with CAS semantics
- [ ] `RunGit` steps actually execute git commands
- [ ] Failures roll back completely (no partial state)
- [ ] Undo works for bootstrap fixes (existing undo mechanism)
- [ ] `DoctorProposed` event recorded when fix options are presented
- [ ] `DoctorApplied` event recorded after successful fix application
- [ ] Post-verify confirms graph validity after execution
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `run_git_step_executes_command` | `exec.rs` | RunGit step execution |
| `run_git_step_reports_failure` | `exec.rs` | RunGit failure handling |
| `run_command_returns_stdout` | `interface.rs` | Git command execution |
| `run_command_captures_stderr` | `interface.rs` | Error capture |

### Integration Tests

| Test | Description |
|------|-------------|
| `test_track_existing_branch_from_pr` | Full TrackExisting workflow |
| `test_fetch_and_track_pr_branch` | Full FetchAndTrack workflow |
| `test_link_pr_to_tracked_branch` | Full LinkPR workflow |
| `test_rollback_on_fetch_failure` | Rollback on partial failure |
| `test_doctor_events_recorded` | Event ledger recording |

---

## Dependencies

- **Milestone 5.4:** Bootstrap fix generators (Complete)
- **Milestone 1.1:** Doctor fix execution infrastructure (Complete)

---

## Estimated Scope

- **Lines of code changed:** ~100 in `exec.rs`, ~50 in `interface.rs`, ~30 in `doctor_cmd.rs`, ~20 in `verify.rs`
- **New functions:** 2 (`run_command`, `verify_post_bootstrap`)
- **Integration tests:** 5 new test files
- **Risk:** Low - mostly wiring existing infrastructure together

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
cargo test exec
cargo test bootstrap
cargo test doctor

# Integration tests
cargo test --test bootstrap_track_existing
cargo test --test bootstrap_fetch_and_track
cargo test --test bootstrap_link_pr
cargo test --test bootstrap_rollback
cargo test --test bootstrap_events

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** Uses existing executor contract from ARCHITECTURE.md Section 6.2
- **Simplicity:** Most infrastructure already exists; this milestone connects the pieces
- **Reuse:** Leverages existing journal, ledger, and executor
- **Purity:** Fix generators remain pure; execution is the imperative shell

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.5 as complete
2. Create `implementation_notes.md` in this directory
3. The bootstrap workflow (5.1-5.5) is now complete
4. Milestone 5.6 (Init Hint) and 5.7 (Local-Only Bootstrap) can proceed
