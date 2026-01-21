//! engine::engine_hooks
//!
//! Test-only hooks for fault injection and out-of-band drift testing.
//!
//! # Architecture
//!
//! Per ROADMAP.md Anti-Drift Mechanisms item 5:
//!
//! > Test-only pause hook in Engine. Enables drift harness to inject out-of-band
//! > operations **after planning, before lock acquisition** (compiled under
//! > `cfg(test)` or `fault_injection` feature).
//!
//! These hooks allow the out-of-band drift harness (`tests/oob_fuzz.rs`) to inject
//! mutations at precise points in the execution flow, verifying that the executor
//! correctly detects and handles:
//!
//! - CAS (compare-and-swap) failures when refs change between plan and execute
//! - Occupancy violations when branches are checked out in other worktrees
//! - Metadata corruption introduced between operations
//!
//! # Usage
//!
//! ```ignore
//! use latticework::engine::engine_hooks;
//!
//! // Set up hook before running a command
//! engine_hooks::set_before_execute(|info| {
//!     // info contains RepoInfo with git_dir, work_dir, etc.
//!     // Perform out-of-band git mutations here
//!     run_git(info.work_dir.as_ref().unwrap(), &["branch", "-f", "feature", "HEAD~1"]);
//! });
//!
//! // Run command - hook fires between plan and execute
//! let result = some_lattice_command();
//!
//! // Always clean up!
//! engine_hooks::clear();
//!
//! // Assert the expected behavior (CAS failure detection, etc.)
//! ```
//!
//! # Thread Safety
//!
//! Hooks are stored in thread-local storage, so they are safe to use in
//! concurrent tests as long as each test runs in its own thread (the default
//! for `cargo test`).
//!
//! # Invariants
//!
//! - Hooks are only available under `cfg(test)` or `fault_injection` feature
//! - Hooks have zero runtime cost in production builds
//! - Each test must call `clear()` to avoid polluting other tests

use std::cell::RefCell;
use std::path::PathBuf;

/// Repository information passed to hooks.
///
/// This is a simplified version of `git::RepoInfo` that doesn't require
/// importing the git module in tests.
#[derive(Debug, Clone)]
pub struct HookRepoInfo {
    /// Path to the git directory.
    pub git_dir: PathBuf,
    /// Path to the common directory (shared across worktrees).
    pub common_dir: PathBuf,
    /// Path to working directory (None for bare repos).
    pub work_dir: Option<PathBuf>,
}

impl HookRepoInfo {
    /// Create from git::RepoInfo.
    pub(crate) fn from_git_info(info: &crate::git::RepoInfo) -> Self {
        Self {
            git_dir: info.git_dir.clone(),
            common_dir: info.common_dir.clone(),
            work_dir: info.work_dir.clone(),
        }
    }
}

/// Container for engine hooks.
///
/// All fields are optional; if None, no action is taken at that hook point.
pub struct EngineHooks {
    /// Called after plan generation, before lock acquisition.
    ///
    /// This is the precise point where out-of-band changes can cause CAS
    /// failures. The plan has been generated with expected OIDs, but the
    /// lock has NOT been acquired yet. Any mutations here will be detected
    /// by the executor's CAS checks.
    pub before_execute: Option<Box<dyn Fn(&HookRepoInfo) + Send + Sync>>,
}

impl Default for EngineHooks {
    fn default() -> Self {
        Self {
            before_execute: None,
        }
    }
}

thread_local! {
    static HOOKS: RefCell<Option<EngineHooks>> = const { RefCell::new(None) };
}

/// Set a hook to run before plan execution.
///
/// The hook receives `HookRepoInfo` and can perform out-of-band mutations
/// to test CAS detection and occupancy conflict handling.
///
/// # Example
///
/// ```ignore
/// engine_hooks::set_before_execute(|info| {
///     // Modify a ref to cause CAS failure
///     let work_dir = info.work_dir.as_ref().unwrap();
///     std::process::Command::new("git")
///         .args(["branch", "-f", "feature", "HEAD~1"])
///         .current_dir(work_dir)
///         .output()
///         .expect("git command failed");
/// });
/// ```
pub fn set_before_execute<F>(f: F)
where
    F: Fn(&HookRepoInfo) + Send + Sync + 'static,
{
    HOOKS.with(|h| {
        let mut hooks = h.borrow_mut();
        if hooks.is_none() {
            *hooks = Some(EngineHooks::default());
        }
        hooks.as_mut().unwrap().before_execute = Some(Box::new(f));
    });
}

/// Clear all hooks.
///
/// **Important:** Always call this in test teardown to avoid polluting other tests.
///
/// # Example
///
/// ```ignore
/// #[test]
/// fn my_test() {
///     engine_hooks::set_before_execute(|_| { /* ... */ });
///
///     // ... test code ...
///
///     engine_hooks::clear(); // Always clean up!
/// }
/// ```
pub fn clear() {
    HOOKS.with(|h| *h.borrow_mut() = None);
}

/// Check if any hooks are currently set.
///
/// Useful for debugging or verifying test setup.
pub fn has_hooks() -> bool {
    HOOKS.with(|h| h.borrow().is_some())
}

/// Internal: invoke the before_execute hook if set.
///
/// Called by the engine runner after planning, before execution.
/// This is a no-op if no hook is set.
pub(crate) fn invoke_before_execute(info: &crate::git::RepoInfo) {
    HOOKS.with(|h| {
        if let Some(ref hooks) = *h.borrow() {
            if let Some(ref f) = hooks.before_execute {
                let hook_info = HookRepoInfo::from_git_info(info);
                f(&hook_info);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn set_and_clear_hooks() {
        assert!(!has_hooks());

        set_before_execute(|_| {});
        assert!(has_hooks());

        clear();
        assert!(!has_hooks());
    }

    #[test]
    fn hook_receives_info() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = called.clone();

        set_before_execute(move |info| {
            // Verify we received valid info
            assert!(!info.git_dir.as_os_str().is_empty());
            called_clone.store(true, Ordering::SeqCst);
        });

        // Create a mock RepoInfo
        let info = crate::git::RepoInfo {
            git_dir: PathBuf::from("/tmp/test/.git"),
            common_dir: PathBuf::from("/tmp/test/.git"),
            work_dir: Some(PathBuf::from("/tmp/test")),
            context: crate::git::RepoContext::Normal,
        };

        invoke_before_execute(&info);
        assert!(called.load(Ordering::SeqCst));

        clear();
    }

    #[test]
    fn no_hook_is_noop() {
        // Should not panic when no hook is set
        let info = crate::git::RepoInfo {
            git_dir: PathBuf::from("/tmp/test/.git"),
            common_dir: PathBuf::from("/tmp/test/.git"),
            work_dir: Some(PathBuf::from("/tmp/test")),
            context: crate::git::RepoContext::Normal,
        };

        invoke_before_execute(&info);
        // If we got here without panic, the test passes
    }

    #[test]
    fn multiple_sets_replace_hook() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let count1 = count.clone();
        set_before_execute(move |_| {
            count1.fetch_add(1, Ordering::SeqCst);
        });

        let count2 = count.clone();
        set_before_execute(move |_| {
            count2.fetch_add(10, Ordering::SeqCst);
        });

        let info = crate::git::RepoInfo {
            git_dir: PathBuf::from("/tmp/test/.git"),
            common_dir: PathBuf::from("/tmp/test/.git"),
            work_dir: Some(PathBuf::from("/tmp/test")),
            context: crate::git::RepoContext::Normal,
        };

        invoke_before_execute(&info);

        // Only the second hook should have run (added 10, not 1)
        assert_eq!(count.load(Ordering::SeqCst), 10);

        clear();
    }
}
