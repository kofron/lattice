//! git::interface
//!
//! Git interface implementation using git2.
//!
//! This module provides the **single doorway** to all Git operations in Lattice.
//! Per ARCHITECTURE.md Section 10.1, all Git interactions flow through this
//! interface, which provides structured results and normalizes errors into
//! typed failure categories.
//!
//! # Architecture
//!
//! The `Git` struct is the only way to interact with a Git repository.
//! No other module should import `git2` directly. This ensures:
//!
//! - Consistent error handling across all Git operations
//! - Strong type guarantees at the boundary
//! - CAS (compare-and-swap) semantics for all ref mutations
//!
//! # Error Handling
//!
//! Git errors are categorized into typed variants:
//! - [`GitError::NotARepo`]: Not inside a Git repository
//! - [`GitError::RefNotFound`]: Requested ref does not exist
//! - [`GitError::CasFailed`]: Compare-and-swap precondition failed
//! - [`GitError::OperationInProgress`]: Rebase/merge/cherry-pick in progress
//! - [`GitError::DirtyWorktree`]: Working tree has uncommitted changes
//!
//! # Example
//!
//! ```ignore
//! use latticework::git::Git;
//! use std::path::Path;
//!
//! let git = Git::open(Path::new("."))?;
//! let oid = git.resolve_ref("refs/heads/main")?;
//! println!("main is at {}", oid.short(7));
//! ```

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::core::types::{BranchName, Oid, RefName, TypeError};

/// Errors from Git operations.
///
/// These error types cover all categories of Git failures that Lattice
/// needs to handle distinctly. The categorization enables proper error
/// handling at higher layers (e.g., doctor can offer fixes for specific
/// error types).
#[derive(Debug, Error)]
pub enum GitError {
    /// Not inside a Git repository.
    #[error("not a git repository: {path}")]
    NotARepo {
        /// The path that was searched
        path: PathBuf,
    },

    /// Repository is bare (no working directory).
    #[error("bare repository not supported")]
    BareRepo,

    /// Requested ref does not exist.
    #[error("ref not found: {refname}")]
    RefNotFound {
        /// The ref that was not found
        refname: String,
    },

    /// Compare-and-swap precondition failed.
    ///
    /// This occurs when attempting to update a ref but its current value
    /// doesn't match the expected value. This is critical for correctness -
    /// it prevents applying changes to a repository that has changed since
    /// planning.
    #[error("CAS failed for {refname}: expected {expected}, found {actual}")]
    CasFailed {
        /// The ref being updated
        refname: String,
        /// The expected old value
        expected: String,
        /// The actual current value
        actual: String,
    },

    /// Git operation in progress (rebase, merge, etc.).
    #[error("{operation} in progress")]
    OperationInProgress {
        /// The type of operation in progress
        operation: GitState,
    },

    /// Working tree has uncommitted changes.
    #[error("working tree is dirty: {details}")]
    DirtyWorktree {
        /// Description of what's dirty
        details: String,
    },

    /// Object not found in repository.
    #[error("object not found: {oid}")]
    ObjectNotFound {
        /// The OID that was not found
        oid: String,
    },

    /// Invalid object id format.
    #[error("invalid object id: {oid}")]
    InvalidOid {
        /// The invalid OID string
        oid: String,
    },

    /// Invalid ref name format.
    #[error("invalid ref name: {message}")]
    InvalidRefName {
        /// Description of the problem
        message: String,
    },

    /// Blob content is not valid UTF-8.
    #[error("blob is not valid UTF-8: {oid}")]
    InvalidUtf8 {
        /// The OID of the blob
        oid: String,
    },

    /// Permission or filesystem error.
    #[error("repository access error: {message}")]
    AccessError {
        /// Description of the error
        message: String,
    },

    /// Internal git2 error.
    #[error("git error: {message}")]
    Internal {
        /// The error message
        message: String,
    },
}

impl GitError {
    /// Create a GitError from a git2::Error with richer context.
    fn from_git2(err: git2::Error, context: &str) -> Self {
        match err.code() {
            git2::ErrorCode::NotFound => {
                if context.starts_with("refs/") || context.contains("ref") {
                    GitError::RefNotFound {
                        refname: context.to_string(),
                    }
                } else {
                    GitError::ObjectNotFound {
                        oid: context.to_string(),
                    }
                }
            }
            git2::ErrorCode::InvalidSpec => GitError::InvalidOid {
                oid: context.to_string(),
            },
            git2::ErrorCode::Locked => GitError::AccessError {
                message: format!("repository is locked: {}", err.message()),
            },
            _ => GitError::Internal {
                message: format!("{}: {}", context, err.message()),
            },
        }
    }
}

impl From<git2::Error> for GitError {
    fn from(err: git2::Error) -> Self {
        match err.code() {
            git2::ErrorCode::NotFound => GitError::RefNotFound {
                refname: err.message().to_string(),
            },
            git2::ErrorCode::InvalidSpec => GitError::InvalidOid {
                oid: err.message().to_string(),
            },
            _ => GitError::Internal {
                message: err.message().to_string(),
            },
        }
    }
}

impl From<TypeError> for GitError {
    fn from(err: TypeError) -> Self {
        match err {
            TypeError::InvalidOid(msg) => GitError::InvalidOid { oid: msg },
            TypeError::InvalidRefName(msg) => GitError::InvalidRefName { message: msg },
            TypeError::InvalidBranchName(msg) => GitError::InvalidRefName { message: msg },
        }
    }
}

/// Information about a Git repository.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    /// Path to .git directory
    pub git_dir: PathBuf,
    /// Path to working directory
    pub work_dir: PathBuf,
}

/// State of in-progress Git operations.
///
/// This enum represents the various states a Git repository can be in
/// when an operation is paused (usually due to conflicts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitState {
    /// No operation in progress.
    Clean,

    /// Rebase in progress.
    Rebase {
        /// Current step in the rebase (1-indexed), if available.
        current: Option<usize>,
        /// Total steps in the rebase, if available.
        total: Option<usize>,
    },

    /// Merge in progress.
    Merge,

    /// Cherry-pick in progress.
    CherryPick,

    /// Revert in progress.
    Revert,

    /// Bisect in progress.
    Bisect,

    /// Apply mailbox in progress.
    ApplyMailbox,
}

impl GitState {
    /// Check if any operation is in progress.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::git::GitState;
    ///
    /// assert!(!GitState::Clean.is_in_progress());
    /// assert!(GitState::Merge.is_in_progress());
    /// ```
    pub fn is_in_progress(&self) -> bool {
        !matches!(self, GitState::Clean)
    }

    /// Get a human-readable description of the state.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::git::GitState;
    ///
    /// assert_eq!(GitState::Clean.description(), "clean");
    /// assert_eq!(GitState::Merge.description(), "merge");
    /// ```
    pub fn description(&self) -> &'static str {
        match self {
            GitState::Clean => "clean",
            GitState::Rebase { .. } => "rebase",
            GitState::Merge => "merge",
            GitState::CherryPick => "cherry-pick",
            GitState::Revert => "revert",
            GitState::Bisect => "bisect",
            GitState::ApplyMailbox => "apply-mailbox",
        }
    }
}

impl std::fmt::Display for GitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitState::Rebase {
                current: Some(c),
                total: Some(t),
            } => write!(f, "rebase ({}/{})", c, t),
            _ => write!(f, "{}", self.description()),
        }
    }
}

/// A ref with its name and target OID.
///
/// Used when enumerating refs in a namespace.
#[derive(Debug, Clone)]
pub struct RefEntry {
    /// The full ref name
    pub name: RefName,
    /// The OID the ref points to
    pub oid: Oid,
}

/// Summary of working tree status.
///
/// Provides counts of different types of changes in the working tree,
/// useful for pre-command checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    /// Number of staged changes
    pub staged: usize,
    /// Number of unstaged changes to tracked files
    pub unstaged: usize,
    /// Number of untracked files (if requested)
    pub untracked: usize,
    /// Whether there are unresolved conflicts
    pub has_conflicts: bool,
}

impl WorktreeStatus {
    /// Check if the worktree is completely clean (no changes at all).
    pub fn is_clean(&self) -> bool {
        self.staged == 0 && self.unstaged == 0 && !self.has_conflicts
    }

    /// Check if there are any staged changes ready to commit.
    pub fn has_staged(&self) -> bool {
        self.staged > 0
    }
}

/// Information about a commit.
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// The commit OID
    pub oid: Oid,
    /// First line of the commit message
    pub summary: String,
    /// Full commit message
    pub message: String,
    /// Author name
    pub author_name: String,
    /// Author email
    pub author_email: String,
    /// Author timestamp
    pub author_time: chrono::DateTime<chrono::Utc>,
}

/// The Git interface.
///
/// This is the **single point of interaction** with Git. All repository
/// reads and writes flow through this interface. No other module should
/// import `git2` directly.
///
/// # CAS Semantics
///
/// All ref mutation operations use compare-and-swap (CAS) semantics.
/// This means updates only succeed if the ref's current value matches
/// an expected value. This is critical for correctness - it prevents
/// the executor from applying changes to a repository that has been
/// modified since planning.
///
/// # Example
///
/// ```ignore
/// use latticework::git::Git;
/// use std::path::Path;
///
/// let git = Git::open(Path::new("."))?;
///
/// // Read operations
/// let oid = git.resolve_ref("refs/heads/main")?;
/// let branches = git.list_branches()?;
///
/// // CAS update (fails if ref changed)
/// git.update_ref_cas(
///     "refs/heads/feature",
///     &new_oid,
///     Some(&old_oid),
///     "lattice: restack"
/// )?;
/// ```
pub struct Git {
    /// The underlying git2 repository
    repo: git2::Repository,
}

impl std::fmt::Debug for Git {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Git")
            .field("path", &self.repo.path())
            .finish()
    }
}

impl Git {
    // =========================================================================
    // Repository Opening and Info
    // =========================================================================

    /// Open a repository at the given path.
    ///
    /// Uses `git2::Repository::discover` to find the repository root,
    /// so `path` can be any directory within the repository.
    ///
    /// # Errors
    ///
    /// - [`GitError::NotARepo`] if no repository is found
    /// - [`GitError::BareRepo`] if the repository has no working directory
    ///
    /// # Example
    ///
    /// ```ignore
    /// use latticework::git::Git;
    /// use std::path::Path;
    ///
    /// let git = Git::open(Path::new("./src"))?;  // Works from subdirectory
    /// ```
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let repo = git2::Repository::discover(path).map_err(|_| GitError::NotARepo {
            path: path.to_path_buf(),
        })?;

        // Ensure it's not a bare repository
        if repo.is_bare() {
            return Err(GitError::BareRepo);
        }

        Ok(Self { repo })
    }

    /// Get repository information (git_dir and work_dir paths).
    pub fn info(&self) -> Result<RepoInfo, GitError> {
        let git_dir = self.repo.path().to_path_buf();
        let work_dir = self.repo.workdir().ok_or(GitError::BareRepo)?.to_path_buf();

        Ok(RepoInfo { git_dir, work_dir })
    }

    /// Get direct access to the .git directory path.
    pub fn git_dir(&self) -> &Path {
        self.repo.path()
    }

    // =========================================================================
    // State Detection
    // =========================================================================

    /// Get the current Git state (rebase, merge, etc.).
    ///
    /// This detects in-progress operations that require user intervention
    /// (usually conflict resolution).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use latticework::git::{Git, GitState};
    ///
    /// let git = Git::open(Path::new("."))?;
    /// if git.state().is_in_progress() {
    ///     println!("Operation in progress: {}", git.state());
    /// }
    /// ```
    pub fn state(&self) -> GitState {
        match self.repo.state() {
            git2::RepositoryState::Clean => GitState::Clean,
            git2::RepositoryState::Rebase
            | git2::RepositoryState::RebaseInteractive
            | git2::RepositoryState::RebaseMerge => {
                // Try to read rebase progress
                let (current, total) = self.read_rebase_progress();
                GitState::Rebase { current, total }
            }
            git2::RepositoryState::Merge => GitState::Merge,
            git2::RepositoryState::CherryPick | git2::RepositoryState::CherryPickSequence => {
                GitState::CherryPick
            }
            git2::RepositoryState::Revert | git2::RepositoryState::RevertSequence => {
                GitState::Revert
            }
            git2::RepositoryState::Bisect => GitState::Bisect,
            git2::RepositoryState::ApplyMailbox | git2::RepositoryState::ApplyMailboxOrRebase => {
                GitState::ApplyMailbox
            }
        }
    }

    /// Read rebase progress from .git/rebase-merge or .git/rebase-apply.
    fn read_rebase_progress(&self) -> (Option<usize>, Option<usize>) {
        let git_dir = self.repo.path();

        // Try rebase-merge first (interactive rebase)
        let rebase_merge = git_dir.join("rebase-merge");
        if rebase_merge.exists() {
            let current = std::fs::read_to_string(rebase_merge.join("msgnum"))
                .ok()
                .and_then(|s| s.trim().parse().ok());
            let total = std::fs::read_to_string(rebase_merge.join("end"))
                .ok()
                .and_then(|s| s.trim().parse().ok());
            return (current, total);
        }

        // Try rebase-apply (non-interactive rebase)
        let rebase_apply = git_dir.join("rebase-apply");
        if rebase_apply.exists() {
            let current = std::fs::read_to_string(rebase_apply.join("next"))
                .ok()
                .and_then(|s| s.trim().parse().ok());
            let total = std::fs::read_to_string(rebase_apply.join("last"))
                .ok()
                .and_then(|s| s.trim().parse().ok());
            return (current, total);
        }

        (None, None)
    }

    /// Check if there are unresolved conflicts in the index.
    pub fn has_conflicts(&self) -> Result<bool, GitError> {
        let index = self.repo.index().map_err(|e| GitError::Internal {
            message: e.message().to_string(),
        })?;

        Ok(index.has_conflicts())
    }

    // =========================================================================
    // Working Tree Status
    // =========================================================================

    /// Get working tree status summary.
    ///
    /// If `include_untracked` is false, untracked files are not counted.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let status = git.worktree_status(false)?;
    /// if !status.is_clean() {
    ///     println!("Working tree has changes");
    /// }
    /// ```
    pub fn worktree_status(&self, include_untracked: bool) -> Result<WorktreeStatus, GitError> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(include_untracked)
            .include_ignored(false);

        let statuses = self
            .repo
            .statuses(Some(&mut opts))
            .map_err(|e| GitError::Internal {
                message: e.message().to_string(),
            })?;

        let mut result = WorktreeStatus::default();

        for entry in statuses.iter() {
            let status = entry.status();

            // Check for conflicts
            if status.is_conflicted() {
                result.has_conflicts = true;
            }

            // Count staged changes
            if status.is_index_new()
                || status.is_index_modified()
                || status.is_index_deleted()
                || status.is_index_renamed()
                || status.is_index_typechange()
            {
                result.staged += 1;
            }

            // Count unstaged changes
            if status.is_wt_modified()
                || status.is_wt_deleted()
                || status.is_wt_renamed()
                || status.is_wt_typechange()
            {
                result.unstaged += 1;
            }

            // Count untracked
            if status.is_wt_new() {
                result.untracked += 1;
            }
        }

        Ok(result)
    }

    /// Check if working tree is clean (no staged or unstaged changes).
    ///
    /// Does not consider untracked files.
    pub fn is_worktree_clean(&self) -> Result<bool, GitError> {
        let status = self.worktree_status(false)?;
        Ok(status.is_clean())
    }

    // =========================================================================
    // Ref Resolution
    // =========================================================================

    /// Resolve a ref to its target OID.
    ///
    /// This peels through symbolic refs and tags to get the commit OID.
    ///
    /// # Errors
    ///
    /// - [`GitError::RefNotFound`] if the ref doesn't exist
    ///
    /// # Example
    ///
    /// ```ignore
    /// let oid = git.resolve_ref("refs/heads/main")?;
    /// println!("main is at {}", oid.short(7));
    /// ```
    pub fn resolve_ref(&self, refname: &str) -> Result<Oid, GitError> {
        let reference = self
            .repo
            .find_reference(refname)
            .map_err(|e| GitError::from_git2(e, refname))?;

        let oid = reference
            .peel_to_commit()
            .map_err(|e| GitError::from_git2(e, refname))?
            .id();

        Oid::new(oid.to_string()).map_err(|e| e.into())
    }

    /// Resolve a ref, returning None if it doesn't exist.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(oid) = git.try_resolve_ref("refs/heads/feature")? {
    ///     println!("feature branch exists at {}", oid.short(7));
    /// }
    /// ```
    pub fn try_resolve_ref(&self, refname: &str) -> Result<Option<Oid>, GitError> {
        match self.resolve_ref(refname) {
            Ok(oid) => Ok(Some(oid)),
            Err(GitError::RefNotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get HEAD commit OID.
    ///
    /// # Errors
    ///
    /// - [`GitError::RefNotFound`] if HEAD is unborn (new repository)
    pub fn head_oid(&self) -> Result<Oid, GitError> {
        let head = self
            .repo
            .head()
            .map_err(|e| GitError::from_git2(e, "HEAD"))?;

        let oid = head
            .peel_to_commit()
            .map_err(|e| GitError::from_git2(e, "HEAD"))?
            .id();

        Oid::new(oid.to_string()).map_err(|e| e.into())
    }

    /// Check if a ref exists.
    pub fn ref_exists(&self, refname: &str) -> bool {
        self.repo.find_reference(refname).is_ok()
    }

    /// Get the current branch name, if on a branch.
    ///
    /// Returns `None` if HEAD is detached or unborn.
    pub fn current_branch(&self) -> Result<Option<BranchName>, GitError> {
        let head = match self.repo.head() {
            Ok(h) => h,
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        if head.is_branch() {
            if let Some(name) = head.shorthand() {
                return Ok(Some(BranchName::new(name)?));
            }
        }

        Ok(None) // Detached HEAD
    }

    // =========================================================================
    // Ref Enumeration
    // =========================================================================

    /// List all refs matching a prefix.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // List all branch metadata refs
    /// let metadata_refs = git.list_refs_by_prefix("refs/branch-metadata/")?;
    /// for entry in metadata_refs {
    ///     println!("{} -> {}", entry.name, entry.oid.short(7));
    /// }
    /// ```
    pub fn list_refs_by_prefix(&self, prefix: &str) -> Result<Vec<RefEntry>, GitError> {
        let pattern = format!("{}*", prefix);
        let refs = self
            .repo
            .references_glob(&pattern)
            .map_err(|e| GitError::Internal {
                message: e.message().to_string(),
            })?;

        let mut entries = Vec::new();
        for reference in refs {
            let reference = reference.map_err(|e| GitError::Internal {
                message: e.message().to_string(),
            })?;

            // Get ref name
            let name = match reference.name() {
                Some(n) => n,
                None => continue, // Skip refs with non-UTF8 names
            };

            // Skip invalid ref names
            let ref_name = match RefName::new(name) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Resolve to OID
            let oid = match reference.peel_to_commit() {
                Ok(commit) => commit.id(),
                Err(_) => {
                    // For non-commit refs (like metadata blobs), get direct target
                    match reference.target() {
                        Some(oid) => oid,
                        None => continue,
                    }
                }
            };

            let oid = match Oid::new(oid.to_string()) {
                Ok(o) => o,
                Err(_) => continue,
            };

            entries.push(RefEntry {
                name: ref_name,
                oid,
            });
        }

        Ok(entries)
    }

    /// List all local branches.
    ///
    /// Returns validated `BranchName` instances.
    pub fn list_branches(&self) -> Result<Vec<BranchName>, GitError> {
        let branches = self
            .repo
            .branches(Some(git2::BranchType::Local))
            .map_err(|e| GitError::Internal {
                message: e.message().to_string(),
            })?;

        let mut names = Vec::new();
        for branch in branches {
            let (branch, _) = branch.map_err(|e| GitError::Internal {
                message: e.message().to_string(),
            })?;
            if let Some(name) = branch.name().ok().flatten() {
                // Skip invalid branch names
                if let Ok(branch_name) = BranchName::new(name) {
                    names.push(branch_name);
                }
            }
        }

        Ok(names)
    }

    /// List all metadata refs and return as (BranchName, Oid) pairs.
    ///
    /// Convenience method for scanner to enumerate tracked branches.
    pub fn list_metadata_refs(&self) -> Result<Vec<(BranchName, Oid)>, GitError> {
        let entries = self.list_refs_by_prefix("refs/branch-metadata/")?;

        let mut result = Vec::new();
        for entry in entries {
            // Extract branch name from ref
            if let Some(name) = entry.name.strip_prefix("refs/branch-metadata/") {
                if let Ok(branch_name) = BranchName::new(name) {
                    result.push((branch_name, entry.oid));
                }
            }
        }

        Ok(result)
    }

    // =========================================================================
    // CAS Ref Operations
    // =========================================================================

    /// Update a ref with compare-and-swap semantics.
    ///
    /// The update only succeeds if the ref's current value matches `expected_old`.
    /// If `expected_old` is `None`, the ref must not exist (create case).
    ///
    /// This is the **only** way to update refs in Lattice, ensuring correctness
    /// even when the repository is modified externally.
    ///
    /// # Errors
    ///
    /// - [`GitError::CasFailed`] if the current value doesn't match expected
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Create a new ref (must not exist)
    /// git.update_ref_cas(
    ///     "refs/branch-metadata/feature",
    ///     &blob_oid,
    ///     None,  // Must not exist
    ///     "lattice: track branch"
    /// )?;
    ///
    /// // Update existing ref (must match expected)
    /// git.update_ref_cas(
    ///     "refs/branch-metadata/feature",
    ///     &new_blob_oid,
    ///     Some(&old_blob_oid),
    ///     "lattice: update metadata"
    /// )?;
    /// ```
    pub fn update_ref_cas(
        &self,
        refname: &str,
        new_oid: &Oid,
        expected_old: Option<&Oid>,
        message: &str,
    ) -> Result<(), GitError> {
        // Check current value
        let current = self.try_resolve_ref_raw(refname)?;

        // Verify CAS precondition
        match (expected_old, current.as_ref()) {
            (Some(expected), Some(actual)) if expected.as_str() != actual => {
                return Err(GitError::CasFailed {
                    refname: refname.to_string(),
                    expected: expected.to_string(),
                    actual: actual.clone(),
                });
            }
            (Some(expected), None) => {
                return Err(GitError::CasFailed {
                    refname: refname.to_string(),
                    expected: expected.to_string(),
                    actual: "<none>".to_string(),
                });
            }
            (None, Some(actual)) => {
                return Err(GitError::CasFailed {
                    refname: refname.to_string(),
                    expected: "<none>".to_string(),
                    actual: actual.clone(),
                });
            }
            _ => {} // Precondition satisfied
        }

        // Perform the update
        let oid = git2::Oid::from_str(new_oid.as_str())
            .map_err(|e| GitError::from_git2(e, new_oid.as_str()))?;

        self.repo
            .reference(refname, oid, true, message)
            .map_err(|e| GitError::from_git2(e, refname))?;

        Ok(())
    }

    /// Delete a ref with compare-and-swap semantics.
    ///
    /// The delete only succeeds if the ref's current value matches `expected_old`.
    ///
    /// # Errors
    ///
    /// - [`GitError::CasFailed`] if the current value doesn't match expected
    /// - [`GitError::RefNotFound`] if the ref doesn't exist
    pub fn delete_ref_cas(&self, refname: &str, expected_old: &Oid) -> Result<(), GitError> {
        // Check current value
        let current = self.try_resolve_ref_raw(refname)?;

        match current {
            None => {
                return Err(GitError::RefNotFound {
                    refname: refname.to_string(),
                });
            }
            Some(actual) if actual != expected_old.as_str() => {
                return Err(GitError::CasFailed {
                    refname: refname.to_string(),
                    expected: expected_old.to_string(),
                    actual,
                });
            }
            _ => {} // Precondition satisfied
        }

        // Find and delete the reference
        let mut reference = self
            .repo
            .find_reference(refname)
            .map_err(|e| GitError::from_git2(e, refname))?;

        reference
            .delete()
            .map_err(|e| GitError::from_git2(e, refname))?;

        Ok(())
    }

    /// Resolve a ref to its target OID without peeling to commit.
    ///
    /// Unlike `resolve_ref` which peels through tags to commits, this method
    /// returns the direct target of the ref. Use this for refs that point to
    /// non-commit objects like blobs (e.g., metadata refs).
    ///
    /// Returns `Ok(None)` if the ref doesn't exist.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // For metadata refs pointing to blobs
    /// if let Some(blob_oid) = git.try_resolve_ref_to_object("refs/branch-metadata/feature")? {
    ///     let content = git.read_blob(&blob_oid)?;
    /// }
    /// ```
    pub fn try_resolve_ref_to_object(&self, refname: &str) -> Result<Option<Oid>, GitError> {
        match self.repo.find_reference(refname) {
            Ok(reference) => {
                // Resolve symbolic refs to final target
                let resolved = reference.resolve().unwrap_or(reference);
                let oid = resolved.target().ok_or_else(|| GitError::Internal {
                    message: format!("ref {} has no target", refname),
                })?;
                Ok(Some(Oid::new(oid.to_string())?))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(GitError::from_git2(e, refname)),
        }
    }

    /// Try to resolve a ref to its raw OID string (without validation).
    ///
    /// Used internally for CAS operations where we need the raw value.
    fn try_resolve_ref_raw(&self, refname: &str) -> Result<Option<String>, GitError> {
        match self.repo.find_reference(refname) {
            Ok(reference) => {
                // Get the target OID - for symbolic refs, resolve to final target
                let resolved = reference.resolve().unwrap_or(reference);
                let oid = resolved.target().ok_or_else(|| GitError::Internal {
                    message: format!("ref {} has no target", refname),
                })?;
                Ok(Some(oid.to_string()))
            }
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(GitError::from_git2(e, refname)),
        }
    }

    // =========================================================================
    // Ancestry Queries
    // =========================================================================

    /// Find the merge base (common ancestor) of two commits.
    ///
    /// Returns `None` if there is no common ancestor.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(base) = git.merge_base(&oid1, &oid2)? {
    ///     println!("Common ancestor: {}", base.short(7));
    /// }
    /// ```
    pub fn merge_base(&self, oid1: &Oid, oid2: &Oid) -> Result<Option<Oid>, GitError> {
        let git_oid1 = git2::Oid::from_str(oid1.as_str())
            .map_err(|e| GitError::from_git2(e, oid1.as_str()))?;
        let git_oid2 = git2::Oid::from_str(oid2.as_str())
            .map_err(|e| GitError::from_git2(e, oid2.as_str()))?;

        match self.repo.merge_base(git_oid1, git_oid2) {
            Ok(oid) => Ok(Some(Oid::new(oid.to_string())?)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(GitError::Internal {
                message: e.message().to_string(),
            }),
        }
    }

    /// Check if `ancestor` is an ancestor of `descendant`.
    ///
    /// Returns true if ancestor == descendant (a commit is its own ancestor).
    ///
    /// # Example
    ///
    /// ```ignore
    /// if git.is_ancestor(&base_oid, &tip_oid)? {
    ///     println!("base is an ancestor of tip");
    /// }
    /// ```
    pub fn is_ancestor(&self, ancestor: &Oid, descendant: &Oid) -> Result<bool, GitError> {
        // A commit is its own ancestor
        if ancestor == descendant {
            return Ok(true);
        }

        let ancestor_oid = git2::Oid::from_str(ancestor.as_str())
            .map_err(|e| GitError::from_git2(e, ancestor.as_str()))?;
        let descendant_oid = git2::Oid::from_str(descendant.as_str())
            .map_err(|e| GitError::from_git2(e, descendant.as_str()))?;

        self.repo
            .graph_descendant_of(descendant_oid, ancestor_oid)
            .map_err(|e| GitError::Internal {
                message: e.message().to_string(),
            })
    }

    /// Count commits between two OIDs.
    ///
    /// Counts commits reachable from `tip` but not from `base`.
    /// Useful for determining if a branch has commits beyond its base.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let count = git.commit_count(&base, &tip)?;
    /// if count == 0 {
    ///     println!("Branch has no unique commits");
    /// }
    /// ```
    pub fn commit_count(&self, base: &Oid, tip: &Oid) -> Result<usize, GitError> {
        let base_oid = git2::Oid::from_str(base.as_str())
            .map_err(|e| GitError::from_git2(e, base.as_str()))?;
        let tip_oid =
            git2::Oid::from_str(tip.as_str()).map_err(|e| GitError::from_git2(e, tip.as_str()))?;

        let mut revwalk = self.repo.revwalk().map_err(|e| GitError::Internal {
            message: e.message().to_string(),
        })?;

        revwalk.push(tip_oid).map_err(|e| GitError::Internal {
            message: e.message().to_string(),
        })?;
        revwalk.hide(base_oid).map_err(|e| GitError::Internal {
            message: e.message().to_string(),
        })?;

        let count = revwalk.count();
        Ok(count)
    }

    // =========================================================================
    // Blob Operations
    // =========================================================================

    /// Write content as a blob and return its OID.
    ///
    /// Used for storing metadata as Git objects.
    pub fn write_blob(&self, content: &[u8]) -> Result<Oid, GitError> {
        let oid = self.repo.blob(content).map_err(|e| GitError::Internal {
            message: e.message().to_string(),
        })?;

        Oid::new(oid.to_string()).map_err(|e| e.into())
    }

    /// Read a blob by OID.
    ///
    /// # Errors
    ///
    /// - [`GitError::ObjectNotFound`] if the blob doesn't exist
    pub fn read_blob(&self, oid: &Oid) -> Result<Vec<u8>, GitError> {
        let git_oid =
            git2::Oid::from_str(oid.as_str()).map_err(|e| GitError::from_git2(e, oid.as_str()))?;

        let blob = self
            .repo
            .find_blob(git_oid)
            .map_err(|e| GitError::from_git2(e, oid.as_str()))?;

        Ok(blob.content().to_vec())
    }

    /// Read a blob as UTF-8 string.
    ///
    /// # Errors
    ///
    /// - [`GitError::ObjectNotFound`] if the blob doesn't exist
    /// - [`GitError::InvalidUtf8`] if the blob is not valid UTF-8
    pub fn read_blob_as_string(&self, oid: &Oid) -> Result<String, GitError> {
        let content = self.read_blob(oid)?;
        String::from_utf8(content).map_err(|_| GitError::InvalidUtf8 {
            oid: oid.to_string(),
        })
    }

    // =========================================================================
    // Commit Information
    // =========================================================================

    /// Get information about a commit.
    ///
    /// # Errors
    ///
    /// - [`GitError::ObjectNotFound`] if the commit doesn't exist
    pub fn commit_info(&self, oid: &Oid) -> Result<CommitInfo, GitError> {
        let git_oid =
            git2::Oid::from_str(oid.as_str()).map_err(|e| GitError::from_git2(e, oid.as_str()))?;

        let commit = self
            .repo
            .find_commit(git_oid)
            .map_err(|e| GitError::from_git2(e, oid.as_str()))?;

        let author = commit.author();
        let author_time = chrono::DateTime::from_timestamp(author.when().seconds(), 0)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH)
            .with_timezone(&chrono::Utc);

        Ok(CommitInfo {
            oid: oid.clone(),
            summary: commit.summary().unwrap_or("").to_string(),
            message: commit.message().unwrap_or("").to_string(),
            author_name: author.name().unwrap_or("").to_string(),
            author_email: author.email().unwrap_or("").to_string(),
            author_time,
        })
    }

    /// Get the parent OIDs of a commit.
    ///
    /// Returns empty vec for root commits, multiple OIDs for merge commits.
    pub fn commit_parents(&self, oid: &Oid) -> Result<Vec<Oid>, GitError> {
        let git_oid =
            git2::Oid::from_str(oid.as_str()).map_err(|e| GitError::from_git2(e, oid.as_str()))?;

        let commit = self
            .repo
            .find_commit(git_oid)
            .map_err(|e| GitError::from_git2(e, oid.as_str()))?;

        let mut parents = Vec::new();
        for parent in commit.parents() {
            parents.push(Oid::new(parent.id().to_string())?);
        }

        Ok(parents)
    }

    // =========================================================================
    // Remote Operations
    // =========================================================================

    /// Get the URL for a remote.
    ///
    /// Returns `None` if the remote doesn't exist.
    pub fn remote_url(&self, name: &str) -> Result<Option<String>, GitError> {
        match self.repo.find_remote(name) {
            Ok(remote) => Ok(remote.url().map(String::from)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(GitError::Internal {
                message: e.message().to_string(),
            }),
        }
    }

    /// Get the default remote name (usually "origin").
    ///
    /// Returns the first remote found, or `None` if no remotes exist.
    pub fn default_remote(&self) -> Result<Option<String>, GitError> {
        let remotes = self.repo.remotes().map_err(|e| GitError::Internal {
            message: e.message().to_string(),
        })?;

        // Prefer "origin" if it exists
        for name in remotes.iter().flatten() {
            if name == "origin" {
                return Ok(Some(name.to_string()));
            }
        }

        // Otherwise return first remote
        Ok(remotes.iter().flatten().next().map(String::from))
    }

    /// Parse a remote URL into owner/repo for GitHub.
    ///
    /// Handles both HTTPS and SSH URLs:
    /// - `https://github.com/owner/repo.git` -> `Some(("owner", "repo"))`
    /// - `git@github.com:owner/repo.git` -> `Some(("owner", "repo"))`
    ///
    /// Returns `None` for non-GitHub URLs.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::git::Git;
    ///
    /// assert_eq!(
    ///     Git::parse_github_remote("https://github.com/owner/repo.git"),
    ///     Some(("owner".to_string(), "repo".to_string()))
    /// );
    /// assert_eq!(
    ///     Git::parse_github_remote("git@github.com:owner/repo.git"),
    ///     Some(("owner".to_string(), "repo".to_string()))
    /// );
    /// assert_eq!(
    ///     Git::parse_github_remote("https://gitlab.com/owner/repo.git"),
    ///     None
    /// );
    /// ```
    pub fn parse_github_remote(url: &str) -> Option<(String, String)> {
        // HTTPS format: https://github.com/owner/repo.git
        if let Some(rest) = url.strip_prefix("https://github.com/") {
            return Self::parse_owner_repo(rest);
        }

        // SSH format: git@github.com:owner/repo.git
        if let Some(rest) = url.strip_prefix("git@github.com:") {
            return Self::parse_owner_repo(rest);
        }

        None
    }

    /// Parse "owner/repo.git" or "owner/repo" into (owner, repo).
    fn parse_owner_repo(path: &str) -> Option<(String, String)> {
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, repo) = path.split_once('/')?;

        if owner.is_empty() || repo.is_empty() {
            return None;
        }

        Some((owner.to_string(), repo.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod git_error {
        use super::*;

        #[test]
        fn error_variants_constructible() {
            let _ = GitError::NotARepo {
                path: PathBuf::from("/tmp"),
            };
            let _ = GitError::BareRepo;
            let _ = GitError::RefNotFound {
                refname: "refs/heads/main".to_string(),
            };
            let _ = GitError::CasFailed {
                refname: "refs/heads/main".to_string(),
                expected: "abc123".to_string(),
                actual: "def456".to_string(),
            };
            let _ = GitError::OperationInProgress {
                operation: GitState::Rebase {
                    current: Some(1),
                    total: Some(3),
                },
            };
            let _ = GitError::DirtyWorktree {
                details: "staged changes".to_string(),
            };
            let _ = GitError::ObjectNotFound {
                oid: "abc123".to_string(),
            };
            let _ = GitError::InvalidOid {
                oid: "not-hex".to_string(),
            };
            let _ = GitError::InvalidUtf8 {
                oid: "abc123".to_string(),
            };
            let _ = GitError::AccessError {
                message: "locked".to_string(),
            };
            let _ = GitError::Internal {
                message: "oops".to_string(),
            };
        }

        #[test]
        fn error_display_formatting() {
            let err = GitError::CasFailed {
                refname: "refs/heads/main".to_string(),
                expected: "abc".to_string(),
                actual: "def".to_string(),
            };
            assert!(err.to_string().contains("CAS failed"));
            assert!(err.to_string().contains("refs/heads/main"));
        }
    }

    mod git_state {
        use super::*;

        #[test]
        fn clean_is_not_in_progress() {
            assert!(!GitState::Clean.is_in_progress());
        }

        #[test]
        fn operations_are_in_progress() {
            assert!(GitState::Merge.is_in_progress());
            assert!(GitState::CherryPick.is_in_progress());
            assert!(GitState::Revert.is_in_progress());
            assert!(GitState::Bisect.is_in_progress());
            assert!(GitState::ApplyMailbox.is_in_progress());
            assert!(GitState::Rebase {
                current: None,
                total: None
            }
            .is_in_progress());
        }

        #[test]
        fn descriptions() {
            assert_eq!(GitState::Clean.description(), "clean");
            assert_eq!(GitState::Merge.description(), "merge");
            assert_eq!(
                GitState::Rebase {
                    current: None,
                    total: None
                }
                .description(),
                "rebase"
            );
        }

        #[test]
        fn display_formatting() {
            assert_eq!(format!("{}", GitState::Clean), "clean");
            assert_eq!(format!("{}", GitState::Merge), "merge");
            assert_eq!(
                format!(
                    "{}",
                    GitState::Rebase {
                        current: Some(2),
                        total: Some(5)
                    }
                ),
                "rebase (2/5)"
            );
        }
    }

    mod worktree_status {
        use super::*;

        #[test]
        fn default_is_clean() {
            let status = WorktreeStatus::default();
            assert!(status.is_clean());
            assert!(!status.has_staged());
        }

        #[test]
        fn staged_changes() {
            let status = WorktreeStatus {
                staged: 3,
                ..Default::default()
            };
            assert!(!status.is_clean());
            assert!(status.has_staged());
        }

        #[test]
        fn unstaged_changes() {
            let status = WorktreeStatus {
                unstaged: 2,
                ..Default::default()
            };
            assert!(!status.is_clean());
        }

        #[test]
        fn conflicts_make_dirty() {
            let status = WorktreeStatus {
                has_conflicts: true,
                ..Default::default()
            };
            assert!(!status.is_clean());
        }

        #[test]
        fn untracked_not_dirty() {
            // Untracked files don't make the worktree "dirty"
            let status = WorktreeStatus {
                untracked: 5,
                ..Default::default()
            };
            assert!(status.is_clean());
        }
    }

    mod parse_github_remote {
        use super::*;

        #[test]
        fn https_url() {
            assert_eq!(
                Git::parse_github_remote("https://github.com/owner/repo.git"),
                Some(("owner".to_string(), "repo".to_string()))
            );
        }

        #[test]
        fn https_url_without_git_suffix() {
            assert_eq!(
                Git::parse_github_remote("https://github.com/owner/repo"),
                Some(("owner".to_string(), "repo".to_string()))
            );
        }

        #[test]
        fn ssh_url() {
            assert_eq!(
                Git::parse_github_remote("git@github.com:owner/repo.git"),
                Some(("owner".to_string(), "repo".to_string()))
            );
        }

        #[test]
        fn ssh_url_without_git_suffix() {
            assert_eq!(
                Git::parse_github_remote("git@github.com:owner/repo"),
                Some(("owner".to_string(), "repo".to_string()))
            );
        }

        #[test]
        fn non_github_returns_none() {
            assert_eq!(
                Git::parse_github_remote("https://gitlab.com/owner/repo.git"),
                None
            );
            assert_eq!(
                Git::parse_github_remote("git@gitlab.com:owner/repo.git"),
                None
            );
        }

        #[test]
        fn malformed_returns_none() {
            assert_eq!(Git::parse_github_remote("not-a-url"), None);
            assert_eq!(Git::parse_github_remote("https://github.com/"), None);
            assert_eq!(Git::parse_github_remote("https://github.com/owner"), None);
        }
    }
}
