//! core::ops::lock
//!
//! Exclusive repository lock for Lattice operations.
//!
//! # Architecture
//!
//! The repo lock ensures only one Lattice operation can mutate the repository
//! at a time. This prevents race conditions and data corruption when multiple
//! Lattice processes attempt concurrent mutations.
//!
//! Per SPEC.md Section 4.6.4, the lock is **repo-scoped** (not worktree-scoped).
//! This means the lock is acquired at `<common_dir>/lattice/lock`, which is
//! shared across all worktrees in a repository.
//!
//! # Storage
//!
//! - `<common_dir>/lattice/lock` - Lock file with OS-level exclusive lock
//!
//! # Invariants
//!
//! - Lock must be held for entire plan execution
//! - Lock is automatically released on drop (RAII pattern)
//! - Lock acquisition is non-blocking (fails fast if locked)
//! - Lock is shared across all worktrees (single-writer per repository)
//!
//! # Example
//!
//! ```ignore
//! use latticework::core::ops::lock::RepoLock;
//! use latticework::core::paths::LatticePaths;
//! use std::path::PathBuf;
//!
//! let paths = LatticePaths::new(
//!     PathBuf::from("/repo/.git"),
//!     PathBuf::from("/repo/.git"),
//! );
//! let lock = RepoLock::acquire(&paths)?;
//!
//! // Perform operations while holding lock
//! // ...
//!
//! // Lock automatically released when dropped
//! drop(lock);
//! ```

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use thiserror::Error;

use crate::core::paths::LatticePaths;

/// Errors from locking operations.
#[derive(Debug, Error)]
pub enum LockError {
    /// Another process already holds the lock.
    #[error("repository is locked by another Lattice process")]
    AlreadyLocked,

    /// Failed to create lock file or directory.
    #[error("failed to create lock: {0}")]
    CreateFailed(String),

    /// Failed to acquire the OS lock.
    #[error("failed to acquire lock: {0}")]
    AcquireFailed(String),

    /// Failed to release the lock.
    #[error("failed to release lock: {0}")]
    ReleaseFailed(String),

    /// I/O error during lock operations.
    #[error("lock i/o error: {0}")]
    IoError(#[from] std::io::Error),
}

/// An exclusive lock on the repository.
///
/// The lock is automatically released when this guard is dropped (RAII pattern).
/// This ensures the lock is always released, even if the operation panics.
///
/// # Example
///
/// ```ignore
/// use latticework::core::ops::lock::RepoLock;
///
/// let lock = RepoLock::acquire(git_dir)?;
/// assert!(lock.is_held());
///
/// // Lock is released when `lock` goes out of scope
/// ```
#[derive(Debug)]
pub struct RepoLock {
    /// Path to the lock file.
    path: PathBuf,
    /// The open file handle with the lock held.
    /// When this is Some, we hold the lock.
    file: Option<File>,
}

impl RepoLock {
    /// Attempt to acquire the repository lock.
    ///
    /// This uses OS-level file locking via `fs2`, which works across
    /// processes. The lock is non-blocking - if another process holds
    /// the lock, this returns `LockError::AlreadyLocked` immediately.
    ///
    /// The lock is repo-scoped: it uses `paths.common_dir` which is shared
    /// across all worktrees. This ensures only one Lattice operation can
    /// mutate the repository at a time, regardless of which worktree
    /// initiated the operation.
    ///
    /// # Arguments
    ///
    /// * `paths` - LatticePaths for the repository
    ///
    /// # Errors
    ///
    /// - [`LockError::AlreadyLocked`] if another process holds the lock
    /// - [`LockError::CreateFailed`] if the lock file cannot be created
    /// - [`LockError::AcquireFailed`] if the OS lock cannot be acquired
    ///
    /// # Example
    ///
    /// ```ignore
    /// let paths = LatticePaths::from_repo_info(&info);
    /// let lock = RepoLock::acquire(&paths)?;
    /// // ... perform operations ...
    /// // lock released on drop
    /// ```
    pub fn acquire(paths: &LatticePaths) -> Result<Self, LockError> {
        // Create <common_dir>/lattice directory if it doesn't exist
        let lattice_dir = paths.repo_lattice_dir();
        fs::create_dir_all(&lattice_dir).map_err(|e| {
            LockError::CreateFailed(format!("cannot create {}: {}", lattice_dir.display(), e))
        })?;

        let path = paths.repo_lock_path();

        // Open or create the lock file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| {
                LockError::CreateFailed(format!("cannot open {}: {}", path.display(), e))
            })?;

        // Try to acquire an exclusive lock (non-blocking)
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self {
                path,
                file: Some(file),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Err(LockError::AlreadyLocked),
            Err(e) => Err(LockError::AcquireFailed(e.to_string())),
        }
    }

    /// Attempt to acquire the repository lock using a path directly.
    ///
    /// This is a convenience method for cases where only the common_dir
    /// is available. Prefer `acquire(&LatticePaths)` when possible.
    ///
    /// # Arguments
    ///
    /// * `common_dir` - Path to the common git directory
    pub fn acquire_at(common_dir: &Path) -> Result<Self, LockError> {
        let paths = LatticePaths::new(common_dir.to_path_buf(), common_dir.to_path_buf());
        Self::acquire(&paths)
    }

    /// Try to acquire the lock, returning None if already held.
    ///
    /// This is a convenience method that converts `AlreadyLocked` to `None`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(lock) = RepoLock::try_acquire(&paths)? {
    ///     // We got the lock
    /// } else {
    ///     // Another process has it
    /// }
    /// ```
    pub fn try_acquire(paths: &LatticePaths) -> Result<Option<Self>, LockError> {
        match Self::acquire(paths) {
            Ok(lock) => Ok(Some(lock)),
            Err(LockError::AlreadyLocked) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Check if the lock is currently held.
    ///
    /// Returns `true` if this guard still holds the lock.
    pub fn is_held(&self) -> bool {
        self.file.is_some()
    }

    /// Get the path to the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Release the lock explicitly.
    ///
    /// This is called automatically on drop, but can be called early
    /// if you need to release the lock before the guard goes out of scope.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut lock = RepoLock::acquire(git_dir)?;
    /// // ... do work ...
    /// lock.release()?;  // Explicit early release
    /// ```
    pub fn release(&mut self) -> Result<(), LockError> {
        if let Some(file) = self.file.take() {
            file.unlock()
                .map_err(|e| LockError::ReleaseFailed(e.to_string()))?;
        }
        Ok(())
    }
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        // Best-effort release on drop - ignore errors since we're dropping
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a temporary git directory structure for testing.
    fn create_test_git_dir() -> TempDir {
        // The lock will create .git/lattice if needed
        TempDir::new().expect("create temp dir")
    }

    /// Create LatticePaths for a test directory (simulating a normal repo).
    fn test_paths(dir: &Path) -> LatticePaths {
        LatticePaths::new(dir.to_path_buf(), dir.to_path_buf())
    }

    #[test]
    fn lock_acquire_succeeds() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let lock = RepoLock::acquire(&paths).expect("acquire lock");
        assert!(lock.is_held());
        assert!(lock.path().exists());
    }

    #[test]
    fn lock_creates_lattice_directory() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let lattice_dir = paths.repo_lattice_dir();
        assert!(!lattice_dir.exists());

        let _lock = RepoLock::acquire(&paths).expect("acquire lock");
        assert!(lattice_dir.exists());
    }

    #[test]
    fn lock_prevents_second_acquire() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let lock1 = RepoLock::acquire(&paths).expect("first acquire");
        assert!(lock1.is_held());

        // Second acquire should fail
        let result = RepoLock::acquire(&paths);
        assert!(matches!(result, Err(LockError::AlreadyLocked)));
    }

    #[test]
    fn lock_released_on_drop() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        {
            let lock = RepoLock::acquire(&paths).expect("first acquire");
            assert!(lock.is_held());
            // lock dropped here
        }

        // Should be able to acquire again
        let lock2 = RepoLock::acquire(&paths).expect("second acquire");
        assert!(lock2.is_held());
    }

    #[test]
    fn lock_released_explicitly() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let mut lock = RepoLock::acquire(&paths).expect("acquire");
        assert!(lock.is_held());

        lock.release().expect("release");
        assert!(!lock.is_held());

        // Should be able to acquire again
        let lock2 = RepoLock::acquire(&paths).expect("reacquire");
        assert!(lock2.is_held());
    }

    #[test]
    fn try_acquire_returns_none_when_locked() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let _lock1 = RepoLock::acquire(&paths).expect("first acquire");

        let result = RepoLock::try_acquire(&paths).expect("try_acquire");
        assert!(result.is_none());
    }

    #[test]
    fn try_acquire_returns_lock_when_available() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let lock = RepoLock::try_acquire(&paths)
            .expect("try_acquire")
            .expect("should get lock");
        assert!(lock.is_held());
    }

    #[test]
    fn multiple_release_calls_are_safe() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let mut lock = RepoLock::acquire(&paths).expect("acquire");

        lock.release().expect("first release");
        lock.release().expect("second release should be ok");
        assert!(!lock.is_held());
    }

    #[test]
    fn lock_path_is_correct() {
        let temp = create_test_git_dir();
        let paths = test_paths(temp.path());

        let lock = RepoLock::acquire(&paths).expect("acquire");
        let expected = paths.repo_lock_path();
        assert_eq!(lock.path(), expected);
    }

    #[test]
    fn acquire_at_convenience_method() {
        let temp = create_test_git_dir();
        let common_dir = temp.path();

        let lock = RepoLock::acquire_at(common_dir).expect("acquire_at");
        assert!(lock.is_held());
        assert_eq!(lock.path(), common_dir.join("lattice").join("lock"));
    }

    #[test]
    fn worktree_shares_lock_with_parent() {
        // Simulate a worktree scenario where git_dir != common_dir
        // Both should lock the same file (in common_dir)
        let temp = create_test_git_dir();
        let common_dir = temp.path().to_path_buf();
        let worktree_git_dir = common_dir.join("worktrees").join("feature");

        // Create paths for "main" repo and "worktree"
        let main_paths = LatticePaths::new(common_dir.clone(), common_dir.clone());
        let worktree_paths = LatticePaths::new(worktree_git_dir, common_dir.clone());

        // Lock from main
        let lock1 = RepoLock::acquire(&main_paths).expect("acquire from main");
        assert!(lock1.is_held());

        // Worktree should see the same lock
        let result = RepoLock::acquire(&worktree_paths);
        assert!(matches!(result, Err(LockError::AlreadyLocked)));
    }

    #[test]
    fn error_display_formatting() {
        let err = LockError::AlreadyLocked;
        assert!(err.to_string().contains("locked"));

        let err = LockError::CreateFailed("test".into());
        assert!(err.to_string().contains("create"));

        let err = LockError::AcquireFailed("test".into());
        assert!(err.to_string().contains("acquire"));

        let err = LockError::ReleaseFailed("test".into());
        assert!(err.to_string().contains("release"));
    }
}
