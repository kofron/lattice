//! core::paths
//!
//! Centralized path routing for Lattice storage locations.
//!
//! # Architecture
//!
//! Per SPEC.md Section 4.6.3, all Lattice storage locations must be routed
//! through a centralized helper to ensure correct handling of:
//! - Normal repositories (git_dir == common_dir)
//! - Linked worktrees (common_dir is the parent repo's git dir)
//! - Bare repositories (no work_dir)
//!
//! **Hard rule:** No code may assume `.git/` is a directory or that
//! `git_dir == common_dir`. All paths must go through `LatticePaths`.
//!
//! # Storage Layout
//!
//! All Lattice data is stored under `<common_dir>/lattice/`:
//! - `config.toml` - Repository configuration
//! - `lock` - Exclusive lock file
//! - `op-state.json` - Current operation marker
//! - `ops/` - Operation journals
//! - `cache/` - Optional cached data
//!
//! # Example
//!
//! ```
//! use latticework::core::paths::LatticePaths;
//! use std::path::PathBuf;
//!
//! let paths = LatticePaths::new(
//!     PathBuf::from("/repo/.git"),
//!     PathBuf::from("/repo/.git"),
//! );
//!
//! assert_eq!(
//!     paths.repo_config_path(),
//!     PathBuf::from("/repo/.git/lattice/config.toml")
//! );
//! ```

use std::path::{Path, PathBuf};

use crate::git::RepoInfo;

/// Centralized path routing for Lattice storage.
///
/// This struct ensures all Lattice storage locations are computed
/// consistently, using `common_dir` for repo-scoped storage.
///
/// # Invariants
///
/// - All repo-scoped storage uses `common_dir` (shared across worktrees)
/// - `git_dir` is only used for worktree-specific state (if any)
/// - No code outside this module should compute `*.join("lattice")` paths
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatticePaths {
    /// Path to the per-worktree .git directory.
    /// For normal repos, this equals common_dir.
    /// For linked worktrees, this is `.git/worktrees/<name>/`.
    pub git_dir: PathBuf,

    /// Path to the shared git directory (refs, objects, config).
    /// For normal repos, this equals git_dir.
    /// For linked worktrees, this is the parent repo's git dir.
    pub common_dir: PathBuf,
}

impl LatticePaths {
    /// Create a new LatticePaths from git_dir and common_dir.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::core::paths::LatticePaths;
    /// use std::path::PathBuf;
    ///
    /// // Normal repository
    /// let paths = LatticePaths::new(
    ///     PathBuf::from("/repo/.git"),
    ///     PathBuf::from("/repo/.git"),
    /// );
    /// assert_eq!(paths.git_dir, paths.common_dir);
    ///
    /// // Linked worktree
    /// let paths = LatticePaths::new(
    ///     PathBuf::from("/repo/.git/worktrees/feature"),
    ///     PathBuf::from("/repo/.git"),
    /// );
    /// assert_ne!(paths.git_dir, paths.common_dir);
    /// ```
    pub fn new(git_dir: PathBuf, common_dir: PathBuf) -> Self {
        Self {
            git_dir,
            common_dir,
        }
    }

    /// Create LatticePaths from a RepoInfo.
    ///
    /// This is the preferred way to create LatticePaths after scanning.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let git = Git::open(Path::new("."))?;
    /// let info = git.info()?;
    /// let paths = LatticePaths::from_repo_info(&info);
    /// ```
    pub fn from_repo_info(info: &RepoInfo) -> Self {
        Self {
            git_dir: info.git_dir.clone(),
            common_dir: info.common_dir.clone(),
        }
    }

    // =========================================================================
    // Repo-scoped paths (shared across worktrees)
    // =========================================================================

    /// Get the root Lattice directory under common_dir.
    ///
    /// All Lattice data is stored under this directory.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::core::paths::LatticePaths;
    /// use std::path::PathBuf;
    ///
    /// let paths = LatticePaths::new(
    ///     PathBuf::from("/repo/.git"),
    ///     PathBuf::from("/repo/.git"),
    /// );
    /// assert_eq!(paths.repo_lattice_dir(), PathBuf::from("/repo/.git/lattice"));
    /// ```
    pub fn repo_lattice_dir(&self) -> PathBuf {
        self.common_dir.join("lattice")
    }

    /// Get the path to the repository configuration file.
    ///
    /// This is `<common_dir>/lattice/config.toml`.
    pub fn repo_config_path(&self) -> PathBuf {
        self.repo_lattice_dir().join("config.toml")
    }

    /// Get the path to the repository lock file.
    ///
    /// This is `<common_dir>/lattice/lock`.
    pub fn repo_lock_path(&self) -> PathBuf {
        self.repo_lattice_dir().join("lock")
    }

    /// Get the path to the operation state marker.
    ///
    /// This is `<common_dir>/lattice/op-state.json`.
    pub fn repo_op_state_path(&self) -> PathBuf {
        self.repo_lattice_dir().join("op-state.json")
    }

    /// Get the directory for operation journals.
    ///
    /// This is `<common_dir>/lattice/ops/`.
    pub fn repo_ops_dir(&self) -> PathBuf {
        self.repo_lattice_dir().join("ops")
    }

    /// Get the path to a specific operation journal.
    ///
    /// This is `<common_dir>/lattice/ops/<op_id>.json`.
    pub fn repo_op_journal_path(&self, op_id: &str) -> PathBuf {
        self.repo_ops_dir().join(format!("{}.json", op_id))
    }

    /// Get the directory for cached data.
    ///
    /// This is `<common_dir>/lattice/cache/`.
    pub fn repo_cache_dir(&self) -> PathBuf {
        self.repo_lattice_dir().join("cache")
    }

    // =========================================================================
    // Helpers
    // =========================================================================

    /// Check if this is a linked worktree (common_dir != git_dir).
    pub fn is_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }

    /// Get the common_dir as a Path reference.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// Get the git_dir as a Path reference.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Ensure the Lattice directory structure exists.
    ///
    /// Creates `<common_dir>/lattice/` and `<common_dir>/lattice/ops/` if needed.
    ///
    /// # Errors
    ///
    /// Returns an IO error if directory creation fails.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.repo_lattice_dir())?;
        std::fs::create_dir_all(self.repo_ops_dir())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_paths() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(paths.git_dir, PathBuf::from("/repo/.git"));
        assert_eq!(paths.common_dir, PathBuf::from("/repo/.git"));
    }

    #[test]
    fn normal_repo_git_dir_equals_common_dir() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert!(!paths.is_worktree());
    }

    #[test]
    fn worktree_git_dir_differs_from_common_dir() {
        let paths = LatticePaths::new(
            PathBuf::from("/repo/.git/worktrees/feature"),
            PathBuf::from("/repo/.git"),
        );
        assert!(paths.is_worktree());
    }

    #[test]
    fn repo_lattice_dir() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(
            paths.repo_lattice_dir(),
            PathBuf::from("/repo/.git/lattice")
        );
    }

    #[test]
    fn repo_config_path() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(
            paths.repo_config_path(),
            PathBuf::from("/repo/.git/lattice/config.toml")
        );
    }

    #[test]
    fn repo_lock_path() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(
            paths.repo_lock_path(),
            PathBuf::from("/repo/.git/lattice/lock")
        );
    }

    #[test]
    fn repo_op_state_path() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(
            paths.repo_op_state_path(),
            PathBuf::from("/repo/.git/lattice/op-state.json")
        );
    }

    #[test]
    fn repo_ops_dir() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(
            paths.repo_ops_dir(),
            PathBuf::from("/repo/.git/lattice/ops")
        );
    }

    #[test]
    fn repo_op_journal_path() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(
            paths.repo_op_journal_path("abc123"),
            PathBuf::from("/repo/.git/lattice/ops/abc123.json")
        );
    }

    #[test]
    fn repo_cache_dir() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(
            paths.repo_cache_dir(),
            PathBuf::from("/repo/.git/lattice/cache")
        );
    }

    #[test]
    fn worktree_paths_use_common_dir() {
        // For a linked worktree, all repo-scoped paths should use common_dir
        let paths = LatticePaths::new(
            PathBuf::from("/repo/.git/worktrees/feature"),
            PathBuf::from("/repo/.git"),
        );

        // All paths should go to the parent repo's git dir
        assert_eq!(
            paths.repo_lattice_dir(),
            PathBuf::from("/repo/.git/lattice")
        );
        assert_eq!(
            paths.repo_config_path(),
            PathBuf::from("/repo/.git/lattice/config.toml")
        );
        assert_eq!(
            paths.repo_lock_path(),
            PathBuf::from("/repo/.git/lattice/lock")
        );
        assert_eq!(
            paths.repo_op_state_path(),
            PathBuf::from("/repo/.git/lattice/op-state.json")
        );
    }

    #[test]
    fn from_repo_info() {
        use crate::git::RepoContext;

        let info = RepoInfo {
            git_dir: PathBuf::from("/repo/.git"),
            common_dir: PathBuf::from("/repo/.git"),
            work_dir: Some(PathBuf::from("/repo")),
            context: RepoContext::Normal,
        };

        let paths = LatticePaths::from_repo_info(&info);
        assert_eq!(paths.git_dir, info.git_dir);
        assert_eq!(paths.common_dir, info.common_dir);
    }

    #[test]
    fn path_accessors() {
        let paths = LatticePaths::new(PathBuf::from("/repo/.git"), PathBuf::from("/repo/.git"));
        assert_eq!(paths.git_dir(), Path::new("/repo/.git"));
        assert_eq!(paths.common_dir(), Path::new("/repo/.git"));
    }
}
