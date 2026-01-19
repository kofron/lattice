# Milestone 5.8: Synthetic Stack Detection (Two-Tiered)

## Goal

Detect synthetic remote stack patterns and surface them as informational issues, enabling users to understand the history context of PRs that have accumulated commits from merged sub-PRs.

**Core principle from ARCHITECTURE.md Section 8.1:** "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

---

## Background

A **synthetic remote stack** exists when:
- An open PR P0 targets trunk with head branch H
- Closed/merged PRs exist whose base branch is H

This pattern occurs when a developer:
1. Opens a PR against trunk (PR P0, head branch H)
2. Creates sub-branches off H and opens PRs targeting H
3. Merges those sub-PRs into H while P0 remains open
4. P0 accumulates all the merged work

**Interpretation:** Prior reviewed work was merged into H while P0 remains open. This is useful context but not automatically reconstructable as a Lattice stack - the commits have already been squash-merged or rebased.

| Milestone | Component | Status |
|-----------|-----------|--------|
| 5.2 | `list_open_prs` capability | Complete |
| 5.3 | Bootstrap issue detection | Complete |
| 5.4 | Bootstrap fix generators (remote) | Complete |
| 5.5 | Bootstrap fix execution | Complete |
| 5.6 | Init hint for bootstrap | Complete |
| 5.7 | Local-only bootstrap | Complete |
| **5.8** | **Synthetic stack detection** | **This milestone** |

---

## Spec References

- **ROADMAP.md Milestone 5.8** - Synthetic stack detection deliverables
- **ARCHITECTURE.md Section 8** - Doctor framework
- **ARCHITECTURE.md Section 8.2** - Issues and fix options
- **SPEC.md Section 8E.1** - Forge trait definition
- **ARCHITECTURE.md Section 11** - Host adapter architecture

---

## Design Decisions

### Two-Tiered Approach

Per ROADMAP.md, we use a two-tiered approach to balance API cost vs. information:

**Tier 1 (Default - Cheap):**
- Fetch open PRs only via existing `list_open_prs`
- Identify trunk-bound PRs as "potential synthetic stack heads"
- Emit Info issue: "Potential synthetic stack head detected: trunk PR head `H`"
- Do NOT enumerate closed PRs automatically

**Tier 2 (Explicit - Deep Remote):**
- Flag: `lt doctor --deep-remote`
- Config: `doctor.bootstrap.deep_remote = true|false` (default false)
- Budgets (configurable):
  - `max_synthetic_heads`: 3 (default)
  - `max_closed_prs_per_head`: 20 (default)
- Truncation explicitly reported when budgets exceeded
- Closed PR enumeration happens during fix plan construction, not baseline scan

### Why Two Tiers?

1. **API Cost:** GitHub charges by API call. Enumerating closed PRs for every synthetic head is expensive.
2. **Rate Limits:** Aggressive closed PR queries can exhaust rate limits quickly.
3. **User Intent:** Most users don't need full synthetic stack analysis - just knowing they exist is often enough.
4. **Explicit Opt-in:** Deep analysis should be a conscious choice, not automatic.

### Synthetic Stack Head Definition

A branch H is a **potential synthetic stack head** if:
1. An open PR exists with head_ref = H
2. The PR's base_ref = configured trunk

This is the minimal definition for Tier 1. Tier 2 confirms by finding closed PRs that targeted H.

### Issue Structure

**Tier 1 Issue (always detected when conditions met):**
```rust
KnownIssue::PotentialSyntheticStackHead {
    /// The branch that may be a synthetic stack head.
    branch: String,
    /// PR number targeting trunk.
    pr_number: u64,
    /// PR URL.
    pr_url: String,
}
```
- Severity: Info
- No fix options in Tier 1 (informational only)

**Tier 2 Evidence (when --deep-remote):**
```rust
Evidence::SyntheticStackChildren {
    /// The synthetic head branch.
    head_branch: String,
    /// Closed PRs that targeted this head.
    closed_prs: Vec<ClosedPrInfo>,
    /// Whether the result was truncated.
    truncated: bool,
}

struct ClosedPrInfo {
    number: u64,
    head_ref: String,
    merged: bool,  // true if merged, false if just closed
    url: String,
}
```

### Forge Extension: list_closed_prs_targeting

To support Tier 2, we need to query closed PRs that targeted a specific base branch.

**New trait method:**
```rust
/// Options for listing closed PRs targeting a specific base.
#[derive(Debug, Clone, Default)]
pub struct ListClosedPrsOpts {
    /// Base branch to filter by.
    pub base: String,
    /// Maximum results per query.
    pub max_results: Option<usize>,
}

/// List closed PRs targeting a specific base branch.
async fn list_closed_prs_targeting(
    &self,
    opts: ListClosedPrsOpts,
) -> Result<ListPullsResult, ForgeError>;
```

**GitHub implementation:**
- REST API: `GET /repos/{owner}/{repo}/pulls?state=closed&base={base}`
- Returns both merged and unmerged closed PRs
- Pagination handled internally

### Configuration Schema

Add to `core/config/schema.rs`:

```rust
/// Doctor bootstrap configuration.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DoctorBootstrapConfig {
    /// Enable deep remote analysis (query closed PRs).
    /// Default: false
    #[serde(default)]
    pub deep_remote: bool,

    /// Maximum synthetic stack heads to analyze in deep mode.
    /// Default: 3
    #[serde(default = "default_max_synthetic_heads")]
    pub max_synthetic_heads: usize,

    /// Maximum closed PRs to query per synthetic head.
    /// Default: 20
    #[serde(default = "default_max_closed_prs_per_head")]
    pub max_closed_prs_per_head: usize,
}

fn default_max_synthetic_heads() -> usize { 3 }
fn default_max_closed_prs_per_head() -> usize { 20 }
```

### Command-Line Flag

Add to `doctor` command in `cli/args.rs`:

```rust
/// Perform deep remote analysis (query closed PRs for synthetic stacks).
#[arg(long)]
pub deep_remote: bool,
```

---

## Implementation Steps

### Step 1: Add KnownIssue Variant for Potential Synthetic Stack Head

**File:** `src/doctor/issues.rs`

```rust
/// A PR targeting trunk may be a synthetic stack head.
/// This indicates prior work may have been merged into the branch.
#[error("PR #{pr_number} targeting trunk may be a synthetic stack head (branch '{branch}')")]
PotentialSyntheticStackHead {
    /// The branch that may be a synthetic stack head.
    branch: String,
    /// PR number targeting trunk.
    pr_number: u64,
    /// PR URL.
    pr_url: String,
},
```

Update `issue_id()`:
```rust
KnownIssue::PotentialSyntheticStackHead { pr_number, .. } => {
    IssueId::new("synthetic-stack-head", &pr_number.to_string())
}
```

Update `severity()`:
```rust
KnownIssue::PotentialSyntheticStackHead { .. } => Severity::Info,
```

Update `to_issue()`:
```rust
KnownIssue::PotentialSyntheticStackHead {
    branch,
    pr_number,
    pr_url,
} => issues::potential_synthetic_stack_head(branch, *pr_number, pr_url),
```

### Step 2: Add Issue Constructor in health::issues

**File:** `src/engine/health.rs` (in the `issues` module)

```rust
/// Create an issue for a potential synthetic stack head.
///
/// This is an informational issue indicating that a PR targeting trunk
/// may have accumulated commits from merged sub-PRs.
pub fn potential_synthetic_stack_head(branch: &str, pr_number: u64, pr_url: &str) -> Issue {
    Issue::new(
        &IssueId::new("synthetic-stack-head", &pr_number.to_string()).to_string(),
        Severity::Info,
        &format!(
            "PR #{} targeting trunk may be a synthetic stack head (branch '{}')",
            pr_number, branch
        ),
    )
    .with_evidence(Evidence::PrReference {
        number: pr_number,
        url: pr_url.to_string(),
        context: format!("Open PR targeting trunk with head branch '{}'", branch),
    })
}
```

### Step 3: Add Evidence Types for Synthetic Stack

**File:** `src/engine/health.rs`

```rust
/// Information about a closed PR that targeted a synthetic stack head.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedPrInfo {
    /// PR number.
    pub number: u64,
    /// Head branch of the closed PR.
    pub head_ref: String,
    /// Whether the PR was merged (true) or just closed (false).
    pub merged: bool,
    /// PR URL.
    pub url: String,
}

/// Evidence associated with an issue.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    // ... existing variants ...

    /// PR reference for context.
    PrReference {
        number: u64,
        url: String,
        context: String,
    },

    /// Closed PRs that targeted a synthetic stack head (Tier 2 deep analysis).
    SyntheticStackChildren {
        /// The synthetic head branch.
        head_branch: String,
        /// Closed PRs that targeted this head.
        closed_prs: Vec<ClosedPrInfo>,
        /// Whether the result was truncated due to budget limits.
        truncated: bool,
    },
}
```

### Step 4: Add Configuration Schema

**File:** `src/core/config/schema.rs`

```rust
/// Doctor bootstrap configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct DoctorBootstrapConfig {
    /// Enable deep remote analysis (query closed PRs).
    pub deep_remote: bool,
    /// Maximum synthetic stack heads to analyze in deep mode.
    pub max_synthetic_heads: usize,
    /// Maximum closed PRs to query per synthetic head.
    pub max_closed_prs_per_head: usize,
}

impl Default for DoctorBootstrapConfig {
    fn default() -> Self {
        Self {
            deep_remote: false,
            max_synthetic_heads: 3,
            max_closed_prs_per_head: 20,
        }
    }
}
```

Add to global config:
```rust
pub struct GlobalConfig {
    // ... existing fields ...
    
    /// Doctor bootstrap settings.
    #[serde(default)]
    pub doctor: DoctorConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DoctorConfig {
    /// Bootstrap-related settings.
    #[serde(default)]
    pub bootstrap: DoctorBootstrapConfig,
}
```

### Step 5: Add --deep-remote Flag to Doctor Command

**File:** `src/cli/args.rs`

In the doctor command struct:
```rust
#[derive(Debug, Parser)]
pub struct DoctorArgs {
    // ... existing fields ...

    /// Perform deep remote analysis.
    ///
    /// Queries closed PRs to confirm synthetic stack patterns.
    /// This may consume additional API quota.
    #[arg(long)]
    pub deep_remote: bool,
}
```

### Step 6: Add Forge Method for Closed PRs

**File:** `src/forge/traits.rs`

```rust
/// Options for listing closed PRs targeting a specific base branch.
#[derive(Debug, Clone)]
pub struct ListClosedPrsOpts {
    /// Base branch to filter by (PRs that targeted this branch).
    pub base: String,
    /// Maximum number of PRs to return.
    pub max_results: Option<usize>,
}

impl ListClosedPrsOpts {
    /// Create options for a specific base branch.
    pub fn for_base(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            max_results: None,
        }
    }

    /// Set the maximum results.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.max_results = Some(limit);
        self
    }

    /// Get the effective limit (default 100).
    pub fn effective_limit(&self) -> usize {
        self.max_results.unwrap_or(100)
    }
}
```

Add to `Forge` trait:
```rust
/// List closed PRs that targeted a specific base branch.
///
/// This is used for deep synthetic stack analysis to find PRs
/// that were merged into a potential synthetic stack head.
///
/// # Arguments
///
/// * `opts` - Options including the base branch to filter by
///
/// # Returns
///
/// A [`ListPullsResult`] containing closed PRs (merged or unmerged).
async fn list_closed_prs_targeting(
    &self,
    opts: ListClosedPrsOpts,
) -> Result<ListPullsResult, ForgeError>;
```

### Step 7: Implement GitHub list_closed_prs_targeting

**File:** `src/forge/github.rs`

```rust
async fn list_closed_prs_targeting(
    &self,
    opts: ListClosedPrsOpts,
) -> Result<ListPullsResult, ForgeError> {
    let limit = opts.effective_limit();
    let mut pulls = Vec::with_capacity(limit.min(100));
    let mut page = 1;
    let per_page = 100.min(limit);

    loop {
        let url = format!(
            "{}/repos/{}/{}/pulls?state=closed&base={}&per_page={}&page={}",
            self.api_base,
            self.owner,
            self.repo,
            urlencoding::encode(&opts.base),
            per_page,
            page
        );

        let response = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token_provider.bearer_token().await?))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "lattice")
            .send()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        if response.status() == 403 {
            return Err(ForgeError::RateLimited);
        }

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ForgeError::ApiError {
                status,
                message: text,
            });
        }

        let page_pulls: Vec<GitHubPr> = response
            .json()
            .await
            .map_err(|e| ForgeError::NetworkError(e.to_string()))?;

        let page_count = page_pulls.len();
        
        for pr in page_pulls {
            if pulls.len() >= limit {
                return Ok(ListPullsResult {
                    pulls,
                    truncated: true,
                });
            }
            pulls.push(PullRequestSummary {
                number: pr.number,
                head_ref: pr.head.ref_name,
                head_repo_owner: pr.head.repo.as_ref().map(|r| r.owner.login.clone()),
                base_ref: pr.base.ref_name,
                is_draft: pr.draft.unwrap_or(false),
                url: pr.html_url,
                updated_at: pr.updated_at,
            });
        }

        // Check if we've exhausted results
        if page_count < per_page {
            break;
        }

        // Check if we've hit the limit
        if pulls.len() >= limit {
            return Ok(ListPullsResult {
                pulls,
                truncated: true,
            });
        }

        page += 1;
    }

    Ok(ListPullsResult {
        pulls,
        truncated: false,
    })
}
```

### Step 8: Add MockForge Implementation

**File:** `src/forge/mock.rs`

```rust
impl MockForge {
    /// Add a closed PR for testing.
    pub fn add_closed_pr(&mut self, pr: PullRequestSummary) {
        self.closed_prs.push(pr);
    }
}

#[async_trait]
impl Forge for MockForge {
    // ... existing methods ...

    async fn list_closed_prs_targeting(
        &self,
        opts: ListClosedPrsOpts,
    ) -> Result<ListPullsResult, ForgeError> {
        let limit = opts.effective_limit();
        let pulls: Vec<_> = self.closed_prs
            .iter()
            .filter(|pr| pr.base_ref == opts.base)
            .take(limit)
            .cloned()
            .collect();
        
        let truncated = self.closed_prs
            .iter()
            .filter(|pr| pr.base_ref == opts.base)
            .count() > limit;

        Ok(ListPullsResult { pulls, truncated })
    }
}
```

### Step 9: Add Tier 1 Detection to Scanner

**File:** `src/engine/scan.rs`

Add a function to detect potential synthetic stack heads:

```rust
/// Detect potential synthetic stack heads from open PRs.
///
/// A potential synthetic stack head is an open PR that:
/// 1. Targets trunk (base_ref = trunk)
/// 2. Has a head branch that exists locally or could be fetched
///
/// This is Tier 1 detection - cheap, uses only open PR data.
pub fn detect_potential_synthetic_heads(
    snapshot: &RepoSnapshot,
    open_prs: &[PullRequestSummary],
) -> Vec<KnownIssue> {
    let trunk = match &snapshot.trunk {
        Some(t) => t.as_str(),
        None => return vec![], // No trunk configured
    };

    open_prs
        .iter()
        .filter(|pr| pr.base_ref == trunk)
        .map(|pr| KnownIssue::PotentialSyntheticStackHead {
            branch: pr.head_ref.clone(),
            pr_number: pr.number,
            pr_url: pr.url.clone(),
        })
        .collect()
}
```

Wire this into the remote scan flow:
```rust
// In scan_with_remote or wherever remote issues are detected:
pub async fn detect_remote_bootstrap_issues(
    snapshot: &RepoSnapshot,
    forge: &dyn Forge,
    _config: &ScanConfig,
) -> Vec<Issue> {
    let mut issues = Vec::new();

    // ... existing remote issue detection ...

    // Tier 1: Detect potential synthetic stack heads
    if let Ok(result) = forge.list_open_prs(ListPullsOpts::default()).await {
        let synthetic_heads = detect_potential_synthetic_heads(snapshot, &result.pulls);
        for known in synthetic_heads {
            issues.push(known.to_issue());
        }
    }

    issues
}
```

### Step 10: Add Tier 2 Deep Analysis (Fix Generator Phase)

**File:** `src/doctor/generators.rs`

Tier 2 analysis happens when generating fix options, not during baseline scan:

```rust
/// Generate synthetic stack analysis for a potential head.
///
/// This is Tier 2 analysis - queries closed PRs to confirm
/// the synthetic stack pattern.
///
/// Returns evidence to attach to the issue, not fix options.
/// (Synthetic stacks are informational, not directly repairable.)
pub async fn analyze_synthetic_stack_deep(
    issue: &Issue,
    forge: &dyn Forge,
    config: &DoctorBootstrapConfig,
) -> Option<Evidence> {
    // Extract branch from issue
    let branch = extract_synthetic_head_branch(issue)?;

    // Query closed PRs targeting this branch
    let opts = ListClosedPrsOpts::for_base(&branch)
        .with_limit(config.max_closed_prs_per_head);

    let result = forge.list_closed_prs_targeting(opts).await.ok()?;

    if result.pulls.is_empty() {
        return None; // No closed PRs - not actually a synthetic stack
    }

    let closed_prs: Vec<ClosedPrInfo> = result.pulls
        .iter()
        .map(|pr| ClosedPrInfo {
            number: pr.number,
            head_ref: pr.head_ref.clone(),
            merged: true, // GitHub closed+merged PRs are in this list
            url: pr.url.clone(),
        })
        .collect();

    Some(Evidence::SyntheticStackChildren {
        head_branch: branch,
        closed_prs,
        truncated: result.truncated,
    })
}

fn extract_synthetic_head_branch(issue: &Issue) -> Option<String> {
    // Parse from issue ID: "synthetic-stack-head:<number>"
    // Or from evidence/message
    if issue.id.as_str().starts_with("synthetic-stack-head:") {
        // Extract from message which contains the branch name
        // "PR #N targeting trunk may be a synthetic stack head (branch 'X')"
        let msg = &issue.message;
        if let Some(start) = msg.find("(branch '") {
            if let Some(end) = msg[start + 9..].find("')") {
                return Some(msg[start + 9..start + 9 + end].to_string());
            }
        }
    }
    None
}
```

### Step 11: Wire Deep Analysis into Doctor Command

**File:** `src/cli/commands/doctor_cmd.rs` (or equivalent)

```rust
pub async fn run_doctor(args: DoctorArgs, git: &Git, forge: Option<&dyn Forge>) -> Result<()> {
    let config = load_config()?;
    let deep_remote = args.deep_remote || config.doctor.bootstrap.deep_remote;

    // Scan and get baseline diagnosis
    let snapshot = scan(git)?;
    let mut diagnosis = Doctor::new().diagnose(&snapshot);

    // Tier 2: Deep synthetic stack analysis
    if deep_remote {
        if let Some(forge) = forge {
            let bootstrap_config = &config.doctor.bootstrap;
            let mut analyzed = 0;

            for issue in &mut diagnosis.issues {
                if !issue.id.as_str().starts_with("synthetic-stack-head:") {
                    continue;
                }

                if analyzed >= bootstrap_config.max_synthetic_heads {
                    // Budget exceeded - add note
                    println!(
                        "Note: Skipping synthetic stack analysis for remaining heads \
                         (budget: {} heads)",
                        bootstrap_config.max_synthetic_heads
                    );
                    break;
                }

                if let Some(evidence) = analyze_synthetic_stack_deep(
                    issue,
                    forge,
                    bootstrap_config,
                ).await {
                    issue.evidence.push(evidence);
                    analyzed += 1;
                }
            }
        }
    }

    // Display results
    display_diagnosis(&diagnosis, args.verbose)?;

    Ok(())
}
```

### Step 12: Unit Tests

**File:** `src/doctor/issues.rs`

```rust
#[cfg(test)]
mod synthetic_stack_tests {
    use super::*;

    #[test]
    fn potential_synthetic_stack_head_issue_id() {
        let issue = KnownIssue::PotentialSyntheticStackHead {
            branch: "feature".to_string(),
            pr_number: 42,
            pr_url: "https://github.com/org/repo/pull/42".to_string(),
        };
        assert!(issue.issue_id().as_str().starts_with("synthetic-stack-head:"));
        assert_eq!(issue.severity(), Severity::Info);
    }

    #[test]
    fn potential_synthetic_stack_head_to_issue() {
        let known = KnownIssue::PotentialSyntheticStackHead {
            branch: "feature".to_string(),
            pr_number: 42,
            pr_url: "https://github.com/org/repo/pull/42".to_string(),
        };
        let issue = known.to_issue();
        assert!(!issue.is_blocking());
        assert!(issue.message.contains("feature"));
        assert!(issue.message.contains("42"));
    }
}
```

**File:** `src/engine/scan.rs`

```rust
#[cfg(test)]
mod synthetic_detection_tests {
    use super::*;

    fn make_pr_summary(number: u64, head: &str, base: &str) -> PullRequestSummary {
        PullRequestSummary {
            number,
            head_ref: head.to_string(),
            head_repo_owner: None,
            base_ref: base.to_string(),
            is_draft: false,
            url: format!("https://github.com/org/repo/pull/{}", number),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn detects_trunk_targeting_pr_as_potential_head() {
        let mut snapshot = minimal_snapshot();
        snapshot.trunk = Some(BranchName::new("main").unwrap());

        let open_prs = vec![
            make_pr_summary(42, "feature", "main"),
        ];

        let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

        assert_eq!(issues.len(), 1);
        if let KnownIssue::PotentialSyntheticStackHead { branch, pr_number, .. } = &issues[0] {
            assert_eq!(branch, "feature");
            assert_eq!(*pr_number, 42);
        } else {
            panic!("Expected PotentialSyntheticStackHead");
        }
    }

    #[test]
    fn ignores_non_trunk_targeting_prs() {
        let mut snapshot = minimal_snapshot();
        snapshot.trunk = Some(BranchName::new("main").unwrap());

        let open_prs = vec![
            make_pr_summary(42, "sub-feature", "feature"), // targets feature, not main
        ];

        let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

        assert!(issues.is_empty());
    }

    #[test]
    fn returns_empty_when_no_trunk() {
        let snapshot = minimal_snapshot(); // No trunk

        let open_prs = vec![
            make_pr_summary(42, "feature", "main"),
        ];

        let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

        assert!(issues.is_empty());
    }

    #[test]
    fn detects_multiple_potential_heads() {
        let mut snapshot = minimal_snapshot();
        snapshot.trunk = Some(BranchName::new("main").unwrap());

        let open_prs = vec![
            make_pr_summary(42, "feature-a", "main"),
            make_pr_summary(43, "feature-b", "main"),
            make_pr_summary(44, "sub-feature", "feature-a"), // Not trunk-targeting
        ];

        let issues = detect_potential_synthetic_heads(&snapshot, &open_prs);

        assert_eq!(issues.len(), 2);
    }
}
```

**File:** `src/forge/mock.rs`

```rust
#[cfg(test)]
mod closed_pr_tests {
    use super::*;

    #[tokio::test]
    async fn list_closed_prs_filters_by_base() {
        let mut mock = MockForge::new();
        mock.add_closed_pr(PullRequestSummary {
            number: 1,
            head_ref: "sub-a".into(),
            head_repo_owner: None,
            base_ref: "feature".into(),
            is_draft: false,
            url: "u".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
        });
        mock.add_closed_pr(PullRequestSummary {
            number: 2,
            head_ref: "sub-b".into(),
            head_repo_owner: None,
            base_ref: "feature".into(),
            is_draft: false,
            url: "u".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
        });
        mock.add_closed_pr(PullRequestSummary {
            number: 3,
            head_ref: "other".into(),
            head_repo_owner: None,
            base_ref: "main".into(), // Different base
            is_draft: false,
            url: "u".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
        });

        let result = mock
            .list_closed_prs_targeting(ListClosedPrsOpts::for_base("feature"))
            .await
            .unwrap();

        assert_eq!(result.pulls.len(), 2);
        assert!(result.pulls.iter().all(|pr| pr.base_ref == "feature"));
    }

    #[tokio::test]
    async fn list_closed_prs_respects_limit() {
        let mut mock = MockForge::new();
        for i in 1..=10 {
            mock.add_closed_pr(PullRequestSummary {
                number: i,
                head_ref: format!("sub-{}", i),
                head_repo_owner: None,
                base_ref: "feature".into(),
                is_draft: false,
                url: "u".into(),
                updated_at: "2024-01-01T00:00:00Z".into(),
            });
        }

        let result = mock
            .list_closed_prs_targeting(ListClosedPrsOpts::for_base("feature").with_limit(3))
            .await
            .unwrap();

        assert_eq!(result.pulls.len(), 3);
        assert!(result.truncated);
    }
}
```

### Step 13: Integration Tests

**File:** `tests/integration/synthetic_stack_detection.rs` (new)

```rust
//! Integration tests for synthetic stack detection

use latticework::doctor::Doctor;
use latticework::engine::scan::scan;
use latticework::forge::{MockForge, PullRequestSummary, ListPullsOpts};

mod test_helpers;
use test_helpers::TestRepo;

#[tokio::test]
async fn test_tier1_detects_potential_synthetic_head() {
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Create a local branch matching the PR head
    repo.create_branch("feature");
    repo.commit("feature work");

    // Mock forge with open PR targeting trunk
    let mut mock = MockForge::new();
    mock.add_open_pr(PullRequestSummary {
        number: 42,
        head_ref: "feature".to_string(),
        head_repo_owner: None,
        base_ref: "main".to_string(),
        is_draft: false,
        url: "https://github.com/org/repo/pull/42".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
    });

    // Scan
    let git = Git::open(repo.path()).unwrap();
    let snapshot = scan(&git).unwrap();

    // Diagnose with remote
    let open_prs = mock.list_open_prs(ListPullsOpts::default()).await.unwrap();
    let synthetic_issues = detect_potential_synthetic_heads(&snapshot, &open_prs.pulls);

    assert_eq!(synthetic_issues.len(), 1);
    assert!(synthetic_issues[0].to_string().contains("feature"));
}

#[tokio::test]
async fn test_tier2_finds_closed_children() {
    let repo = TestRepo::new();
    repo.init_with_trunk("main");

    // Mock forge with closed PRs targeting the feature branch
    let mut mock = MockForge::new();
    mock.add_closed_pr(PullRequestSummary {
        number: 10,
        head_ref: "sub-feature-a".to_string(),
        head_repo_owner: None,
        base_ref: "feature".to_string(),
        is_draft: false,
        url: "https://github.com/org/repo/pull/10".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
    });
    mock.add_closed_pr(PullRequestSummary {
        number: 11,
        head_ref: "sub-feature-b".to_string(),
        head_repo_owner: None,
        base_ref: "feature".to_string(),
        is_draft: false,
        url: "https://github.com/org/repo/pull/11".to_string(),
        updated_at: "2024-01-02T00:00:00Z".to_string(),
    });

    // Create a potential head issue
    let issue = KnownIssue::PotentialSyntheticStackHead {
        branch: "feature".to_string(),
        pr_number: 42,
        pr_url: "https://github.com/org/repo/pull/42".to_string(),
    }.to_issue();

    let config = DoctorBootstrapConfig::default();
    let evidence = analyze_synthetic_stack_deep(&issue, &mock, &config).await;

    assert!(evidence.is_some());
    if let Some(Evidence::SyntheticStackChildren { head_branch, closed_prs, truncated }) = evidence {
        assert_eq!(head_branch, "feature");
        assert_eq!(closed_prs.len(), 2);
        assert!(!truncated);
    } else {
        panic!("Expected SyntheticStackChildren evidence");
    }
}

#[tokio::test]
async fn test_tier2_respects_budget() {
    let mut mock = MockForge::new();
    
    // Add many closed PRs
    for i in 1..=50 {
        mock.add_closed_pr(PullRequestSummary {
            number: i,
            head_ref: format!("sub-{}", i),
            head_repo_owner: None,
            base_ref: "feature".to_string(),
            is_draft: false,
            url: format!("https://github.com/org/repo/pull/{}", i),
            updated_at: "2024-01-01T00:00:00Z".to_string(),
        });
    }

    let issue = KnownIssue::PotentialSyntheticStackHead {
        branch: "feature".to_string(),
        pr_number: 42,
        pr_url: "u".to_string(),
    }.to_issue();

    let config = DoctorBootstrapConfig {
        max_closed_prs_per_head: 10,
        ..Default::default()
    };

    let evidence = analyze_synthetic_stack_deep(&issue, &mock, &config).await;

    if let Some(Evidence::SyntheticStackChildren { closed_prs, truncated, .. }) = evidence {
        assert_eq!(closed_prs.len(), 10);
        assert!(truncated);
    } else {
        panic!("Expected evidence");
    }
}

#[test]
fn test_config_defaults() {
    let config = DoctorBootstrapConfig::default();
    assert!(!config.deep_remote);
    assert_eq!(config.max_synthetic_heads, 3);
    assert_eq!(config.max_closed_prs_per_head, 20);
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/doctor/issues.rs` | MODIFY | Add `PotentialSyntheticStackHead` variant |
| `src/engine/health.rs` | MODIFY | Add `ClosedPrInfo`, `Evidence::SyntheticStackChildren`, `Evidence::PrReference`, issue constructor |
| `src/core/config/schema.rs` | MODIFY | Add `DoctorBootstrapConfig` |
| `src/cli/args.rs` | MODIFY | Add `--deep-remote` flag to doctor |
| `src/forge/traits.rs` | MODIFY | Add `ListClosedPrsOpts`, `list_closed_prs_targeting` method |
| `src/forge/github.rs` | MODIFY | Implement `list_closed_prs_targeting` |
| `src/forge/mock.rs` | MODIFY | Add `add_closed_pr`, implement `list_closed_prs_targeting` |
| `src/engine/scan.rs` | MODIFY | Add `detect_potential_synthetic_heads` |
| `src/doctor/generators.rs` | MODIFY | Add `analyze_synthetic_stack_deep` |
| `src/cli/commands/doctor_cmd.rs` | MODIFY | Wire deep analysis with budget enforcement |
| `tests/integration/synthetic_stack_detection.rs` | NEW | Integration tests |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.8:

- [ ] Tier 1: Trunk-bound open PRs flagged as potential synthetic heads
- [ ] Tier 2: `--deep-remote` enables closed PR enumeration
- [ ] Budgets enforced with explicit truncation reporting
- [ ] Config options `doctor.bootstrap.*` respected
- [ ] API failures do not block other diagnosis
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `potential_synthetic_stack_head_issue_id` | issues.rs | Issue ID generation |
| `potential_synthetic_stack_head_to_issue` | issues.rs | Issue conversion |
| `detects_trunk_targeting_pr_as_potential_head` | scan.rs | Tier 1 detection |
| `ignores_non_trunk_targeting_prs` | scan.rs | Filtering |
| `returns_empty_when_no_trunk` | scan.rs | Edge case |
| `detects_multiple_potential_heads` | scan.rs | Multiple detection |
| `list_closed_prs_filters_by_base` | mock.rs | Forge method |
| `list_closed_prs_respects_limit` | mock.rs | Budget enforcement |

### Integration Tests

| Test | Description |
|------|-------------|
| `test_tier1_detects_potential_synthetic_head` | Full Tier 1 workflow |
| `test_tier2_finds_closed_children` | Full Tier 2 workflow |
| `test_tier2_respects_budget` | Budget enforcement |
| `test_config_defaults` | Config schema |

---

## Dependencies

- **Milestone 5.2:** `list_open_prs` capability (Complete)
- **Milestone 5.3:** Bootstrap issue detection (Complete) - Issue framework
- **Existing:** MockForge infrastructure

---

## Estimated Scope

- **Lines of code changed:** ~40 in `issues.rs`, ~60 in `health.rs`, ~40 in `schema.rs`, ~15 in `args.rs`, ~50 in `traits.rs`, ~80 in `github.rs`, ~40 in `mock.rs`, ~50 in `scan.rs`, ~60 in `generators.rs`, ~50 in `doctor_cmd.rs`
- **New functions:** 4 (`detect_potential_synthetic_heads`, `analyze_synthetic_stack_deep`, `extract_synthetic_head_branch`, `list_closed_prs_targeting`)
- **New types:** 4 (`PotentialSyntheticStackHead`, `ClosedPrInfo`, `DoctorBootstrapConfig`, `ListClosedPrsOpts`)
- **Risk:** Low - Informational feature, does not affect core operations

---

## Verification Commands

```bash
cargo check
cargo clippy -- -D warnings
cargo test
cargo test synthetic
cargo test closed_pr
cargo test doctor_bootstrap
cargo fmt --check
```

---

## Notes

- **Follow the leader:** Uses existing issue/evidence framework from Milestones 5.3-5.4
- **Simplicity:** Two-tiered approach avoids unnecessary API calls
- **Reuse:** Extends existing Forge trait, MockForge
- **Purity:** Detection is pure function of snapshot + PR data; deep analysis is async but side-effect-free
- **No stubs:** All features fully implemented

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.8 as complete
2. Create `implementation_notes.md` in `.agents/v2/milestones/milestone-5-8-synthetic-stack-detection/`
3. Milestone 5.9 (Snapshot Materialization) can proceed
