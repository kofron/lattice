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
//! # Storage
//!
//! - `.git/lattice/lock` - Lock file with OS-level exclusive lock
//!
//! # Invariants
//!
//! - Lock must be held for entire plan execution
//! - Lock is automatically released on drop (RAII pattern)
//! - Lock acquisition is non-blocking (fails fast if locked)
//!
//! # Example
//!
//! ```ignore
//! use latticework::core::ops::lock::RepoLock;
//! use std::path::Path;
//!
//! let git_dir = Path::new(".git");
//! let lock = RepoLock::acquire(git_dir)?;
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
    /// # Arguments
    ///
    /// * `git_dir` - Path to the `.git` directory
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
    /// let lock = RepoLock::acquire(Path::new(".git"))?;
    /// // ... perform operations ...
    /// // lock released on drop
    /// ```
    pub fn acquire(git_dir: &Path) -> Result<Self, LockError> {
        // Create .git/lattice directory if it doesn't exist
        let lattice_dir = git_dir.join("lattice");
        fs::create_dir_all(&lattice_dir).map_err(|e| {
            LockError::CreateFailed(format!("cannot create {}: {}", lattice_dir.display(), e))
        })?;

        let path = lattice_dir.join("lock");

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

    /// Try to acquire the lock, returning None if already held.
    ///
    /// This is a convenience method that converts `AlreadyLocked` to `None`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(lock) = RepoLock::try_acquire(git_dir)? {
    ///     // We got the lock
    /// } else {
    ///     // Another process has it
    /// }
    /// ```
    pub fn try_acquire(git_dir: &Path) -> Result<Option<Self>, LockError> {
        match Self::acquire(git_dir) {
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

    #[test]
    fn lock_acquire_succeeds() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let lock = RepoLock::acquire(git_dir).expect("acquire lock");
        assert!(lock.is_held());
        assert!(lock.path().exists());
    }

    #[test]
    fn lock_creates_lattice_directory() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let lattice_dir = git_dir.join("lattice");
        assert!(!lattice_dir.exists());

        let _lock = RepoLock::acquire(git_dir).expect("acquire lock");
        assert!(lattice_dir.exists());
    }

    #[test]
    fn lock_prevents_second_acquire() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let lock1 = RepoLock::acquire(git_dir).expect("first acquire");
        assert!(lock1.is_held());

        // Second acquire should fail
        let result = RepoLock::acquire(git_dir);
        assert!(matches!(result, Err(LockError::AlreadyLocked)));
    }

    #[test]
    fn lock_released_on_drop() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        {
            let lock = RepoLock::acquire(git_dir).expect("first acquire");
            assert!(lock.is_held());
            // lock dropped here
        }

        // Should be able to acquire again
        let lock2 = RepoLock::acquire(git_dir).expect("second acquire");
        assert!(lock2.is_held());
    }

    #[test]
    fn lock_released_explicitly() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let mut lock = RepoLock::acquire(git_dir).expect("acquire");
        assert!(lock.is_held());

        lock.release().expect("release");
        assert!(!lock.is_held());

        // Should be able to acquire again
        let lock2 = RepoLock::acquire(git_dir).expect("reacquire");
        assert!(lock2.is_held());
    }

    #[test]
    fn try_acquire_returns_none_when_locked() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let _lock1 = RepoLock::acquire(git_dir).expect("first acquire");

        let result = RepoLock::try_acquire(git_dir).expect("try_acquire");
        assert!(result.is_none());
    }

    #[test]
    fn try_acquire_returns_lock_when_available() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let lock = RepoLock::try_acquire(git_dir)
            .expect("try_acquire")
            .expect("should get lock");
        assert!(lock.is_held());
    }

    #[test]
    fn multiple_release_calls_are_safe() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let mut lock = RepoLock::acquire(git_dir).expect("acquire");

        lock.release().expect("first release");
        lock.release().expect("second release should be ok");
        assert!(!lock.is_held());
    }

    #[test]
    fn lock_path_is_correct() {
        let temp = create_test_git_dir();
        let git_dir = temp.path();

        let lock = RepoLock::acquire(git_dir).expect("acquire");
        let expected = git_dir.join("lattice").join("lock");
        assert_eq!(lock.path(), expected);
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
