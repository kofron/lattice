# Milestone 10: Implementation Notes

## Summary

Milestone 10 implements multi-forge scaffolding, proving the architecture boundary works: core depends on `Forge` trait, not GitHub. A GitLab stub behind a feature flag demonstrates that adding new forges is purely additive.

## Key Design Decisions

### 1. Forge Factory Pattern

Created `src/forge/factory.rs` as the central forge selection mechanism:

- `ForgeProvider` enum for type-safe provider identification
- `detect_provider(url)` for automatic provider detection from remote URLs
- `create_forge(url, token, override)` as the single entry point for commands

This pattern ensures commands never import forge-specific types directly.

### 2. Method Naming: `parse()` Instead of `from_str()`

Renamed `ForgeProvider::from_str()` to `ForgeProvider::parse()` to avoid clippy's `should_implement_trait` warning. The method returns `Option<Self>` rather than `Result`, so implementing `FromStr` would require a different error type. Using `parse()` is clearer and avoids confusion with the standard library trait.

### 3. Config Validation vs Feature Flags

`valid_forge_names()` returns `["github", "gitlab"]` always, even without the `gitlab` feature enabled. This allows users to:

1. Pre-configure GitLab in their config before enabling the feature
2. Get clear error messages when trying to use an unconfigured forge

The actual forge creation handles the feature flag check and returns `ForgeError::NotImplemented` with actionable guidance.

### 4. GitLab URL Parsing

GitLab supports nested groups (e.g., `group/subgroup/project`), unlike GitHub's flat `owner/repo` structure. The `parse_gitlab_url()` function handles this by treating everything before the last path segment as the "owner".

### 5. Error Messages Are Actionable

All forge-related errors include:
- What went wrong
- Available options
- Next steps (e.g., "Rebuild with `--features gitlab`")

## Files Changed

### New Files

| File | Purpose |
|------|---------|
| `src/forge/gitlab.rs` | GitLab stub returning NotImplemented |
| `src/forge/factory.rs` | Forge selection and creation logic |
| `tests/multi_forge.rs` | Integration tests for multi-forge support |
| `.agents/v1/milestones/milestone-10-multi-forge/PLAN.md` | Milestone plan |
| `.agents/v1/milestones/milestone-10-multi-forge/implementation_notes.md` | This file |

### Modified Files

| File | Changes |
|------|---------|
| `Cargo.toml` | Added `gitlab` feature flag |
| `src/forge/mod.rs` | Export gitlab (behind feature), factory |
| `src/core/config/schema.rs` | Use `valid_forge_names()` for validation |
| `src/cli/commands/submit.rs` | Use `create_forge()` instead of `GitHubForge` |
| `src/cli/commands/sync.rs` | Use `create_forge()` instead of `GitHubForge` |
| `src/cli/commands/get.rs` | Use `create_forge()` instead of `GitHubForge` |
| `src/cli/commands/merge.rs` | Use `create_forge()` instead of `GitHubForge` |

## Architecture Validation

This milestone validates ARCHITECTURE.md Section 11 (Host Adapter Architecture):

> "The adapter boundary ensures core logic remains independent of specific forge implementations."

Evidence:
1. No command code imports `github::` types - verified via `grep` showing no matches
2. Commands use `Box<dyn Forge>` from factory, not concrete types
3. Adding GitLab required no changes to planner/executor logic
4. Mock forge continues to work for testing

## Test Coverage

### Without `gitlab` Feature

```
cargo test
```

- Provider detection tests for GitHub
- Factory tests including "gitlab not enabled" error
- Valid forge names includes both github and gitlab

### With `gitlab` Feature

```
cargo test --features gitlab
```

- All GitLab stub methods return NotImplemented
- GitLab URL parsing handles nested groups
- Provider detection correctly identifies GitLab URLs
- Factory creates GitLab forge when feature enabled

## Acceptance Gates

All gates pass:

- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes  
- [x] `cargo test` passes
- [x] `cargo doc --no-deps` succeeds
- [x] `cargo test --features gitlab` passes
- [x] GitLabForge stub implemented behind feature flag
- [x] No command code imports `github::` types directly
- [x] Unsupported forge produces clear error message
- [x] Existing GitHub integration tests still pass
- [x] Swapping forge selection doesn't require touching planner/executor logic
- [x] Milestone documentation complete

## Future Work

When implementing actual GitLab support:

1. Replace `NotImplemented` returns with real API calls
2. GitLab uses "Merge Requests" terminology (MR vs PR)
3. GitLab API uses project IDs differently (numeric or encoded path)
4. Draft MRs use the `work_in_progress` field
5. Consider GraphQL vs REST tradeoffs (GitLab's GraphQL API differs from GitHub's)
