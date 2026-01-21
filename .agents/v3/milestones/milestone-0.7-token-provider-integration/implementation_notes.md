# Milestone 0.7: Implementation Notes

## Completion Date
2026-01-20

## Summary
Successfully integrated `TokenProvider` with `GitHubForge` to enable per-request token refresh and 401/403 retry logic.

## Key Implementation Decisions

### 1. Dual Token Sources
The `GitHubForge` struct now supports two authentication modes:
- `token_provider: Option<Arc<dyn TokenProvider>>` - Preferred, enables automatic refresh
- `static_token: Option<String>` - Deprecated, for backwards compatibility

This approach allows gradual migration without breaking existing code.

### 2. Retry Strategy
All Forge trait methods now implement a single-retry pattern:
1. Execute request with fresh headers (calls `get_bearer_token()`)
2. If 401/403 and TokenProvider is present: refresh and retry once
3. If retry fails or no TokenProvider: return error

Rationale: If the first refresh doesn't fix the auth issue, further retries won't help.

### 3. Per-Request Token Retrieval
The `headers()` method is now async and calls `get_bearer_token()` for every request. This ensures:
- Tokens are always fresh (TokenProvider handles refresh logic)
- The auth lock in `GitHubAuthManager` prevents concurrent refresh races

### 4. Deprecation Strategy
Old constructors are marked with `#[deprecated(since = "0.7.0", ...)]`:
- `GitHubForge::new()`
- `GitHubForge::with_api_base()`
- `GitHubForge::from_remote_url()`

Test files and the factory use `#[allow(deprecated)]` for backwards compatibility.

## Files Modified

| File | Changes |
|------|---------|
| `src/forge/github.rs` | Core TokenProvider integration, retry logic, new constructors |
| `src/cli/commands/mod.rs` | Updated `create_forge_for_deep_analysis()` to use TokenProvider |
| `src/engine/scan.rs` | Updated `create_forge_and_query()` to use TokenProvider |
| `src/cli/commands/init.rs` | Updated `try_show_bootstrap_hint()` to use TokenProvider |
| `src/forge/factory.rs` | Added `#[allow(deprecated)]` for backwards compat |
| `tests/github_integration.rs` | Fixed argument order, added `#[allow(deprecated)]` |

## Tests Added

New test module `github_forge_with_provider` with:
- `new_with_provider_creates_forge`
- `new_with_provider_and_api_base`
- `from_remote_url_with_provider`
- `from_remote_url_with_provider_invalid_url`
- `debug_does_not_expose_token_provider`
- `get_bearer_token_uses_provider`
- `get_bearer_token_uses_static_token`
- `is_retryable_auth_error_returns_true_for_auth_failed`
- `is_retryable_auth_error_returns_false_for_other_errors`

## Bug Fix During Implementation
Fixed argument order in `tests/github_integration.rs` - tests were calling `GitHubForge::new(owner, repo, token)` but the correct signature is `GitHubForge::new(token, owner, repo)`.

## Acceptance Gates Status

- [x] `GitHubForge` uses `TokenProvider` not raw token
- [x] `bearer_token()` called per request (via `get_bearer_token()`)
- [x] 401/403 triggers one retry with fresh token
- [x] TokenProvider refresh holds auth-scoped lock (via `GitHubAuthManager`)
- [x] Tokens never appear in Debug output
- [x] `cargo test` passes (843 unit tests + 64 doc tests)
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo fmt --check` passes

## Verification Commands Run
```bash
cargo check        # Type checking
cargo clippy -- -D warnings  # Linting
cargo test         # All tests
cargo fmt --check  # Formatting
cargo test github_forge  # Specific module tests (16 passed)
```
