# Milestone 5.6: Init Hint - Implementation Notes

## Overview

Implemented the post-init bootstrap hint feature that shows users a helpful message about existing open PRs after `lattice init` completes successfully.

## Implementation Summary

### Files Modified

1. **`src/cli/commands/init.rs`**
   - Added `show_bootstrap_hint_sync()` - Bridges sync/async for the hint check
   - Added `maybe_show_bootstrap_hint()` - Async wrapper that swallows errors
   - Added `try_show_bootstrap_hint()` - Core implementation with error handling
   - Wired hint call at end of `init()` after successful initialization

2. **`tests/init_hint_integration.rs`** (NEW)
   - 7 integration tests covering various scenarios
   - 1 ignored test for live auth scenario

### Key Design Decisions

#### 1. Non-Fatal by Design

The hint is purely informational and MUST NOT block init from succeeding. All errors in the hint code path are silently swallowed:

```rust
async fn maybe_show_bootstrap_hint(git: &Git) {
    let _ = try_show_bootstrap_hint(git).await;
}
```

This ensures init works correctly in all environments, regardless of:
- Auth availability
- Network connectivity
- Remote configuration
- API rate limits

#### 2. Sync/Async Bridge

The `init` function is synchronous, but forge API calls are async. We bridge this with a minimal tokio runtime:

```rust
fn show_bootstrap_hint_sync(git: &Git) {
    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        rt.block_on(maybe_show_bootstrap_hint(git));
    }
}
```

This is acceptable because:
- It's a one-shot operation at the end of init
- The hint is optional and non-blocking
- Using `block_on` avoids making init async (which would require broader changes)

#### 3. Lightweight Check

Uses `list_open_prs` with a limit of 10 to quickly detect presence of PRs without fetching the full list. This minimizes API usage and response time.

#### 4. Hint Conditions

The hint is skipped when:
- `--quiet` mode is active
- `--reset` flag is used
- Auth is not available for github.com
- Remote URL is not a GitHub URL
- No origin remote exists

#### 5. Message Format

The message is designed to be:
- Brief (single line)
- Actionable (tells user what command to run)
- Non-alarming (phrased as an opportunity)

```
Found 3 open PRs. Run `lattice doctor` to import them.
```

For truncated results (10+ PRs):
```
Found 10+ open PRs. Run `lattice doctor` to import them.
```

### Reuse

The implementation leverages existing infrastructure:

- `has_github_auth()` from `src/auth/mod.rs` for quick auth check
- `parse_github_url()` from `src/forge/github.rs` for remote URL parsing
- `GitHubForge::list_open_prs()` from `src/forge/github.rs` for PR enumeration
- `Git::remote_url()` from `src/git/interface.rs` for remote URL resolution

### Tests

| Test | Description |
|------|-------------|
| `init_succeeds_without_auth` | Init works when auth is unavailable |
| `init_succeeds_with_non_github_remote` | Init works with GitLab/other remotes |
| `init_succeeds_without_origin` | Init works with no origin remote |
| `init_quiet_mode_skips_hint` | Quiet mode suppresses hint |
| `init_reset_skips_hint` | Reset mode doesn't show hint |
| `init_already_initialized_skips_hint` | Re-init doesn't show hint |
| `init_shows_hint_when_prs_exist` | (ignored) Requires real/mock auth |

## Verification

All tests pass:
```
cargo test init
# 6 passed, 1 ignored
```

Clippy clean:
```
cargo clippy -- -D warnings
# No warnings
```

## Future Considerations

1. **Mock Forge for Testing**: The positive case (hint shown when PRs exist) requires either real auth or a mock forge. A mock forge infrastructure would enable comprehensive testing.

2. **Configurable Behavior**: Could add a config option to disable the hint globally if users find it annoying.

3. **GitHub Enterprise**: Currently hardcoded to `github.com`. Would need extension for GHE support.
