# Milestone 1.3: Implementation Notes

## Summary

Implemented the `RepoAuthorized(owner, repo)` capability per SPEC.md Section 8E.0.1. This involved creating a GitHub installations API client, an authorization cache with 10-minute TTL, and integrating authorization checking into the scanner and gating system.

## Key Implementation Decisions

### 1. API Discovery Pattern (SPEC.md 8E.0.1)

Implemented the two-step discovery algorithm:

1. `GET /user/installations` - List all GitHub App installations accessible to the user
2. `GET /user/installations/{id}/repositories` - For each installation, list accessible repos

This supports pagination for users with many installations/repos:

```rust
// Pagination loop
loop {
    let response = /* fetch page */;
    // Check repos in this page
    // If found, return early
    // If more pages, continue
}
```

### 2. Authorization Cache (SPEC.md 8E.0.1)

Created `src/auth/cache.rs` with:

- **10-minute TTL** per SPEC.md requirement
- **Storage location**: `<common_dir>/lattice/cache/github_auth.json`
- **Case-insensitive keys**: `host/owner/repo` format
- **Cache invalidation**: Entries removed on 403/404 from API

```rust
pub struct AuthCacheEntry {
    pub installation_id: u64,
    pub repository_id: u64,
    pub cached_at: DateTime<Utc>,
}
```

### 3. Scanner Integration

The scanner now derives `RepoAuthorized` capability after verifying:
1. `AuthAvailable` - Valid token exists
2. `RemoteResolved` - Remote URL parsed to owner/repo

The capability check uses cached results when valid, falling back to API queries.

### 4. Blocking Issues

Added two new blocking issues to `src/engine/health.rs`:

- **`app_not_installed`** - GitHub App not installed for the repository
- **`repo_authorization_check_failed`** - API error during authorization check

Both provide clear user-action guidance per ARCHITECTURE.md Section 8.2:
```
GitHub App not installed for {owner}/{repo}.
Install at: https://github.com/apps/lattice/installations/new
```

### 5. Gating Updates

Updated `src/engine/gate.rs` to include `RepoAuthorized` in:
- `REMOTE` requirement set (for submit, sync with remote ops, etc.)
- `REMOTE_BARE_ALLOWED` requirement set

## Files Created

### New Modules
- `src/auth/installations.rs` - GitHub installations API client with pagination support
- `src/auth/cache.rs` - Authorization cache with TTL support

## Files Modified

### Auth Module
- `src/auth/mod.rs` - Export new modules (`pub mod cache`, `pub mod installations`)

### Engine
- `src/engine/capabilities.rs` - Added `RepoAuthorized` capability
- `src/engine/health.rs` - Added `app_not_installed` and `repo_authorization_check_failed` issues
- `src/engine/scan.rs` - Added `RepoAuthorized` capability derivation during scan
- `src/engine/gate.rs` - Added `RepoAuthorized` to `REMOTE` and `REMOTE_BARE_ALLOWED` requirement sets

## Test Coverage

### New Tests
- `src/auth/installations.rs` - Tests for API response parsing and pagination
- `src/auth/cache.rs` - Tests for cache TTL, key normalization, save/load

### Acceptance Gates Verified
- [x] `GET /user/installations` fetches user's app installations
- [x] `GET /user/installations/{id}/repositories` checks repo access (with pagination)
- [x] `RepoAuthorized(owner, repo)` capability derived by scanner
- [x] Authorization cached in `<common_dir>/lattice/cache/github_auth.json`
- [x] Cache TTL is 10 minutes
- [x] Cache invalidated on 403/404 from API
- [x] App-not-installed message includes install link

## Notes

### Security Considerations

Per SPEC.md Section 4.4.4, the implementation ensures:
- Tokens never appear in error messages
- API responses are not logged at debug level
- Custom Debug implementations redact sensitive fields

### Performance

The 10-minute cache significantly reduces API calls. For typical workflows:
- First command: API query
- Subsequent commands (within 10 min): Cache hit
- After TTL expires or on auth change: Re-query

### Error Handling

API errors are handled gracefully:
- 401/403: Suggest re-authentication
- 404: App not installed message with install link
- 5xx: Retry with backoff (handled by HTTP client)
