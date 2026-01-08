//! forge::mock
//!
//! Mock forge implementation for deterministic testing.
//!
//! # Design
//!
//! The mock forge provides a deterministic implementation of the `Forge` trait
//! for use in tests. It stores PRs in memory and allows configuring failure
//! scenarios.
//!
//! # Example
//!
//! ```
//! use lattice::forge::mock::MockForge;
//! use lattice::forge::{Forge, CreatePrRequest, PullRequest, PrState};
//!
//! # tokio_test::block_on(async {
//! let forge = MockForge::new();
//!
//! // Create a PR
//! let pr = forge.create_pr(CreatePrRequest {
//!     head: "feature".to_string(),
//!     base: "main".to_string(),
//!     title: "Add feature".to_string(),
//!     body: None,
//!     draft: false,
//! }).await.unwrap();
//!
//! assert_eq!(pr.number, 1);
//! assert_eq!(pr.state, PrState::Open);
//!
//! // Retrieve it
//! let retrieved = forge.get_pr(1).await.unwrap();
//! assert_eq!(retrieved.title, "Add feature");
//! # });
//! ```

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::traits::{
    CreatePrRequest, Forge, ForgeError, MergeMethod, PrState, PullRequest, Reviewers,
    UpdatePrRequest,
};

/// Mock forge for testing.
///
/// Thread-safe via internal `Arc<Mutex<...>>` wrapping.
#[derive(Debug, Clone)]
pub struct MockForge {
    /// Internal state shared across clones.
    inner: Arc<Mutex<MockForgeInner>>,
}

/// Internal mutable state.
#[derive(Debug)]
struct MockForgeInner {
    /// Stored PRs by number.
    prs: HashMap<u64, PullRequest>,
    /// Next PR number to assign.
    next_pr_number: u64,
    /// Method to fail on (for testing error paths).
    fail_on: Option<FailOn>,
    /// Recorded operations for verification.
    operations: Vec<MockOperation>,
}

/// Configuration for which operation should fail.
#[derive(Debug, Clone)]
pub enum FailOn {
    /// Fail create_pr with the given error.
    CreatePr(ForgeError),
    /// Fail update_pr with the given error.
    UpdatePr(ForgeError),
    /// Fail get_pr with the given error.
    GetPr(ForgeError),
    /// Fail find_pr_by_head with the given error.
    FindPrByHead(ForgeError),
    /// Fail set_draft with the given error.
    SetDraft(ForgeError),
    /// Fail request_reviewers with the given error.
    RequestReviewers(ForgeError),
    /// Fail merge_pr with the given error.
    MergePr(ForgeError),
}

/// Recorded operation for test verification.
#[derive(Debug, Clone)]
pub enum MockOperation {
    CreatePr {
        head: String,
        base: String,
        title: String,
        draft: bool,
    },
    UpdatePr {
        number: u64,
        title: Option<String>,
        body: Option<String>,
        base: Option<String>,
    },
    GetPr {
        number: u64,
    },
    FindPrByHead {
        head: String,
    },
    SetDraft {
        number: u64,
        draft: bool,
    },
    RequestReviewers {
        number: u64,
        users: Vec<String>,
        teams: Vec<String>,
    },
    MergePr {
        number: u64,
        method: MergeMethod,
    },
}

impl MockForge {
    /// Create a new empty mock forge.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockForgeInner {
                prs: HashMap::new(),
                next_pr_number: 1,
                fail_on: None,
                operations: Vec::new(),
            })),
        }
    }

    /// Create a mock forge with pre-existing PRs.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::forge::mock::MockForge;
    /// use lattice::forge::{PullRequest, PrState};
    ///
    /// let pr = PullRequest {
    ///     number: 42,
    ///     url: "https://github.com/owner/repo/pull/42".to_string(),
    ///     state: PrState::Open,
    ///     is_draft: false,
    ///     head: "feature".to_string(),
    ///     base: "main".to_string(),
    ///     title: "Existing PR".to_string(),
    ///     body: Some("PR description".to_string()),
    ///     node_id: Some("PR_123".to_string()),
    /// };
    ///
    /// let forge = MockForge::with_prs(vec![pr]);
    /// ```
    pub fn with_prs(prs: Vec<PullRequest>) -> Self {
        let max_number = prs.iter().map(|p| p.number).max().unwrap_or(0);
        let prs_map: HashMap<u64, PullRequest> = prs.into_iter().map(|p| (p.number, p)).collect();

        Self {
            inner: Arc::new(Mutex::new(MockForgeInner {
                prs: prs_map,
                next_pr_number: max_number + 1,
                fail_on: None,
                operations: Vec::new(),
            })),
        }
    }

    /// Configure the mock to fail on a specific operation.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::forge::mock::{MockForge, FailOn};
    /// use lattice::forge::ForgeError;
    ///
    /// let forge = MockForge::new()
    ///     .fail_on(FailOn::CreatePr(ForgeError::RateLimited));
    /// ```
    pub fn fail_on(self, fail_on: FailOn) -> Self {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.fail_on = Some(fail_on);
        }
        self
    }

    /// Clear the failure configuration.
    pub fn clear_fail_on(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.fail_on = None;
    }

    /// Get all recorded operations.
    ///
    /// Useful for verifying the mock was called correctly.
    pub fn operations(&self) -> Vec<MockOperation> {
        let inner = self.inner.lock().unwrap();
        inner.operations.clone()
    }

    /// Clear recorded operations.
    pub fn clear_operations(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.operations.clear();
    }

    /// Get a PR by number (for test verification).
    pub fn get_pr_sync(&self, number: u64) -> Option<PullRequest> {
        let inner = self.inner.lock().unwrap();
        inner.prs.get(&number).cloned()
    }

    /// Get all PRs (for test verification).
    pub fn all_prs(&self) -> Vec<PullRequest> {
        let inner = self.inner.lock().unwrap();
        inner.prs.values().cloned().collect()
    }

    /// Get the count of PRs.
    pub fn pr_count(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.prs.len()
    }

    /// Record an operation.
    fn record(&self, op: MockOperation) {
        let mut inner = self.inner.lock().unwrap();
        inner.operations.push(op);
    }

    /// Check if we should fail and return the error if so.
    fn check_fail<T>(&self, expected: &str) -> Option<Result<T, ForgeError>> {
        let inner = self.inner.lock().unwrap();
        match &inner.fail_on {
            Some(FailOn::CreatePr(e)) if expected == "create_pr" => Some(Err(clone_error(e))),
            Some(FailOn::UpdatePr(e)) if expected == "update_pr" => Some(Err(clone_error(e))),
            Some(FailOn::GetPr(e)) if expected == "get_pr" => Some(Err(clone_error(e))),
            Some(FailOn::FindPrByHead(e)) if expected == "find_pr_by_head" => {
                Some(Err(clone_error(e)))
            }
            Some(FailOn::SetDraft(e)) if expected == "set_draft" => Some(Err(clone_error(e))),
            Some(FailOn::RequestReviewers(e)) if expected == "request_reviewers" => {
                Some(Err(clone_error(e)))
            }
            Some(FailOn::MergePr(e)) if expected == "merge_pr" => Some(Err(clone_error(e))),
            _ => None,
        }
    }
}

impl Default for MockForge {
    fn default() -> Self {
        Self::new()
    }
}

/// Clone a ForgeError (needed because Error types aren't Clone).
fn clone_error(e: &ForgeError) -> ForgeError {
    match e {
        ForgeError::AuthRequired => ForgeError::AuthRequired,
        ForgeError::AuthFailed(s) => ForgeError::AuthFailed(s.clone()),
        ForgeError::NotFound(s) => ForgeError::NotFound(s.clone()),
        ForgeError::RateLimited => ForgeError::RateLimited,
        ForgeError::ApiError { status, message } => ForgeError::ApiError {
            status: *status,
            message: message.clone(),
        },
        ForgeError::NetworkError(s) => ForgeError::NetworkError(s.clone()),
        ForgeError::NotImplemented(s) => ForgeError::NotImplemented(s.clone()),
    }
}

#[async_trait]
impl Forge for MockForge {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn create_pr(&self, request: CreatePrRequest) -> Result<PullRequest, ForgeError> {
        self.record(MockOperation::CreatePr {
            head: request.head.clone(),
            base: request.base.clone(),
            title: request.title.clone(),
            draft: request.draft,
        });

        if let Some(result) = self.check_fail("create_pr") {
            return result;
        }

        let mut inner = self.inner.lock().unwrap();
        let number = inner.next_pr_number;
        inner.next_pr_number += 1;

        let pr = PullRequest {
            number,
            url: format!("https://github.com/mock/repo/pull/{}", number),
            state: PrState::Open,
            is_draft: request.draft,
            head: request.head,
            base: request.base,
            title: request.title,
            body: request.body,
            node_id: Some(format!("PR_{}", number)),
        };

        inner.prs.insert(number, pr.clone());
        Ok(pr)
    }

    async fn update_pr(&self, request: UpdatePrRequest) -> Result<PullRequest, ForgeError> {
        self.record(MockOperation::UpdatePr {
            number: request.number,
            title: request.title.clone(),
            body: request.body.clone(),
            base: request.base.clone(),
        });

        if let Some(result) = self.check_fail("update_pr") {
            return result;
        }

        let mut inner = self.inner.lock().unwrap();
        let pr = inner
            .prs
            .get_mut(&request.number)
            .ok_or_else(|| ForgeError::NotFound(format!("PR #{}", request.number)))?;

        if let Some(title) = request.title {
            pr.title = title;
        }
        if let Some(body) = request.body {
            pr.body = Some(body);
        }
        if let Some(base) = request.base {
            pr.base = base;
        }

        Ok(pr.clone())
    }

    async fn get_pr(&self, number: u64) -> Result<PullRequest, ForgeError> {
        self.record(MockOperation::GetPr { number });

        if let Some(result) = self.check_fail("get_pr") {
            return result;
        }

        let inner = self.inner.lock().unwrap();
        inner
            .prs
            .get(&number)
            .cloned()
            .ok_or_else(|| ForgeError::NotFound(format!("PR #{}", number)))
    }

    async fn find_pr_by_head(&self, head: &str) -> Result<Option<PullRequest>, ForgeError> {
        self.record(MockOperation::FindPrByHead {
            head: head.to_string(),
        });

        if let Some(result) = self.check_fail("find_pr_by_head") {
            return result;
        }

        let inner = self.inner.lock().unwrap();
        let pr = inner
            .prs
            .values()
            .find(|p| p.head == head && p.state == PrState::Open)
            .cloned();

        Ok(pr)
    }

    async fn set_draft(&self, number: u64, draft: bool) -> Result<(), ForgeError> {
        self.record(MockOperation::SetDraft { number, draft });

        if let Some(result) = self.check_fail::<()>("set_draft") {
            return result;
        }

        let mut inner = self.inner.lock().unwrap();
        let pr = inner
            .prs
            .get_mut(&number)
            .ok_or_else(|| ForgeError::NotFound(format!("PR #{}", number)))?;

        pr.is_draft = draft;
        Ok(())
    }

    async fn request_reviewers(&self, number: u64, reviewers: Reviewers) -> Result<(), ForgeError> {
        self.record(MockOperation::RequestReviewers {
            number,
            users: reviewers.users.clone(),
            teams: reviewers.teams.clone(),
        });

        if let Some(result) = self.check_fail::<()>("request_reviewers") {
            return result;
        }

        // Verify PR exists
        let inner = self.inner.lock().unwrap();
        if !inner.prs.contains_key(&number) {
            return Err(ForgeError::NotFound(format!("PR #{}", number)));
        }

        Ok(())
    }

    async fn merge_pr(&self, number: u64, method: MergeMethod) -> Result<(), ForgeError> {
        self.record(MockOperation::MergePr { number, method });

        if let Some(result) = self.check_fail::<()>("merge_pr") {
            return result;
        }

        let mut inner = self.inner.lock().unwrap();
        let pr = inner
            .prs
            .get_mut(&number)
            .ok_or_else(|| ForgeError::NotFound(format!("PR #{}", number)))?;

        if pr.state != PrState::Open {
            return Err(ForgeError::ApiError {
                status: 405,
                message: "Pull request is not open".into(),
            });
        }

        pr.state = PrState::Merged;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_pr_assigns_sequential_numbers() {
        let forge = MockForge::new();

        let pr1 = forge
            .create_pr(CreatePrRequest {
                head: "feature-1".into(),
                base: "main".into(),
                title: "First PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let pr2 = forge
            .create_pr(CreatePrRequest {
                head: "feature-2".into(),
                base: "main".into(),
                title: "Second PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        assert_eq!(pr1.number, 1);
        assert_eq!(pr2.number, 2);
    }

    #[tokio::test]
    async fn create_pr_draft() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Draft PR".into(),
                body: None,
                draft: true,
            })
            .await
            .unwrap();

        assert!(pr.is_draft);
    }

    #[tokio::test]
    async fn get_pr_returns_created() {
        let forge = MockForge::new();

        let created = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let retrieved = forge.get_pr(created.number).await.unwrap();
        assert_eq!(retrieved.title, "Test PR");
        assert_eq!(retrieved.head, "feature");
    }

    #[tokio::test]
    async fn get_pr_not_found() {
        let forge = MockForge::new();

        let result = forge.get_pr(999).await;
        assert!(matches!(result, Err(ForgeError::NotFound(_))));
    }

    #[tokio::test]
    async fn update_pr_changes_fields() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Original Title".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let updated = forge
            .update_pr(UpdatePrRequest {
                number: pr.number,
                title: Some("New Title".into()),
                body: None,
                base: Some("develop".into()),
            })
            .await
            .unwrap();

        assert_eq!(updated.title, "New Title");
        assert_eq!(updated.base, "develop");
    }

    #[tokio::test]
    async fn find_pr_by_head() {
        let forge = MockForge::new();

        forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let found = forge.find_pr_by_head("feature").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().head, "feature");

        let not_found = forge.find_pr_by_head("nonexistent").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn find_pr_by_head_only_open() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        // Merge the PR
        forge
            .merge_pr(pr.number, MergeMethod::Squash)
            .await
            .unwrap();

        // Should not find merged PR
        let found = forge.find_pr_by_head("feature").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn set_draft() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        assert!(!pr.is_draft);

        forge.set_draft(pr.number, true).await.unwrap();
        let updated = forge.get_pr(pr.number).await.unwrap();
        assert!(updated.is_draft);

        forge.set_draft(pr.number, false).await.unwrap();
        let updated = forge.get_pr(pr.number).await.unwrap();
        assert!(!updated.is_draft);
    }

    #[tokio::test]
    async fn merge_pr() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        forge
            .merge_pr(pr.number, MergeMethod::Squash)
            .await
            .unwrap();

        let merged = forge.get_pr(pr.number).await.unwrap();
        assert_eq!(merged.state, PrState::Merged);
    }

    #[tokio::test]
    async fn merge_already_merged_fails() {
        let forge = MockForge::new();

        let pr = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        forge
            .merge_pr(pr.number, MergeMethod::Squash)
            .await
            .unwrap();

        let result = forge.merge_pr(pr.number, MergeMethod::Squash).await;
        assert!(matches!(result, Err(ForgeError::ApiError { .. })));
    }

    #[tokio::test]
    async fn fail_on_create_pr() {
        let forge = MockForge::new().fail_on(FailOn::CreatePr(ForgeError::RateLimited));

        let result = forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test".into(),
                body: None,
                draft: false,
            })
            .await;

        assert!(matches!(result, Err(ForgeError::RateLimited)));
    }

    #[tokio::test]
    async fn operations_recorded() {
        let forge = MockForge::new();

        forge
            .create_pr(CreatePrRequest {
                head: "feature".into(),
                base: "main".into(),
                title: "Test".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        let ops = forge.operations();
        assert_eq!(ops.len(), 1);
        assert!(matches!(ops[0], MockOperation::CreatePr { .. }));
    }

    #[tokio::test]
    async fn with_prs_starts_with_existing() {
        let existing = PullRequest {
            number: 42,
            url: "https://github.com/owner/repo/pull/42".into(),
            state: PrState::Open,
            is_draft: false,
            head: "existing".into(),
            base: "main".into(),
            title: "Existing PR".into(),
            body: Some("Description".into()),
            node_id: Some("PR_42".into()),
        };

        let forge = MockForge::with_prs(vec![existing]);

        let pr = forge.get_pr(42).await.unwrap();
        assert_eq!(pr.title, "Existing PR");

        // New PRs should start after max number
        let new_pr = forge
            .create_pr(CreatePrRequest {
                head: "new".into(),
                base: "main".into(),
                title: "New PR".into(),
                body: None,
                draft: false,
            })
            .await
            .unwrap();

        assert_eq!(new_pr.number, 43);
    }

    #[test]
    fn forge_name() {
        let forge = MockForge::new();
        assert_eq!(forge.name(), "mock");
    }

    #[test]
    fn all_prs() {
        let forge = MockForge::with_prs(vec![
            PullRequest {
                number: 1,
                url: "https://github.com/owner/repo/pull/1".into(),
                state: PrState::Open,
                is_draft: false,
                head: "a".into(),
                base: "main".into(),
                title: "A".into(),
                body: None,
                node_id: None,
            },
            PullRequest {
                number: 2,
                url: "https://github.com/owner/repo/pull/2".into(),
                state: PrState::Open,
                is_draft: false,
                head: "b".into(),
                base: "main".into(),
                title: "B".into(),
                body: None,
                node_id: None,
            },
        ]);

        assert_eq!(forge.pr_count(), 2);
        let all = forge.all_prs();
        assert_eq!(all.len(), 2);
    }
}
