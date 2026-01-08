# Milestone 8: GitHub Integration - Implementation Notes

## Summary

Successfully implemented GitHub integration for Lattice, transforming it from a local-only stacked branch tool into one that can submit, sync, and manage PRs on GitHub. This milestone delivers:

1. **Forge abstraction** with async trait and full GitHub adapter (REST + GraphQL)
2. **MockForge** for deterministic testing
3. **Auth command** for secure token storage
4. **Remote-aware scanner** capabilities (RemoteResolved, AuthAvailable)
5. **Seven new commands**: auth, submit, sync, get, merge, pr, unlink

## Architecture Decisions

### Async Runtime Strategy

Rather than converting the entire CLI to async with `#[tokio::main]`, each command that needs async creates its own runtime:

```rust
pub fn submit(ctx: &Context, ...) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(submit_async(ctx, opts))
}
```

**Rationale:**
- Keeps main entry point synchronous
- Only commands needing network I/O pay the runtime cost
- Simpler integration with existing synchronous codebase
- No changes needed to main.rs

### Forge Trait Design

The `Forge` trait uses `async_trait` for async methods:

```rust
#[async_trait]
pub trait Forge: Send + Sync {
    fn name(&self) -> &'static str;
    async fn create_pr(&self, request: CreatePrRequest) -> Result<PullRequest, ForgeError>;
    async fn update_pr(&self, request: UpdatePrRequest) -> Result<PullRequest, ForgeError>;
    async fn get_pr(&self, number: u64) -> Result<PullRequest, ForgeError>;
    async fn find_pr_by_head(&self, head: &str) -> Result<Option<PullRequest>, ForgeError>;
    async fn set_draft(&self, number: u64, draft: bool) -> Result<(), ForgeError>;
    async fn request_reviewers(&self, number: u64, reviewers: Reviewers) -> Result<(), ForgeError>;
    async fn merge_pr(&self, number: u64, method: MergeMethod) -> Result<(), ForgeError>;
}
```

Key design choices:
- `Send + Sync` bounds for thread-safe usage
- `ForgeError` is `Clone` to support MockForge failure injection
- `PullRequest` includes optional `node_id` for GraphQL operations
- `find_pr_by_head` returns `Option` to distinguish "not found" from errors

### Token Storage

Tokens are stored using the existing `SecretStore` abstraction:
- Key: `github.pat`
- Backend: FileSecretStore (Unix: ~/.config/lattice/secrets with 0600 permissions)
- Environment variable override: `GITHUB_TOKEN`

### Scanner Capability Detection

Two new capabilities added to the scanner:

1. **RemoteResolved**: Origin remote is parseable as a GitHub URL
2. **AuthAvailable**: GitHub token exists in secret store or environment

These capabilities gate the GitHub commands without blocking local operations.

## Implementation Details

### Phase 1: Async Dependencies

Added to `Cargo.toml`:
```toml
[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
async-trait = "0.1"
reqwest = { version = "0.12", features = ["json"] }
rpassword = "7"

[dev-dependencies]
tokio-test = "0.4"
wiremock = "0.6"
```

### Phase 2: GitHubForge Implementation

**REST API Endpoints:**
- `POST /repos/{owner}/{repo}/pulls` - create PR
- `PATCH /repos/{owner}/{repo}/pulls/{number}` - update PR
- `GET /repos/{owner}/{repo}/pulls/{number}` - get PR
- `GET /repos/{owner}/{repo}/pulls?head={owner}:{branch}` - find by head
- `PUT /repos/{owner}/{repo}/pulls/{number}/merge` - merge
- `POST /repos/{owner}/{repo}/pulls/{number}/requested_reviewers` - request reviewers

**GraphQL for Draft Toggle:**
```graphql
mutation($id: ID!) {
    convertPullRequestToDraft(input: {pullRequestId: $id}) {
        pullRequest { id isDraft }
    }
}

mutation($id: ID!) {
    markPullRequestReadyForReview(input: {pullRequestId: $id}) {
        pullRequest { id isDraft }
    }
}
```

**Error Mapping:**
- 401 → `AuthFailed`
- 403 → `AuthFailed` (permissions)
- 404 → `NotFound`
- 429 → `RateLimited`
- 5xx → `ApiError`

**URL Parsing:**
```rust
pub fn parse_github_url(url: &str) -> Option<(String, String)>
```
Handles both HTTPS and SSH URL formats with optional `.git` suffix.

### Phase 3: MockForge Implementation

Thread-safe mock with `Arc<Mutex<...>>`:

```rust
pub struct MockForge {
    inner: Arc<Mutex<MockForgeInner>>,
}

pub enum FailOn {
    CreatePr(ForgeError),
    UpdatePr(ForgeError),
    // ... etc
}

pub enum MockOperation {
    CreatePr { head, base, title, draft },
    UpdatePr { number, title, body, base },
    // ... etc
}
```

Features:
- In-memory PR storage with auto-incrementing numbers
- Configurable failure injection via `fail_on()`
- Operation recording for test verification
- `with_prs()` constructor for pre-populated state

### Phase 4: Auth Command

```rust
pub fn auth(ctx: &Context, token: Option<&str>, host: &str, status: bool, logout: bool) -> Result<()>
```

Modes:
- `--token <TOKEN>`: Non-interactive token input
- Interactive: Masked prompt via `rpassword::read_password()`
- `--status`: Show authentication status
- `--logout`: Remove stored token

Security:
- Token never printed to stdout/stderr
- Validation: not empty, no spaces/newlines
- Stored under `github.pat` key

Helper functions exported for other commands:
```rust
pub fn get_github_token() -> Result<String>
pub fn has_github_token() -> bool
```

### Phase 5: Scanner Enhancements

Added to `src/engine/scan.rs`:

```rust
// Check for RemoteResolved capability
if let Ok(Some(remote_url)) = git.remote_url("origin") {
    if crate::forge::github::parse_github_url(&remote_url).is_some() {
        health.add_capability(Capability::RemoteResolved);
    } else {
        health.add_issue(issues::remote_not_github(&remote_url));
    }
} else {
    health.add_issue(issues::no_remote_configured());
}

// Check for AuthAvailable capability
if crate::cli::commands::has_github_token() {
    health.add_capability(Capability::AuthAvailable);
}
```

New issue generators in `src/engine/health.rs`:
- `issues::no_remote_configured()` - Warning severity
- `issues::remote_not_github(url)` - Warning severity

### Phase 6: GitHub Commands

**Submit Command** (`src/cli/commands/submit.rs`):
- Iterates stack branches in order
- Creates new PRs or updates existing
- Handles draft toggle via `--draft`/`--publish`
- Requests reviewers if specified
- Dry-run mode shows plan without executing

**Sync Command** (`src/cli/commands/sync.rs`):
- Fetches from origin
- Fast-forwards trunk (or errors if diverged)
- Checks PR states for merged/closed branches
- Optionally restacks after sync

**Get Command** (`src/cli/commands/get.rs`):
- Accepts branch name or PR number
- Fetches from remote and tracks locally
- Defaults to frozen (use `--unfrozen` to override)

**Merge Command** (`src/cli/commands/merge.rs`):
- Merges PRs in stack order via GitHub API
- Supports `--method merge|squash|rebase`
- Dry-run shows plan without executing

**PR Command** (`src/cli/commands/pr.rs`):
- Opens PR URL in browser (or prints if non-interactive)
- `--stack` shows URLs for entire stack
- Falls back to `find_pr_by_head` if not linked

**Unlink Command** (`src/cli/commands/unlink.rs`):
- Removes PR linkage from metadata via CAS
- Does not affect PR on GitHub

## Dependencies Added

| Dependency | Version | Purpose |
|------------|---------|---------|
| tokio | 1 (rt-multi-thread, macros) | Async runtime |
| async-trait | 0.1 | Async methods in traits |
| reqwest | 0.12 (json) | HTTP client |
| rpassword | 7 | Masked password input |
| tokio-test | 0.4 (dev) | Async test utilities |
| wiremock | 0.6 (dev) | HTTP mocking (future use) |

## Test Summary

| Test File | Tests | Description |
|-----------|-------|-------------|
| `tests/github_integration.rs` | 33 | MockForge unit tests, forge flows, URL parsing |
| `src/forge/mock.rs` | 14 (inline) | MockForge internal tests |
| `src/engine/health.rs` | 2 (new) | Remote issue generators |

### Integration Test Categories

1. **MockForge Unit Tests** (15 tests): CRUD operations, state transitions
2. **Failure Injection Tests** (3 tests): Error path verification
3. **Operation Recording Tests** (4 tests): Audit trail verification
4. **Submit Flow Tests** (3 tests): PR creation/update logic
5. **Merge Flow Tests** (2 tests): Stack merge ordering
6. **GitHub URL Parsing** (6 tests): SSH/HTTPS URL parsing
7. **Auth Tests** (2 tests): Token presence checks

## Files Created/Modified

| File | Change |
|------|--------|
| `Cargo.toml` | Added async dependencies |
| `src/forge/traits.rs` | Rewrote with async Forge trait |
| `src/forge/github.rs` | NEW: GitHub REST/GraphQL implementation (~400 lines) |
| `src/forge/mock.rs` | NEW: Mock forge for testing (~700 lines) |
| `src/forge/mod.rs` | Export new modules |
| `src/cli/args.rs` | Added 7 new command definitions |
| `src/cli/commands/mod.rs` | Export new command modules |
| `src/cli/commands/auth.rs` | NEW: Auth command (~230 lines) |
| `src/cli/commands/submit.rs` | NEW: Submit command (~250 lines) |
| `src/cli/commands/sync.rs` | NEW: Sync command (~150 lines) |
| `src/cli/commands/get.rs` | NEW: Get command (~120 lines) |
| `src/cli/commands/merge.rs` | NEW: Merge command (~130 lines) |
| `src/cli/commands/pr.rs` | NEW: PR command (~100 lines) |
| `src/cli/commands/unlink.rs` | NEW: Unlink command (~60 lines) |
| `src/engine/scan.rs` | Added RemoteResolved, AuthAvailable detection |
| `src/engine/health.rs` | Added remote-related issue generators |
| `tests/github_integration.rs` | NEW: Integration tests (~750 lines) |

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `GITHUB_TOKEN` | - | Alternative token source for CI |
| `LATTICE_GITHUB_API` | api.github.com | API base URL override (for testing) |
| `LATTICE_LIVE_TESTS` | false | Enable live GitHub API tests |
| `LATTICE_TEST_OWNER` | - | Owner for live tests |
| `LATTICE_TEST_REPO` | - | Repo for live tests |

## Acceptance Gate Status

- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes (including new integration tests)
- [x] `cargo doc --no-deps` succeeds
- [x] All 7 new commands implemented
- [x] MockForge enables deterministic testing
- [x] Auth never prints tokens
- [x] Remote failures do not corrupt local state
- [x] Submit creates/updates PRs correctly
