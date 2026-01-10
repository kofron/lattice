# Milestone 1: GitHub App OAuth Authentication

## Goal

Replace PAT-based authentication with GitHub App device flow OAuth as the **only** authentication method.

## Client ID

```
Iv23liIqb9vJ8kaRyZaU
```

This is the canonical, hardcoded client ID for the Lattice GitHub App.

## Dependencies

- v1 Milestones 0-8 (existing GitHub integration infrastructure)

---

## Phases

### Phase A: Auth Infrastructure (Foundation)

**Deliverables:**

1. **TokenBundle schema** (`src/auth/token_bundle.rs`)
   - JSON schema for stored OAuth tokens
   - Fields: kind, schema_version, host, client_id, user info, access_token, refresh_token, expirations, timestamps
   - Serialization/deserialization with serde

2. **AuthLock** (`src/auth/lock.rs`)
   - File-based lock for token refresh operations
   - Lock path: `~/.lattice/auth/lock.<host>`
   - Blocking acquisition with timeout
   - RAII guard pattern for automatic release

3. **TokenProvider trait** (`src/auth/provider.rs`)
   ```rust
   #[async_trait::async_trait]
   pub trait TokenProvider: Send + Sync {
       async fn bearer_token(&self) -> Result<String, AuthError>;
       fn is_authenticated(&self) -> bool;
   }
   ```

4. **GitHubAuthManager** (`src/auth/provider.rs`)
   - Implementation of TokenProvider
   - Load token bundle from SecretStore
   - Refresh logic with lock acquisition
   - Expiry checking with buffer (refresh 5 minutes before expiry)

### Phase B: Device Flow Implementation

**Deliverables:**

1. **DeviceFlowClient** (`src/auth/device_flow.rs`)
   - HTTP client for GitHub device flow endpoints
   - `POST https://github.com/login/device/code`
     - Request: `client_id`, `scope`
     - Response: `device_code`, `user_code`, `verification_uri`, `expires_in`, `interval`
   - `POST https://github.com/login/oauth/access_token`
     - Request: `client_id`, `device_code`, `grant_type=urn:ietf:params:oauth:grant-type:device_code`
     - Response: `access_token`, `refresh_token`, `expires_in`, `refresh_token_expires_in`, `token_type`, `scope`

2. **Polling logic**
   - Handle `authorization_pending` (continue polling)
   - Handle `slow_down` (increase interval by 5 seconds)
   - Handle `expired_token` (abort with error)
   - Handle `access_denied` (abort with error)
   - Handle success (return tokens)

3. **Token refresh logic**
   - `POST https://github.com/login/oauth/access_token`
     - Request: `client_id`, `refresh_token`, `grant_type=refresh_token`
   - Handle single-use rotation (new refresh token replaces old)
   - Acquire auth lock before refresh
   - Re-check token validity after acquiring lock (another process may have refreshed)

### Phase C: Auth Command Updates

**Deliverables:**

1. **`lattice auth login`** (`src/cli/commands/auth.rs`)
   - Initiate device flow
   - Display user code and verification URL
   - Attempt to open browser (unless `--no-browser`)
   - Poll for authorization
   - Store tokens in SecretStore
   - Display success with user login name

2. **`lattice auth status`** (`src/cli/commands/auth.rs`)
   - Display logged-in hosts
   - For each host: user login, token expiry, refresh token expiry
   - If in a repo with `[forge.github]` config: show authorization status

3. **`lattice auth logout`** (`src/cli/commands/auth.rs`)
   - Delete tokens from SecretStore for specified host
   - Confirm deletion

4. **Remove PAT support entirely**
   - No `--token` flag
   - No `LATTICE_GITHUB_TOKEN` or `GITHUB_TOKEN` env var support
   - No `github.pat` secret key

### Phase D: Repo Authorization

**Deliverables:**

1. **Installation discovery**
   - `GET /user/installations`
   - Parse response for installation IDs and account names

2. **Repository access check**
   - `GET /user/installations/{installation_id}/repositories`
   - Check if target repository is in the list
   - Handle pagination

3. **Authorization cache** (`.git/lattice/cache/github_auth.json`)
   - Cache installation_id and repository_id
   - TTL: 10 minutes
   - Invalidate on 403/404 from API

4. **Repo config additions** (`src/core/config/schema.rs`)
   - Add `[forge.github]` table to config schema
   - Fields: `host`, `owner`, `repo`, `installation_id`, `repository_id`, `authorized_at`

### Phase E: Integration

**Deliverables:**

1. **Update GitHubForge** (`src/forge/github.rs`)
   - Accept `TokenProvider` instead of static token
   - Call `bearer_token()` per request
   - Retry once on 401/403 after refresh

2. **Update scanner** (`src/engine/scan.rs`)
   - Derive `AuthAvailable(host)` capability
   - Derive `RemoteResolved(owner, repo)` capability
   - Derive `RepoAuthorized(owner, repo)` capability

3. **Update gating** (`src/engine/capabilities.rs`)
   - Add auth-related blocking issues
   - Route to doctor with user-action fix options

4. **Update commands**
   - `submit`, `sync`, `get`, `merge`, `pr` require `AuthAvailable` + `RepoAuthorized`

### Phase F: Testing

**Deliverables:**

1. **Mock device flow endpoint tests**
   - Happy path: device code -> poll -> success
   - `slow_down` interval handling
   - `expired_token` error handling
   - `access_denied` error handling

2. **Token refresh tests**
   - Refresh before expiry
   - Single-use rotation (new refresh token)
   - Concurrent refresh with locking

3. **Auth lock contention tests**
   - Multiple processes attempting refresh
   - Lock timeout behavior

4. **Redaction tests**
   - Tokens (`ghu_*`, `ghr_*`) never appear in:
     - stdout/stderr
     - log output
     - error messages
     - debug output

5. **Integration tests**
   - Full auth flow with mock GitHub
   - Command gating with auth capabilities

---

## Acceptance Gates

- [ ] Device flow login works with real GitHub App (client_id: `Iv23liIqb9vJ8kaRyZaU`)
- [ ] Token refresh works automatically before expiry
- [ ] Concurrent refresh is safe (lock prevents races)
- [ ] Tokens never appear in any output (redaction enforced)
- [ ] App-not-installed produces clear install link: `https://github.com/apps/lattice/installations/new`
- [ ] All existing GitHub integration tests pass with new auth
- [ ] PAT support completely removed (no `--token`, no env vars)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes with no warnings
- [ ] Type checks pass

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/auth/mod.rs` | NEW | Auth module root |
| `src/auth/token_bundle.rs` | NEW | Token storage schema |
| `src/auth/lock.rs` | NEW | Auth refresh lock |
| `src/auth/provider.rs` | NEW | TokenProvider trait + GitHubAuthManager |
| `src/auth/device_flow.rs` | NEW | Device flow HTTP client |
| `src/forge/github.rs` | MODIFY | Use TokenProvider |
| `src/cli/commands/auth.rs` | MODIFY | Rewrite for device flow |
| `src/engine/scan.rs` | MODIFY | Add auth capabilities |
| `src/engine/capabilities.rs` | MODIFY | Add AuthAvailable, RepoAuthorized |
| `src/core/config/schema.rs` | MODIFY | Add [forge.github] table |

---

## Notes

- The GitHub App slug is `lattice` (install URL: `https://github.com/apps/lattice/installations/new`)
- Device flow requires no client secret, making it safe for CLI distribution
- Refresh tokens are single-use and rotate on each refresh
- All token operations must be guarded by auth-scoped file locks to prevent races between concurrent Lattice processes
