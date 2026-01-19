# Milestone 1.3: OAuth RepoAuthorized Capability

## Goal

**Implement the `RepoAuthorized(owner, repo)` capability per SPEC.md Section 8E.0.1.**

This capability verifies that the authenticated GitHub user has access to the target repository via an installed GitHub App, enabling remote commands (`submit`, `sync`, `get`, `merge`, `pr`) to proceed safely.

**Governing Principle:** Per CLAUDE.md "Reuse" - we build on the extensive existing auth infrastructure (TokenProvider, SecretStore, AuthError with AppNotInstalled variant) rather than creating parallel paths.

---

## Background

### What Already Exists

The authentication infrastructure is comprehensive:

| Component | Location | Status |
|-----------|----------|--------|
| Device flow OAuth | `src/auth/device_flow.rs` | Complete |
| Token bundle storage | `src/auth/token_bundle.rs` | Complete |
| Concurrency-safe refresh | `src/auth/lock.rs` | Complete |
| TokenProvider trait | `src/auth/provider.rs` | Complete |
| AppNotInstalled error | `src/auth/errors.rs` | **Defined but unused** |
| SecretStore abstraction | `src/secrets/` | Complete |
| AuthAvailable capability | `src/engine/capabilities.rs` | Complete |
| RemoteResolved capability | `src/engine/capabilities.rs` | Complete |

### What's Missing

Per SPEC.md Section 8E.0.1, we need:

1. **GitHub API calls** to query installations and repositories
2. **RepoAuthorized capability** in the capability enum
3. **Scanner logic** to derive the capability
4. **Caching layer** with 10-minute TTL
5. **AppNotInstalled blocking issue** in doctor framework

---

## Spec References

**SPEC.md Section 8E.0.1 - Determining "RepoAuthorized":**

> Given `host`, `owner`, `repo`:
> 1. Query `GET /user/installations` to list installations accessible to the user token
> 2. For each installation, query `GET /user/installations/{installation_id}/repositories`
> 3. If repo found: cache `installation_id` and `repository_id`, return `RepoAuthorized` capability
> 4. If not found: output install instructions, exit code 1

**SPEC.md Caching Requirements:**
- Cache in `<common_dir>/lattice/cache/github_auth.json`
- TTL: 10 minutes
- Repo config caches must never be trusted without validation

**ARCHITECTURE.md Section 5.2 - Capability derivation:**

> * `RepoAuthorized(owner, repo)` – Query the GitHub installations API to verify the GitHub App is installed and authorized for the repository. Cache the result for a short TTL (e.g., 10 minutes).

**ARCHITECTURE.md Section 8.2 - Authentication issues:**

> * `AppNotInstalled` (Blocking)
>   * Condition: GitHub App is not installed or not authorized for the repository
>   * Fix: User action – install the GitHub App
>   * Message: "GitHub App not installed for {owner}/{repo}. Install at: https://github.com/apps/lattice/installations/new"

---

## Implementation Steps

### Step 1: Add GitHub Installation API Client

**File:** `src/auth/installations.rs` (NEW)

This module handles GitHub App installation and repository authorization queries.

```rust
//! GitHub App installation and repository authorization queries.
//!
//! Per SPEC.md Section 8E.0.1, determines if the authenticated user has
//! access to a repository via an installed GitHub App.

use crate::auth::errors::AuthError;
use crate::auth::provider::TokenProvider;
use serde::Deserialize;

/// Result of checking repository authorization.
#[derive(Debug, Clone)]
pub struct RepoAuthResult {
    pub installation_id: u64,
    pub repository_id: u64,
}

/// Response from GET /user/installations
#[derive(Debug, Deserialize)]
struct InstallationsResponse {
    installations: Vec<Installation>,
}

#[derive(Debug, Deserialize)]
struct Installation {
    id: u64,
    account: InstallationAccount,
}

#[derive(Debug, Deserialize)]
struct InstallationAccount {
    login: String,
}

/// Response from GET /user/installations/{id}/repositories
#[derive(Debug, Deserialize)]
struct RepositoriesResponse {
    repositories: Vec<Repository>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    id: u64,
    name: String,
    owner: RepositoryOwner,
}

#[derive(Debug, Deserialize)]
struct RepositoryOwner {
    login: String,
}

/// Check if the authenticated user has access to a repository via GitHub App.
///
/// Per SPEC.md 8E.0.1:
/// 1. GET /user/installations - list user's app installations
/// 2. GET /user/installations/{id}/repositories - check each installation
/// 3. Return installation_id and repository_id if found
pub async fn check_repo_authorization<T: TokenProvider>(
    token_provider: &T,
    host: &str,
    owner: &str,
    repo: &str,
) -> Result<Option<RepoAuthResult>, AuthError> {
    let token = token_provider.bearer_token().await?;
    let client = reqwest::Client::new();
    let base_url = if host == "github.com" {
        "https://api.github.com".to_string()
    } else {
        format!("https://{}/api/v3", host)
    };

    // Step 1: Get user installations
    let installations_url = format!("{}/user/installations", base_url);
    let response = client
        .get(&installations_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "lattice-cli")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| AuthError::NetworkError(e.to_string()))?;

    if response.status() == 401 || response.status() == 403 {
        return Err(AuthError::NotAuthenticated);
    }

    let installations: InstallationsResponse = response
        .json()
        .await
        .map_err(|e| AuthError::NetworkError(e.to_string()))?;

    // Step 2: For each installation, check repositories
    for installation in installations.installations {
        let repos_url = format!(
            "{}/user/installations/{}/repositories",
            base_url, installation.id
        );

        // Paginate through repositories
        let mut page = 1;
        loop {
            let response = client
                .get(&repos_url)
                .query(&[("page", page.to_string()), ("per_page", "100".to_string())])
                .header("Authorization", format!("Bearer {}", token))
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "lattice-cli")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .send()
                .await
                .map_err(|e| AuthError::NetworkError(e.to_string()))?;

            if !response.status().is_success() {
                break; // Skip this installation on error
            }

            let repos: RepositoriesResponse = response
                .json()
                .await
                .map_err(|e| AuthError::NetworkError(e.to_string()))?;

            // Check if target repo is in this page
            for repository in &repos.repositories {
                if repository.owner.login.eq_ignore_ascii_case(owner)
                    && repository.name.eq_ignore_ascii_case(repo)
                {
                    return Ok(Some(RepoAuthResult {
                        installation_id: installation.id,
                        repository_id: repository.id,
                    }));
                }
            }

            // Check if more pages
            if repos.repositories.len() < 100 {
                break;
            }
            page += 1;
        }
    }

    // Not found in any installation
    Ok(None)
}
```

**Rationale:** This follows the exact algorithm specified in SPEC.md 8E.0.1. The function is async and uses the existing TokenProvider trait, keeping it decoupled from concrete implementations.

---

### Step 2: Add Authorization Cache

**File:** `src/auth/cache.rs` (NEW)

```rust
//! Authorization cache with TTL support.
//!
//! Per SPEC.md Section 8E.0.1, caches authorization results for 10 minutes
//! at `<common_dir>/lattice/cache/github_auth.json`.

use crate::auth::installations::RepoAuthResult;
use crate::core::paths::LatticePaths;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Cache TTL in minutes (per SPEC.md: 10 minutes)
const CACHE_TTL_MINUTES: i64 = 10;

/// Cache entry for a repository authorization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthCacheEntry {
    pub installation_id: u64,
    pub repository_id: u64,
    pub cached_at: DateTime<Utc>,
}

impl AuthCacheEntry {
    pub fn is_expired(&self) -> bool {
        Utc::now() - self.cached_at > Duration::minutes(CACHE_TTL_MINUTES)
    }
}

/// Cache key: "host/owner/repo"
fn cache_key(host: &str, owner: &str, repo: &str) -> String {
    format!("{}/{}/{}", host.to_lowercase(), owner.to_lowercase(), repo.to_lowercase())
}

/// Authorization cache stored at `<common_dir>/lattice/cache/github_auth.json`
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AuthCache {
    entries: HashMap<String, AuthCacheEntry>,
}

impl AuthCache {
    /// Load cache from disk, returning empty cache on any error
    pub fn load(paths: &LatticePaths) -> Self {
        let path = Self::cache_path(paths);
        fs::read_to_string(&path)
            .ok()
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    /// Save cache to disk (best-effort, errors ignored)
    pub fn save(&self, paths: &LatticePaths) {
        let path = Self::cache_path(paths);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&path, content);
        }
    }

    /// Get cached authorization if present and not expired
    pub fn get(&self, host: &str, owner: &str, repo: &str) -> Option<&AuthCacheEntry> {
        let key = cache_key(host, owner, repo);
        self.entries.get(&key).filter(|e| !e.is_expired())
    }

    /// Store authorization result in cache
    pub fn set(&mut self, host: &str, owner: &str, repo: &str, result: &RepoAuthResult) {
        let key = cache_key(host, owner, repo);
        self.entries.insert(
            key,
            AuthCacheEntry {
                installation_id: result.installation_id,
                repository_id: result.repository_id,
                cached_at: Utc::now(),
            },
        );
    }

    /// Remove entry (used when API returns 403/404)
    pub fn invalidate(&mut self, host: &str, owner: &str, repo: &str) {
        let key = cache_key(host, owner, repo);
        self.entries.remove(&key);
    }

    /// Prune expired entries
    pub fn prune_expired(&mut self) {
        self.entries.retain(|_, entry| !entry.is_expired());
    }

    fn cache_path(paths: &LatticePaths) -> PathBuf {
        paths.repo_cache_dir().join("github_auth.json")
    }
}
```

**Rationale:** Follows SPEC.md caching requirements exactly. The cache is keyed by host/owner/repo, uses 10-minute TTL, and stores at the specified path.

---

### Step 3: Add RepoAuthorized Capability

**File:** `src/engine/capabilities.rs`

**Location:** Add to the `Capability` enum (after `AuthAvailable`)

```rust
/// Repository authorization verified via GitHub App installation.
/// The authenticated user has access to the target repository.
RepoAuthorized,
```

**Location:** Add to `description()` match arm

```rust
Capability::RepoAuthorized => "repository authorization verified",
```

**Location:** Add to `Display` impl

```rust
Capability::RepoAuthorized => write!(f, "RepoAuthorized"),
```

---

### Step 4: Add AppNotInstalled Blocking Issue

**File:** `src/engine/health.rs`

**Location:** Add in the `issues` module

```rust
/// GitHub App is not installed or not authorized for the repository.
/// Per ARCHITECTURE.md Section 8.2, this is a blocking issue requiring user action.
pub fn app_not_installed(host: &str, owner: &str, repo: &str) -> Issue {
    Issue::new(
        format!("app-not-installed:{}/{}/{}", host, owner, repo),
        Severity::Blocking,
        format!(
            "GitHub App not installed for {}/{}. Install at: https://github.com/apps/lattice/installations/new",
            owner, repo
        ),
    )
    .with_evidence(Evidence::Config {
        key: format!("forge.github.{}/{}", owner, repo),
        problem: "GitHub App not installed or not authorized".to_string(),
    })
    .blocks(Capability::RepoAuthorized)
}

/// Failed to check repository authorization (network error, etc).
/// This is a warning - may still proceed with cached data or user override.
pub fn repo_authorization_check_failed(owner: &str, repo: &str, error: &str) -> Issue {
    Issue::new(
        format!("repo-auth-check-failed:{}/{}", owner, repo),
        Severity::Warning,
        format!("Could not verify repository authorization for {}/{}: {}", owner, repo, error),
    )
    .with_evidence(Evidence::Config {
        key: "github.authorization-check".to_string(),
        problem: error.to_string(),
    })
}
```

**Rationale:** Follows the exact message format from ARCHITECTURE.md Section 8.2. The `app_not_installed` issue blocks `RepoAuthorized` capability, which gates remote commands.

---

### Step 5: Integrate into Scanner

**File:** `src/engine/scan.rs`

**Location:** After `RemoteResolved` capability check (approximately line 180-200)

```rust
// Check RepoAuthorized capability (per SPEC.md 8E.0.1)
// Only check if we have auth and resolved remote
if health.capabilities().has(&Capability::AuthAvailable)
    && health.capabilities().has(&Capability::RemoteResolved)
{
    if let Some((owner, repo)) = parsed_github_remote.as_ref() {
        let host = "github.com"; // v1: only github.com supported

        // Load cache
        let mut cache = crate::auth::cache::AuthCache::load(&paths);

        // Check cache first (10-minute TTL per SPEC.md)
        if let Some(cached) = cache.get(host, owner, repo) {
            // Cache hit - capability satisfied
            health.add_capability(Capability::RepoAuthorized);
            // Store cached IDs in snapshot for later use
            snapshot.cached_installation_id = Some(cached.installation_id);
            snapshot.cached_repository_id = Some(cached.repository_id);
        } else {
            // Cache miss - need to query API
            // Note: This is a blocking call during scan. For v1 this is acceptable.
            // Future optimization: make scan async or defer check to execution.
            match check_repo_authorization_sync(host, owner, repo) {
                Ok(Some(result)) => {
                    // Authorized - cache and add capability
                    cache.set(host, owner, repo, &result);
                    cache.prune_expired();
                    cache.save(&paths);
                    health.add_capability(Capability::RepoAuthorized);
                    snapshot.cached_installation_id = Some(result.installation_id);
                    snapshot.cached_repository_id = Some(result.repository_id);
                }
                Ok(None) => {
                    // Not authorized - add blocking issue
                    health.add_issue(issues::app_not_installed(host, owner, repo));
                }
                Err(e) => {
                    // Check failed - add warning (non-blocking)
                    health.add_issue(issues::repo_authorization_check_failed(owner, repo, &e.to_string()));
                    // Don't add capability - commands requiring it will be gated
                }
            }
        }
    }
}
```

**Synchronous wrapper for async check:**

```rust
/// Synchronous wrapper for repo authorization check.
/// Uses tokio runtime block_on for v1 simplicity.
fn check_repo_authorization_sync(
    host: &str,
    owner: &str,
    repo: &str,
) -> Result<Option<crate::auth::installations::RepoAuthResult>, crate::auth::errors::AuthError> {
    // Get or create tokio runtime
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| crate::auth::errors::AuthError::NetworkError(e.to_string()))?;

    rt.block_on(async {
        let auth_manager = crate::auth::provider::GitHubAuthManager::new(host)?;
        crate::auth::installations::check_repo_authorization(&auth_manager, host, owner, repo).await
    })
}
```

**Rationale:** The scanner needs to be able to check authorization. For v1, we use a synchronous wrapper around the async API call. The cache prevents repeated API calls within the 10-minute window.

---

### Step 6: Update Requirement Sets

**File:** `src/engine/gate.rs`

**Location:** Update `REMOTE` requirement set

```rust
pub const REMOTE: RequirementSet = RequirementSet::new(
    "remote",
    &[
        Capability::RepoOpen,
        Capability::TrunkKnown,
        Capability::NoLatticeOpInProgress,
        Capability::NoExternalGitOpInProgress,
        Capability::MetadataReadable,
        Capability::GraphValid,
        Capability::FrozenPolicySatisfied,
        Capability::WorkingDirectoryAvailable,
        Capability::RemoteResolved,
        Capability::AuthAvailable,
        Capability::RepoAuthorized,  // NEW: per SPEC.md 8E.0
    ],
);
```

**Also add `REMOTE_BARE_ALLOWED`** (for bare repo operations):

```rust
pub const REMOTE_BARE_ALLOWED: RequirementSet = RequirementSet::new(
    "remote-bare-allowed",
    &[
        Capability::RepoOpen,
        Capability::TrunkKnown,
        Capability::NoLatticeOpInProgress,
        Capability::NoExternalGitOpInProgress,
        Capability::MetadataReadable,
        Capability::GraphValid,
        Capability::FrozenPolicySatisfied,
        // Note: WorkingDirectoryAvailable NOT required
        Capability::RemoteResolved,
        Capability::AuthAvailable,
        Capability::RepoAuthorized,  // NEW
    ],
);
```

---

### Step 7: Update Auth Module Exports

**File:** `src/auth/mod.rs`

**Location:** Add module declarations and re-exports

```rust
pub mod cache;
pub mod installations;

pub use cache::{AuthCache, AuthCacheEntry};
pub use installations::{check_repo_authorization, RepoAuthResult};
```

---

### Step 8: Add RepoSnapshot Fields

**File:** `src/engine/scan.rs` (or wherever `RepoSnapshot` is defined)

**Location:** Add fields to `RepoSnapshot` struct

```rust
/// Cached installation ID from authorization check (for API calls)
pub cached_installation_id: Option<u64>,
/// Cached repository ID from authorization check (for API calls)
pub cached_repository_id: Option<u64>,
```

---

## Critical Files Summary

| File | Action | Purpose |
|------|--------|---------|
| `src/auth/installations.rs` | NEW | GitHub installations API client |
| `src/auth/cache.rs` | NEW | Authorization cache with 10-min TTL |
| `src/auth/mod.rs` | MODIFY | Export new modules |
| `src/engine/capabilities.rs` | MODIFY | Add `RepoAuthorized` capability |
| `src/engine/health.rs` | MODIFY | Add `app_not_installed` issue |
| `src/engine/scan.rs` | MODIFY | Derive `RepoAuthorized` capability |
| `src/engine/gate.rs` | MODIFY | Add to `REMOTE` requirement set |
| `.agents/v2/ROADMAP.md` | MODIFY | Update status to Complete |

---

## Acceptance Gates

Per SPEC.md Section 8E.0.1 and ROADMAP.md:

- [ ] `GET /user/installations` fetches user's app installations
- [ ] `GET /user/installations/{id}/repositories` checks repo access (with pagination)
- [ ] `RepoAuthorized` capability derived by scanner
- [ ] Authorization cached in `<common_dir>/lattice/cache/github_auth.json`
- [ ] Cache TTL is 10 minutes
- [ ] Cache invalidated on 403/404 from API
- [ ] App-not-installed message: `https://github.com/apps/lattice/installations/new`
- [ ] `AppNotInstalled` is a blocking issue with user-action fix (per ARCHITECTURE.md 8.2)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Rubric

### Unit Tests

| Test | File | Purpose |
|------|------|---------|
| `test_cache_ttl_expiry` | `src/auth/cache.rs` | Verify 10-minute TTL enforcement |
| `test_cache_key_normalization` | `src/auth/cache.rs` | Verify case-insensitive key matching |
| `test_cache_load_save_roundtrip` | `src/auth/cache.rs` | Verify persistence works |
| `test_capability_description` | `src/engine/capabilities.rs` | Verify RepoAuthorized has description |
| `test_app_not_installed_blocks_capability` | `src/engine/health.rs` | Verify issue blocks RepoAuthorized |

### Integration Tests

| Test | File | Purpose |
|------|------|---------|
| `test_scanner_derives_repo_authorized` | `tests/integration/scan_auth.rs` | Verify capability derived when authorized |
| `test_scanner_blocks_without_auth` | `tests/integration/scan_auth.rs` | Verify blocking issue when not authorized |
| `test_remote_commands_gated` | `tests/integration/gate_remote.rs` | Verify submit/sync/get require RepoAuthorized |
| `test_cache_prevents_repeated_api_calls` | `tests/integration/scan_auth.rs` | Verify caching works |

### Manual Verification

| Step | Expected Result |
|------|-----------------|
| Run `lattice submit` without app installed | Error with install link |
| Run `lattice submit` with app installed | Proceeds to submit |
| Run `lattice sync` within 10 min of submit | No API call (cache hit) |
| Remove app from repo, wait 10 min, run sync | Error with install link |

---

## Verification Commands

```bash
# Build and type check
cargo check

# Lint
cargo clippy -- -D warnings

# Run all tests
cargo test

# Run specific auth tests
cargo test auth::

# Run capability tests
cargo test capabilities::

# Run scanner tests
cargo test scan::

# Format check
cargo fmt --check
```

---

## Risk Assessment

**Medium risk** - This milestone involves:
- New async API calls during scan (potential latency)
- New caching layer (potential staleness issues)
- Integration across multiple modules

**Mitigations:**
- Cache with 10-minute TTL per spec
- Non-blocking warning on API failure (graceful degradation)
- Existing auth infrastructure is well-tested

---

## Dependencies

- `reqwest` (already in Cargo.toml for device flow)
- `chrono` (already in Cargo.toml for timestamps)
- `serde`/`serde_json` (already in Cargo.toml)

No new dependencies required.

---

## Notes

**Principles Applied:**

- **Reuse:** Leverages existing TokenProvider, AuthError (AppNotInstalled variant), LatticePaths infrastructure
- **Follow the Leader:** Implements exactly what SPEC.md 8E.0.1 specifies
- **Simplicity:** Synchronous wrapper for v1; async optimization can come later
- **Purity:** API client functions are pure (take dependencies as parameters)
- **Tests are Everything:** Comprehensive test coverage at unit and integration levels

**Future Considerations:**

- Make scanner fully async to avoid blocking on API calls
- Add retry logic with exponential backoff for transient failures
- Support GitHub Enterprise (host parameter already threaded through)
