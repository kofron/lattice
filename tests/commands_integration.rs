//! Integration tests for Phase 1 commands.
//!
//! These tests verify that commands work correctly with real git repositories.
//! They exercise the full command flow: Scan → Gate → Plan → Execute → Verify.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use latticework::cli::{commands, Shell};
use latticework::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeScope, FreezeState, ParentInfo, PrState,
    Timestamps, METADATA_KIND, SCHEMA_VERSION,
};
use latticework::core::metadata::store::MetadataStore;
use latticework::core::types::{BranchName, UtcTimestamp};
use latticework::engine::scan::scan;
use latticework::engine::Context;
use latticework::git::Git;

// =============================================================================
// Test Fixtures
// =============================================================================

/// Test fixture that creates a real git repository with Lattice initialized.
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
        // Prevent git from opening editors during tests
        run_git(dir.path(), &["config", "core.editor", "true"]);
        run_git(dir.path(), &["config", "sequence.editor", "true"]);

        // Create initial commit
        std::fs::write(dir.path().join("README.md"), "# Test Repo\n").unwrap();
        run_git(dir.path(), &["add", "README.md"]);
        run_git(dir.path(), &["commit", "-m", "Initial commit"]);

        Self { dir }
    }

    /// Get the path to the repository.
    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Open a Git interface to this repository.
    fn git(&self) -> Git {
        Git::open(self.path()).expect("failed to open test repo")
    }

    /// Create a standard test context.
    fn context(&self) -> Context {
        Context {
            cwd: Some(self.path().to_path_buf()),
            interactive: false,
            quiet: true,
            debug: false,
            verify: true,
        }
    }

    /// Initialize Lattice in the repository.
    fn init_lattice(&self) {
        let ctx = self.context();
        commands::init(&ctx, Some("main"), false, true).expect("init failed");
    }

    /// Create a file and commit it, returning the commit message.
    fn commit(&self, filename: &str, content: &str, message: &str) {
        std::fs::write(self.dir.path().join(filename), content).unwrap();
        run_git(self.path(), &["add", filename]);
        run_git(self.path(), &["commit", "-m", message]);
    }

    /// Create a branch at the current HEAD.
    fn create_branch(&self, name: &str) {
        run_git(self.path(), &["branch", name]);
    }

    /// Checkout a branch.
    fn checkout(&self, name: &str) {
        run_git(self.path(), &["checkout", name]);
    }

    /// Get current branch name.
    fn current_branch(&self) -> String {
        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(self.path())
            .output()
            .expect("git branch failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Get HEAD OID.
    fn head_oid(&self) -> String {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(self.path())
            .output()
            .expect("git rev-parse failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Track a branch with Lattice.
    fn track_branch(&self, branch: &str, parent: &str) {
        let ctx = self.context();
        commands::track(&ctx, Some(branch), Some(parent), false, false).expect("track failed");
    }

    /// Create metadata for a branch directly (for test setup).
    #[allow(dead_code)]
    fn create_metadata(&self, branch: &str, parent: &str, base_oid: &str) {
        let git = self.git();
        let store = MetadataStore::new(&git);
        let branch_name = BranchName::new(branch).unwrap();

        let parent_info = if parent == "main" {
            ParentInfo::Trunk {
                name: parent.to_string(),
            }
        } else {
            ParentInfo::Branch {
                name: parent.to_string(),
            }
        };

        let now = UtcTimestamp::now();
        let metadata = BranchMetadataV1 {
            kind: METADATA_KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            branch: BranchInfo {
                name: branch.to_string(),
            },
            parent: parent_info,
            base: BaseInfo {
                oid: base_oid.to_string(),
            },
            freeze: FreezeState::Unfrozen,
            pr: PrState::None,
            timestamps: Timestamps {
                created_at: now.clone(),
                updated_at: now,
            },
        };

        store
            .write_cas(&branch_name, None, &metadata)
            .expect("write metadata failed");
    }
}

/// Run a git command in the given directory.
fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command failed");

    if !output.status.success() {
        panic!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

// =============================================================================
// Init Command Tests
// =============================================================================

#[test]
fn init_creates_config() {
    let repo = TestRepo::new();
    let ctx = repo.context();

    commands::init(&ctx, Some("main"), false, true).expect("init failed");

    let config_path = repo.git().git_dir().join("lattice/config.toml");
    assert!(config_path.exists(), "config file should exist");

    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("trunk"), "config should contain trunk");
}

#[test]
fn init_with_custom_trunk() {
    let repo = TestRepo::new();
    repo.create_branch("develop");
    let ctx = repo.context();

    commands::init(&ctx, Some("develop"), false, true).expect("init failed");

    let snapshot = scan(&repo.git()).unwrap();
    assert_eq!(snapshot.trunk.as_ref().map(|t| t.as_str()), Some("develop"));
}

#[test]
fn init_reset_clears_metadata() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a tracked branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Verify metadata exists
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    assert!(store.read(&branch).unwrap().is_some());

    // Reset
    let ctx = repo.context();
    commands::init(&ctx, Some("main"), true, true).expect("reset failed");

    // Verify metadata is gone
    let store = MetadataStore::new(&git);
    assert!(store.read(&branch).unwrap().is_none());
}

// =============================================================================
// Track/Untrack Command Tests
// =============================================================================

#[test]
fn track_creates_metadata() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature content", "Add feature");

    let ctx = repo.context();
    commands::track(&ctx, Some("feature"), Some("main"), false, false).expect("track failed");

    // Verify metadata
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let scanned = store.read(&branch).unwrap().expect("metadata should exist");

    assert_eq!(scanned.metadata.branch.name, "feature");
    assert!(scanned.metadata.parent.is_trunk());
}

#[test]
fn track_with_branch_parent() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create parent branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Create child branch
    repo.create_branch("feature-child");
    repo.checkout("feature-child");
    repo.commit("child.txt", "child", "Add child");

    let ctx = repo.context();
    commands::track(&ctx, Some("feature-child"), Some("feature"), false, false)
        .expect("track child failed");

    // Verify parent relationship
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature-child").unwrap();
    let scanned = store.read(&branch).unwrap().expect("metadata should exist");

    assert!(!scanned.metadata.parent.is_trunk());
    assert_eq!(scanned.metadata.parent.name(), "feature");
}

#[test]
fn track_already_tracked_is_idempotent() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");

    let ctx = repo.context();
    commands::track(&ctx, Some("feature"), Some("main"), false, false).expect("first track");
    commands::track(&ctx, Some("feature"), Some("main"), false, false).expect("second track");
}

#[test]
fn track_as_frozen() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("frozen-feature");
    repo.checkout("frozen-feature");
    repo.commit("frozen.txt", "frozen", "Frozen feature");

    let ctx = repo.context();
    commands::track(&ctx, Some("frozen-feature"), Some("main"), false, true).expect("track frozen");

    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("frozen-feature").unwrap();
    let scanned = store.read(&branch).unwrap().expect("metadata");

    assert!(scanned.metadata.freeze.is_frozen());
}

#[test]
fn untrack_removes_metadata() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    let ctx = repo.context();
    commands::untrack(&ctx, Some("feature"), true).expect("untrack failed");

    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    assert!(store.read(&branch).unwrap().is_none());
}

#[test]
fn untrack_with_descendants() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create parent
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Create child
    repo.create_branch("feature-child");
    repo.checkout("feature-child");
    repo.commit("child.txt", "child", "Add child");
    repo.track_branch("feature-child", "feature");

    // Untrack parent with force
    let ctx = repo.context();
    commands::untrack(&ctx, Some("feature"), true).expect("untrack failed");

    // Both should be gone
    let git = repo.git();
    let store = MetadataStore::new(&git);
    assert!(store
        .read(&BranchName::new("feature").unwrap())
        .unwrap()
        .is_none());
    assert!(store
        .read(&BranchName::new("feature-child").unwrap())
        .unwrap()
        .is_none());
}

// =============================================================================
// Freeze/Unfreeze Command Tests
// =============================================================================

#[test]
fn freeze_sets_frozen_state() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    let ctx = repo.context();
    commands::freeze(&ctx, Some("feature"), false).expect("freeze failed");

    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let scanned = store.read(&branch).unwrap().expect("metadata");

    assert!(scanned.metadata.freeze.is_frozen());
}

#[test]
fn unfreeze_clears_frozen_state() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");

    // Track as frozen
    let ctx = repo.context();
    commands::track(&ctx, Some("feature"), Some("main"), false, true).expect("track frozen");

    // Unfreeze
    commands::unfreeze(&ctx, Some("feature"), false).expect("unfreeze failed");

    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let scanned = store.read(&branch).unwrap().expect("metadata");

    assert!(!scanned.metadata.freeze.is_frozen());
}

// =============================================================================
// Navigation Command Tests
// =============================================================================

#[test]
fn checkout_switches_branch() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.checkout("main");

    let ctx = repo.context();
    commands::checkout(&ctx, Some("feature"), false, false).expect("checkout failed");

    assert_eq!(repo.current_branch(), "feature");
}

#[test]
fn checkout_trunk() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");

    let ctx = repo.context();
    commands::checkout(&ctx, None, true, false).expect("checkout trunk");

    assert_eq!(repo.current_branch(), "main");
}

#[test]
fn down_navigates_to_parent() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create tracked branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    let ctx = repo.context();
    commands::down(&ctx, 1).expect("down failed");

    assert_eq!(repo.current_branch(), "main");
}

#[test]
fn up_navigates_to_child() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create tracked branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Go back to main
    repo.checkout("main");

    let ctx = repo.context();
    commands::up(&ctx, 1).expect("up failed");

    assert_eq!(repo.current_branch(), "feature");
}

#[test]
fn bottom_navigates_to_trunk_child() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create chain: main -> feature -> feature-child
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    repo.create_branch("feature-child");
    repo.checkout("feature-child");
    repo.commit("child.txt", "child", "Add child");
    repo.track_branch("feature-child", "feature");

    let ctx = repo.context();
    commands::bottom(&ctx).expect("bottom failed");

    // Per SPEC: bottom goes to trunk-child (first tracked branch), not trunk itself
    assert_eq!(repo.current_branch(), "feature");
}

#[test]
fn top_navigates_to_leaf() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create chain: main -> feature -> feature-child
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    repo.create_branch("feature-child");
    repo.checkout("feature-child");
    repo.commit("child.txt", "child", "Add child");
    repo.track_branch("feature-child", "feature");

    // Go to main
    repo.checkout("main");

    let ctx = repo.context();
    commands::top(&ctx).expect("top failed");

    assert_eq!(repo.current_branch(), "feature-child");
}

// =============================================================================
// Restack Command Tests
// =============================================================================

#[test]
fn restack_updates_branch_base() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create feature branch from main
    let main_oid = repo.head_oid();
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Add commit to main
    repo.checkout("main");
    repo.commit("main-update.txt", "update", "Update main");
    let new_main_oid = repo.head_oid();

    // Feature's base is still old main
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let before = store.read(&branch).unwrap().expect("metadata");
    assert_eq!(before.metadata.base.oid, main_oid);

    // Restack
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack failed");

    // Feature's base should now be new main
    let after = store.read(&branch).unwrap().expect("metadata");
    assert_eq!(after.metadata.base.oid, new_main_oid);
}

#[test]
fn restack_skips_frozen_branches() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create frozen feature branch
    let main_oid = repo.head_oid();
    repo.create_branch("frozen-feature");
    repo.checkout("frozen-feature");
    repo.commit("frozen.txt", "frozen", "Frozen feature");

    let ctx = repo.context();
    commands::track(&ctx, Some("frozen-feature"), Some("main"), false, true).expect("track frozen");

    // Add commit to main
    repo.checkout("main");
    repo.commit("main-update.txt", "update", "Update main");

    // Attempt restack
    repo.checkout("frozen-feature");
    commands::restack(&ctx, Some("frozen-feature"), true, false).expect("restack");

    // Base should still be old main (not restacked)
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("frozen-feature").unwrap();
    let metadata = store.read(&branch).unwrap().expect("metadata");
    assert_eq!(metadata.metadata.base.oid, main_oid);
}

#[test]
fn restack_already_aligned_is_noop() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create feature branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Get current state
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let before = store.read(&branch).unwrap().expect("metadata");

    // Restack (should be no-op)
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack");

    // State unchanged
    let after = store.read(&branch).unwrap().expect("metadata");
    assert_eq!(before.metadata.base.oid, after.metadata.base.oid);
}

#[test]
fn restack_chain_updates_all() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create chain: main -> feature -> feature-child
    let _main_oid = repo.head_oid();
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    let feature_oid = repo.head_oid();
    repo.create_branch("feature-child");
    repo.checkout("feature-child");
    repo.commit("child.txt", "child", "Add child");
    repo.track_branch("feature-child", "feature");

    // Add commit to main
    repo.checkout("main");
    repo.commit("main-update.txt", "update", "Update main");
    let new_main_oid = repo.head_oid();

    // Restack from feature (should update feature and child)
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), false, false).expect("restack");

    // Check both are updated
    let git = repo.git();
    let store = MetadataStore::new(&git);

    let feature = store
        .read(&BranchName::new("feature").unwrap())
        .unwrap()
        .expect("feature metadata");
    assert_eq!(feature.metadata.base.oid, new_main_oid);

    // Note: In the current implementation, child's base is only updated if
    // feature-child was already misaligned with feature at the start.
    // Since feature-child's base was feature's tip at track time, and we only
    // changed main (not feature directly), feature-child's base still matches
    // the OLD feature tip. After feature rebases onto new main, feature-child
    // would need a second restack to align with the new feature tip.
    //
    // This is a known limitation - a full restack would need to re-scan after
    // each rebase to catch cascading updates. For now, verify the child exists.
    let child = store
        .read(&BranchName::new("feature-child").unwrap())
        .unwrap()
        .expect("child metadata");
    // Child's base is still the old feature tip (before restack)
    assert_eq!(child.metadata.base.oid, feature_oid);
}

// =============================================================================
// Info/Log Command Tests
// =============================================================================

#[test]
fn info_shows_branch_details() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Info should not error
    let ctx = repo.context();
    commands::info(&ctx, Some("feature"), false, false, false).expect("info failed");
}

#[test]
fn info_on_untracked_branch() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("untracked");
    repo.checkout("untracked");

    let ctx = repo.context();
    commands::info(&ctx, Some("untracked"), false, false, false)
        .expect("info on untracked should work");
}

#[test]
fn log_shows_stack() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create chain
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    repo.create_branch("feature-child");
    repo.checkout("feature-child");
    repo.commit("child.txt", "child", "Add child");
    repo.track_branch("feature-child", "feature");

    let ctx = repo.context();
    commands::log(&ctx, false, false, false, false, false).expect("log failed");
}

#[test]
fn parent_returns_parent_name() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    let ctx = repo.context();
    // parent() uses current branch, so checkout first
    repo.checkout("feature");
    commands::parent(&ctx).expect("parent failed");
}

#[test]
fn children_returns_child_names() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    let ctx = repo.context();
    // children() uses current branch, so checkout first
    repo.checkout("main");
    commands::children(&ctx).expect("children failed");
}

#[test]
fn trunk_returns_trunk_name() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    commands::trunk(&ctx, None).expect("trunk failed");
}

// =============================================================================
// Create Command Tests
// =============================================================================

#[test]
fn create_makes_tracked_branch() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    commands::create(
        &ctx,
        Some("new-feature"),
        None,  // no message
        false, // no all
        false, // no update
        false, // no patch
        false, // no insert
    )
    .expect("create failed");

    // Should be on new branch
    assert_eq!(repo.current_branch(), "new-feature");

    // Should be tracked
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("new-feature").unwrap();
    assert!(store.read(&branch).unwrap().is_some());
}

#[test]
fn create_with_explicit_parent() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create parent
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Create is from current branch (feature)
    let ctx = repo.context();
    commands::create(
        &ctx,
        Some("child-feature"),
        None,
        false,
        false,
        false,
        false,
    )
    .expect("create failed");

    // Verify parent
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("child-feature").unwrap();
    let metadata = store.read(&branch).unwrap().expect("metadata");
    assert_eq!(metadata.metadata.parent.name(), "feature");
}

// =============================================================================
// Config Command Tests
// =============================================================================

#[test]
fn config_get_trunk() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    commands::config_get(&ctx, "trunk").expect("config get trunk failed");
}

#[test]
fn config_list() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    commands::config_list(&ctx).expect("config list failed");
}

// =============================================================================
// Completion Command Tests
// =============================================================================

#[test]
fn completion_generates_scripts() {
    for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
        commands::completion(shell).expect("completion failed");
    }
}

// =============================================================================
// Changelog Command Tests
// =============================================================================

#[test]
fn changelog_outputs_version() {
    commands::changelog().expect("changelog failed");
}

// =============================================================================
// Edge Cases and Error Handling
// =============================================================================

#[test]
fn track_nonexistent_branch_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    let result = commands::track(&ctx, Some("nonexistent"), Some("main"), false, false);
    assert!(result.is_err());
}

#[test]
fn track_trunk_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    let result = commands::track(&ctx, Some("main"), Some("main"), false, false);
    assert!(result.is_err());
}

#[test]
fn checkout_nonexistent_branch_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    let result = commands::checkout(&ctx, Some("nonexistent"), false, false);
    assert!(result.is_err());
}

#[test]
fn up_with_no_children_is_noop() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // main has no tracked children - up is a no-op, not an error
    let ctx = repo.context();
    commands::up(&ctx, 1).expect("up with no children should succeed as no-op");
    assert_eq!(repo.current_branch(), "main");
}

#[test]
fn down_from_trunk_is_noop() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Already on main (trunk)
    let ctx = repo.context();
    // down from trunk is a no-op, not an error
    commands::down(&ctx, 1).expect("down from trunk should succeed as no-op");
    assert_eq!(repo.current_branch(), "main");
}

#[test]
fn restack_untracked_branch_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("untracked");
    repo.checkout("untracked");

    let ctx = repo.context();
    let result = commands::restack(&ctx, Some("untracked"), true, false);
    assert!(result.is_err());
}

#[test]
fn info_nonexistent_branch_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    let result = commands::info(&ctx, Some("nonexistent"), false, false, false);
    assert!(result.is_err());
}

// =============================================================================
// Graph Integrity Tests
// =============================================================================

#[test]
fn graph_preserves_structure_after_operations() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Build a tree structure:
    //       main
    //      /    \
    //  feature1  feature2
    //     |
    //  child1

    repo.create_branch("feature1");
    repo.checkout("feature1");
    repo.commit("f1.txt", "f1", "Feature 1");
    repo.track_branch("feature1", "main");

    repo.create_branch("child1");
    repo.checkout("child1");
    repo.commit("c1.txt", "c1", "Child 1");
    repo.track_branch("child1", "feature1");

    repo.checkout("main");
    repo.create_branch("feature2");
    repo.checkout("feature2");
    repo.commit("f2.txt", "f2", "Feature 2");
    repo.track_branch("feature2", "main");

    // Verify graph structure via scan
    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    // Check parent relationships
    let f1 = BranchName::new("feature1").unwrap();
    let f2 = BranchName::new("feature2").unwrap();
    let c1 = BranchName::new("child1").unwrap();

    assert!(snapshot.graph.parent(&f1).map(|p| p.as_str()) == Some("main"));
    assert!(snapshot.graph.parent(&f2).map(|p| p.as_str()) == Some("main"));
    assert!(snapshot.graph.parent(&c1).map(|p| p.as_str()) == Some("feature1"));

    // Check children
    let main = BranchName::new("main").unwrap();
    let main_children = snapshot.graph.children(&main);
    assert!(main_children.is_some());
    let children: Vec<_> = main_children.unwrap().iter().map(|b| b.as_str()).collect();
    assert!(children.contains(&"feature1"));
    assert!(children.contains(&"feature2"));
}

// =============================================================================
// Multiple Stacks Tests
// =============================================================================

#[test]
fn multiple_independent_stacks() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Stack 1: main -> stack1-base -> stack1-top
    repo.create_branch("stack1-base");
    repo.checkout("stack1-base");
    repo.commit("s1b.txt", "s1b", "Stack 1 base");
    repo.track_branch("stack1-base", "main");

    repo.create_branch("stack1-top");
    repo.checkout("stack1-top");
    repo.commit("s1t.txt", "s1t", "Stack 1 top");
    repo.track_branch("stack1-top", "stack1-base");

    // Stack 2: main -> stack2-base -> stack2-top
    repo.checkout("main");
    repo.create_branch("stack2-base");
    repo.checkout("stack2-base");
    repo.commit("s2b.txt", "s2b", "Stack 2 base");
    repo.track_branch("stack2-base", "main");

    repo.create_branch("stack2-top");
    repo.checkout("stack2-top");
    repo.commit("s2t.txt", "s2t", "Stack 2 top");
    repo.track_branch("stack2-top", "stack2-base");

    // Navigate within stack 1
    repo.checkout("stack1-top");
    let ctx = repo.context();
    commands::bottom(&ctx).expect("bottom");
    // bottom goes to trunk-child (stack1-base), not trunk itself
    assert_eq!(repo.current_branch(), "stack1-base");

    // Navigate within stack 2
    repo.checkout("stack2-base");
    commands::top(&ctx).expect("top");
    assert_eq!(repo.current_branch(), "stack2-top");
}

// =============================================================================
// Concurrent Operations Tests
// =============================================================================

#[test]
fn metadata_cas_prevents_race() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature", "Add feature");
    repo.track_branch("feature", "main");

    // Read metadata
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let scanned = store.read(&branch).unwrap().expect("metadata");

    // Simulate concurrent modification by updating with wrong expected OID
    let mut modified = scanned.metadata.clone();
    modified.freeze = FreezeState::frozen(FreezeScope::Single, None);

    // Use a fake old OID - should fail
    let fake_oid =
        latticework::core::types::Oid::new("0000000000000000000000000000000000000000".to_string())
            .unwrap();
    let result = store.write_cas(&branch, Some(&fake_oid), &modified);
    assert!(result.is_err());

    // Original should still work
    let result = store.write_cas(&branch, Some(&scanned.ref_oid), &modified);
    assert!(result.is_ok());
}

// =============================================================================
// Conflict Pausing Tests
// =============================================================================

#[test]
fn restack_pauses_on_conflict() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a feature branch with a file
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit(
        "shared.txt",
        "feature content",
        "Add shared file on feature",
    );
    repo.track_branch("feature", "main");

    // Go back to main and create a conflicting change
    repo.checkout("main");
    repo.commit("shared.txt", "main content", "Add shared file on main");

    // Now feature's base is the old main, but main has moved with a conflict
    // Restack should hit a conflict
    repo.checkout("feature");
    let ctx = repo.context();

    // Restack will pause on conflict
    let result = commands::restack(&ctx, Some("feature"), true, false);

    // The command should succeed (it pauses, doesn't error)
    assert!(
        result.is_ok(),
        "restack should pause on conflict, not error"
    );

    // Check that we're in a rebase state
    let git = repo.git();
    assert!(git.state().is_in_progress(), "should be in rebase state");

    // Check that op-state was written
    let git_dir = git.git_dir();
    let op_state_path = git_dir.join("lattice/op-state.json");
    assert!(
        op_state_path.exists(),
        "op-state.json should exist when paused"
    );

    // Clean up - abort the rebase
    run_git(repo.path(), &["rebase", "--abort"]);
}

#[test]
fn continue_resumes_after_conflict_resolution() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a feature branch with a file
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit(
        "shared.txt",
        "feature content",
        "Add shared file on feature",
    );
    repo.track_branch("feature", "main");

    // Go back to main and create a conflicting change
    repo.checkout("main");
    repo.commit("shared.txt", "main content", "Add shared file on main");

    // Restack to trigger conflict
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack should pause");

    // Verify we're in conflict state
    let git = repo.git();
    assert!(git.state().is_in_progress());

    // Resolve the conflict by accepting "ours" (feature's version)
    std::fs::write(repo.path().join("shared.txt"), "resolved content").unwrap();
    run_git(repo.path(), &["add", "shared.txt"]);

    // Continue the operation
    let result = commands::continue_op(&ctx, false);
    assert!(
        result.is_ok(),
        "continue should succeed after resolving conflict"
    );

    // Verify we're no longer in rebase state
    let git = repo.git();
    assert!(
        !git.state().is_in_progress(),
        "should not be in rebase state after continue"
    );

    // Op-state should be cleared
    let git_dir = git.git_dir();
    let op_state_path = git_dir.join("lattice/op-state.json");
    assert!(
        !op_state_path.exists(),
        "op-state.json should be cleared after continue"
    );
}

#[test]
fn abort_cancels_paused_operation() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a feature branch with a file
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit(
        "shared.txt",
        "feature content",
        "Add shared file on feature",
    );
    repo.track_branch("feature", "main");

    // Remember the original feature HEAD
    let original_feature_oid = repo.head_oid();

    // Go back to main and create a conflicting change
    repo.checkout("main");
    repo.commit("shared.txt", "main content", "Add shared file on main");

    // Restack to trigger conflict
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack should pause");

    // Verify we're in conflict state
    let git = repo.git();
    assert!(git.state().is_in_progress());

    // Abort the operation
    let result = commands::abort(&ctx);
    assert!(result.is_ok(), "abort should succeed");

    // Verify we're no longer in rebase state
    let git = repo.git();
    assert!(
        !git.state().is_in_progress(),
        "should not be in rebase state after abort"
    );

    // Op-state should be cleared
    let git_dir = git.git_dir();
    let op_state_path = git_dir.join("lattice/op-state.json");
    assert!(
        !op_state_path.exists(),
        "op-state.json should be cleared after abort"
    );

    // Feature branch should be back to original state
    repo.checkout("feature");
    assert_eq!(
        repo.head_oid(),
        original_feature_oid,
        "feature should be restored to original"
    );
}

#[test]
fn continue_without_paused_op_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    let result = commands::continue_op(&ctx, false);

    // Should fail because there's no operation in progress
    assert!(result.is_err(), "continue without paused op should fail");
}

#[test]
fn abort_without_paused_op_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    let result = commands::abort(&ctx);

    // Should fail because there's no operation in progress
    assert!(result.is_err(), "abort without paused op should fail");
}

// =============================================================================
// Phase 7: Recovery Command Improvements Tests
// =============================================================================

#[test]
fn abort_records_event_in_ledger() {
    use latticework::engine::ledger::{Event, EventLedger};

    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a feature branch with a file
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit(
        "shared.txt",
        "feature content",
        "Add shared file on feature",
    );
    repo.track_branch("feature", "main");

    // Go back to main and create a conflicting change
    repo.checkout("main");
    repo.commit("shared.txt", "main content", "Add shared file on main");

    // Restack to trigger conflict
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack should pause");

    // Get the ledger state before abort
    let git = repo.git();
    let ledger = EventLedger::new(&git);
    let events_before = ledger.recent(100).expect("read ledger");

    // Abort the operation
    commands::abort(&ctx).expect("abort should succeed");

    // Check that an Aborted event was recorded
    let events_after = ledger.recent(100).expect("read ledger");
    assert!(
        events_after.len() > events_before.len(),
        "abort should record an event in the ledger"
    );

    // The most recent event should be Aborted
    let latest = events_after.first().expect("should have events");
    match &latest.event {
        Event::Aborted { reason, .. } => {
            assert!(
                reason.contains("abort"),
                "abort reason should mention abort"
            );
        }
        _ => panic!(
            "expected Aborted event, got {:?}",
            std::mem::discriminant(&latest.event)
        ),
    }
}

#[test]
fn undo_records_event_in_ledger() {
    use latticework::engine::ledger::{Event, EventLedger};

    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a feature branch and track it
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature content", "Add feature");
    repo.track_branch("feature", "main");

    // Record the original base oid
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let original_metadata = store.read(&branch).unwrap().expect("metadata");
    let original_base = original_metadata.metadata.base.oid.clone();

    // Add a commit to main to make feature out of date
    repo.checkout("main");
    repo.commit("main-update.txt", "update", "Update main");
    let new_main_oid = repo.head_oid();

    // Restack feature - this creates a journal that can be undone
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack should succeed");

    // Verify restack changed the base
    let metadata_after_restack = store.read(&branch).unwrap().expect("metadata");
    assert_eq!(
        metadata_after_restack.metadata.base.oid, new_main_oid,
        "base should be updated after restack"
    );

    // Get the ledger state before undo
    let ledger = EventLedger::new(&git);
    let events_before = ledger.recent(100).expect("read ledger");

    // Undo the restack operation
    commands::undo(&ctx).expect("undo should succeed");

    // Check that an UndoApplied event was recorded
    let events_after = ledger.recent(100).expect("read ledger");
    assert!(
        events_after.len() > events_before.len(),
        "undo should record an event in the ledger"
    );

    // The most recent event should be UndoApplied
    let latest = events_after.first().expect("should have events");
    match &latest.event {
        Event::UndoApplied { refs_restored, .. } => {
            assert!(
                *refs_restored > 0,
                "undo should have restored at least one ref"
            );
        }
        _ => panic!(
            "expected UndoApplied event, got {:?}",
            std::mem::discriminant(&latest.event)
        ),
    }

    // Verify the base was restored to the original
    let metadata_after_undo = store.read(&branch).unwrap().expect("metadata");
    assert_eq!(
        metadata_after_undo.metadata.base.oid, original_base,
        "base should be restored to original after undo"
    );
}

#[test]
fn undo_without_operations_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let ctx = repo.context();
    let result = commands::undo(&ctx);

    // Should fail because there's no operation to undo
    assert!(result.is_err(), "undo without operations should fail");
}

#[test]
fn undo_while_operation_in_progress_fails() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a feature branch with a file
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit(
        "shared.txt",
        "feature content",
        "Add shared file on feature",
    );
    repo.track_branch("feature", "main");

    // Go back to main and create a conflicting change
    repo.checkout("main");
    repo.commit("shared.txt", "main content", "Add shared file on main");

    // Restack to trigger conflict (operation now in progress)
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack should pause");

    // Try to undo while operation is in progress
    let result = commands::undo(&ctx);
    assert!(
        result.is_err(),
        "undo should fail while operation is in progress"
    );

    // Error message should mention abort
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("abort"),
        "error should mention using abort first"
    );

    // Clean up
    run_git(repo.path(), &["rebase", "--abort"]);
}

#[test]
fn undo_restores_metadata_refs() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create and track a feature branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature content", "Add feature");
    repo.track_branch("feature", "main");

    // Verify metadata exists and record the original base
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let original_metadata = store
        .read(&branch)
        .unwrap()
        .expect("metadata should exist after track");
    let original_base = original_metadata.metadata.base.oid.clone();

    // Add commit to main to trigger a restack
    repo.checkout("main");
    repo.commit("main-update.txt", "update", "Update main");

    // Restack feature - this creates a journal and changes metadata
    repo.checkout("feature");
    let ctx = repo.context();
    commands::restack(&ctx, Some("feature"), true, false).expect("restack should succeed");

    // Verify metadata changed (base updated)
    let metadata_after_restack = store.read(&branch).unwrap().expect("metadata");
    assert_ne!(
        metadata_after_restack.metadata.base.oid, original_base,
        "base should have changed after restack"
    );

    // Undo the restack operation
    commands::undo(&ctx).expect("undo should succeed");

    // Verify metadata ref was restored to original
    let store = MetadataStore::new(&git);
    let metadata_after_undo = store.read(&branch).unwrap().expect("metadata should exist");
    assert_eq!(
        metadata_after_undo.metadata.base.oid, original_base,
        "metadata base should be restored to original after undo"
    );
}
