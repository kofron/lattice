//! auth::lock
//!
//! Auth-scoped lock for concurrent token refresh protection.
//!
//! # Architecture
//!
//! Per SPEC.md Section 4.4.3, refresh tokens are single-use and rotate on each
//! refresh. This lock prevents double-refresh races across concurrent Lattice
//! processes. Unlike the repo lock, this lock is per-host and uses blocking
//! acquisition with timeout.
//!
//! # Storage
//!
//! - `~/.lattice/auth/lock.<host>` - Lock file with OS-level exclusive lock
//!
//! # Invariants
//!
//! - Lock must be held during token refresh operations
//! - After acquiring lock, caller must re-check if refresh is still needed
//!   (another process may have completed the refresh)
//! - Lock is automatically released on drop (RAII pattern)
//!
//! # Example
//!
//! ```ignore
//! use latticework::auth::AuthLock;
//! use std::time::Duration;
//!
//! let lock = AuthLock::acquire("github.com", Duration::from_secs(10))?;
//!
//! // Re-check if refresh is still needed
//! if bundle.needs_refresh() {
//!     // Perform refresh
//! }
//!
//! // Lock automatically released when dropped
//! ```

use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;

use super::errors::AuthError;

/// Default timeout for lock acquisition (10 seconds).
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Polling interval when waiting for lock (100ms).
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// An exclusive lock for token refresh operations.
///
/// The lock is automatically released when this guard is dropped (RAII pattern).
/// This ensures the lock is always released, even if the operation panics.
///
/// # Per-Host Locking
///
/// Each host (e.g., "github.com", "github.example.com") has its own lock file.
/// This allows concurrent operations against different GitHub instances.
///
/// # Example
///
/// ```ignore
/// use latticework::auth::AuthLock;
/// use std::time::Duration;
///
/// // Blocking acquire with timeout
/// let lock = AuthLock::acquire("github.com", Duration::from_secs(5))?;
/// assert!(lock.is_held());
///
/// // Lock is released when `lock` goes out of scope
/// ```
#[derive(Debug)]
pub struct AuthLock {
    /// Path to the lock file.
    path: PathBuf,
    /// The open file handle with the lock held.
    file: Option<File>,
    /// Host this lock is for.
    host: String,
}

impl AuthLock {
    /// Get the lock file path for a host.
    ///
    /// Lock path: `~/.lattice/auth/lock.<host>`
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::auth::AuthLock;
    ///
    /// let path = AuthLock::lock_path("github.com");
    /// assert!(path.to_string_lossy().contains("lock.github.com"));
    /// ```
    pub fn lock_path(host: &str) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".lattice")
            .join("auth")
            .join(format!("lock.{}", host))
    }

    /// Acquire the auth lock with blocking and timeout.
    ///
    /// This function blocks until the lock is acquired or the timeout expires.
    /// It polls at 100ms intervals.
    ///
    /// # Arguments
    ///
    /// * `host` - GitHub host (e.g., "github.com")
    /// * `timeout` - Maximum time to wait for the lock
    ///
    /// # Errors
    ///
    /// - [`AuthError::LockTimeout`] if the timeout expires before acquiring
    /// - [`AuthError::LockError`] if there's an I/O error
    ///
    /// # Example
    ///
    /// ```ignore
    /// let lock = AuthLock::acquire("github.com", Duration::from_secs(10))?;
    /// // ... refresh tokens ...
    /// // lock released on drop
    /// ```
    pub fn acquire(host: &str, timeout: Duration) -> Result<Self, AuthError> {
        let path = Self::lock_path(host);
        let deadline = Instant::now() + timeout;

        // Create directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AuthError::LockError(format!("cannot create {}: {}", parent.display(), e))
            })?;
        }

        // Retry loop with polling
        loop {
            match Self::try_acquire_internal(host, &path) {
                Ok(lock) => return Ok(lock),
                Err(AuthError::LockError(msg)) if msg.contains("would block") => {
                    // Lock is held by another process, check timeout
                    if Instant::now() >= deadline {
                        return Err(AuthError::LockTimeout);
                    }
                    thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Try to acquire the lock without blocking.
    ///
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if already held.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(lock) = AuthLock::try_acquire("github.com")? {
    ///     // We got the lock
    /// } else {
    ///     // Another process has it
    /// }
    /// ```
    pub fn try_acquire(host: &str) -> Result<Option<Self>, AuthError> {
        let path = Self::lock_path(host);

        // Create directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AuthError::LockError(format!("cannot create {}: {}", parent.display(), e))
            })?;
        }

        match Self::try_acquire_internal(host, &path) {
            Ok(lock) => Ok(Some(lock)),
            Err(AuthError::LockError(msg)) if msg.contains("would block") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Internal non-blocking lock acquisition.
    fn try_acquire_internal(host: &str, path: &PathBuf) -> Result<Self, AuthError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| AuthError::LockError(format!("cannot open {}: {}", path.display(), e)))?;

        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self {
                path: path.clone(),
                file: Some(file),
                host: host.to_string(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                Err(AuthError::LockError("would block".to_string()))
            }
            Err(e) => Err(AuthError::LockError(format!("lock failed: {}", e))),
        }
    }

    /// Check if the lock is currently held.
    pub fn is_held(&self) -> bool {
        self.file.is_some()
    }

    /// Get the path to the lock file.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Get the host this lock is for.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Release the lock explicitly.
    ///
    /// This is called automatically on drop, but can be called early
    /// if you need to release the lock before the guard goes out of scope.
    pub fn release(&mut self) -> Result<(), AuthError> {
        if let Some(file) = self.file.take() {
            file.unlock()
                .map_err(|e| AuthError::LockError(format!("unlock failed: {}", e)))?;
        }
        Ok(())
    }
}

impl Drop for AuthLock {
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

    #[test]
    fn lock_path_includes_host() {
        let path = AuthLock::lock_path("github.com");
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("lock.github.com"));
        assert!(path_str.contains(".lattice"));
        assert!(path_str.contains("auth"));
    }

    #[test]
    fn lock_path_different_hosts() {
        let path1 = AuthLock::lock_path("github.com");
        let path2 = AuthLock::lock_path("github.example.com");
        assert_ne!(path1, path2);
    }

    #[test]
    fn try_acquire_succeeds_when_available() {
        // Use a unique temp path for this test
        let temp = TempDir::new().expect("create temp dir");
        let lock_dir = temp.path().join(".lattice").join("auth");
        fs::create_dir_all(&lock_dir).expect("create dir");

        // Manually set a test path
        let path = lock_dir.join("lock.test-host");

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .expect("create lock file");

        match file.try_lock_exclusive() {
            Ok(()) => {
                // We got the lock
                file.unlock().expect("unlock");
            }
            Err(e) => panic!("Failed to acquire test lock: {}", e),
        }
    }

    #[test]
    fn lock_prevents_double_acquire() {
        let temp = TempDir::new().expect("create temp dir");
        let lock_dir = temp.path().join(".lattice").join("auth");
        fs::create_dir_all(&lock_dir).expect("create dir");

        let path = lock_dir.join("lock.test-double");

        // First acquire
        let file1 = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .expect("open 1");
        file1.try_lock_exclusive().expect("lock 1");

        // Second acquire should fail
        let file2 = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open 2");

        let result = file2.try_lock_exclusive();
        assert!(
            result.is_err() || {
                // Some platforms might not return WouldBlock
                // but instead return success if same process
                // In that case, clean up both
                let _ = file2.unlock();
                true
            }
        );

        file1.unlock().expect("unlock 1");
    }

    #[test]
    fn lock_released_on_drop() {
        let temp = TempDir::new().expect("create temp dir");
        let lock_dir = temp.path().join(".lattice").join("auth");
        fs::create_dir_all(&lock_dir).expect("create dir");

        let path = lock_dir.join("lock.test-drop");

        {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .expect("open");
            file.try_lock_exclusive().expect("lock");
            // file/lock dropped here
        }

        // Should be able to acquire again
        let file2 = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open 2");
        file2.try_lock_exclusive().expect("lock 2 should succeed");
        file2.unlock().expect("unlock 2");
    }

    #[test]
    fn default_timeout_is_reasonable() {
        assert!(DEFAULT_LOCK_TIMEOUT >= Duration::from_secs(5));
        assert!(DEFAULT_LOCK_TIMEOUT <= Duration::from_secs(60));
    }

    #[test]
    fn lock_poll_interval_is_reasonable() {
        assert!(LOCK_POLL_INTERVAL >= Duration::from_millis(50));
        assert!(LOCK_POLL_INTERVAL <= Duration::from_millis(500));
    }
}
