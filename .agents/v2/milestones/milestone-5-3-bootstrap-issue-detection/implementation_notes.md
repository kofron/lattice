# Milestone 5.3: Bootstrap Issue Detection - Implementation Notes

## Summary

Successfully implemented bootstrap issue detection for the Doctor framework. The scanner can now query the forge for open PRs and generate appropriate issues to help users import existing PRs into Lattice tracking.

## Changes Made

### 1. `src/doctor/issues.rs`

Added 4 new `KnownIssue` variants:

- `RemoteOpenPullRequestsDetected` - Info severity, indicates open PRs exist on remote
- `RemoteOpenPrBranchMissingLocally` - Warning severity, PR head branch doesn't exist locally
- `RemoteOpenPrBranchUntracked` - Warning severity, local branch exists but isn't tracked
- `RemoteOpenPrNotLinkedInMetadata` - Info severity, tracked branch but no PR linkage

Updated `issue_id()`, `severity()`, and `to_issue()` implementations for each variant.

### 2. `src/engine/health.rs`

Added 4 new issue constructor functions in the `issues` module:

- `remote_open_prs_detected(count: usize, truncated: bool)`
- `remote_pr_branch_missing(number: u64, head_ref: &str, base_ref: &str, url: &str)`
- `remote_pr_branch_untracked(branch: &str, number: u64, url: &str)`
- `remote_pr_not_linked(branch: &str, number: u64, url: &str)`

### 3. `src/engine/scan.rs`

Added:

- `RemotePrEvidence` struct to store forge query results
- `remote_prs: Option<RemotePrEvidence>` field to `RepoSnapshot`
- `scan_with_remote()` async function for remote-aware scanning
- `query_remote_prs()` helper to check capabilities and query forge
- `create_forge_and_query()` helper to create forge and execute query
- `generate_bootstrap_issues()` to match PRs against local state

### 4. Test Helpers

Updated all test snapshot helper functions across the codebase to include `remote_prs: None`:

- `src/engine/scan.rs` (2 helpers)
- `src/engine/gate.rs`
- `src/engine/verify.rs`
- `src/cli/commands/stack_comment_ops.rs`
- `src/doctor/generators.rs`
- `src/doctor/planner.rs`
- `src/doctor/mod.rs`

## Design Decisions

### Severity Levels

- **Info:** `RemoteOpenPullRequestsDetected` and `RemoteOpenPrNotLinkedInMetadata` - informational, no action required
- **Warning:** `RemoteOpenPrBranchMissingLocally` and `RemoteOpenPrBranchUntracked` - suggest actions to improve workflow
- **None are Blocking:** Bootstrap issues don't prevent local operations

### Capability Gating

Remote PR queries require all of:
- `TrunkKnown` - Need context for meaningful matching
- `RemoteResolved` - GitHub remote configured
- `AuthAvailable` - Token present
- `RepoAuthorized` - GitHub App installed

If any capability is missing, remote scanning is skipped silently (expected behavior).

### Fork PR Handling

Fork PRs (where `head_repo_owner.is_some()`) are skipped due to complex ownership semantics. This can be enhanced in a future milestone.

### Error Handling

API failures are logged with `eprintln!` as warnings but don't fail the scan. This follows the "graceful degradation" principle - users can still work locally even if remote queries fail.

## Acceptance Criteria Status

- [x] `RemoteOpenPullRequestsDetected` issue generated when open PRs exist
- [x] `RemoteOpenPrBranchMissingLocally` for PRs with no local branch (Warning)
- [x] `RemoteOpenPrBranchUntracked` for untracked local branches matching PRs (Warning)
- [x] `RemoteOpenPrNotLinkedInMetadata` for tracked branches without PR linkage (Info)
- [x] Scanner works offline (no remote issues, no errors)
- [x] Scanner survives API failures gracefully (log warning, continue)
- [x] Fork PRs are skipped
- [x] `cargo test` passes (668 tests)
- [x] `cargo clippy` passes
- [x] `cargo fmt --check` passes

## Test Coverage

Added comprehensive tests in `src/engine/scan.rs::bootstrap_issues`:

- `generates_open_prs_detected_issue` - Basic detection
- `generates_branch_missing_issue` - Missing local branch
- `generates_untracked_issue` - Untracked local branch
- `generates_not_linked_issue` - Tracked but unlinked
- `no_issue_when_pr_already_linked` - Skip already linked PRs
- `skips_fork_prs` - Fork handling
- `empty_evidence_no_issues` - Empty case
- `truncated_evidence_noted` - Truncation flag in message

Also added tests in `src/doctor/issues.rs` and `src/engine/health.rs` for the new issue types.

## Dependencies

- **Milestone 5.2:** Uses `list_open_prs()` method from `Forge` trait (completed)

## Next Steps

Milestone 5.4 (Bootstrap Fix Generators) can now use these issues to generate fix options for importing PRs into Lattice tracking.
