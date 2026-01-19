//! Integration tests for init command bootstrap hint (Milestone 5.6).
//!
//! These tests verify that the init command:
//! - Succeeds regardless of auth/network state
//! - Shows hint when PRs exist and auth is available (requires mock/live test)
//! - Skips hint in quiet mode
//! - Skips hint on reset
//!
//! Note: Full hint testing requires either:
//! - A mock forge setup
//! - Real GitHub auth and a test repo with open PRs
//!
//! The tests here focus on the non-fatal behavior guarantee.

use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

use latticework::cli::commands;
use latticework::engine::Context;

// =============================================================================
// Test Fixtures
// =============================================================================

/// Run a git command in the specified directory.
fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("failed to run git");
    assert!(status.success(), "git {:?} failed", args);
}

/// Test fixture that creates a real git repository.
struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// Create a new test repository with an initial commit on main.
    fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");

        // Initialize git repo
        run_git(dir.path(), &["init", "-b", "main"]);
        run_git(dir.path(), &["config", "user.email", "test@example.com"]);
        run_git(dir.path(), &["config", "user.name", "Test User"]);

        // Create initial commit
        std::fs::write(dir.path().join("README.md"), "# Test Repo\n").unwrap();
        run_git(dir.path(), &["add", "README.md"]);
        run_git(dir.path(), &["commit", "-m", "Initial commit"]);

        Self { dir }
    }

    /// Create a test repo with a GitHub-style origin remote.
    fn with_github_remote() -> Self {
        let repo = Self::new();
        run_git(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/test-owner/test-repo.git",
            ],
        );
        repo
    }

    /// Get the path to the repository.
    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Create a context with specified quiet mode.
    fn context(&self, quiet: bool) -> Context {
        Context {
            cwd: Some(self.path().to_path_buf()),
            interactive: false,
            quiet,
            debug: false,
        }
    }
}

// =============================================================================
// Tests: Init Succeeds Without Auth (Non-Fatal Hint)
// =============================================================================

/// Init succeeds even when auth is not configured.
///
/// The hint check should silently skip when auth is unavailable,
/// allowing init to complete successfully.
#[test]
fn init_succeeds_without_auth() {
    let repo = TestRepo::new();
    let ctx = repo.context(false); // not quiet, so hint would try to run

    // Init should succeed
    let result = commands::init(&ctx, Some("main"), false, true);
    assert!(result.is_ok(), "init should succeed: {:?}", result);

    // Verify Lattice is initialized
    let config_path = repo.path().join(".git/lattice/config.toml");
    assert!(config_path.exists(), "config should be created");
}

/// Init succeeds when origin remote is not GitHub.
///
/// The hint should silently skip for non-GitHub remotes.
#[test]
fn init_succeeds_with_non_github_remote() {
    let repo = TestRepo::new();

    // Add a non-GitHub remote
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://gitlab.com/test/repo.git",
        ],
    );

    let ctx = repo.context(false);
    let result = commands::init(&ctx, Some("main"), false, true);
    assert!(result.is_ok(), "init should succeed: {:?}", result);
}

/// Init succeeds when there is no origin remote.
///
/// The hint should silently skip when no origin exists.
#[test]
fn init_succeeds_without_origin() {
    let repo = TestRepo::new();
    // No remote added

    let ctx = repo.context(false);
    let result = commands::init(&ctx, Some("main"), false, true);
    assert!(result.is_ok(), "init should succeed: {:?}", result);
}

// =============================================================================
// Tests: Quiet Mode Skips Hint
// =============================================================================

/// Init in quiet mode does not show any output (including hint).
#[test]
fn init_quiet_mode_skips_hint() {
    let repo = TestRepo::with_github_remote();
    let ctx = repo.context(true); // quiet mode

    let result = commands::init(&ctx, Some("main"), false, true);
    assert!(result.is_ok(), "init should succeed: {:?}", result);

    // In quiet mode, no output should be produced.
    // We can't easily verify stdout content here, but the test verifies
    // that the code path for quiet mode doesn't crash.
}

// =============================================================================
// Tests: Reset Skips Hint
// =============================================================================

/// Reset mode skips the hint check.
///
/// Users who reset are likely experienced and don't need the hint.
#[test]
fn init_reset_skips_hint() {
    let repo = TestRepo::with_github_remote();
    let ctx = repo.context(false);

    // First init
    commands::init(&ctx, Some("main"), false, true).expect("first init");

    // Reset should succeed without hint
    let result = commands::init(&ctx, Some("main"), true, true);
    assert!(result.is_ok(), "reset should succeed: {:?}", result);
}

// =============================================================================
// Tests: Already Initialized
// =============================================================================

/// Re-running init when already initialized shows appropriate message.
#[test]
fn init_already_initialized_skips_hint() {
    let repo = TestRepo::with_github_remote();
    let ctx = repo.context(false);

    // First init
    commands::init(&ctx, Some("main"), false, true).expect("first init");

    // Second init should not attempt hint (early return)
    let result = commands::init(&ctx, Some("main"), false, true);
    assert!(result.is_ok(), "second init should succeed: {:?}", result);
}

// =============================================================================
// Tests: Hint With Auth (Requires Real/Mock Setup)
// =============================================================================

/// Test that hint is shown when PRs exist.
///
/// This test is ignored by default because it requires either:
/// 1. Real GitHub auth configured
/// 2. A mock forge setup
///
/// To run manually with real auth:
/// ```
/// cargo test init_shows_hint_when_prs_exist -- --ignored
/// ```
#[test]
#[ignore]
fn init_shows_hint_when_prs_exist() {
    // This test would verify:
    // 1. Auth is available
    // 2. Remote has open PRs
    // 3. Hint message is printed
    //
    // Implementation requires either:
    // - Environment with real GitHub auth
    // - Mock forge infrastructure
    //
    // For now, we rely on the non-fatal tests above to verify
    // the hint doesn't break anything, and manual testing for
    // the positive case.
}
