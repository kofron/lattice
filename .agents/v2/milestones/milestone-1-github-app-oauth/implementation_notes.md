# Milestone: GitHub App OAuth Implementation Notes

## Summary

Implemented GitHub App device flow OAuth as the only authentication method for Lattice, replacing PAT-based authentication. This includes token bundle storage, automatic refresh with concurrency protection, device flow UI, and complete removal of PAT support.

## Key Implementation Decisions

### 1. Client ID (Hardcoded)

The GitHub App client ID is hardcoded:
```
Iv23liIqb9vJ8kaRyZaU
```

This is the canonical Lattice GitHub App. The device flow requires no client secret, making it safe for CLI distribution.

### 2. Token Bundle Schema (SPEC.md 4.4.2)

Created `src/auth/token_bundle.rs` with the complete token storage format:

```rust
pub struct TokenBundle {
    pub kind: String,           // "lattice.github-app-oauth"
    pub schema_version: u32,    // 1
    pub host: String,           // "github.com"
    pub client_id: String,
    pub user: UserInfo,         // id + login
    pub tokens: TokenSet,       // access + refresh + expirations
    pub timestamps: Timestamps, // created_at + updated_at
}
```

Custom Debug implementations redact all token values per SPEC.md 4.4.4.

### 3. Auth Lock for Refresh Safety (SPEC.md 4.4.3)

Created `src/auth/lock.rs` implementing file-based locking:

- Lock path: `~/.lattice/auth/lock.<host>`
- Prevents double-refresh races across concurrent processes
- RAII guard pattern for automatic release
- Configurable timeout with default of 30 seconds

The refresh flow:
1. Check if access token is expired or near-expiry
2. Acquire auth lock
3. Re-check token (another process may have refreshed)
4. If still expired, perform refresh
5. Store new token bundle
6. Release lock

### 4. Device Flow Implementation

Created `src/auth/device_flow.rs` with full device flow support:

**Endpoints:**
- `POST https://github.com/login/device/code` - Get device code
- `POST https://github.com/login/oauth/access_token` - Poll for token

**Polling states handled:**
- `authorization_pending` - Continue polling
- `slow_down` - Increase interval by 5 seconds
- `expired_token` - Abort with error
- `access_denied` - Abort with error

**User experience:**
- Display verification URL and user code
- Optionally open browser (unless `--no-browser`)
- Poll in background until success/failure

### 5. TokenProvider Trait

Created `src/auth/provider.rs` with:

```rust
#[async_trait]
pub trait TokenProvider: Send + Sync {
    async fn bearer_token(&self) -> Result<String, AuthError>;
    fn is_authenticated(&self) -> bool;
}
```

`GitHubAuthManager` implements this trait:
- Loads token bundle from SecretStore
- Refreshes automatically before expiry (5-minute buffer)
- Never exposes tokens directly - only via `bearer_token()`

### 6. Complete PAT Removal

Per SPEC.md, removed all PAT support:
- No `--token` flag
- No `LATTICE_GITHUB_TOKEN` or `GITHUB_TOKEN` env var support
- No `github.pat` secret key

## Files Created

### Auth Module (`src/auth/`)
- `mod.rs` - Module root with architecture documentation
- `token_bundle.rs` - Token storage schema with redaction
- `lock.rs` - File-based auth lock for refresh safety
- `provider.rs` - TokenProvider trait and GitHubAuthManager
- `device_flow.rs` - Device flow HTTP client
- `errors.rs` - Auth-specific error types

## Files Modified

### CLI Commands
- `src/cli/commands/auth.rs` - Rewrote for device flow (login, status, logout)

### Forge Integration
- `src/forge/github.rs` - Use TokenProvider instead of static token

### Scanner/Gating
- `src/engine/scan.rs` - Derive `AuthAvailable(host)` capability
- `src/engine/capabilities.rs` - Add `AuthAvailable` capability

## Test Coverage

### Auth Tests
- Device flow polling state handling
- Token refresh with rotation
- Auth lock contention
- Redaction verification (tokens never in output)

### Integration Tests
- Full auth flow with mocked GitHub
- Command gating with auth capabilities

## Acceptance Gates Verified

- [x] Device flow login works with real GitHub App
- [x] Token refresh works automatically before expiry
- [x] Concurrent refresh is safe (lock prevents races)
- [x] Tokens never appear in any output (redaction enforced)
- [x] App-not-installed produces clear install link
- [x] All existing GitHub integration tests pass with new auth
- [x] PAT support completely removed

## Notes

### Security Considerations

Per SPEC.md 4.4.4, tokens are redacted from:
- stdout/stderr
- `--debug` output
- Error messages
- JSON outputs
- Journal/op-state markers

All auth types implement custom `Debug` that shows `[REDACTED]`.

### Refresh Token Rotation

GitHub App refresh tokens are single-use. Each refresh returns:
- New access token
- New refresh token

The implementation stores both atomically to prevent token loss on crash.

### Auth Lock Granularity

The lock is per-host (`~/.lattice/auth/lock.github.com`), allowing:
- Future multi-host support
- Independent locking for different GitHub Enterprise instances

### Error Messages

Auth errors provide actionable guidance:
- "Not authenticated. Run `lattice auth login`."
- "Authentication expired. Run `lattice auth login` again."
- "Device flow expired. Please try again."
