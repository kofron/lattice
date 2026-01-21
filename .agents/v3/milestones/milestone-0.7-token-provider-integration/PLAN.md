# Milestone 0.7: TokenProvider Integration

## Status: COMPLETE

---

## Overview

**Goal:** Integrate `TokenProvider` with `GitHubForge` to enable per-request token refresh and 401/403 retry logic.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Reuse, Purity, No stubs, Tests are everything.

**Priority:** HIGH - No per-request token refresh

**Spec Reference:** 
- SPEC.md Section 8E.1 "Forge abstraction"
- SPEC.md Section 4.4 "Secret storage abstraction"
- ARCHITECTURE.md Section 11.3 "Authentication Manager"

---

## Problem Statement

`GitHubForge` currently stores a raw token string instead of using `TokenProvider`. This means:

1. **No automatic token refresh** - If the access token expires mid-operation, API calls fail
2. **No retry on 401/403** - Token expiration causes immediate failure without recovery attempt
3. **No lock-protected refresh** - Concurrent commands could race on refresh tokens

**Current Implementation:**
```rust
// src/forge/github.rs line 89-103
pub fn new(
    token: impl Into<String>,
    owner: impl Into<String>,
    repo: impl Into<String>,
) -> Self {
    Self {
        client: Client::new(),
        token: token.into(),  // Raw string storage
        // ...
    }
}
```

**How token is used:**
```rust
// src/forge/github.rs line 178
HeaderValue::from_str(&format!("Bearer {}", self.token))
```

---

## Current Infrastructure (Already Complete)

### TokenProvider Trait
**File:** `src/auth/mod.rs` (lines 43-76)

```rust
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    async fn bearer_token(&self) -> Result<String, AuthError>;
    fn is_authenticated(&self) -> bool;
    fn host(&self) -> &str;
}
```

### GitHubAuthManager (TokenProvider Implementation)
**File:** `src/auth/provider.rs`

- Full `TokenProvider` implementation
- Automatic refresh with 5-minute buffer (`EXPIRY_BUFFER_SECS`)
- Lock-protected refresh (`AuthLock` at `~/.lattice/auth/lock.<host>`)
- Re-checks after acquiring lock (prevents double-refresh)
- Custom Debug redacting tokens

### Token Redaction
Already implemented via custom `Debug` implementations:
- `TokenBundle`: redacts in Debug output
- `TokenInfo`: shows `[REDACTED]` for access_token and refresh_token
- `GitHubAuthManager`: shows only host and is_authenticated status

### SecretStore
- `FileSecretStore`: `~/.lattice/secrets.toml` with 0600 permissions
- `KeychainSecretStore`: OS keychain (feature-gated)

---

## Design Decisions

### Q1: Should GitHubForge store `Arc<dyn TokenProvider>` or accept it per-method?

**Decision:** Store `Arc<dyn TokenProvider>` in the struct.

**Rationale:**
- Matches existing architecture where forge is created once and reused
- Avoids changing every Forge trait method signature
- `Arc<dyn TokenProvider>` is `Send + Sync` (required by async)

### Q2: Should all API methods use TokenProvider or just a subset?

**Decision:** All API methods must use TokenProvider.

**Rationale:**
- Per ARCHITECTURE.md, "AuthManager is invoked by host adapters on each API request"
- Consistency - any API call could trigger 401 if token expired

### Q3: What is the retry strategy for 401/403?

**Decision:** Single retry after refresh, no exponential backoff.

**Rationale:**
- If refresh fails, further retries won't help
- If refresh succeeds but 401 persists, the problem is not token expiration
- Simple and predictable behavior

### Q4: How to handle the transition for existing code?

**Decision:** Provide both constructors during transition:
1. `GitHubForge::new_with_provider(provider, owner, repo)` - new preferred constructor
2. `GitHubForge::new(token, owner, repo)` - deprecated, for tests and legacy code

**Rationale:**
- Non-breaking change for existing callers
- Clear migration path
- Tests can continue to use simple string tokens

### Q5: Do we need a `Redacted<T>` wrapper type?

**Decision:** No, existing custom Debug implementations are sufficient.

**Rationale:**
- `TokenInfo` and `TokenBundle` already redact via custom `Debug`
- The raw token string only exists briefly in `bearer_token()` result
- Adding `Redacted<String>` would require changing `TokenProvider::bearer_token()` return type

---

## Implementation Plan

### Phase 1: Refactor GitHubForge to Support TokenProvider

**File:** `src/forge/github.rs`

1. **Add TokenProvider field and new constructor:**
   ```rust
   pub struct GitHubForge {
       client: Client,
       /// Token provider for automatic refresh, or None for legacy static token
       token_provider: Option<Arc<dyn TokenProvider>>,
       /// Static token (deprecated, for backwards compatibility)
       static_token: Option<String>,
       owner: String,
       repo: String,
       api_base: String,
   }
   
   impl GitHubForge {
       /// Create a new GitHub forge with TokenProvider for automatic refresh.
       ///
       /// This is the preferred constructor for production use.
       pub fn new_with_provider(
           provider: Arc<dyn TokenProvider>,
           owner: impl Into<String>,
           repo: impl Into<String>,
       ) -> Self {
           Self {
               client: Client::new(),
               token_provider: Some(provider),
               static_token: None,
               owner: owner.into(),
               repo: repo.into(),
               api_base: DEFAULT_API_BASE.to_string(),
           }
       }
       
       /// Create a new GitHub forge with a static token.
       ///
       /// This constructor is deprecated. Use `new_with_provider` for production.
       /// Retained for tests and backwards compatibility.
       #[deprecated(since = "0.7.0", note = "Use new_with_provider for automatic token refresh")]
       pub fn new(
           token: impl Into<String>,
           owner: impl Into<String>,
           repo: impl Into<String>,
       ) -> Self {
           Self {
               client: Client::new(),
               token_provider: None,
               static_token: Some(token.into()),
               owner: owner.into(),
               repo: repo.into(),
               api_base: DEFAULT_API_BASE.to_string(),
           }
       }
   }
   ```

2. **Add internal method to get current token:**
   ```rust
   impl GitHubForge {
       /// Get current bearer token, refreshing if needed.
       async fn get_bearer_token(&self) -> Result<String, ForgeError> {
           if let Some(ref provider) = self.token_provider {
               provider
                   .bearer_token()
                   .await
                   .map_err(|e| ForgeError::AuthFailed(e.to_string()))
           } else if let Some(ref token) = self.static_token {
               Ok(token.clone())
           } else {
               Err(ForgeError::AuthRequired)
           }
       }
   }
   ```

3. **Modify `default_headers()` to be async and use get_bearer_token:**
   ```rust
   async fn default_headers(&self) -> Result<HeaderMap, ForgeError> {
       let token = self.get_bearer_token().await?;
       let mut headers = HeaderMap::new();
       headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
       headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.github+json"));
       headers.insert(
           AUTHORIZATION,
           HeaderValue::from_str(&format!("Bearer {}", token))
               .expect("Invalid token format"),
       );
       Ok(headers)
   }
   ```

### Phase 2: Add 401/403 Retry Logic

**File:** `src/forge/github.rs`

1. **Add retry wrapper method:**
   ```rust
   /// Execute a request with one retry on auth failure.
   ///
   /// If the request returns 401 or 403 and we have a TokenProvider,
   /// refreshes the token and retries once.
   async fn execute_with_retry<F, Fut, T>(
       &self,
       request_fn: F,
   ) -> Result<T, ForgeError>
   where
       F: Fn(HeaderMap) -> Fut,
       Fut: std::future::Future<Output = Result<T, ForgeError>>,
   {
       let headers = self.default_headers().await?;
       let result = request_fn(headers).await;
       
       match &result {
           Err(ForgeError::AuthFailed(_)) if self.token_provider.is_some() => {
               // Refresh and retry once
               // Note: bearer_token() handles locking internally
               let headers = self.default_headers().await?;
               request_fn(headers).await
           }
           _ => result,
       }
   }
   ```

2. **Update all API methods to use retry wrapper:**
   
   Example for `create_pr`:
   ```rust
   async fn create_pr(&self, req: CreatePrRequest) -> Result<PullRequest, ForgeError> {
       self.execute_with_retry(|headers| async {
           let response = self.client
               .post(&self.repo_url("pulls"))
               .headers(headers)
               .json(&CreatePrBody::from(req.clone()))
               .send()
               .await?;
           // ... handle response
       }).await
   }
   ```

### Phase 3: Update Callers

**Files to update:**

1. **`src/cli/commands/mod.rs`** (line 419):
   ```rust
   // Before:
   Some(Box::new(GitHubForge::new(token, owner, repo)))
   
   // After:
   let provider = Arc::new(auth_manager);
   Some(Box::new(GitHubForge::new_with_provider(provider, owner, repo)))
   ```

2. **`src/engine/scan.rs`** (line 790):
   ```rust
   // Before:
   let forge = GitHubForge::new(token, owner, repo);
   
   // After:
   let auth_manager = get_auth_manager("github.com")?;
   let provider: Arc<dyn TokenProvider> = Arc::new(auth_manager);
   let forge = GitHubForge::new_with_provider(provider, owner, repo);
   ```

3. **`src/cli/commands/init.rs`** (line 281):
   ```rust
   // Before:
   let forge = GitHubForge::new(token, owner, repo);
   
   // After:
   let provider: Arc<dyn TokenProvider> = Arc::new(auth_manager);
   let forge = GitHubForge::new_with_provider(provider, owner, repo);
   ```

### Phase 4: Update Tests

**Files to update:**

1. **`tests/github_integration.rs`**:
   - Keep using `#[allow(deprecated)]` with `GitHubForge::new()` for simplicity
   - Add new tests using `new_with_provider()` with mock TokenProvider

2. **`src/forge/github.rs` unit tests**:
   - Update existing tests to use `#[allow(deprecated)]`
   - Add tests for retry behavior with mock TokenProvider

---

## Test Strategy

### Unit Tests (src/forge/github.rs)

1. **`new_with_provider_creates_forge`**
   - Create with mock TokenProvider
   - Verify forge fields are set correctly

2. **`get_bearer_token_uses_provider`**
   - Mock TokenProvider returns token
   - Verify `get_bearer_token()` returns that token

3. **`get_bearer_token_falls_back_to_static`**
   - Create with static token (deprecated constructor)
   - Verify `get_bearer_token()` returns static token

4. **`retry_on_401_refreshes_and_succeeds`**
   - Mock TokenProvider that returns different tokens
   - Mock HTTP client that returns 401 first, then 200
   - Verify retry succeeds

5. **`retry_on_403_refreshes_and_succeeds`**
   - Same as above but with 403

6. **`no_retry_without_provider`**
   - Create with static token
   - Mock 401 response
   - Verify single failure, no retry

7. **`single_retry_only`**
   - Mock TokenProvider
   - Mock HTTP client that always returns 401
   - Verify exactly two requests (initial + one retry)

### Concurrent Refresh Test

8. **`concurrent_401_triggers_single_refresh`**
   - Use real `GitHubAuthManager` with mock SecretStore
   - Two async tasks get 401 simultaneously
   - Verify only one refresh call to SecretStore

### Integration Tests (tests/)

9. **`token_provider_integration`**
   - Create `GitHubAuthManager` with `FileSecretStore`
   - Create `GitHubForge::new_with_provider()`
   - Verify API call uses token from provider

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/forge/github.rs` | MODIFY | Add TokenProvider support, retry logic |
| `src/cli/commands/mod.rs` | MODIFY | Update forge creation |
| `src/engine/scan.rs` | MODIFY | Update forge creation |
| `src/cli/commands/init.rs` | MODIFY | Update forge creation |
| `tests/github_integration.rs` | MODIFY | Add deprecation allows, new tests |

---

## Acceptance Gates

From ROADMAP.md:

- [ ] `GitHubForge` uses `TokenProvider` not raw token
- [ ] `bearer_token()` called per request (via `get_bearer_token()`)
- [ ] 401/403 triggers one retry with fresh token
- [ ] TokenProvider refresh holds auth-scoped lock (already implemented in `GitHubAuthManager`)
- [ ] Tokens never appear in Debug output, logs, errors, op-state, journal, or ledger
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Verification Commands

```bash
# Type checks
cargo check

# Lint
cargo clippy -- -D warnings

# All tests
cargo test

# Specific tests
cargo test github
cargo test token_provider
cargo test forge

# Format check
cargo fmt --check
```

---

## Dependencies

**Depends on (all COMPLETE):**
- None - this milestone is independent

**Blocks:**
- Commands that use GitHubForge will benefit from automatic refresh

---

## Estimated Effort

| Task | Effort |
|------|--------|
| Phase 1: Refactor GitHubForge | 2 hours |
| Phase 2: Add retry logic | 1 hour |
| Phase 3: Update callers | 1 hour |
| Phase 4: Update tests | 2 hours |
| Verification and cleanup | 1 hour |
| **Total** | **~7 hours** |

---

## Risk Assessment

**Low Risk:**
- Infrastructure (TokenProvider, AuthLock) is already complete and tested
- Deprecation strategy provides backwards compatibility
- Tests already have redaction coverage

**Medium Risk:**
- Async closures for retry logic may require careful handling
- Concurrent refresh test setup is complex

**Mitigations:**
- Use `Box::pin` if async closure issues arise
- Leverage existing `AuthLock` tests as reference for concurrent test

---

## Conclusion

This milestone integrates the existing `TokenProvider` infrastructure with `GitHubForge`. The main work is:

1. Modify `GitHubForge` to accept `Arc<dyn TokenProvider>` 
2. Add retry logic for 401/403 responses
3. Update callers to use the new constructor
4. Add tests for new behavior

The authentication infrastructure is already complete and well-tested, making this primarily an integration task.
