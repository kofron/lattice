# Milestone 5.5: Bootstrap Fix Execution - Implementation Notes

## Summary

This milestone implemented the execution path for bootstrap fixes through the standard Doctor/Executor infrastructure. The key insight was that most of the execution infrastructure was already in place from earlier milestones.

## Implementation Details

### 1. RunGit Execution in exec.rs

Updated the `PlanStep::RunGit` handler to actually execute git commands instead of being a no-op:

```rust
PlanStep::RunGit {
    args,
    description,
    expected_effects,
} => {
    // Record intent in journal before executing
    journal.record_git_process(args.clone(), description);

    // Execute the git command
    let result = self.git.run_command(args)?;

    if !result.success {
        return Ok(StepResult::Abort { ... });
    }

    // Check for conflicts after git command (rebase/merge/cherry-pick)
    let git_state = self.git.state();
    if git_state.is_in_progress() {
        return Ok(StepResult::Pause { branch, git_state });
    }

    // Verify expected effects (refs were created/updated as expected)
    for effect in expected_effects {
        if self.git.try_resolve_ref(effect)?.is_none() {
            return Ok(StepResult::Abort { ... });
        }
    }

    Ok(StepResult::Continue)
}
```

### 2. Git::run_command Method

Added `run_command` to `src/git/interface.rs` for executing arbitrary git commands:

```rust
pub fn run_command(&self, args: &[String]) -> Result<GitCommandResult, GitError>
```

This method:
- Determines the correct working directory (work_dir or git_dir for bare repos)
- Executes git with the provided args
- Captures stdout, stderr, exit code
- Returns a `GitCommandResult` struct

### 3. Existing Infrastructure (No Changes Needed)

Found that several features were already implemented in earlier milestones:

- **DoctorProposed/DoctorApplied Events**: Already recorded in `src/cli/commands/mod.rs` (lines 353-371 and 400-412)
- **Post-bootstrap verification**: `fast_verify` already called in `src/engine/mod.rs` line 193, plus doctor re-scans after fixes

### 4. Integration Tests

Created `tests/bootstrap_fixes_integration.rs` with 17 tests covering:

- Fix generator correctness for TrackExisting, FetchAndTrack, LinkPR
- Precondition checking
- Parent inference from PR base_ref
- Edge cases (slashes in branch names, special URLs)
- Fix ID parsing roundtrips
- Doctor integration

## Key Design Decisions

### Follow the Leader

Per ARCHITECTURE.md Section 8.1, "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

This means:
- Bootstrap fixes use the same `Executor::execute()` path as all other commands
- Event recording happens in the command layer, not executor
- Verification uses the same `fast_verify` infrastructure

### RunGit Step Verification

The `RunGit` step now verifies expected effects by checking if refs exist after execution. This catches cases where git commands succeed but don't produce the expected results.

### Conflict Handling

If a git operation results in conflicts (detected via `git.state().is_in_progress()`), execution pauses and transitions to `awaiting_user` state per the standard conflict handling flow.

## Files Modified

| File | Changes |
|------|---------|
| `src/git/interface.rs` | Added `GitCommandResult` struct and `run_command` method |
| `src/git/mod.rs` | Exported `GitCommandResult` |
| `src/engine/exec.rs` | Updated `RunGit` handler to execute commands |
| `tests/bootstrap_fixes_integration.rs` | New integration test file |

## Testing

All tests pass:
- 680+ unit tests
- 17 new bootstrap fix integration tests
- Clippy clean with `-D warnings`

## What's Next

The bootstrap fix execution path is now complete. To fully utilize this for importing remote PRs, the scanner would need to be enhanced to:

1. Call `Forge::list_open_prs()` during scanning (gated by auth availability)
2. Detect the three bootstrap issue types from scan results
3. Generate appropriate issues in the health report

This scanner enhancement is outside the scope of this milestone but the execution path is ready.
