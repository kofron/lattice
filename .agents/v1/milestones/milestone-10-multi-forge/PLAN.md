# Milestone 10: Phase 4 Multi-Forge Scaffolding Activation

## Summary

Prove the architecture boundary works: core depends on `Forge`, not GitHub. This milestone adds a GitLab stub behind a feature flag and refactors command code to use forge selection, ensuring that swapping forge selection does not require touching planner/executor logic.

**Goal:** Demonstrate that adding a new forge is purely additive and doesn't destabilize the core.

---

## Deliverables

Per ROADMAP.md:

1. **GitLabForge stub** behind `gitlab` feature flag
   - Returns `NotImplemented` for all operations
   - Compiles and is selectable in config

2. **Command abstraction**
   - No command code imports GitHub-specific types except inside adapter module
   - All commands use a forge factory/selector function

3. **Integration tests**
   - Selecting unsupported forge produces stable, actionable errors
   - Forge selection respects config

---

## Implementation Steps

### Step 1: Add Feature Flag (`Cargo.toml`)

Add `gitlab` feature flag:
```toml
[features]
gitlab = []  # Enable GitLab forge (stub)
```

### Step 2: Create GitLabForge Stub (`src/forge/gitlab.rs`) - NEW

Implement stub that returns `NotImplemented` for all operations:

```rust
pub struct GitLabForge {
    // Configuration fields
}

impl Forge for GitLabForge {
    fn name(&self) -> &'static str { "gitlab" }
    
    // All methods return NotImplemented error
}
```

Include URL parsing for GitLab remotes:
- `git@gitlab.com:owner/repo.git`
- `https://gitlab.com/owner/repo.git`

### Step 3: Create Forge Factory (`src/forge/factory.rs`) - NEW

Central forge selection logic:

```rust
/// Forge provider enum for selection
pub enum ForgeProvider {
    GitHub,
    #[cfg(feature = "gitlab")]
    GitLab,
}

/// Create a forge from remote URL and token
pub fn create_forge(
    remote_url: &str,
    token: &str,
    provider_override: Option<&str>,
) -> Result<Box<dyn Forge>, ForgeError>

/// Detect forge provider from remote URL
pub fn detect_provider(remote_url: &str) -> Option<ForgeProvider>
```

### Step 4: Update Forge Module (`src/forge/mod.rs`)

- Export `gitlab` module behind feature flag
- Export `factory` module
- Re-export factory functions

### Step 5: Update Config Schema (`src/core/config/schema.rs`)

Update `VALID_FORGES` to conditionally include gitlab:

```rust
pub fn valid_forges() -> &'static [&'static str] {
    #[cfg(feature = "gitlab")]
    return &["github", "gitlab"];
    #[cfg(not(feature = "gitlab"))]
    return &["github"];
}
```

### Step 6: Refactor Commands to Use Factory

Update these files to use `forge::create_forge()` instead of direct `GitHubForge`:

- `src/cli/commands/submit.rs`
- `src/cli/commands/sync.rs`
- `src/cli/commands/get.rs`
- `src/cli/commands/merge.rs`

Pattern:
```rust
// Before:
let forge = crate::forge::github::GitHubForge::from_remote_url(&remote_url, &token)
    .ok_or_else(|| anyhow!("Not a GitHub remote"))?;

// After:
let forge = crate::forge::create_forge(&remote_url, &token, config_forge_override)?;
```

### Step 7: Integration Tests (`tests/multi_forge.rs`) - NEW

Tests:
1. **Unsupported forge error**: Selecting gitlab without feature returns clear error
2. **GitLab stub returns NotImplemented**: When feature enabled, all operations fail gracefully
3. **GitHub still works**: Ensure refactor didn't break existing functionality
4. **URL detection**: Correct provider detected from remote URLs

### Step 8: Milestone Documentation

Create `.agents/v1/milestones/milestone-10-multi-forge/`:
- `PLAN.md` - This plan
- `implementation_notes.md` - Post-implementation notes

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` | Modify | Add `gitlab` feature flag |
| `src/forge/mod.rs` | Modify | Export gitlab and factory modules |
| `src/forge/gitlab.rs` | NEW | GitLab stub implementation |
| `src/forge/factory.rs` | NEW | Forge selection logic |
| `src/core/config/schema.rs` | Modify | Update valid forges |
| `src/cli/commands/submit.rs` | Modify | Use forge factory |
| `src/cli/commands/sync.rs` | Modify | Use forge factory |
| `src/cli/commands/get.rs` | Modify | Use forge factory |
| `src/cli/commands/merge.rs` | Modify | Use forge factory |
| `tests/multi_forge.rs` | NEW | Integration tests |

---

## Test Requirements

| Test | Description |
|------|-------------|
| `gitlab_not_implemented` | All GitLab operations return NotImplemented |
| `url_detection_github` | GitHub URLs detected correctly |
| `url_detection_gitlab` | GitLab URLs detected correctly |
| `unknown_forge_error` | Unknown forge returns actionable error |
| `config_forge_override` | Config can override auto-detection |
| `github_commands_still_work` | Existing GitHub tests pass after refactor |

---

## Acceptance Gates

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] `cargo doc --no-deps` succeeds
- [ ] `cargo test --features gitlab` passes
- [ ] GitLabForge stub implemented behind feature flag
- [ ] No command code imports `github::` types directly
- [ ] Unsupported forge produces clear error message
- [ ] Existing GitHub integration tests still pass
- [ ] Swapping forge selection doesn't require touching planner/executor logic
- [ ] Milestone documentation complete

---

## Architecture Validation

This milestone validates ARCHITECTURE.md Section 11 (Host Adapter Architecture):

> "The adapter boundary ensures core logic remains independent of specific forge implementations."

By adding a stub forge that returns `NotImplemented`:
1. We prove commands compile without GitHub-specific imports
2. We prove forge selection is config-driven
3. We prove error handling is graceful and actionable

---

## Notes

- GitLab URL formats to support:
  - SSH: `git@gitlab.com:owner/repo.git`
  - HTTPS: `https://gitlab.com/owner/repo.git`
  - Self-hosted: Configurable base URL
  
- Feature flag naming: `gitlab` (lowercase, matching forge name)

- The stub is intentionally minimal - it exists to prove the architecture, not to implement GitLab support
