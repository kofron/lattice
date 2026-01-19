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

/// The type of repository context.
///
/// Lattice needs to know what kind of repository it's operating in to:
/// - Route storage to the correct location (common_dir vs git_dir)
/// - Gate commands that require a working directory
/// - Handle worktree-specific constraints
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoContext {
    /// A normal repository (not bare, not a linked worktree).
    /// In this case, git_dir == common_dir.
    Normal,

    /// A bare repository (no working directory).
    /// Remote operations may work, but checkout/restack/etc cannot.
    Bare,

    /// A linked worktree created via `git worktree add`.
    /// The git_dir is per-worktree, common_dir is shared with parent.
    Worktree,
}

impl RepoContext {
    /// Check if this context has a working directory.
    pub fn has_workdir(&self) -> bool {
        !matches!(self, RepoContext::Bare)
    }

    /// Check if this is a bare repository.
    pub fn is_bare(&self) -> bool {
        matches!(self, RepoContext::Bare)
    }

    /// Check if this is a linked worktree.
    pub fn is_worktree(&self) -> bool {
        matches!(self, RepoContext::Worktree)
    }
}

impl std::fmt::Display for RepoContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RepoContext::Normal => write!(f, "normal"),
            RepoContext::Bare => write!(f, "bare"),
            RepoContext::Worktree => write!(f, "worktree"),
        }
    }
}

/// Information about a Git repository.
///
/// This struct captures the three key paths for Git repository contexts:
/// - `git_dir`: The per-worktree `.git` directory (or gitdir for linked worktrees)
/// - `common_dir`: The shared directory for refs, objects, and config
/// - `work_dir`: The working directory (None for bare repos)
///
/// For normal repositories, `git_dir == common_dir`.
/// For linked worktrees, `common_dir` points to the parent repo's git dir.
/// For bare repos, `work_dir` is None.
#[derive(Debug, Clone)]
pub struct RepoInfo {
    /// Path to the per-worktree .git directory.
    /// For normal repos, this is `.git/`.
    /// For linked worktrees, this is `.git/worktrees/<name>/`.
    pub git_dir: PathBuf,

    /// Path to the shared git directory (refs, objects, config).
    /// For normal repos, this equals git_dir.
    /// For linked worktrees, this is the parent repo's git dir.
    pub common_dir: PathBuf,

    /// Path to working directory (None for bare repos).
    pub work_dir: Option<PathBuf>,

    /// The type of repository context.
    pub context: RepoContext,
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

/// Reason why a working directory is unavailable.
///
/// Per SPEC.md §4.6.9, this enum provides specific context for why
/// worktree status cannot be determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeUnavailableReason {
    /// Repository is a bare clone with no working directory.
    BareRepository,
    /// Git dir exists but work_dir is None (unusual configuration).
    NoWorkDir,
    /// Failed to probe worktree status (git command failed).
    ProbeFailed,
}

impl std::fmt::Display for WorktreeUnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BareRepository => write!(f, "bare repository has no working directory"),
            Self::NoWorkDir => write!(f, "no working directory configured"),
            Self::ProbeFailed => write!(f, "failed to probe worktree status"),
        }
    }
}

/// Status of the working directory.
///
/// Per SPEC.md §4.6.9, this enum distinguishes between clean, dirty,
/// and unavailable worktree states. The `Unavailable` variant explicitly
/// captures why status cannot be determined rather than using `Option`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WorktreeStatus {
    /// Working directory has no uncommitted changes.
    #[default]
    Clean,
    /// Working directory has uncommitted changes.
    Dirty {
        /// Number of staged changes.
        staged: u32,
        /// Number of unstaged changes to tracked files.
        unstaged: u32,
        /// Number of merge conflicts.
        conflicts: u32,
    },
    /// Working directory is not available.
    Unavailable {
        /// Reason why the worktree is unavailable.
        reason: WorktreeUnavailableReason,
    },
}

impl WorktreeStatus {
    /// Check if the worktree is completely clean (no changes at all).
    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }

    /// Check if the worktree is dirty (has uncommitted changes).
    pub fn is_dirty(&self) -> bool {
        matches!(self, Self::Dirty { .. })
    }

    /// Check if the worktree is unavailable.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }

    /// Check if there are any staged changes ready to commit.
    pub fn has_staged(&self) -> bool {
        matches!(self, Self::Dirty { staged, .. } if *staged > 0)
    }

    /// Check if there are merge conflicts.
    pub fn has_conflicts(&self) -> bool {
        matches!(self, Self::Dirty { conflicts, .. } if *conflicts > 0)
    }

    /// Create an unavailable status for a bare repository.
    pub fn bare_repository() -> Self {
        Self::Unavailable {
            reason: WorktreeUnavailableReason::BareRepository,
        }
    }

    /// Create an unavailable status when there's no work dir.
    pub fn no_work_dir() -> Self {
        Self::Unavailable {
            reason: WorktreeUnavailableReason::NoWorkDir,
        }
    }

    /// Create an unavailable status when probing failed.
    pub fn probe_failed() -> Self {
        Self::Unavailable {
            reason: WorktreeUnavailableReason::ProbeFailed,
        }
    }
}

/// Information about a linked worktree.
///
/// Per SPEC.md §4.6.8, Lattice needs to detect when a branch is checked out
/// in another worktree to prevent operations that would fail or cause confusion.
///
/// This struct represents a single worktree entry as returned by
/// `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Path to the worktree directory.
    pub path: PathBuf,
    /// HEAD commit of the worktree, if available.
    pub head: Option<Oid>,
    /// Branch checked out in this worktree (None if detached HEAD).
    pub branch: Option<BranchName>,
    /// Whether this is the bare repository entry.
    pub is_bare: bool,
}

impl WorktreeEntry {
    /// Check if this worktree has a specific branch checked out.
    pub fn has_branch(&self, name: &BranchName) -> bool {
        self.branch.as_ref() == Some(name)
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
/// Result of running a git command via [`Git::run_command`].
///
/// This struct captures the full output of a git command execution,
/// including stdout, stderr, and exit status. It's used for low-level
/// git command execution where no typed interface exists.
#[derive(Debug, Clone)]
pub struct GitCommandResult {
    /// Whether the command exited successfully (exit code 0).
    pub success: bool,
    /// Standard output from the command.
    pub stdout: String,
    /// Standard error from the command.
    pub stderr: String,
    /// Exit code of the command (-1 if not available).
    pub exit_code: i32,
}

/// The primary Git interface.
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
    /// This method supports all repository types:
    /// - Normal repositories (with working directory)
    /// - Bare repositories (no working directory)
    /// - Linked worktrees (created via `git worktree add`)
    ///
    /// # Errors
    ///
    /// - [`GitError::NotARepo`] if no repository is found
    ///
    /// # Example
    ///
    /// ```ignore
    /// use latticework::git::Git;
    /// use std::path::Path;
    ///
    /// let git = Git::open(Path::new("./src"))?;  // Works from subdirectory
    /// let info = git.info()?;
    /// if info.context.is_bare() {
    ///     println!("This is a bare repository");
    /// }
    /// ```
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let repo = git2::Repository::discover(path).map_err(|_| GitError::NotARepo {
            path: path.to_path_buf(),
        })?;

        Ok(Self { repo })
    }

    /// Get repository information (git_dir, common_dir, work_dir, context).
    ///
    /// This method detects the repository context and populates all path fields:
    /// - For normal repos: git_dir == common_dir, work_dir is Some
    /// - For bare repos: git_dir == common_dir, work_dir is None
    /// - For worktrees: git_dir != common_dir, work_dir is Some
    pub fn info(&self) -> Result<RepoInfo, GitError> {
        let git_dir = self.repo.path().to_path_buf();
        let work_dir = self.repo.workdir().map(|p| p.to_path_buf());

        // Get the common directory (shared across worktrees).
        // For normal and bare repos, this equals git_dir.
        // For linked worktrees, this is the parent repo's git dir.
        let common_dir = self.repo.commondir().to_path_buf();

        // Determine the repository context
        let context = if self.repo.is_bare() {
            RepoContext::Bare
        } else if self.is_linked_worktree() {
            RepoContext::Worktree
        } else {
            RepoContext::Normal
        };

        Ok(RepoInfo {
            git_dir,
            common_dir,
            work_dir,
            context,
        })
    }

    /// Check if this repository is a linked worktree.
    ///
    /// A linked worktree is created via `git worktree add` and has its
    /// git_dir inside the parent repo's `.git/worktrees/` directory.
    fn is_linked_worktree(&self) -> bool {
        // A linked worktree has commondir != path (git_dir)
        self.repo.commondir() != self.repo.path()
    }

    /// Check if the repository has a working directory.
    pub fn has_workdir(&self) -> bool {
        self.repo.workdir().is_some()
    }

    /// Check if the repository is bare.
    pub fn is_bare(&self) -> bool {
        self.repo.is_bare()
    }

    /// Get the common directory (shared across worktrees).
    ///
    /// For normal repos, this equals git_dir.
    /// For linked worktrees, this is the parent repo's git dir.
    pub fn common_dir(&self) -> PathBuf {
        self.repo.commondir().to_path_buf()
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
    pub fn worktree_status(&self, _include_untracked: bool) -> Result<WorktreeStatus, GitError> {
        // Check if we have a working directory
        if self.repo.is_bare() {
            return Ok(WorktreeStatus::bare_repository());
        }

        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(false).include_ignored(false);

        let statuses = self
            .repo
            .statuses(Some(&mut opts))
            .map_err(|e| GitError::Internal {
                message: format!("Failed to probe worktree status: {}", e.message()),
            })?;

        let mut staged: u32 = 0;
        let mut unstaged: u32 = 0;
        let mut conflicts: u32 = 0;

        for entry in statuses.iter() {
            let status = entry.status();

            // Check for conflicts
            if status.is_conflicted() {
                conflicts += 1;
            }

            // Count staged changes
            if status.is_index_new()
                || status.is_index_modified()
                || status.is_index_deleted()
                || status.is_index_renamed()
                || status.is_index_typechange()
            {
                staged += 1;
            }

            // Count unstaged changes
            if status.is_wt_modified()
                || status.is_wt_deleted()
                || status.is_wt_renamed()
                || status.is_wt_typechange()
            {
                unstaged += 1;
            }
        }

        if staged == 0 && unstaged == 0 && conflicts == 0 {
            Ok(WorktreeStatus::Clean)
        } else {
            Ok(WorktreeStatus::Dirty {
                staged,
                unstaged,
                conflicts,
            })
        }
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

    /// List all worktrees associated with this repository.
    ///
    /// Per SPEC.md §4.6.8, this is used to detect when a branch is checked out
    /// in another worktree. Operations that would rewrite such a branch must
    /// be refused with a clear error message.
    ///
    /// Uses `git worktree list --porcelain` for reliable parsing.
    ///
    /// # Returns
    ///
    /// A list of `WorktreeEntry` structs describing each worktree.
    /// The main worktree is always included (possibly as bare if it's a bare repo).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let worktrees = git.list_worktrees()?;
    /// for wt in &worktrees {
    ///     if let Some(branch) = &wt.branch {
    ///         println!("{} has {} checked out", wt.path.display(), branch);
    ///     }
    /// }
    /// ```
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeEntry>, GitError> {
        use std::process::Command;

        let git_dir = self.git_dir();
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(git_dir)
            .output()
            .map_err(|e| GitError::Internal {
                message: format!("failed to run git worktree list: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::Internal {
                message: format!("git worktree list failed: {}", stderr),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_worktree_list_porcelain(&stdout)
    }

    /// Find which worktree (if any) has a branch checked out.
    ///
    /// Returns the path to the worktree that has the branch checked out,
    /// or None if the branch is not checked out anywhere.
    ///
    /// This excludes the current worktree from the search, since we're
    /// looking for *other* worktrees that would block an operation.
    pub fn branch_checked_out_elsewhere(
        &self,
        branch: &BranchName,
    ) -> Result<Option<PathBuf>, GitError> {
        let worktrees = self.list_worktrees()?;
        let current_work_dir = self.info().ok().and_then(|i| i.work_dir);

        for wt in worktrees {
            // Skip bare entries
            if wt.is_bare {
                continue;
            }

            // Skip current worktree
            if current_work_dir.as_ref() == Some(&wt.path) {
                continue;
            }

            if wt.has_branch(branch) {
                return Ok(Some(wt.path));
            }
        }

        Ok(None)
    }

    /// Find all branches that are checked out in worktrees other than the current one.
    ///
    /// Returns a map from branch name to the worktree path where it's checked out.
    /// This is useful for gating operations that touch multiple branches.
    pub fn branches_checked_out_elsewhere(
        &self,
    ) -> Result<std::collections::HashMap<BranchName, PathBuf>, GitError> {
        let worktrees = self.list_worktrees()?;
        let current_work_dir = self.info().ok().and_then(|i| i.work_dir);

        let mut result = std::collections::HashMap::new();

        for wt in worktrees {
            // Skip bare entries
            if wt.is_bare {
                continue;
            }

            // Skip current worktree
            if current_work_dir.as_ref() == Some(&wt.path) {
                continue;
            }

            if let Some(branch) = wt.branch {
                result.insert(branch, wt.path);
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

    /// Fetch a specific ref from a remote.
    ///
    /// This fetches a ref from the specified remote using the given refspec.
    /// The refspec can be a simple branch name or a full refspec like
    /// `refs/heads/feature:refs/heads/feature`.
    ///
    /// # Arguments
    ///
    /// * `remote` - Remote name (e.g., "origin")
    /// * `refspec` - Refspec to fetch. Can be:
    ///   - A branch name: `feature` (fetches `refs/heads/feature`)
    ///   - A full refspec: `refs/heads/feature:refs/heads/feature`
    ///   - A PR ref: `refs/pull/123/head:refs/heads/pr-123`
    ///
    /// # Returns
    ///
    /// The OID of the fetched ref tip.
    ///
    /// # Errors
    ///
    /// - [`GitError::Internal`] if the fetch fails
    /// - [`GitError::RefNotFound`] if the ref cannot be resolved after fetch
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Fetch a branch from origin
    /// let oid = git.fetch_ref("origin", "refs/heads/feature:refs/heads/feature")?;
    ///
    /// // Fetch a PR head ref
    /// let oid = git.fetch_ref("origin", "refs/pull/42/head:refs/heads/pr-42")?;
    /// ```
    pub fn fetch_ref(&self, remote: &str, refspec: &str) -> Result<Oid, GitError> {
        use std::process::Command;

        // Determine the working directory for the command
        let work_dir = self.info().ok().and_then(|i| i.work_dir);
        let run_dir = work_dir.as_deref().unwrap_or_else(|| self.repo.path());

        let output = Command::new("git")
            .args(["fetch", remote, refspec])
            .current_dir(run_dir)
            .output()
            .map_err(|e| GitError::Internal {
                message: format!("failed to run git fetch: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::Internal {
                message: format!("git fetch {} {} failed: {}", remote, refspec, stderr.trim()),
            });
        }

        // Extract the target ref from refspec and resolve it
        // Refspec format: source:destination or just source
        let target_ref = if refspec.contains(':') {
            // Full refspec - use destination
            refspec.split(':').next_back().unwrap_or(refspec)
        } else {
            // Simple refspec - construct the full ref
            // If it looks like a branch name, prefix with refs/heads/
            if !refspec.starts_with("refs/") {
                // For simple branch names, FETCH_HEAD contains the result
                // We need to read FETCH_HEAD to get the OID
                return self.read_fetch_head();
            }
            refspec
        };

        self.resolve_ref(target_ref)
    }

    /// Run a git command with the given arguments.
    ///
    /// This is a low-level method for executing arbitrary git commands.
    /// Prefer specific typed methods (like [`fetch_ref`], [`update_ref_cas`])
    /// when available, as they provide better error handling and type safety.
    ///
    /// The command is executed in the repository's working directory (or git
    /// directory for bare repos).
    ///
    /// # Arguments
    ///
    /// * `args` - Command arguments (excluding "git" itself)
    ///
    /// # Returns
    ///
    /// A [`GitCommandResult`] with stdout, stderr, and success status.
    /// The method itself only fails if the git process couldn't be spawned.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Run a simple git command
    /// let result = git.run_command(&["status".to_string()])?;
    /// if result.success {
    ///     println!("Status: {}", result.stdout);
    /// } else {
    ///     eprintln!("Failed: {}", result.stderr);
    /// }
    ///
    /// // Fetch a branch
    /// let result = git.run_command(&[
    ///     "fetch".to_string(),
    ///     "origin".to_string(),
    ///     "feature:refs/heads/feature".to_string(),
    /// ])?;
    /// ```
    pub fn run_command(&self, args: &[String]) -> Result<GitCommandResult, GitError> {
        use std::process::Command;

        // Determine the working directory for the command
        let work_dir = self.info().ok().and_then(|i| i.work_dir);
        let run_dir = work_dir.as_deref().unwrap_or_else(|| self.repo.path());

        let output = Command::new("git")
            .args(args)
            .current_dir(run_dir)
            .output()
            .map_err(|e| GitError::Internal {
                message: format!(
                    "failed to run git {}: {}",
                    args.first().unwrap_or(&String::new()),
                    e
                ),
            })?;

        Ok(GitCommandResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    /// Read the OID from FETCH_HEAD after a fetch operation.
    ///
    /// FETCH_HEAD contains the OID of the most recently fetched ref.
    fn read_fetch_head(&self) -> Result<Oid, GitError> {
        let fetch_head_path = self.repo.path().join("FETCH_HEAD");
        let content =
            std::fs::read_to_string(&fetch_head_path).map_err(|e| GitError::Internal {
                message: format!("failed to read FETCH_HEAD: {}", e),
            })?;

        // FETCH_HEAD format: <oid> <tab> <info>
        // We just need the first 40 characters (the OID)
        let oid_str = content
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .ok_or_else(|| GitError::Internal {
                message: "FETCH_HEAD is empty or malformed".to_string(),
            })?;

        Oid::new(oid_str).map_err(|e| e.into())
    }
}

/// Parse the output of `git worktree list --porcelain`.
///
/// The porcelain format outputs one worktree per block, separated by blank lines.
/// Each block contains lines like:
/// ```text
/// worktree /path/to/worktree
/// HEAD abcd1234...
/// branch refs/heads/main
/// ```
///
/// For bare repos, the block looks like:
/// ```text
/// worktree /path/to/bare.git
/// bare
/// ```
///
/// For detached HEAD:
/// ```text
/// worktree /path/to/worktree
/// HEAD abcd1234...
/// detached
/// ```
fn parse_worktree_list_porcelain(output: &str) -> Result<Vec<WorktreeEntry>, GitError> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<Oid> = None;
    let mut current_branch: Option<BranchName> = None;
    let mut is_bare = false;

    for line in output.lines() {
        if line.is_empty() {
            // End of a worktree block
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    head: current_head.take(),
                    branch: current_branch.take(),
                    is_bare,
                });
                is_bare = false;
            }
            continue;
        }

        if let Some(path_str) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path_str));
        } else if let Some(head_str) = line.strip_prefix("HEAD ") {
            current_head = Oid::new(head_str).ok();
        } else if let Some(branch_str) = line.strip_prefix("branch ") {
            // Branch is in full ref form: refs/heads/name
            if let Some(name) = branch_str.strip_prefix("refs/heads/") {
                current_branch = BranchName::new(name).ok();
            }
        } else if line == "bare" {
            is_bare = true;
        }
        // "detached" line is ignored - we just won't have a branch
    }

    // Handle last block if output doesn't end with blank line
    if let Some(path) = current_path {
        entries.push(WorktreeEntry {
            path,
            head: current_head,
            branch: current_branch,
            is_bare,
        });
    }

    Ok(entries)
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
            assert!(!status.is_dirty());
            assert!(!status.is_unavailable());
        }

        #[test]
        fn clean_variant() {
            let status = WorktreeStatus::Clean;
            assert!(status.is_clean());
            assert!(!status.is_dirty());
            assert!(!status.has_staged());
            assert!(!status.has_conflicts());
        }

        #[test]
        fn staged_changes() {
            let status = WorktreeStatus::Dirty {
                staged: 3,
                unstaged: 0,
                conflicts: 0,
            };
            assert!(!status.is_clean());
            assert!(status.is_dirty());
            assert!(status.has_staged());
            assert!(!status.has_conflicts());
        }

        #[test]
        fn unstaged_changes() {
            let status = WorktreeStatus::Dirty {
                staged: 0,
                unstaged: 2,
                conflicts: 0,
            };
            assert!(!status.is_clean());
            assert!(status.is_dirty());
            assert!(!status.has_staged());
        }

        #[test]
        fn conflicts_make_dirty() {
            let status = WorktreeStatus::Dirty {
                staged: 0,
                unstaged: 0,
                conflicts: 1,
            };
            assert!(!status.is_clean());
            assert!(status.is_dirty());
            assert!(status.has_conflicts());
        }

        #[test]
        fn unavailable_bare_repository() {
            let status = WorktreeStatus::bare_repository();
            assert!(!status.is_clean());
            assert!(!status.is_dirty());
            assert!(status.is_unavailable());
            assert!(!status.has_staged());
            assert!(!status.has_conflicts());
            assert!(matches!(
                status,
                WorktreeStatus::Unavailable {
                    reason: WorktreeUnavailableReason::BareRepository
                }
            ));
        }

        #[test]
        fn unavailable_no_work_dir() {
            let status = WorktreeStatus::no_work_dir();
            assert!(status.is_unavailable());
            assert!(matches!(
                status,
                WorktreeStatus::Unavailable {
                    reason: WorktreeUnavailableReason::NoWorkDir
                }
            ));
        }

        #[test]
        fn unavailable_probe_failed() {
            let status = WorktreeStatus::probe_failed();
            assert!(status.is_unavailable());
            assert!(matches!(
                status,
                WorktreeStatus::Unavailable {
                    reason: WorktreeUnavailableReason::ProbeFailed
                }
            ));
        }

        #[test]
        fn reason_display() {
            assert_eq!(
                WorktreeUnavailableReason::BareRepository.to_string(),
                "bare repository has no working directory"
            );
            assert_eq!(
                WorktreeUnavailableReason::NoWorkDir.to_string(),
                "no working directory configured"
            );
            assert_eq!(
                WorktreeUnavailableReason::ProbeFailed.to_string(),
                "failed to probe worktree status"
            );
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

    mod repo_context {
        use super::*;

        #[test]
        fn normal_has_workdir() {
            assert!(RepoContext::Normal.has_workdir());
        }

        #[test]
        fn bare_has_no_workdir() {
            assert!(!RepoContext::Bare.has_workdir());
        }

        #[test]
        fn worktree_has_workdir() {
            assert!(RepoContext::Worktree.has_workdir());
        }

        #[test]
        fn is_bare() {
            assert!(!RepoContext::Normal.is_bare());
            assert!(RepoContext::Bare.is_bare());
            assert!(!RepoContext::Worktree.is_bare());
        }

        #[test]
        fn is_worktree() {
            assert!(!RepoContext::Normal.is_worktree());
            assert!(!RepoContext::Bare.is_worktree());
            assert!(RepoContext::Worktree.is_worktree());
        }

        #[test]
        fn display_formatting() {
            assert_eq!(format!("{}", RepoContext::Normal), "normal");
            assert_eq!(format!("{}", RepoContext::Bare), "bare");
            assert_eq!(format!("{}", RepoContext::Worktree), "worktree");
        }

        #[test]
        fn equality() {
            assert_eq!(RepoContext::Normal, RepoContext::Normal);
            assert_eq!(RepoContext::Bare, RepoContext::Bare);
            assert_eq!(RepoContext::Worktree, RepoContext::Worktree);
            assert_ne!(RepoContext::Normal, RepoContext::Bare);
            assert_ne!(RepoContext::Normal, RepoContext::Worktree);
            assert_ne!(RepoContext::Bare, RepoContext::Worktree);
        }

        #[test]
        fn copy_semantics() {
            let ctx = RepoContext::Normal;
            let ctx2 = ctx; // Copy
            assert_eq!(ctx, ctx2);
        }
    }

    mod repo_info {
        use super::*;

        #[test]
        fn normal_repo_info() {
            let info = RepoInfo {
                git_dir: PathBuf::from("/repo/.git"),
                common_dir: PathBuf::from("/repo/.git"),
                work_dir: Some(PathBuf::from("/repo")),
                context: RepoContext::Normal,
            };

            assert_eq!(info.git_dir, info.common_dir);
            assert!(info.work_dir.is_some());
            assert_eq!(info.context, RepoContext::Normal);
        }

        #[test]
        fn bare_repo_info() {
            let info = RepoInfo {
                git_dir: PathBuf::from("/repo.git"),
                common_dir: PathBuf::from("/repo.git"),
                work_dir: None,
                context: RepoContext::Bare,
            };

            assert_eq!(info.git_dir, info.common_dir);
            assert!(info.work_dir.is_none());
            assert_eq!(info.context, RepoContext::Bare);
        }

        #[test]
        fn worktree_repo_info() {
            let info = RepoInfo {
                git_dir: PathBuf::from("/main-repo/.git/worktrees/feature"),
                common_dir: PathBuf::from("/main-repo/.git"),
                work_dir: Some(PathBuf::from("/worktrees/feature")),
                context: RepoContext::Worktree,
            };

            assert_ne!(info.git_dir, info.common_dir);
            assert!(info.work_dir.is_some());
            assert_eq!(info.context, RepoContext::Worktree);
        }
    }

    mod worktree_parsing {
        use super::*;

        #[test]
        fn parse_single_worktree() {
            let output = "worktree /path/to/repo\n\
                          HEAD abc123def4567890abc123def4567890abc12345\n\
                          branch refs/heads/main\n\
                          \n";

            let entries = parse_worktree_list_porcelain(output).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].path, PathBuf::from("/path/to/repo"));
            assert!(entries[0].head.is_some());
            assert_eq!(entries[0].branch.as_ref().map(|b| b.as_str()), Some("main"));
            assert!(!entries[0].is_bare);
        }

        #[test]
        fn parse_bare_worktree() {
            let output = "worktree /path/to/repo.git\n\
                          bare\n\
                          \n";

            let entries = parse_worktree_list_porcelain(output).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].path, PathBuf::from("/path/to/repo.git"));
            assert!(entries[0].head.is_none());
            assert!(entries[0].branch.is_none());
            assert!(entries[0].is_bare);
        }

        #[test]
        fn parse_detached_head() {
            let output = "worktree /path/to/worktree\n\
                          HEAD abc123def4567890abc123def4567890abc12345\n\
                          detached\n\
                          \n";

            let entries = parse_worktree_list_porcelain(output).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].path, PathBuf::from("/path/to/worktree"));
            assert!(entries[0].head.is_some());
            assert!(entries[0].branch.is_none()); // Detached = no branch
            assert!(!entries[0].is_bare);
        }

        #[test]
        fn parse_multiple_worktrees() {
            let output = "worktree /main/repo\n\
                          HEAD abc123def4567890abc123def4567890abc12345\n\
                          branch refs/heads/main\n\
                          \n\
                          worktree /worktrees/feature\n\
                          HEAD def456abc7890123def456abc7890123def45678\n\
                          branch refs/heads/feature\n\
                          \n";

            let entries = parse_worktree_list_porcelain(output).unwrap();
            assert_eq!(entries.len(), 2);

            assert_eq!(entries[0].path, PathBuf::from("/main/repo"));
            assert_eq!(entries[0].branch.as_ref().map(|b| b.as_str()), Some("main"));

            assert_eq!(entries[1].path, PathBuf::from("/worktrees/feature"));
            assert_eq!(
                entries[1].branch.as_ref().map(|b| b.as_str()),
                Some("feature")
            );
        }

        #[test]
        fn parse_without_trailing_newline() {
            // Some git versions might not include trailing blank line
            let output = "worktree /path/to/repo\n\
                          HEAD abc123def4567890abc123def4567890abc12345\n\
                          branch refs/heads/main";

            let entries = parse_worktree_list_porcelain(output).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].path, PathBuf::from("/path/to/repo"));
        }

        #[test]
        fn worktree_entry_has_branch() {
            let entry = WorktreeEntry {
                path: PathBuf::from("/test"),
                head: None,
                branch: Some(BranchName::new("feature").unwrap()),
                is_bare: false,
            };

            assert!(entry.has_branch(&BranchName::new("feature").unwrap()));
            assert!(!entry.has_branch(&BranchName::new("other").unwrap()));
        }

        #[test]
        fn worktree_entry_has_branch_none() {
            let entry = WorktreeEntry {
                path: PathBuf::from("/test"),
                head: None,
                branch: None, // Detached
                is_bare: false,
            };

            assert!(!entry.has_branch(&BranchName::new("any").unwrap()));
        }
    }
}
