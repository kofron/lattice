# Milestone 5.6: Init Hint for Bootstrap

## Goal

Show a helpful hint after `lattice init` when bootstrap opportunities exist, guiding users to run `lattice doctor` to import their existing open PRs and branches.

**Core principle from ARCHITECTURE.md Section 1.2:** "Out-of-band changes are normal, not exceptional." Lattice should help users who already have work in progress on their repositories.

---

## Background

After completing the bootstrap infrastructure (Milestones 5.1-5.5), users can use `lattice doctor` to discover and import existing open PRs. However, users new to Lattice may not know this capability exists. This milestone improves the onboarding experience by surfacing this option at the most natural moment: right after `lattice init`.

| Component | Milestone | Status |
|-----------|-----------|--------|
| Degraded log mode | 5.1 | Complete |
| `list_open_prs` capability | 5.2 | Complete |
| Bootstrap issue detection | 5.3 | Complete |
| Bootstrap fix generators | 5.4 | Complete |
| Bootstrap fix execution | 5.5 | Complete |
| **Init hint** | **5.6** | **This milestone** |

---

## Spec References

- **ROADMAP.md Milestone 5.6** - Init hint deliverables
- **SPEC.md Section 8A.2** - `lattice init` command specification
- **ARCHITECTURE.md Section 5.2** - Capability gating
- **ARCHITECTURE.md Section 11** - Host adapter architecture

---

## Design Decisions

### Hint is Non-Fatal

The hint is purely informational. It MUST NOT:
- Block init from completing successfully
- Fail if auth is unavailable
- Fail if the API call fails
- Mutate any repository state or metadata

If the hint check fails for any reason, init silently succeeds without the hint.

### Lightweight Check

The hint performs a lightweight check using the already-implemented `list_open_prs` capability from Milestone 5.2. This is a single API call with a small limit (e.g., 10 PRs) to detect "any open PRs exist" without fetching the full list.

### Auth-Gated

The hint only appears when authentication is available. This avoids:
- Prompting users who haven't authenticated yet
- Making unauthenticated API calls that would fail

If `AuthAvailable` capability is not satisfied, the hint is silently skipped.

### Message Format

The hint message should be:
- Brief (one or two lines)
- Actionable (tells user exactly what command to run)
- Non-alarming (phrased as an opportunity, not a problem)

Example:
```
Found 3 open PRs on remote. Run `lattice doctor` to import them.
```

### No Hint on Reset

When `lattice init --reset` is run, the hint is skipped. Users who are resetting likely know what they're doing and don't need guidance.

---

## Implementation Steps

### Step 1: Add Post-Init Hint Function

**File:** `src/cli/commands/init.rs`

Create a helper function that performs the lightweight forge check:

```rust
/// Check for open PRs on remote and print a hint if found.
///
/// This is a best-effort check that silently succeeds if:
/// - Authentication is not available
/// - The API call fails
/// - The remote cannot be resolved
///
/// The function is async because it makes a network call to the forge API.
///
/// # Arguments
///
/// * `git` - Git interface for resolving remote URL
/// * `ctx` - Execution context (for quiet mode check)
///
/// # Returns
///
/// Nothing. Errors are swallowed and result in no hint being shown.
async fn maybe_show_bootstrap_hint(git: &Git, ctx: &Context) {
    // Skip in quiet mode
    if ctx.quiet {
        return;
    }

    // Try to show the hint, swallowing all errors
    if let Err(_) = try_show_bootstrap_hint(git).await {
        // Silently skip hint on any error
    }
}

/// Internal implementation that can return errors for cleaner control flow.
async fn try_show_bootstrap_hint(git: &Git) -> Result<()> {
    use crate::auth::AuthManager;
    use crate::forge::{GitHubForge, ListPullsOpts};

    // Check if auth is available
    let auth_manager = AuthManager::new()?;
    if !auth_manager.is_authenticated("github.com") {
        return Ok(()); // No auth, skip silently
    }

    // Parse remote to get owner/repo
    let remote_url = git.remote_url("origin")?
        .ok_or_else(|| anyhow::anyhow!("no origin remote"))?;
    let (owner, repo) = parse_github_remote(&remote_url)?;

    // Create forge and check for open PRs (small limit for quick check)
    let forge = GitHubForge::new(
        auth_manager.token_provider("github.com"),
        &owner,
        &repo,
    );
    
    let opts = ListPullsOpts::with_limit(10);
    let result = forge.list_open_prs(opts).await?;

    // Show hint if PRs found
    if !result.pulls.is_empty() {
        let count = result.pulls.len();
        let suffix = if result.truncated { "+" } else { "" };
        println!(
            "Found {}{} open PR{}. Run `lattice doctor` to import {}.",
            count,
            suffix,
            if count == 1 { "" } else { "s" },
            if count == 1 { "it" } else { "them" }
        );
    }

    Ok(())
}
```

### Step 2: Wire Hint into Init Command

**File:** `src/cli/commands/init.rs`

Modify the `init` function to call the hint after successful initialization:

```rust
pub fn init(ctx: &Context, trunk: Option<&str>, reset: bool, force: bool) -> Result<()> {
    // ... existing init logic ...

    if !ctx.quiet {
        println!("Initialized Lattice with trunk: {}", trunk_name);
    }

    // Show bootstrap hint (non-fatal, async)
    // Skip on reset - users who reset likely know what they're doing
    if !reset {
        // Run async hint check in a blocking context
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .map(|rt| rt.block_on(maybe_show_bootstrap_hint(&git, ctx)));
    }

    Ok(())
}
```

**Alternative approach (if init is already async):**

If the init command runs in an async context, simply await the hint:

```rust
pub async fn init(ctx: &Context, trunk: Option<&str>, reset: bool, force: bool) -> Result<()> {
    // ... existing init logic ...

    if !ctx.quiet {
        println!("Initialized Lattice with trunk: {}", trunk_name);
    }

    // Show bootstrap hint (non-fatal)
    if !reset {
        maybe_show_bootstrap_hint(&git, ctx).await;
    }

    Ok(())
}
```

### Step 3: Add Helper to Parse GitHub Remote URL

**File:** `src/cli/commands/init.rs` or `src/forge/github.rs`

If not already available, add a helper to parse GitHub remote URLs:

```rust
/// Parse a GitHub remote URL to extract owner and repo.
///
/// Supports:
/// - HTTPS: `https://github.com/owner/repo.git`
/// - SSH: `git@github.com:owner/repo.git`
///
/// # Returns
///
/// Tuple of (owner, repo) as strings.
fn parse_github_remote(url: &str) -> Result<(String, String)> {
    // Try HTTPS format: https://github.com/owner/repo.git
    if let Some(path) = url.strip_prefix("https://github.com/") {
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // Try SSH format: git@github.com:owner/repo.git
    if let Some(path) = url.strip_prefix("git@github.com:") {
        let path = path.trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    bail!("Could not parse GitHub remote URL: {}", url)
}
```

**Note:** This helper may already exist in the codebase (e.g., in the GitHub forge or auth modules). Reuse existing implementation if available per the **Reuse** principle.

### Step 4: Handle Async Context

**File:** `src/cli/commands/init.rs`

The init command is currently synchronous. To call async forge methods, we need to bridge the sync/async boundary. Options:

**Option A: Use `tokio::runtime::Runtime`**

```rust
fn show_hint_sync(git: &Git, ctx: &Context) {
    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        rt.block_on(maybe_show_bootstrap_hint(git, ctx));
    }
}
```

**Option B: Make init async (if not already)**

This may require changes to the command dispatcher. Check how other async commands (like `submit`) handle this.

**Option C: Use existing async runtime**

If the CLI already has a tokio runtime in `main.rs`, use `tokio::task::block_in_place` or ensure init runs in an async context.

### Step 5: Unit Tests

**File:** `src/cli/commands/init.rs`

```rust
#[cfg(test)]
mod hint_tests {
    use super::*;

    #[test]
    fn parse_github_https_url() {
        let (owner, repo) = parse_github_remote("https://github.com/org/myrepo.git").unwrap();
        assert_eq!(owner, "org");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn parse_github_https_url_no_git_suffix() {
        let (owner, repo) = parse_github_remote("https://github.com/org/myrepo").unwrap();
        assert_eq!(owner, "org");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn parse_github_ssh_url() {
        let (owner, repo) = parse_github_remote("git@github.com:org/myrepo.git").unwrap();
        assert_eq!(owner, "org");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn parse_github_ssh_url_no_git_suffix() {
        let (owner, repo) = parse_github_remote("git@github.com:org/myrepo").unwrap();
        assert_eq!(owner, "org");
        assert_eq!(repo, "myrepo");
    }

    #[test]
    fn parse_github_invalid_url() {
        assert!(parse_github_remote("https://gitlab.com/org/repo").is_err());
        assert!(parse_github_remote("not-a-url").is_err());
    }
}
```

### Step 6: Integration Tests

**File:** `tests/integration/init_hint.rs` (new)

```rust
//! Integration tests for init command bootstrap hint

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

/// Setup: Create a test repo with git initialized.
fn setup_test_repo() -> TempDir {
    let temp = TempDir::new().unwrap();
    
    Command::new("git")
        .args(["init"])
        .current_dir(temp.path())
        .assert()
        .success();
    
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial"])
        .current_dir(temp.path())
        .assert()
        .success();
    
    temp
}

#[test]
fn init_succeeds_without_auth() {
    // Test that init works even when auth is not configured
    let temp = setup_test_repo();
    
    Command::cargo_bin("lattice")
        .unwrap()
        .args(["init", "--trunk", "main"])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(predicates::str::contains("Initialized Lattice"));
    
    // Hint is NOT shown because auth is not available
    // (This is correct behavior - we don't want errors)
}

#[test]
fn init_quiet_skips_hint() {
    let temp = setup_test_repo();
    
    Command::cargo_bin("lattice")
        .unwrap()
        .args(["init", "--trunk", "main", "--quiet"])
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(predicates::str::is_empty());
}

#[test]
fn init_reset_skips_hint() {
    let temp = setup_test_repo();
    
    // First init
    Command::cargo_bin("lattice")
        .unwrap()
        .args(["init", "--trunk", "main"])
        .current_dir(temp.path())
        .assert()
        .success();
    
    // Reset should not show hint
    Command::cargo_bin("lattice")
        .unwrap()
        .args(["init", "--reset", "--force"])
        .current_dir(temp.path())
        .assert()
        .success();
    
    // Verify hint NOT present (can't easily test this without mock)
}

#[test]
#[ignore] // Requires mock forge or real auth
fn init_shows_hint_when_prs_exist() {
    // This test would require:
    // 1. A mock forge that returns open PRs
    // 2. Or real authentication and a test repo with open PRs
    //
    // For now, mark as ignored. Can be enabled in CI with proper setup.
}
```

### Step 7: Documentation

**File:** `docs/commands/init.md` (update)

Add a note about the bootstrap hint:

```markdown
## Post-Init Hint

After successful initialization, Lattice checks for existing open PRs on the remote.
If any are found, a hint is displayed:

```
Initialized Lattice with trunk: main
Found 3 open PRs. Run `lattice doctor` to import them.
```

This hint helps users who already have work in progress quickly import their 
existing branches and PRs into Lattice tracking.

The hint:
- Only appears when authenticated with GitHub
- Silently skips if authentication is unavailable or API fails
- Does not appear during `--reset` operations
- Does not appear in `--quiet` mode
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/cli/commands/init.rs` | MODIFY | Add hint logic after init |
| `tests/integration/init_hint.rs` | NEW | Integration tests |
| `docs/commands/init.md` | MODIFY | Document hint behavior |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.6:

- [ ] Hint shown when open PRs detected and auth available
- [ ] No hint when offline or no auth (silent success)
- [ ] No metadata mutations during hint check
- [ ] Init succeeds regardless of hint check result
- [ ] `--quiet` mode suppresses hint
- [ ] `--reset` mode skips hint
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `parse_github_https_url` | `init.rs` | URL parsing |
| `parse_github_https_url_no_git_suffix` | `init.rs` | URL parsing variant |
| `parse_github_ssh_url` | `init.rs` | SSH URL parsing |
| `parse_github_invalid_url` | `init.rs` | Error handling |

### Integration Tests

| Test | Description |
|------|-------------|
| `init_succeeds_without_auth` | Init works without auth |
| `init_quiet_skips_hint` | Quiet mode suppresses hint |
| `init_reset_skips_hint` | Reset mode skips hint |
| `init_shows_hint_when_prs_exist` | Hint appears (requires mock) |

---

## Dependencies

- **Milestone 5.2:** `list_open_prs` capability (Complete)
- **Auth infrastructure:** Token provider and auth manager

---

## Estimated Scope

- **Lines of code changed:** ~80 in `init.rs`
- **New functions:** 3 (`maybe_show_bootstrap_hint`, `try_show_bootstrap_hint`, `parse_github_remote` if not existing)
- **Risk:** Low - hint is non-fatal and optional

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
cargo test init
cargo test hint

# Integration tests
cargo test --test init_hint

# Format check
cargo fmt --check
```

---

## Edge Cases and Error Handling

### Error Scenarios (All Should Be Silent)

1. **No auth configured** - Skip hint, init succeeds
2. **Auth token expired** - Skip hint, init succeeds
3. **Network timeout** - Skip hint, init succeeds
4. **Rate limited** - Skip hint, init succeeds
5. **Remote not GitHub** - Skip hint, init succeeds
6. **No origin remote** - Skip hint, init succeeds
7. **Invalid remote URL** - Skip hint, init succeeds
8. **App not installed** - Skip hint, init succeeds

### Success Scenarios

1. **Auth available, 0 PRs** - No hint shown
2. **Auth available, 1 PR** - "Found 1 open PR. Run `lattice doctor` to import it."
3. **Auth available, 5 PRs** - "Found 5 open PRs. Run `lattice doctor` to import them."
4. **Auth available, 10+ PRs (truncated)** - "Found 10+ open PRs. Run `lattice doctor` to import them."

---

## Notes

- **Follow the leader:** Reuses existing `list_open_prs` infrastructure from Milestone 5.2
- **Simplicity:** Minimal code addition with maximum user benefit
- **Purity:** Hint is read-only; no mutations allowed
- **Reuse:** Leverage existing auth and forge infrastructure

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.6 as complete
2. Create `implementation_notes.md` in this directory
3. Milestone 5.7 (Local-Only Bootstrap) can proceed
