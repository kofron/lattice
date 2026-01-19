# Milestone 5.3: Bootstrap Issue Detection

## Goal

Detect bootstrap-related conditions and surface them as Doctor issues to enable importing existing PRs and branches into Lattice tracking.

**Core principle:** When users run `lattice doctor` on a repository with existing open PRs on the remote, Lattice should detect and surface this information so users can bootstrap their workflow by importing these PRs into Lattice tracking.

---

## Background

The Doctor framework currently detects local issues:
- Missing trunk configuration
- Metadata parse errors
- Graph cycles
- Missing parent branches
- In-progress operations

For bootstrap scenarios, we need to:
1. Query the forge for open PRs (using `list_open_prs` from Milestone 5.2)
2. Match remote PRs against local branches
3. Generate issues for various bootstrap conditions
4. Enable fix generators (Milestone 5.4) to offer remediation

**Current state:**
- `src/doctor/issues.rs` - `KnownIssue` enum with existing issue types
- `src/engine/scan.rs` - `scan()` function produces `RepoSnapshot`
- `src/engine/health.rs` - Issue and capability tracking
- `src/forge/traits.rs` - `list_open_prs` available from Milestone 5.2

---

## Spec References

- **ROADMAP.md Milestone 5.3** - Bootstrap issue detection deliverables
- **SPEC.md Section 8E.1** - Forge abstraction trait
- **ARCHITECTURE.md Section 12** - Command lifecycle (Scan phase)
- **ARCHITECTURE.md Section 11.2** - Cached metadata handling

---

## Design Decisions

### Issue Family: Bootstrap Evidence

These issues are informational or warnings - they surface potential import opportunities rather than blocking problems.

| Issue | Severity | Meaning |
|-------|----------|---------|
| `RemoteOpenPullRequestsDetected` | Info | Forge reports ≥1 open PR for this repo |
| `RemoteOpenPrBranchMissingLocally` | Warning | Open PR exists but head_ref branch doesn't exist locally |
| `RemoteOpenPrBranchUntracked` | Warning | Local branch exists matching PR head but has no Lattice metadata |
| `RemoteOpenPrNotLinkedInMetadata` | Info | Tracked branch exists but no PR linkage in cached metadata |

### Why Info vs Warning?

- **Info:** Informational - user awareness, no action required
- **Warning:** Suggests an action (import/link) that would improve workflow
- **Blocking:** Reserved for structural problems that prevent operations

None of the bootstrap issues are blocking because they don't prevent local operations.

### Evidence Storage

The scanner needs to store remote PR data for issue matching. Two approaches:

**Option A: Store in RepoSnapshot (Selected)**
- Add `remote_prs: Option<Vec<PullRequestSummary>>` to `RepoSnapshot`
- Populated during scan when forge is available
- Available for issue generation and fix generators

**Option B: Separate RemoteState struct**
- More separation but adds complexity
- Not needed since RepoSnapshot is already the "kitchen sink"

We choose Option A for simplicity.

### Gating Remote Queries

Remote PR queries require:
1. `TrunkKnown` - Need context for meaningful matching
2. `RemoteResolved` - GitHub remote configured
3. `AuthAvailable` - Token present
4. `RepoAuthorized` - GitHub App installed (or user token authorized)

If any capability is missing, skip remote scanning gracefully (no error, just no remote issues).

### API Failure Handling

Per ROADMAP.md: "Scanner gracefully handles API failures (log warning, continue)"

When `list_open_prs` fails:
1. Log a tracing warning
2. Set `remote_prs = None` in snapshot
3. Continue scan without remote issues
4. Do NOT add a blocking issue for API failures

### Matching Logic

For each open PR from the forge:

```
if local branch with name == pr.head_ref does NOT exist:
    → RemoteOpenPrBranchMissingLocally
else if local branch exists but is NOT tracked:
    → RemoteOpenPrBranchUntracked
else if tracked but metadata.cached_pr_number is None:
    → RemoteOpenPrNotLinkedInMetadata
else:
    → Already linked, no issue
```

---

## Implementation Steps

### Step 1: Add new `KnownIssue` variants

**File:** `src/doctor/issues.rs`

Add four new variants to the `KnownIssue` enum:

```rust
/// Remote forge reports open pull requests for this repository.
/// This is informational - indicates bootstrap opportunity.
#[error("remote has {count} open pull request(s)")]
RemoteOpenPullRequestsDetected {
    /// Number of open PRs detected.
    count: usize,
    /// Whether the result was truncated (more PRs exist).
    truncated: bool,
},

/// An open PR exists on the remote but the head branch doesn't exist locally.
/// User should fetch the branch to import the PR.
#[error("open PR #{number} has head branch '{head_ref}' which doesn't exist locally")]
RemoteOpenPrBranchMissingLocally {
    /// PR number
    number: u64,
    /// Head branch name from the PR
    head_ref: String,
    /// Base branch name (often trunk)
    base_ref: String,
    /// PR URL for reference
    url: String,
},

/// A local branch exists that matches an open PR's head, but it's not tracked.
/// User should track the branch to link it with the PR.
#[error("branch '{branch}' matches open PR #{number} but is not tracked")]
RemoteOpenPrBranchUntracked {
    /// Local branch name
    branch: String,
    /// PR number
    number: u64,
    /// PR URL
    url: String,
},

/// A tracked branch exists that matches an open PR, but the PR isn't linked in metadata.
/// User should link the PR to the tracked branch.
#[error("tracked branch '{branch}' matches open PR #{number} but PR is not linked in metadata")]
RemoteOpenPrNotLinkedInMetadata {
    /// Branch name
    branch: String,
    /// PR number
    number: u64,
    /// PR URL
    url: String,
},
```

### Step 2: Implement `issue_id()` for new variants

**File:** `src/doctor/issues.rs`

Add to the `issue_id()` method:

```rust
KnownIssue::RemoteOpenPullRequestsDetected { .. } => {
    IssueId::singleton("remote-open-prs-detected")
}
KnownIssue::RemoteOpenPrBranchMissingLocally { number, .. } => {
    IssueId::new("remote-pr-branch-missing", &number.to_string())
}
KnownIssue::RemoteOpenPrBranchUntracked { branch, .. } => {
    IssueId::new("remote-pr-branch-untracked", branch)
}
KnownIssue::RemoteOpenPrNotLinkedInMetadata { branch, .. } => {
    IssueId::new("remote-pr-not-linked", branch)
}
```

### Step 3: Implement `severity()` for new variants

**File:** `src/doctor/issues.rs`

Add to the `severity()` method:

```rust
KnownIssue::RemoteOpenPullRequestsDetected { .. } => Severity::Info,
KnownIssue::RemoteOpenPrBranchMissingLocally { .. } => Severity::Warning,
KnownIssue::RemoteOpenPrBranchUntracked { .. } => Severity::Warning,
KnownIssue::RemoteOpenPrNotLinkedInMetadata { .. } => Severity::Info,
```

### Step 4: Add issue constructors to `engine::health::issues`

**File:** `src/engine/health.rs`

Add new constructor functions in the `issues` module:

```rust
/// Create an issue for detecting open PRs on the remote.
pub fn remote_open_prs_detected(count: usize, truncated: bool) -> Issue {
    let truncation_note = if truncated {
        " (results truncated, more may exist)"
    } else {
        ""
    };
    
    Issue::new(
        "remote-open-prs-detected",
        Severity::Info,
        format!(
            "Remote has {} open pull request(s){}. Run `lattice doctor --fix` to import.",
            count, truncation_note
        ),
    )
}

/// Create an issue for an open PR whose head branch doesn't exist locally.
pub fn remote_pr_branch_missing(number: u64, head_ref: &str, base_ref: &str, url: &str) -> Issue {
    Issue::new(
        "remote-pr-branch-missing",
        Severity::Warning,
        format!(
            "Open PR #{} targets '{}' from '{}' but branch '{}' doesn't exist locally",
            number, base_ref, head_ref, head_ref
        ),
    )
    .with_evidence(Evidence::Ref {
        name: format!("refs/heads/{}", head_ref),
        oid: None,
    })
    .with_evidence(Evidence::Config {
        key: format!("pr.{}", number),
        problem: format!("branch missing, PR URL: {}", url),
    })
}

/// Create an issue for a local branch matching an open PR but not tracked.
pub fn remote_pr_branch_untracked(branch: &str, number: u64, url: &str) -> Issue {
    Issue::new(
        "remote-pr-branch-untracked",
        Severity::Warning,
        format!(
            "Branch '{}' matches open PR #{} but is not tracked by Lattice",
            branch, number
        ),
    )
    .with_evidence(Evidence::Ref {
        name: format!("refs/heads/{}", branch),
        oid: None,
    })
    .with_evidence(Evidence::Config {
        key: format!("pr.{}", number),
        problem: format!("untracked, PR URL: {}", url),
    })
}

/// Create an issue for a tracked branch with an open PR but no linkage.
pub fn remote_pr_not_linked(branch: &str, number: u64, url: &str) -> Issue {
    Issue::new(
        "remote-pr-not-linked",
        Severity::Info,
        format!(
            "Tracked branch '{}' has open PR #{} but PR is not linked in metadata",
            branch, number
        ),
    )
    .with_evidence(Evidence::Ref {
        name: format!("refs/branch-metadata/{}", branch),
        oid: None,
    })
    .with_evidence(Evidence::Config {
        key: format!("pr.{}", number),
        problem: format!("not linked, PR URL: {}", url),
    })
}
```

### Step 5: Implement `to_issue()` for new variants

**File:** `src/doctor/issues.rs`

Add to the `to_issue()` method:

```rust
KnownIssue::RemoteOpenPullRequestsDetected { count, truncated } => {
    issues::remote_open_prs_detected(*count, *truncated)
}
KnownIssue::RemoteOpenPrBranchMissingLocally {
    number,
    head_ref,
    base_ref,
    url,
} => issues::remote_pr_branch_missing(*number, head_ref, base_ref, url),
KnownIssue::RemoteOpenPrBranchUntracked { branch, number, url } => {
    issues::remote_pr_branch_untracked(branch, *number, url)
}
KnownIssue::RemoteOpenPrNotLinkedInMetadata { branch, number, url } => {
    issues::remote_pr_not_linked(branch, *number, url)
}
```

### Step 6: Add `remote_prs` field to `RepoSnapshot`

**File:** `src/engine/scan.rs`

Add the field to `RepoSnapshot`:

```rust
/// Complete snapshot of repository state.
#[derive(Debug)]
pub struct RepoSnapshot {
    // ... existing fields ...

    /// Remote open pull requests (if forge query succeeded).
    ///
    /// Populated when all of these capabilities are present:
    /// - TrunkKnown
    /// - RemoteResolved
    /// - AuthAvailable
    /// - RepoAuthorized
    ///
    /// None if capabilities are missing or API call failed.
    pub remote_prs: Option<RemotePrEvidence>,
}
```

Add a new struct for the evidence:

```rust
/// Evidence of remote pull requests collected during scan.
#[derive(Debug, Clone)]
pub struct RemotePrEvidence {
    /// The open PRs retrieved from the forge.
    pub prs: Vec<crate::forge::PullRequestSummary>,
    /// Whether the result was truncated (more PRs exist).
    pub truncated: bool,
}
```

### Step 7: Create async forge query helper

**File:** `src/engine/scan.rs`

Add a helper function for querying the forge:

```rust
use crate::forge::{Forge, ListPullsOpts, PullRequestSummary};

/// Query the forge for open PRs if capabilities allow.
///
/// Returns None if:
/// - Required capabilities are missing
/// - Forge query fails (logged as warning)
async fn query_remote_prs(
    health: &RepoHealthReport,
    owner: &str,
    repo: &str,
) -> Option<RemotePrEvidence> {
    use crate::engine::capabilities::Capability;

    // Check required capabilities
    let caps = health.capabilities();
    if !caps.has(&Capability::TrunkKnown)
        || !caps.has(&Capability::RemoteResolved)
        || !caps.has(&Capability::AuthAvailable)
        || !caps.has(&Capability::RepoAuthorized)
    {
        tracing::debug!(
            "Skipping remote PR query: missing required capabilities \
             (TrunkKnown={}, RemoteResolved={}, AuthAvailable={}, RepoAuthorized={})",
            caps.has(&Capability::TrunkKnown),
            caps.has(&Capability::RemoteResolved),
            caps.has(&Capability::AuthAvailable),
            caps.has(&Capability::RepoAuthorized),
        );
        return None;
    }

    // Create forge and query
    match create_forge_and_query(owner, repo).await {
        Ok(result) => {
            tracing::debug!(
                "Retrieved {} open PRs from remote (truncated={})",
                result.prs.len(),
                result.truncated
            );
            Some(result)
        }
        Err(e) => {
            tracing::warn!("Failed to query remote PRs: {}", e);
            None
        }
    }
}

/// Create a forge and query for open PRs.
async fn create_forge_and_query(owner: &str, repo: &str) -> Result<RemotePrEvidence, crate::forge::ForgeError> {
    use crate::forge::github::GitHubForge;
    use crate::auth::GitHubAuthManager;
    use crate::secrets;

    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .map_err(|e| crate::forge::ForgeError::AuthFailed(e.to_string()))?;
    let auth_manager = GitHubAuthManager::new("github.com", store);
    
    let forge = GitHubForge::new(owner.to_string(), repo.to_string(), auth_manager);
    let opts = ListPullsOpts::default(); // 200 limit
    
    let result = forge.list_open_prs(opts).await?;
    
    Ok(RemotePrEvidence {
        prs: result.pulls,
        truncated: result.truncated,
    })
}
```

### Step 8: Integrate forge query into scan

**File:** `src/engine/scan.rs`

The scan function is currently synchronous. We have two options:

**Option A: Make scan async (invasive change)**
- Changes the entire scan API
- Affects all callers

**Option B: Add a separate scan_with_remote function (Selected)**
- Keep existing `scan()` for backward compatibility
- Add `scan_with_remote()` that does the async work
- Less disruptive

Add a new function:

```rust
/// Scan a repository with optional remote PR query.
///
/// This is the async version that can query the forge for open PRs.
/// Falls back to local-only scan if forge is unavailable.
pub async fn scan_with_remote(git: &Git) -> Result<RepoSnapshot, ScanError> {
    // Perform the basic scan first
    let mut snapshot = scan(git)?;

    // Try to query remote PRs if capabilities allow
    if let Ok(Some(remote_url)) = git.remote_url("origin") {
        if let Some((owner, repo)) = crate::forge::github::parse_github_url(&remote_url) {
            snapshot.remote_prs = query_remote_prs(&snapshot.health, &owner, &repo).await;
        }
    }

    // Generate bootstrap issues based on remote evidence
    if let Some(ref evidence) = snapshot.remote_prs {
        generate_bootstrap_issues(&mut snapshot, evidence);
    }

    Ok(snapshot)
}

/// Generate bootstrap issues from remote PR evidence.
fn generate_bootstrap_issues(snapshot: &mut RepoSnapshot, evidence: &RemotePrEvidence) {
    // Issue: Remote has open PRs
    if !evidence.prs.is_empty() {
        snapshot.health.add_issue(issues::remote_open_prs_detected(
            evidence.prs.len(),
            evidence.truncated,
        ));
    }

    // Match each PR against local state
    for pr in &evidence.prs {
        // Skip fork PRs for now (complex ownership semantics)
        if pr.is_fork() {
            tracing::debug!("Skipping fork PR #{} from {}", pr.number, pr.head_repo_owner.as_deref().unwrap_or("unknown"));
            continue;
        }

        if let Ok(branch_name) = crate::core::types::BranchName::new(&pr.head_ref) {
            if !snapshot.branches.contains_key(&branch_name) {
                // Branch doesn't exist locally
                snapshot.health.add_issue(issues::remote_pr_branch_missing(
                    pr.number,
                    &pr.head_ref,
                    &pr.base_ref,
                    &pr.url,
                ));
            } else if !snapshot.metadata.contains_key(&branch_name) {
                // Branch exists but not tracked
                snapshot.health.add_issue(issues::remote_pr_branch_untracked(
                    &pr.head_ref,
                    pr.number,
                    &pr.url,
                ));
            } else {
                // Branch is tracked - check if PR is linked
                let scanned = snapshot.metadata.get(&branch_name).unwrap();
                if scanned.metadata.cached_pr_number().is_none() {
                    snapshot.health.add_issue(issues::remote_pr_not_linked(
                        &pr.head_ref,
                        pr.number,
                        &pr.url,
                    ));
                }
                // else: PR is already linked, no issue needed
            }
        }
    }
}
```

### Step 9: Update RepoSnapshot initialization

**File:** `src/engine/scan.rs`

Update the `scan()` function to initialize `remote_prs`:

```rust
Ok(RepoSnapshot {
    info,
    git_state,
    worktree_status,
    current_branch,
    branches,
    metadata,
    repo_config,
    trunk,
    graph,
    fingerprint,
    health,
    remote_prs: None, // Set by scan_with_remote()
})
```

### Step 10: Check metadata for `cached_pr_number` method

**File:** `src/core/metadata/schema.rs`

Verify `BranchMetadataV1` has a method to check PR linkage. If not, add:

```rust
impl BranchMetadataV1 {
    /// Get the cached PR number if linked.
    pub fn cached_pr_number(&self) -> Option<u64> {
        self.cached.as_ref()?.pr_number
    }
}
```

### Step 11: Add unit tests for new KnownIssue variants

**File:** `src/doctor/issues.rs`

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    #[test]
    fn remote_open_prs_detected_issue_id() {
        let issue = KnownIssue::RemoteOpenPullRequestsDetected {
            count: 5,
            truncated: false,
        };
        assert_eq!(issue.issue_id().as_str(), "remote-open-prs-detected");
        assert_eq!(issue.severity(), Severity::Info);
    }

    #[test]
    fn remote_pr_branch_missing_issue_id() {
        let issue = KnownIssue::RemoteOpenPrBranchMissingLocally {
            number: 42,
            head_ref: "feature".to_string(),
            base_ref: "main".to_string(),
            url: "https://example.com".to_string(),
        };
        assert!(issue.issue_id().as_str().starts_with("remote-pr-branch-missing:"));
        assert_eq!(issue.severity(), Severity::Warning);
    }

    #[test]
    fn remote_pr_branch_untracked_issue_id() {
        let issue = KnownIssue::RemoteOpenPrBranchUntracked {
            branch: "feature".to_string(),
            number: 42,
            url: "https://example.com".to_string(),
        };
        assert!(issue.issue_id().as_str().starts_with("remote-pr-branch-untracked:"));
        assert_eq!(issue.severity(), Severity::Warning);
    }

    #[test]
    fn remote_pr_not_linked_issue_id() {
        let issue = KnownIssue::RemoteOpenPrNotLinkedInMetadata {
            branch: "feature".to_string(),
            number: 42,
            url: "https://example.com".to_string(),
        };
        assert!(issue.issue_id().as_str().starts_with("remote-pr-not-linked:"));
        assert_eq!(issue.severity(), Severity::Info);
    }

    #[test]
    fn remote_issues_to_issue() {
        let known = KnownIssue::RemoteOpenPullRequestsDetected {
            count: 3,
            truncated: true,
        };
        let issue = known.to_issue();
        assert!(!issue.is_blocking());
        assert!(issue.message.contains("3 open pull request"));
        assert!(issue.message.contains("truncated"));
    }
}
```

### Step 12: Add unit tests for bootstrap issue generation

**File:** `src/engine/scan.rs`

```rust
#[cfg(test)]
mod tests {
    // ... existing tests ...

    mod bootstrap_issues {
        use super::*;
        use crate::forge::PullRequestSummary;

        fn make_pr_summary(number: u64, head_ref: &str, base_ref: &str) -> PullRequestSummary {
            PullRequestSummary {
                number,
                head_ref: head_ref.to_string(),
                head_repo_owner: None,
                base_ref: base_ref.to_string(),
                is_draft: false,
                url: format!("https://github.com/owner/repo/pull/{}", number),
                updated_at: "2024-01-01T00:00:00Z".to_string(),
            }
        }

        #[test]
        fn generates_open_prs_detected_issue() {
            let mut snapshot = make_snapshot();
            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(1, "feature", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot.health.issues().iter()
                .filter(|i| i.id.as_str() == "remote-open-prs-detected")
                .collect();
            assert_eq!(issues.len(), 1);
        }

        #[test]
        fn generates_branch_missing_issue() {
            let mut snapshot = make_snapshot();
            // PR for a branch that doesn't exist locally
            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "nonexistent-branch", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot.health.issues().iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-branch-missing"))
                .collect();
            assert_eq!(issues.len(), 1);
        }

        #[test]
        fn generates_untracked_issue() {
            let mut snapshot = make_snapshot();
            // Add a local branch that's not tracked
            let branch = BranchName::new("untracked-feature").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            snapshot.branches.insert(branch.clone(), oid);
            // No metadata for this branch

            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "untracked-feature", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot.health.issues().iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-branch-untracked"))
                .collect();
            assert_eq!(issues.len(), 1);
        }

        #[test]
        fn generates_not_linked_issue() {
            let mut snapshot = make_snapshot();
            // Add a tracked branch without PR linkage
            let branch = BranchName::new("tracked-no-pr").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            snapshot.branches.insert(branch.clone(), oid.clone());
            
            let metadata = BranchMetadataV1::new(
                branch.clone(),
                BranchName::new("main").unwrap(),
                oid.clone(),
            );
            snapshot.metadata.insert(branch, ScannedMetadata {
                ref_oid: oid,
                metadata,
            });

            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "tracked-no-pr", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            let issues: Vec<_> = snapshot.health.issues().iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-not-linked"))
                .collect();
            assert_eq!(issues.len(), 1);
        }

        #[test]
        fn no_issue_when_pr_already_linked() {
            let mut snapshot = make_snapshot();
            // Add a tracked branch WITH PR linkage
            let branch = BranchName::new("tracked-with-pr").unwrap();
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            snapshot.branches.insert(branch.clone(), oid.clone());
            
            let mut metadata = BranchMetadataV1::new(
                branch.clone(),
                BranchName::new("main").unwrap(),
                oid.clone(),
            );
            metadata.set_cached_pr(42, "open", "https://github.com/owner/repo/pull/42");
            
            snapshot.metadata.insert(branch, ScannedMetadata {
                ref_oid: oid,
                metadata,
            });

            let evidence = RemotePrEvidence {
                prs: vec![make_pr_summary(42, "tracked-with-pr", "main")],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            // Should only have the "open PRs detected" issue, not any branch-specific issues
            let branch_issues: Vec<_> = snapshot.health.issues().iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-"))
                .filter(|i| i.id.as_str() != "remote-open-prs-detected")
                .collect();
            assert!(branch_issues.is_empty());
        }

        #[test]
        fn skips_fork_prs() {
            let mut snapshot = make_snapshot();
            let mut pr = make_pr_summary(42, "fork-feature", "main");
            pr.head_repo_owner = Some("forker".to_string());

            let evidence = RemotePrEvidence {
                prs: vec![pr],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            // Should have the general "open PRs detected" but no branch-specific issues
            let branch_issues: Vec<_> = snapshot.health.issues().iter()
                .filter(|i| i.id.as_str().starts_with("remote-pr-branch"))
                .collect();
            assert!(branch_issues.is_empty());
        }

        #[test]
        fn empty_evidence_no_issues() {
            let mut snapshot = make_snapshot();
            let evidence = RemotePrEvidence {
                prs: vec![],
                truncated: false,
            };

            generate_bootstrap_issues(&mut snapshot, &evidence);

            // No issues should be generated for empty PR list
            let remote_issues: Vec<_> = snapshot.health.issues().iter()
                .filter(|i| i.id.as_str().starts_with("remote-"))
                .collect();
            assert!(remote_issues.is_empty());
        }
    }
}
```

### Step 13: Add tests for graceful fallback

**File:** `src/engine/scan.rs`

```rust
#[cfg(test)]
mod tests {
    mod remote_query_fallback {
        use super::*;

        #[test]
        fn scan_works_without_remote() {
            // Basic scan should work even without forge
            // This tests the existing scan() function still works
            // (test with a real test repo or mock)
        }

        #[tokio::test]
        async fn scan_with_remote_graceful_when_no_auth() {
            // Test that missing auth doesn't cause errors
            // Just results in remote_prs = None
        }

        #[tokio::test]
        async fn scan_with_remote_graceful_on_api_failure() {
            // Test that API failures are logged but don't fail the scan
        }
    }
}
```

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/doctor/issues.rs` | MODIFY | Add 4 new `KnownIssue` variants |
| `src/engine/health.rs` | MODIFY | Add 4 new issue constructor functions |
| `src/engine/scan.rs` | MODIFY | Add `RemotePrEvidence`, `scan_with_remote()`, forge query logic |

---

## Acceptance Criteria

Per ROADMAP.md Milestone 5.3:

- [ ] Doctor detects open PRs when no local branch exists for head (`RemoteOpenPrBranchMissingLocally`)
- [ ] Doctor detects local branches matching open PR heads but untracked (`RemoteOpenPrBranchUntracked`)
- [ ] Doctor detects tracked branches with missing PR linkage (`RemoteOpenPrNotLinkedInMetadata`)
- [ ] Issues have correct severity levels (Info/Warning per spec)
- [ ] Scanner works offline (no remote issues, no errors)
- [ ] Scanner survives API failures gracefully (log warning, continue)
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Strategy

### Unit Tests

| Test | Location | Description |
|------|----------|-------------|
| `remote_open_prs_detected_issue_id` | `issues.rs` | Issue ID generation |
| `remote_pr_branch_missing_issue_id` | `issues.rs` | Issue ID with hash |
| `remote_pr_branch_untracked_issue_id` | `issues.rs` | Issue ID with hash |
| `remote_pr_not_linked_issue_id` | `issues.rs` | Issue ID with hash |
| `remote_issues_to_issue` | `issues.rs` | Conversion to health Issue |
| `generates_open_prs_detected_issue` | `scan.rs` | Issue generation |
| `generates_branch_missing_issue` | `scan.rs` | Missing branch detection |
| `generates_untracked_issue` | `scan.rs` | Untracked branch detection |
| `generates_not_linked_issue` | `scan.rs` | Unlinked PR detection |
| `no_issue_when_pr_already_linked` | `scan.rs` | Skip already linked |
| `skips_fork_prs` | `scan.rs` | Fork PR handling |
| `empty_evidence_no_issues` | `scan.rs` | Empty case |

### MockForge Integration Tests

Use `MockForge` to test the full flow:

```rust
#[tokio::test]
async fn doctor_shows_bootstrap_issues() {
    let forge = MockForge::new();
    forge.create_pr(CreatePrRequest {
        head: "feature-a".into(),
        base: "main".into(),
        title: "Feature A".into(),
        body: None,
        draft: false,
    }).await.unwrap();

    // Run scan with this mock forge
    // Verify bootstrap issues are generated
}
```

---

## Edge Cases

1. **No open PRs:** `RemoteOpenPullRequestsDetected` is NOT generated (empty is fine)
2. **All PRs already linked:** Only `RemoteOpenPullRequestsDetected` is generated (informational)
3. **Fork PRs:** Skipped with debug log (complex ownership semantics)
4. **Invalid branch names:** Skip PRs with invalid `head_ref` that can't be converted to `BranchName`
5. **Network failure:** Log warning, `remote_prs = None`, no remote issues
6. **Rate limited:** Same as network failure - graceful fallback
7. **Missing capabilities:** Skip remote query entirely, no error

---

## Dependencies

- **Milestone 5.2:** `list_open_prs` method on Forge trait (COMPLETED)

---

## Estimated Scope

- **Lines of code changed:** ~100-150 in `issues.rs`, ~50-80 in `health.rs`, ~150-200 in `scan.rs`
- **New enum variants:** 4 (in `KnownIssue`)
- **New structs:** 1 (`RemotePrEvidence`)
- **New functions:** 4 (issue constructors) + 3 (scan helpers)
- **Risk:** Medium - touches scanning infrastructure, but isolated changes

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

# Specific doctor tests
cargo test doctor

# Specific scan tests
cargo test scan

# Format check
cargo fmt --check
```

---

## Notes

- **Follow the leader:** Uses existing `KnownIssue` pattern from ARCHITECTURE.md
- **Simplicity:** Bootstrap issues are warnings/info, not blocking (minimal disruption)
- **Purity:** Issue generation is pure function of evidence (no I/O)
- **Reuse:** Uses existing `Issue`, `Evidence`, `Severity` types
- **Graceful degradation:** Missing auth or API failures don't break the scanner

---

## Post-Implementation

After this milestone is complete:
1. Update ROADMAP.md to mark 5.3 as complete
2. Create `implementation_notes.md` in this directory
3. Milestone 5.4 (Bootstrap Fix Generators) can begin using these issues
