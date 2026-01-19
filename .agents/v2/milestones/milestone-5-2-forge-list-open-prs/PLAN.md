# Milestone 5.2: Forge `list_open_prs` Capability

## Goal

Enable bulk PR queries for bootstrap evidence collection by adding `list_open_prs` to the `Forge` trait.

**Core principle:** Bootstrap needs to discover existing PRs in bulk to detect branches that could be imported. The current `find_pr_by_head` method requires N+1 queries (one per branch), which is inefficient for bootstrap scenarios. A bulk enumeration method is needed.

---

## Background

Currently, the `Forge` trait provides:
- `find_pr_by_head(head: &str)` - Find a single PR by head branch name

For bootstrap, we need to:
1. Enumerate all open PRs in a repository
2. Match them against local branches
3. Suggest tracking PRs that have local branch equivalents

This requires a new bulk query method that can return all open PRs (up to a configurable limit) in a single API call with pagination handled internally.

**Current implementation:** `src/forge/traits.rs` and `src/forge/github.rs`
- `Forge` trait defines PR operations
- `GitHubForge` implements using REST and GraphQL APIs
- `MockForge` provides deterministic testing

---

## Spec References

- **SPEC.md Section 8E.1** - Forge abstraction trait definition
- **ARCHITECTURE.md Section 11** - Host adapter architecture
- **ARCHITECTURE.md Section 11.2** - Cached metadata handling (PR linkage is cached)
- **ROADMAP.md Milestone 5.2** - Deliverables and acceptance criteria

---

## Design Decisions

### Why a new method instead of using `find_pr_by_head`?

1. **Efficiency:** `find_pr_by_head` requires one API call per branch. For a repo with 50 local branches, that's 50 API calls. `list_open_prs` gets all open PRs in 2-3 paginated calls.

2. **Rate limiting:** GitHub's rate limit (5000 requests/hour for authenticated users) would be quickly exhausted with N+1 queries during bootstrap.

3. **Bootstrap UX:** Users expect bootstrap to be fast. Bulk query enables responsive feedback.

### What data does `PullRequestSummary` need?

The summary is a lightweight struct for listing/matching purposes:

| Field | Type | Purpose |
|-------|------|---------|
| `number` | `u64` | PR identifier |
| `head_ref` | `String` | Head branch name (for matching local branches) |
| `head_repo_owner` | `Option<String>` | Fork owner (None = same repo) |
| `base_ref` | `String` | Base branch (usually trunk) |
| `is_draft` | `bool` | Draft status |
| `url` | `String` | Web URL for display |
| `updated_at` | `String` | ISO 8601 timestamp (for freshness) |

**Why not reuse `PullRequest`?** The full `PullRequest` struct includes `title`, `body`, `node_id`, and `state` which are not needed for listing and would increase memory usage and parsing time for bulk results.

### Pagination strategy

GitHub's REST API returns max 100 items per page. For a limit of 200 PRs:
1. Request page 1 (100 items)
2. If more exist, request page 2 (100 items)
3. Stop at configured limit

Pagination is handled **internally** - callers don't deal with pages.

### Truncation handling

When the repository has more open PRs than `max_results`:
1. Return exactly `max_results` items
2. Set a `truncated` flag in the response
3. Log an info message (not an error)

---

## Implementation Steps

### Step 1: Add `ListPullsOpts` struct

**File:** `src/forge/traits.rs`

```rust
/// Options for listing pull requests.
#[derive(Debug, Clone, Default)]
pub struct ListPullsOpts {
    /// Maximum number of PRs to return.
    /// Default: 200. GitHub returns max 100 per page, so this may require
    /// multiple paginated requests internally.
    pub max_results: Option<usize>,
}

impl ListPullsOpts {
    /// Create options with a specific limit.
    pub fn with_limit(limit: usize) -> Self {
        Self { max_results: Some(limit) }
    }
    
    /// Get the effective limit (default 200).
    pub fn effective_limit(&self) -> usize {
        self.max_results.unwrap_or(200)
    }
}
```

### Step 2: Add `PullRequestSummary` struct

**File:** `src/forge/traits.rs`

```rust
/// Lightweight PR summary for bulk listing.
///
/// Contains only the fields needed for bootstrap matching and display.
/// Use `get_pr` for full PR details when needed.
#[derive(Debug, Clone)]
pub struct PullRequestSummary {
    /// PR number
    pub number: u64,
    /// Head branch name (the branch with changes)
    pub head_ref: String,
    /// Head repository owner (for fork PRs, None if same repo)
    pub head_repo_owner: Option<String>,
    /// Base branch name (the branch to merge into)
    pub base_ref: String,
    /// Whether the PR is a draft
    pub is_draft: bool,
    /// Web URL for the PR
    pub url: String,
    /// Last updated timestamp (ISO 8601)
    pub updated_at: String,
}

impl PullRequestSummary {
    /// Check if this PR is from a fork.
    pub fn is_fork(&self) -> bool {
        self.head_repo_owner.is_some()
    }
}
```

### Step 3: Add `ListPullsResult` struct

**File:** `src/forge/traits.rs`

```rust
/// Result from listing pull requests.
#[derive(Debug, Clone)]
pub struct ListPullsResult {
    /// The pull requests (up to the requested limit).
    pub pulls: Vec<PullRequestSummary>,
    /// Whether there were more PRs than the limit allowed.
    pub truncated: bool,
}
```

### Step 4: Add `list_open_prs` to `Forge` trait

**File:** `src/forge/traits.rs`

Add the method to the `Forge` trait:

```rust
#[async_trait]
pub trait Forge: Send + Sync {
    // ... existing methods ...

    /// List open pull requests.
    ///
    /// Returns open PRs up to the configured limit, ordered by most recently
    /// updated first. Pagination is handled internally.
    ///
    /// # Arguments
    ///
    /// * `opts` - Options controlling the query (limit, etc.)
    ///
    /// # Returns
    ///
    /// A `ListPullsResult` containing the PRs and truncation status.
    ///
    /// # Errors
    ///
    /// - `AuthRequired` if no authentication is configured
    /// - `AuthFailed` if the token is invalid or lacks permissions
    /// - `RateLimited` if API rate limit is exceeded
    async fn list_open_prs(&self, opts: ListPullsOpts) -> Result<ListPullsResult, ForgeError>;
}
```

### Step 5: Add GitHub response types

**File:** `src/forge/github.rs`

Add the deserialize struct for GitHub's list PRs response:

```rust
/// GitHub PR list item (subset of full PR response).
#[derive(Deserialize)]
struct GitHubPullRequestSummary {
    number: u64,
    html_url: String,
    draft: bool,
    head: GitHubHeadRef,
    base: GitHubRef,
    updated_at: String,
}

/// GitHub head ref with repo info (for fork detection).
#[derive(Deserialize)]
struct GitHubHeadRef {
    #[serde(rename = "ref")]
    ref_name: String,
    repo: Option<GitHubRepoRef>,
}

/// GitHub repo reference (minimal).
#[derive(Deserialize)]
struct GitHubRepoRef {
    owner: GitHubOwner,
}

/// GitHub owner (minimal).
#[derive(Deserialize)]
struct GitHubOwner {
    login: String,
}

impl From<GitHubPullRequestSummary> for PullRequestSummary {
    fn from(gh: GitHubPullRequestSummary) -> Self {
        let head_repo_owner = gh.head.repo
            .map(|r| r.owner.login);
        
        PullRequestSummary {
            number: gh.number,
            head_ref: gh.head.ref_name,
            head_repo_owner,
            base_ref: gh.base.ref_name,
            is_draft: gh.draft,
            url: gh.html_url,
            updated_at: gh.updated_at,
        }
    }
}
```

### Step 6: Implement `list_open_prs` for `GitHubForge`

**File:** `src/forge/github.rs`

```rust
impl GitHubForge {
    /// Fetch a single page of PRs.
    async fn fetch_pr_page(&self, page: u32, per_page: u32) -> Result<Vec<GitHubPullRequestSummary>, ForgeError> {
        let url = format!(
            "{}/repos/{}/{}/pulls?state=open&sort=updated&direction=desc&per_page={}&page={}",
            self.api_base, self.owner, self.repo, per_page, page
        );

        let response = self
            .client
            .get(&url)
            .headers(self.headers())
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        self.handle_response(response).await
    }
}

#[async_trait]
impl Forge for GitHubForge {
    // ... existing methods ...

    async fn list_open_prs(&self, opts: ListPullsOpts) -> Result<ListPullsResult, ForgeError> {
        let limit = opts.effective_limit();
        let per_page: u32 = 100; // GitHub's max per page
        
        let mut all_prs = Vec::with_capacity(limit.min(100));
        let mut page: u32 = 1;
        let mut truncated = false;

        loop {
            let page_prs = self.fetch_pr_page(page, per_page).await?;
            let page_count = page_prs.len();
            
            for pr in page_prs {
                if all_prs.len() >= limit {
                    truncated = true;
                    break;
                }
                all_prs.push(pr.into());
            }

            // Stop if we hit the limit or no more pages
            if all_prs.len() >= limit || page_count < per_page as usize {
                break;
            }

            page += 1;
        }

        // If we stopped due to limit and there might be more, mark truncated
        if all_prs.len() == limit {
            // Could be exactly at limit with no more, or truncated
            // We'll be conservative and check if last page was full
            truncated = true;
        }

        Ok(ListPullsResult {
            pulls: all_prs,
            truncated,
        })
    }
}
```

### Step 7: Add `MockOperation::ListOpenPrs` variant

**File:** `src/forge/mock.rs`

```rust
/// Recorded operation for test verification.
#[derive(Debug, Clone)]
pub enum MockOperation {
    // ... existing variants ...
    ListOpenPrs {
        max_results: Option<usize>,
    },
}
```

### Step 8: Add `FailOn::ListOpenPrs` variant

**File:** `src/forge/mock.rs`

```rust
/// Configuration for which operation should fail.
#[derive(Debug, Clone)]
pub enum FailOn {
    // ... existing variants ...
    /// Fail list_open_prs with the given error.
    ListOpenPrs(ForgeError),
}
```

### Step 9: Implement `list_open_prs` for `MockForge`

**File:** `src/forge/mock.rs`

```rust
#[async_trait]
impl Forge for MockForge {
    // ... existing methods ...

    async fn list_open_prs(&self, opts: ListPullsOpts) -> Result<ListPullsResult, ForgeError> {
        self.record(MockOperation::ListOpenPrs {
            max_results: opts.max_results,
        });

        if let Some(result) = self.check_fail("list_open_prs") {
            return result;
        }

        let limit = opts.effective_limit();
        let inner = self.inner.lock().unwrap();
        
        // Get all open PRs, sorted by number descending (simulating updated_at sort)
        let mut open_prs: Vec<_> = inner
            .prs
            .values()
            .filter(|p| p.state == PrState::Open)
            .collect();
        
        open_prs.sort_by(|a, b| b.number.cmp(&a.number));
        
        let truncated = open_prs.len() > limit;
        let pulls: Vec<PullRequestSummary> = open_prs
            .into_iter()
            .take(limit)
            .map(|pr| PullRequestSummary {
                number: pr.number,
                head_ref: pr.head.clone(),
                head_repo_owner: None, // Mock doesn't track forks
                base_ref: pr.base.clone(),
                is_draft: pr.is_draft,
                url: pr.url.clone(),
                updated_at: "2024-01-01T00:00:00Z".to_string(), // Mock timestamp
            })
            .collect();

        Ok(ListPullsResult { pulls, truncated })
    }
}
```

### Step 10: Update `check_fail` for new variant

**File:** `src/forge/mock.rs`

Add to the `check_fail` method:

```rust
fn check_fail<T>(&self, expected: &str) -> Option<Result<T, ForgeError>> {
    let inner = self.inner.lock().unwrap();
    match &inner.fail_on {
        // ... existing matches ...
        Some(FailOn::ListOpenPrs(e)) if expected == "list_open_prs" => {
            Some(Err(clone_error(e)))
        }
        _ => None,
    }
}
```

### Step 11: Export new types from `mod.rs`

**File:** `src/forge/mod.rs`

The types are already exported via `pub use traits::*;`, so no changes needed unless we want explicit re-exports for documentation.

### Step 12: Add unit tests for MockForge

**File:** `src/forge/mock.rs`

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    mod list_open_prs {
        use super::*;

        #[tokio::test]
        async fn returns_open_prs() {
            let forge = MockForge::new();
            
            // Create some PRs
            for i in 1..=5 {
                forge.create_pr(CreatePrRequest {
                    head: format!("feature-{}", i),
                    base: "main".into(),
                    title: format!("PR {}", i),
                    body: None,
                    draft: false,
                }).await.unwrap();
            }

            let result = forge.list_open_prs(ListPullsOpts::default()).await.unwrap();
            assert_eq!(result.pulls.len(), 5);
            assert!(!result.truncated);
        }

        #[tokio::test]
        async fn respects_limit() {
            let forge = MockForge::new();
            
            // Create 10 PRs
            for i in 1..=10 {
                forge.create_pr(CreatePrRequest {
                    head: format!("feature-{}", i),
                    base: "main".into(),
                    title: format!("PR {}", i),
                    body: None,
                    draft: false,
                }).await.unwrap();
            }

            let result = forge.list_open_prs(ListPullsOpts::with_limit(3)).await.unwrap();
            assert_eq!(result.pulls.len(), 3);
            assert!(result.truncated);
        }

        #[tokio::test]
        async fn excludes_merged_and_closed() {
            let forge = MockForge::new();
            
            let pr1 = forge.create_pr(CreatePrRequest {
                head: "open-pr".into(),
                base: "main".into(),
                title: "Open".into(),
                body: None,
                draft: false,
            }).await.unwrap();
            
            let pr2 = forge.create_pr(CreatePrRequest {
                head: "merged-pr".into(),
                base: "main".into(),
                title: "To merge".into(),
                body: None,
                draft: false,
            }).await.unwrap();
            
            forge.merge_pr(pr2.number, MergeMethod::Squash).await.unwrap();

            let result = forge.list_open_prs(ListPullsOpts::default()).await.unwrap();
            assert_eq!(result.pulls.len(), 1);
            assert_eq!(result.pulls[0].number, pr1.number);
        }

        #[tokio::test]
        async fn fail_on_list_open_prs() {
            let forge = MockForge::new()
                .fail_on(FailOn::ListOpenPrs(ForgeError::RateLimited));

            let result = forge.list_open_prs(ListPullsOpts::default()).await;
            assert!(matches!(result, Err(ForgeError::RateLimited)));
        }

        #[tokio::test]
        async fn records_operation() {
            let forge = MockForge::new();
            
            forge.list_open_prs(ListPullsOpts::with_limit(50)).await.unwrap();
            
            let ops = forge.operations();
            assert_eq!(ops.len(), 1);
            match &ops[0] {
                MockOperation::ListOpenPrs { max_results } => {
                    assert_eq!(*max_results, Some(50));
                }
                _ => panic!("Expected ListOpenPrs operation"),
            }
        }

        #[tokio::test]
        async fn empty_repo_returns_empty_list() {
            let forge = MockForge::new();
            
            let result = forge.list_open_prs(ListPullsOpts::default()).await.unwrap();
            assert!(result.pulls.is_empty());
            assert!(!result.truncated);
        }
    }
}
```

### Step 13: Add unit tests for traits

**File:** `src/forge/traits.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ... existing tests ...

    mod list_pulls_opts {
        use super::*;

        #[test]
        fn default_limit_is_200() {
            let opts = ListPullsOpts::default();
            assert_eq!(opts.effective_limit(), 200);
        }

        #[test]
        fn with_limit_sets_max_results() {
            let opts = ListPullsOpts::with_limit(50);
            assert_eq!(opts.max_results, Some(50));
            assert_eq!(opts.effective_limit(), 50);
        }
    }

    mod pull_request_summary {
        use super::*;

        #[test]
        fn is_fork_with_owner() {
            let summary = PullRequestSummary {
                number: 1,
                head_ref: "feature".into(),
                head_repo_owner: Some("forker".into()),
                base_ref: "main".into(),
                is_draft: false,
                url: "https://example.com".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
            };
            assert!(summary.is_fork());
        }

        #[test]
        fn is_fork_without_owner() {
            let summary = PullRequestSummary {
                number: 1,
                head_ref: "feature".into(),
                head_repo_owner: None,
                base_ref: "main".into(),
                is_draft: false,
                url: "https://example.com".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
            };
            assert!(!summary.is_fork());
        }
    }
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/forge/traits.rs` | MODIFY | Add `ListPullsOpts`, `PullRequestSummary`, `ListPullsResult`, and `list_open_prs` to trait |
| `src/forge/github.rs` | MODIFY | Implement `list_open_prs` with pagination |
| `src/forge/mock.rs` | MODIFY | Add mock implementation and test scenarios |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.2:

- [ ] `ListPullsOpts` struct with `max_results: Option<usize>` (default 200)
- [ ] `PullRequestSummary` struct with required fields
- [ ] `ListPullsResult` struct with `pulls` and `truncated` flag
- [ ] `list_open_prs` added to `Forge` trait
- [ ] GitHub implementation uses REST API `GET /repos/{owner}/{repo}/pulls?state=open`
- [ ] Pagination handled internally (follows through all pages up to limit)
- [ ] Rate limit errors surfaced as `ForgeError::RateLimited`
- [ ] Truncation clearly indicated via `truncated` flag when limit exceeded
- [ ] `MockForge` supports test scenarios with configurable PR lists
- [ ] `FailOn::ListOpenPrs` variant for testing error paths
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `list_pulls_opts::default_limit_is_200` | `traits.rs` | Default limit verification |
| `list_pulls_opts::with_limit_sets_max_results` | `traits.rs` | Custom limit setting |
| `pull_request_summary::is_fork_*` | `traits.rs` | Fork detection helper |
| `list_open_prs::returns_open_prs` | `mock.rs` | Basic listing works |
| `list_open_prs::respects_limit` | `mock.rs` | Truncation at limit |
| `list_open_prs::excludes_merged_and_closed` | `mock.rs` | Only open PRs returned |
| `list_open_prs::fail_on_list_open_prs` | `mock.rs` | Error injection works |
| `list_open_prs::records_operation` | `mock.rs` | Operation recording |
| `list_open_prs::empty_repo_returns_empty_list` | `mock.rs` | Empty case handling |

### Integration Tests (Optional - requires real GitHub token)

For manual verification with a real repository:

```bash
# With a test repository that has multiple open PRs
GITHUB_TOKEN=xxx cargo test --features integration-tests -- github_list_open_prs
```

### Manual Verification

1. Set up a test repository with multiple open PRs (or use a public repo)
2. Run a test harness that calls `list_open_prs` with different limits
3. Verify pagination works correctly for repos with >100 open PRs
4. Verify rate limit error handling by exhausting API calls

---

## API Reference

### GitHub REST API: List Pull Requests

**Endpoint:** `GET /repos/{owner}/{repo}/pulls`

**Parameters:**
- `state=open` - Only open PRs
- `sort=updated` - Sort by last updated
- `direction=desc` - Most recent first
- `per_page=100` - Maximum items per page
- `page=N` - Page number (1-indexed)

**Response fields used:**
- `number` - PR number
- `html_url` - Web URL
- `draft` - Draft status
- `head.ref` - Head branch name
- `head.repo.owner.login` - Fork owner (null if same repo)
- `base.ref` - Base branch name
- `updated_at` - Last update timestamp

**Rate limits:**
- Authenticated: 5000 requests/hour
- Returns 403 with `X-RateLimit-Remaining: 0` when exceeded

---

## Edge Cases

1. **Empty repository:** Return empty list, `truncated: false`
2. **Exactly at limit:** Return all items, `truncated: false` (no more exist)
3. **More than limit:** Return `limit` items, `truncated: true`
4. **All PRs are drafts:** Include them (draft status is informational)
5. **Fork PRs:** Include them with `head_repo_owner` set
6. **Rate limited mid-pagination:** Return `ForgeError::RateLimited` (don't return partial results)
7. **Network failure:** Return `ForgeError::NetworkError`

---

## Dependencies

- **None** - This milestone is foundational for 5.3+

---

## Estimated Scope

- **Lines of code changed:** ~150-200 in `traits.rs`, ~50-80 in `github.rs`, ~100-150 in `mock.rs`
- **New structs:** 3 (`ListPullsOpts`, `PullRequestSummary`, `ListPullsResult`)
- **New trait method:** 1 (`list_open_prs`)
- **Risk:** Low - additive change, no existing behavior modified

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

# Specific forge tests
cargo test forge

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** Follows existing patterns in `Forge` trait (async, Result return, ForgeError)
- **Simplicity:** Single method addition with clear purpose
- **Purity:** No state mutation - pure query operation
- **Reuse:** Uses existing `ForgeError` variants, existing HTTP client infrastructure
- **Code is communication:** Comprehensive doc comments on all new types

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.2 as complete
2. Create `implementation_notes.md` in this directory
3. Milestone 5.3 (Bootstrap Issue Detection) can begin using `list_open_prs`
