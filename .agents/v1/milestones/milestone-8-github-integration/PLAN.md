# Milestone 8: Phase 2 GitHub Integration

## Summary

Implement GitHub integration for Lattice, transforming it from a local-only stacked branch tool into one that can submit, sync, and manage PRs on GitHub. This milestone adds:

1. **Forge abstraction** with full GitHub adapter (REST + GraphQL)
2. **Auth command** for secure token storage
3. **Remote-aware scanner** capabilities
4. **GitHub commands**: `auth`, `submit`, `sync`, `get`, `merge`, `pr`, `unlink`

**Architecture constraint**: Remote interactions occur in Phase 3 of plans and must not compromise local correctness.

---

## Implementation Phases

### Phase 1: Async Foundation & Dependencies
Add async runtime and HTTP dependencies to enable network operations.

### Phase 2: GitHub Forge Implementation (8.1)
Implement the `Forge` trait with full GitHub adapter.

### Phase 3: Auth Command (8.2)
Implement `lattice auth` for secure token storage.

### Phase 4: Remote-Aware Scanner (8.3)
Add `RemoteResolved` and `AuthAvailable` capability detection.

### Phase 5: GitHub Commands (8.4)
Implement commands in dependency order: `pr`/`unlink` → `submit` → `sync` → `get` → `merge`

---

## Implementation Steps

### Step 1: Add Dependencies (`Cargo.toml`)
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

### Step 2: Convert Forge Trait to Async (`src/forge/traits.rs`)
Add async methods using `async_trait`:
- `create_pr(&self, req: CreatePrRequest) -> Result<PullRequest, ForgeError>`
- `update_pr(&self, req: UpdatePrRequest) -> Result<PullRequest, ForgeError>`
- `get_pr(&self, number: u64) -> Result<PullRequest, ForgeError>`
- `find_pr_by_head(&self, head: &str) -> Result<Option<PullRequest>, ForgeError>`
- `set_draft(&self, pr: u64, draft: bool) -> Result<(), ForgeError>`
- `request_reviewers(&self, pr: u64, reviewers: Reviewers) -> Result<(), ForgeError>`
- `merge_pr(&self, pr: u64, method: MergeMethod) -> Result<(), ForgeError>`

Add `node_id: Option<String>` to `PullRequest` struct for GraphQL operations.

### Step 3: Implement GitHubForge (`src/forge/github.rs`) - NEW FILE
```rust
pub struct GitHubForge {
    client: reqwest::Client,
    token: String,
    owner: String,
    repo: String,
    api_base: String,
}
```

REST API endpoints:
- POST `/repos/{owner}/{repo}/pulls` - create PR
- PATCH `/repos/{owner}/{repo}/pulls/{number}` - update PR
- GET `/repos/{owner}/{repo}/pulls/{number}` - get PR
- GET `/repos/{owner}/{repo}/pulls?head={owner}:{branch}` - find by head
- PUT `/repos/{owner}/{repo}/pulls/{number}/merge` - merge
- POST `/repos/{owner}/{repo}/pulls/{number}/requested_reviewers` - request reviewers

GraphQL for draft toggle:
- `convertPullRequestToDraft` mutation
- `markPullRequestReadyForReview` mutation

Error mapping:
- 401 → `AuthFailed`
- 403 → `AuthFailed` (permissions)
- 404 → `NotFound`
- 429 → `RateLimited`
- 5xx → `ApiError`

### Step 4: Implement MockForge (`src/forge/mock.rs`) - NEW FILE
Deterministic mock for testing with:
- In-memory PR storage
- Configurable failure injection
- State tracking for verification

### Step 5: Update Forge Module (`src/forge/mod.rs`)
Export new modules: `github`, `mock`

### Step 6: Implement Auth Command (`src/cli/commands/auth.rs`) - NEW FILE
- `--token <TOKEN>`: non-interactive token input
- Interactive: masked prompt via `rpassword`
- Store under key `github.pat` in SecretStore
- NEVER print token in output

Add to `src/cli/args.rs`:
```rust
Auth {
    #[arg(long)]
    token: Option<String>,
    #[arg(long, default_value = "github")]
    host: String,
}
```

### Step 7: Add Scanner Capability Detection (`src/engine/scan.rs`)
Add to `scan()` function:

```rust
// RemoteResolved: origin is parseable as GitHub URL
if let Ok(Some(url)) = git.remote_url("origin") {
    if Git::parse_github_remote(&url).is_some() {
        health.add_capability(Capability::RemoteResolved);
    }
}

// AuthAvailable: token exists in secret store
let store = secrets::create_store(secrets::DEFAULT_PROVIDER)?;
if store.exists("github.pat")? {
    health.add_capability(Capability::AuthAvailable);
}
```

### Step 8: Add Remote Issues (`src/engine/health.rs`)
```rust
pub fn no_remote_configured() -> Issue { ... }
pub fn remote_not_github(url: &str) -> Issue { ... }
```

### Step 9: Implement `pr` Command (`src/cli/commands/pr.rs`) - NEW FILE
- Opens PR URL in browser (or prints if non-interactive)
- `--stack`: show URLs for entire stack
- Falls back to `find_pr_by_head` if not linked

### Step 10: Implement `unlink` Command (`src/cli/commands/unlink.rs`) - NEW FILE
- Remove PR linkage from metadata
- Does not alter PR on GitHub

### Step 11: Implement `submit` Command (`src/cli/commands/submit.rs`) - NEW FILE
Flags:
- `--stack`: include descendants
- `--draft` / `--publish`: draft toggle
- `--force`: override force-with-lease
- `--always`: push even if unchanged
- `--dry-run`: show plan without executing
- `--reviewers <users>` / `--team-reviewers <teams>`
- `--no-restack`: skip restack phase

Algorithm:
1. Gate on `requirements::REMOTE`
2. Optionally restack branches
3. For each branch in stack order:
   - Determine PR base (parent branch or trunk)
   - Push if changed (or `--always`)
   - Create/update PR via forge
   - Handle draft toggle
   - Request reviewers if specified
4. Update metadata with PR linkage

### Step 12: Implement `sync` Command (`src/cli/commands/sync.rs`) - NEW FILE
1. `git fetch origin`
2. Fast-forward trunk (or error if diverged without `--force`)
3. For each tracked branch with linked PR:
   - Check if PR merged/closed
   - Prompt to delete local branch
4. Optionally restack

### Step 13: Implement `get` Command (`src/cli/commands/get.rs`) - NEW FILE
- Accept branch name or PR number
- Fetch from remote
- Determine parent from PR base or trunk
- Track fetched branch (frozen by default, `--unfrozen` to override)
- Optionally restack

### Step 14: Implement `merge` Command (`src/cli/commands/merge.rs`) - NEW FILE
- Merge PRs in stack order via GitHub API
- `--method merge|squash|rebase`
- `--dry-run` shows plan
- Stop on first failure

### Step 15: Update CLI Args (`src/cli/args.rs`)
Add all 7 new commands to `Command` enum with full flag definitions.

### Step 16: Update Command Dispatch (`src/cli/commands/mod.rs`)
Export and dispatch to new command modules.

### Step 17: Add Tokio Runtime (`src/main.rs`)
```rust
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse_args();
    dispatch(cli.command).await
}
```

### Step 18: Integration Tests (`tests/github_integration.rs`) - NEW FILE
Mock tests:
- Submit creates PRs with correct bases
- Submit updates existing PRs (no duplicates)
- Submit skips unchanged branches
- Auth stores token correctly
- Sync detects merged PRs

Live tests (behind `live_github_tests` feature):
- Create and close PR
- Draft toggle
- Merge via API

### Step 19: Milestone Documentation
Create `.agents/v1/milestones/milestone-8-github-integration/`:
- `PLAN.md` - this plan
- `implementation_notes.md` - post-implementation notes

---

## Critical Files

| File | Purpose |
|------|---------|
| `Cargo.toml` | Add tokio, async-trait, reqwest, rpassword |
| `src/forge/traits.rs` | Add async methods to Forge trait |
| `src/forge/github.rs` | NEW: GitHub REST/GraphQL implementation |
| `src/forge/mock.rs` | NEW: Mock forge for testing |
| `src/forge/mod.rs` | Export new modules |
| `src/cli/args.rs` | Add 7 new command definitions |
| `src/cli/commands/mod.rs` | Export new command modules |
| `src/cli/commands/auth.rs` | NEW: Auth command |
| `src/cli/commands/submit.rs` | NEW: Submit command |
| `src/cli/commands/sync.rs` | NEW: Sync command |
| `src/cli/commands/get.rs` | NEW: Get command |
| `src/cli/commands/merge.rs` | NEW: Merge command |
| `src/cli/commands/pr.rs` | NEW: PR command |
| `src/cli/commands/unlink.rs` | NEW: Unlink command |
| `src/engine/scan.rs` | Add RemoteResolved, AuthAvailable detection |
| `src/engine/health.rs` | Add remote-related issues |
| `src/main.rs` | Add tokio runtime |
| `tests/github_integration.rs` | NEW: Integration tests |

---

## Test Requirements

### Auth Tests
- Token stored via `--token` and retrievable
- Interactive prompt works (stdin simulation)
- File permissions correct (Unix: 0600)
- Token NEVER appears in stdout/stderr

### Submit Tests (per SPEC.md)
- New stack creates PRs with correct bases
- Re-run updates existing PRs, no duplicates
- Skip unchanged push behavior
- `--always` forces pushes
- `--force` overwrites remote divergence
- `--dry-run` produces no changes
- Draft create and publish toggling
- Missing auth returns exit 1 with clear message

### Sync Tests
- Merged branch deletion prompt
- Trunk fast-forward update
- Diverged trunk requires `--force`
- Restack happens post-trunk update

### Get Tests
- Get by PR number resolves and fetches
- New fetched branch defaults frozen
- `--unfrozen` overrides freeze default
- `--force` overwrites local divergence

### Merge Tests
- Merge calls happen in correct order
- `--dry-run` makes no API calls
- Stop on first failure

---

## Acceptance Gates

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes (including new integration tests)
- [ ] `cargo doc --no-deps` succeeds
- [ ] All 7 new commands implemented and tested
- [ ] Mock forge enables deterministic testing
- [ ] Auth never prints tokens
- [ ] Remote failures do not corrupt local state
- [ ] Submit creates/updates PRs correctly
- [ ] Live tests pass (optional, behind feature flag)

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `GITHUB_TOKEN` | - | Alternative token source for CI |
| `LATTICE_GITHUB_API` | api.github.com | API base URL override |
| `LATTICE_LIVE_TESTS` | false | Enable live GitHub API tests |
