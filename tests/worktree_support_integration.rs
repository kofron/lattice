//! Integration tests for bare repository and worktree support.
//!
//! Per SPEC.md section 4.6 and Milestone 2, these tests verify that Lattice
//! correctly handles bare repositories, linked worktrees, and the shared
//! state model (lock, op-state, journals, config) across worktrees.
//!
//! Test matrix:
//! - Bare repos: read-only works, workdir-required commands refuse with guidance
//! - Worktrees: operations work normally, state shared via common_dir
//! - Cross-worktree: lock, op-state, and metadata are shared
//! - Branch occupancy: operations refuse when target branch is checked out elsewhere

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use latticework::core::paths::LatticePaths;
use latticework::core::types::BranchName;
use latticework::git::{Git, RepoContext, WorktreeStatus};

// =============================================================================
// Test Fixtures
// =============================================================================

/// Helper to run git commands in a directory.
fn run_git(dir: &Path, args: &[&str]) -> std::process::Output {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command failed to execute");

    output
}

/// Helper to run git commands and assert success.
fn run_git_ok(dir: &Path, args: &[&str]) {
    let output = run_git(dir, args);
    if !output.status.success() {
        panic!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Create a normal git repository with an initial commit.
fn create_normal_repo(dir: &Path) {
    run_git_ok(dir, &["init"]);
    run_git_ok(dir, &["config", "user.email", "test@example.com"]);
    run_git_ok(dir, &["config", "user.name", "Test User"]);

    std::fs::write(dir.join("README.md"), "# Test\n").unwrap();
    run_git_ok(dir, &["add", "README.md"]);
    run_git_ok(dir, &["commit", "-m", "Initial commit"]);
}

/// Create a bare git repository by cloning a normal one.
fn create_bare_repo() -> (TempDir, TempDir) {
    let source_dir = TempDir::new().expect("failed to create source temp dir");
    create_normal_repo(source_dir.path());

    let bare_dir = TempDir::new().expect("failed to create bare temp dir");
    run_git_ok(
        bare_dir.path().parent().unwrap(),
        &[
            "clone",
            "--bare",
            source_dir.path().to_str().unwrap(),
            bare_dir.path().to_str().unwrap(),
        ],
    );

    (source_dir, bare_dir)
}

/// Create a worktree from an existing repository.
fn create_worktree(repo_path: &Path, worktree_path: &Path, branch: &str) {
    // Create a new branch for the worktree
    run_git_ok(repo_path, &["branch", branch]);
    run_git_ok(
        repo_path,
        &["worktree", "add", worktree_path.to_str().unwrap(), branch],
    );
}

// =============================================================================
// Bare Repository Tests
// =============================================================================

mod bare_repo {
    use super::*;

    #[test]
    fn bare_repo_detected_as_bare_context() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get repo info");

        assert!(
            matches!(info.context, RepoContext::Bare),
            "Expected Bare context, got {:?}",
            info.context
        );
        assert!(info.work_dir.is_none(), "Bare repo should have no work_dir");
    }

    #[test]
    fn bare_repo_common_dir_equals_git_dir() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get repo info");

        assert_eq!(
            info.git_dir, info.common_dir,
            "In bare repo, git_dir should equal common_dir"
        );
    }

    #[test]
    fn bare_repo_worktree_status_is_unavailable() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let status = git.worktree_status(false).expect("failed to get status");

        assert!(
            status.is_unavailable(),
            "Bare repo worktree status should be Unavailable"
        );
        assert!(
            matches!(status, WorktreeStatus::Unavailable { .. }),
            "Expected Unavailable variant"
        );
    }

    #[test]
    fn bare_repo_lattice_paths_use_git_dir() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get repo info");
        let paths = LatticePaths::from_repo_info(&info);

        // In bare repo, lattice dir should be under common_dir (which equals git_dir)
        // Use common_dir for comparison since TempDir paths may differ from git's resolved paths
        assert!(
            paths.repo_lattice_dir().starts_with(&info.common_dir),
            "Lattice dir {:?} should be under common_dir {:?}",
            paths.repo_lattice_dir(),
            info.common_dir
        );
    }

    #[test]
    fn bare_repo_can_read_refs() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");

        // Should be able to list branches
        let branches = git.list_branches().expect("failed to list branches");
        assert!(!branches.is_empty(), "Bare repo should have branches");
    }

    #[test]
    fn bare_repo_can_resolve_refs() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");

        // Should be able to resolve HEAD
        let head = git.try_resolve_ref("HEAD");
        assert!(head.is_ok(), "Should be able to resolve HEAD in bare repo");
    }
}

// =============================================================================
// Worktree Tests
// =============================================================================

mod worktree {
    use super::*;

    #[test]
    fn worktree_detected_as_worktree_context() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        let git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let info = git.info().expect("failed to get repo info");

        assert!(
            matches!(info.context, RepoContext::Worktree),
            "Expected Worktree context, got {:?}",
            info.context
        );
    }

    #[test]
    fn worktree_has_work_dir() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        let git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let info = git.info().expect("failed to get repo info");

        assert!(info.work_dir.is_some(), "Worktree should have a work_dir");
    }

    #[test]
    fn worktree_common_dir_points_to_main_repo() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        let main_git = Git::open(main_dir.path()).expect("failed to open main repo");
        let main_info = main_git.info().expect("failed to get main repo info");

        let wt_git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let wt_info = wt_git.info().expect("failed to get worktree info");

        assert_eq!(
            main_info.common_dir, wt_info.common_dir,
            "Main repo and worktree should share common_dir"
        );
        assert_ne!(
            main_info.git_dir, wt_info.git_dir,
            "Main repo and worktree should have different git_dirs"
        );
    }

    #[test]
    fn worktree_lattice_paths_use_common_dir() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        let main_git = Git::open(main_dir.path()).expect("failed to open main repo");
        let main_info = main_git.info().expect("failed to get main repo info");
        let main_paths = LatticePaths::from_repo_info(&main_info);

        let wt_git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let wt_info = wt_git.info().expect("failed to get worktree info");
        let wt_paths = LatticePaths::from_repo_info(&wt_info);

        // Both should use the same lattice directory (under common_dir)
        assert_eq!(
            main_paths.repo_lattice_dir(),
            wt_paths.repo_lattice_dir(),
            "Main repo and worktree should share lattice directory"
        );
    }

    #[test]
    fn worktree_worktree_status_is_clean() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        let git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let status = git.worktree_status(false).expect("failed to get status");

        assert!(status.is_clean(), "Fresh worktree should be clean");
        assert!(
            !status.is_unavailable(),
            "Worktree status should be available"
        );
    }
}

// =============================================================================
// Normal Repository Tests
// =============================================================================

mod normal_repo {
    use super::*;

    #[test]
    fn normal_repo_detected_as_normal_context() {
        let dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(dir.path());

        let git = Git::open(dir.path()).expect("failed to open repo");
        let info = git.info().expect("failed to get repo info");

        assert!(
            matches!(info.context, RepoContext::Normal),
            "Expected Normal context, got {:?}",
            info.context
        );
    }

    #[test]
    fn normal_repo_git_dir_equals_common_dir() {
        let dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(dir.path());

        let git = Git::open(dir.path()).expect("failed to open repo");
        let info = git.info().expect("failed to get repo info");

        assert_eq!(
            info.git_dir, info.common_dir,
            "In normal repo, git_dir should equal common_dir"
        );
    }

    #[test]
    fn normal_repo_has_work_dir() {
        let dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(dir.path());

        let git = Git::open(dir.path()).expect("failed to open repo");
        let info = git.info().expect("failed to get repo info");

        assert!(
            info.work_dir.is_some(),
            "Normal repo should have a work_dir"
        );
    }
}

// =============================================================================
// Shared State Tests (Lock, Op-State, Config)
// =============================================================================

mod shared_state {
    use super::*;
    use latticework::core::ops::lock::RepoLock;

    #[test]
    fn lock_shared_between_worktrees() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        // Get paths for both repos
        let main_git = Git::open(main_dir.path()).expect("failed to open main repo");
        let main_info = main_git.info().expect("failed to get main repo info");
        let main_paths = LatticePaths::from_repo_info(&main_info);

        let wt_git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let wt_info = wt_git.info().expect("failed to get worktree info");
        let wt_paths = LatticePaths::from_repo_info(&wt_info);

        // Acquire lock from main repo
        let lock = RepoLock::acquire(&main_paths).expect("failed to acquire lock");

        // Try to acquire from worktree - should fail (return None)
        let result = RepoLock::try_acquire(&wt_paths).expect("try_acquire failed");
        assert!(
            result.is_none(),
            "Should not be able to acquire lock when main repo holds it"
        );

        // Release and try again
        drop(lock);
        let result = RepoLock::try_acquire(&wt_paths).expect("try_acquire failed");
        assert!(
            result.is_some(),
            "Should be able to acquire lock after main repo releases it"
        );
    }

    #[test]
    fn op_state_shared_between_worktrees() {
        use latticework::core::ops::journal::{Journal, OpState};

        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        // Get paths for both repos
        let main_git = Git::open(main_dir.path()).expect("failed to open main repo");
        let main_info = main_git.info().expect("failed to get main repo info");
        let main_paths = LatticePaths::from_repo_info(&main_info);

        let wt_git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let wt_info = wt_git.info().expect("failed to get worktree info");
        let wt_paths = LatticePaths::from_repo_info(&wt_info);

        // Create a journal and then op-state from main repo
        let journal = Journal::new("test-command".to_string());
        let op_id = journal.op_id.clone();
        journal.write(&main_paths).expect("failed to write journal");

        let state = OpState::from_journal(&journal, &main_paths, main_info.work_dir.clone());
        state.write(&main_paths).expect("failed to write op-state");

        // Read from worktree - should see the same state
        let read_state = OpState::read(&wt_paths)
            .expect("failed to read op-state")
            .expect("op-state should exist");

        assert_eq!(read_state.op_id, op_id, "Op ID should match");
        assert_eq!(read_state.command, "test-command", "Command should match");

        // Clean up
        OpState::remove(&main_paths).expect("failed to remove op-state");
        journal
            .delete(&main_paths)
            .expect("failed to delete journal");
    }

    #[test]
    fn op_state_origin_worktree_enforcement() {
        use latticework::core::ops::journal::{Journal, OpState};

        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        // Get paths for both repos
        let main_git = Git::open(main_dir.path()).expect("failed to open main repo");
        let main_info = main_git.info().expect("failed to get main repo info");
        let main_paths = LatticePaths::from_repo_info(&main_info);

        let wt_git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let wt_info = wt_git.info().expect("failed to get worktree info");

        // Create a journal and then op-state from main repo
        let journal = Journal::new("test-command".to_string());
        journal.write(&main_paths).expect("failed to write journal");

        let state = OpState::from_journal(&journal, &main_paths, main_info.work_dir.clone());
        state.write(&main_paths).expect("failed to write op-state");

        // Check origin from main - should succeed
        let check_main = state.check_origin_worktree(&main_info.git_dir);
        assert!(
            check_main.is_ok(),
            "Check from origin worktree should succeed"
        );

        // Check origin from worktree - should fail
        let check_wt = state.check_origin_worktree(&wt_info.git_dir);
        assert!(
            check_wt.is_err(),
            "Check from different worktree should fail"
        );
        let err_msg = check_wt.unwrap_err();
        assert!(
            err_msg.contains("different worktree"),
            "Error should mention different worktree: {}",
            err_msg
        );

        // Clean up
        OpState::remove(&main_paths).expect("failed to remove op-state");
        journal
            .delete(&main_paths)
            .expect("failed to delete journal");
    }

    #[test]
    fn config_path_shared_between_worktrees() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        // Get paths for both repos
        let main_git = Git::open(main_dir.path()).expect("failed to open main repo");
        let main_info = main_git.info().expect("failed to get main repo info");
        let main_paths = LatticePaths::from_repo_info(&main_info);

        let wt_git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let wt_info = wt_git.info().expect("failed to get worktree info");
        let wt_paths = LatticePaths::from_repo_info(&wt_info);

        assert_eq!(
            main_paths.repo_config_path(),
            wt_paths.repo_config_path(),
            "Config path should be shared between worktrees"
        );
    }
}

// =============================================================================
// Worktree Branch Occupancy Tests
// =============================================================================

mod branch_occupancy {
    use super::*;

    #[test]
    fn list_worktrees_includes_main_and_linked() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        // Rename the default branch to 'main' for consistency
        run_git_ok(main_dir.path(), &["branch", "-M", "main"]);

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        let git = Git::open(main_dir.path()).expect("failed to open main repo");
        let worktrees = git.list_worktrees().expect("failed to list worktrees");

        assert!(
            worktrees.len() >= 2,
            "Should have at least 2 worktrees (main + feature), got {}",
            worktrees.len()
        );

        // Check that we have both branches
        let branches: Vec<_> = worktrees
            .iter()
            .filter_map(|wt| wt.branch.as_ref())
            .collect();

        assert!(
            branches.iter().any(|b| b.as_str() == "main"),
            "Should have main branch checked out somewhere"
        );
        assert!(
            branches.iter().any(|b| b.as_str() == "feature"),
            "Should have feature branch checked out somewhere"
        );
    }

    #[test]
    fn branch_checked_out_elsewhere_detected() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        // Rename the default branch to 'main' for consistency
        run_git_ok(main_dir.path(), &["branch", "-M", "main"]);

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        // Open from the worktree and check if 'main' is checked out elsewhere
        let git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let main_branch = BranchName::new("main").expect("invalid branch name");

        // First, let's verify we can list worktrees correctly
        let worktrees = git.list_worktrees().expect("failed to list worktrees");

        // Get all branches checked out elsewhere for debugging
        let all_elsewhere = git
            .branches_checked_out_elsewhere()
            .expect("failed to get all branches elsewhere");

        let elsewhere = git
            .branch_checked_out_elsewhere(&main_branch)
            .expect("failed to check branch");

        assert!(
            elsewhere.is_some(),
            "main should be checked out in the main worktree.\n\
             Worktrees found: {:?}\n\
             All branches elsewhere: {:?}\n\
             Looking for 'main' branch",
            worktrees
                .iter()
                .map(|w| (&w.path, &w.branch))
                .collect::<Vec<_>>(),
            all_elsewhere
        );
    }

    #[test]
    fn current_branch_not_elsewhere() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        // Rename the default branch to 'main' for consistency
        run_git_ok(main_dir.path(), &["branch", "-M", "main"]);

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        // Open from the worktree and check if 'feature' is checked out elsewhere
        // (it shouldn't be - it's checked out HERE, not elsewhere)
        let git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let feature_branch = BranchName::new("feature").expect("invalid branch name");

        let elsewhere = git
            .branch_checked_out_elsewhere(&feature_branch)
            .expect("failed to check branch");

        // The feature branch IS checked out in THIS worktree, so from here
        // it should NOT be "elsewhere"
        assert!(
            elsewhere.is_none(),
            "feature should not be 'elsewhere' from the worktree where it's checked out"
        );
    }

    #[test]
    fn branches_checked_out_elsewhere_multiple() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        // Rename the default branch to 'main' for consistency
        run_git_ok(main_dir.path(), &["branch", "-M", "main"]);

        let worktree1 = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree1.path(), "feature1");

        let worktree2 = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree2.path(), "feature2");

        // Open from worktree1 and get all branches checked out elsewhere
        let git = Git::open(worktree1.path()).expect("failed to open worktree");

        let conflicts = git
            .branches_checked_out_elsewhere()
            .expect("failed to check branches");

        // main and feature2 are checked out elsewhere (not in worktree1)
        // feature1 is checked out here (so not "elsewhere")
        assert_eq!(
            conflicts.len(),
            2,
            "Should find 2 branches checked out elsewhere (main, feature2), got: {:?}",
            conflicts.keys().collect::<Vec<_>>()
        );

        assert!(conflicts.contains_key(&BranchName::new("main").unwrap()));
        assert!(conflicts.contains_key(&BranchName::new("feature2").unwrap()));
    }
}

// =============================================================================
// Capability Detection Tests
// =============================================================================

mod capabilities {
    use super::*;

    #[test]
    fn bare_repo_missing_workdir_capability() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get repo info");

        // Verify work_dir is None (which means WorkingDirectoryAvailable won't be granted)
        assert!(
            info.work_dir.is_none(),
            "Bare repo should not have work_dir"
        );
    }

    #[test]
    fn normal_repo_has_workdir_capability() {
        let dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(dir.path());

        let git = Git::open(dir.path()).expect("failed to open repo");
        let info = git.info().expect("failed to get repo info");

        assert!(info.work_dir.is_some(), "Normal repo should have work_dir");
    }

    #[test]
    fn worktree_has_workdir_capability() {
        let main_dir = TempDir::new().expect("failed to create temp dir");
        create_normal_repo(main_dir.path());

        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        create_worktree(main_dir.path(), worktree_dir.path(), "feature");

        let git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let info = git.info().expect("failed to get repo info");

        assert!(info.work_dir.is_some(), "Worktree should have work_dir");
    }
}

// =============================================================================
// Config in Bare Repo and Worktree Tests
// =============================================================================

mod config_bare_and_worktree {
    use super::*;
    use latticework::core::config::{Config, RepoConfig};

    #[test]
    fn config_module_write_repo_works_in_bare_repo() {
        // This test exercises Config::write_repo - the actual method used by `lt init`
        // It should write to <bare_repo>/lattice/config.toml, not <bare_repo>/.git/lattice/config.toml
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get repo info");
        let paths = LatticePaths::from_repo_info(&info);

        // Use the Config module's write method (what init uses)
        let config = RepoConfig {
            trunk: Some("main".to_string()),
            ..Default::default()
        };

        // This is what init does - it should work in bare repos
        let result = Config::write_repo(bare.path(), &config);
        assert!(
            result.is_ok(),
            "Config::write_repo should succeed in bare repo: {:?}",
            result.err()
        );

        let written_path = result.unwrap();

        // The written path should be under the bare repo, not under a non-existent .git
        assert!(
            written_path.exists(),
            "Written config should exist at {:?}",
            written_path
        );

        // CRITICAL: The path should match what LatticePaths says, not hardcoded .git
        let expected_path = paths.repo_config_path();
        assert_eq!(
            written_path, expected_path,
            "Config::write_repo should write to LatticePaths location.\n\
             Got: {:?}\n\
             Expected: {:?}",
            written_path, expected_path
        );

        // Verify we can load it back using Config::load
        let loaded = Config::load(Some(bare.path()));
        assert!(
            loaded.is_ok(),
            "Should be able to load config from bare repo"
        );
        let loaded = loaded.unwrap();
        assert_eq!(
            loaded.config.trunk(),
            Some("main"),
            "Loaded config should have correct trunk"
        );
    }

    #[test]
    fn config_module_load_works_in_bare_repo() {
        // Write config directly to the correct location, then verify Config::load finds it
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get repo info");
        let paths = LatticePaths::from_repo_info(&info);

        // Write config at the correct location (using LatticePaths)
        let config = RepoConfig {
            trunk: Some("main".to_string()),
            ..Default::default()
        };
        let config_path = paths.repo_config_path();
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("failed to create dir");
        std::fs::write(&config_path, toml::to_string(&config).unwrap()).expect("failed to write");

        // Now Config::load should find it
        let loaded = Config::load(Some(bare.path()));
        assert!(
            loaded.is_ok(),
            "Config::load should find config in bare repo: {:?}",
            loaded.err()
        );
        let loaded = loaded.unwrap();
        assert_eq!(
            loaded.config.trunk(),
            Some("main"),
            "Config::load should read correct trunk from bare repo"
        );
    }

    #[test]
    fn config_write_and_read_in_bare_repo() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get repo info");
        let paths = LatticePaths::from_repo_info(&info);

        // Write config using LatticePaths
        let config = RepoConfig {
            trunk: Some("main".to_string()),
            ..Default::default()
        };

        // This should write to <bare_repo>/lattice/config.toml, not <bare_repo>/.git/lattice/config.toml
        let config_path = paths.repo_config_path();
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("failed to create dir");
        std::fs::write(&config_path, toml::to_string(&config).unwrap()).expect("failed to write");

        // Verify the file exists at the correct location
        assert!(
            config_path.exists(),
            "Config should exist at {:?}",
            config_path
        );
        assert!(
            config_path.starts_with(bare.path()) || config_path.starts_with(&info.common_dir),
            "Config path {:?} should be under bare repo {:?}",
            config_path,
            bare.path()
        );

        // Read it back
        let content = std::fs::read_to_string(&config_path).expect("failed to read");
        let loaded: RepoConfig = toml::from_str(&content).expect("failed to parse");
        assert_eq!(loaded.trunk, Some("main".to_string()));
    }

    #[test]
    fn config_shared_between_bare_and_worktree() {
        let (_source, bare) = create_bare_repo();

        // Create a worktree from the bare repo
        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        run_git_ok(bare.path(), &["branch", "feature"]);
        run_git_ok(
            bare.path(),
            &[
                "worktree",
                "add",
                worktree_dir.path().to_str().unwrap(),
                "feature",
            ],
        );

        // Get paths from both
        let bare_git = Git::open(bare.path()).expect("failed to open bare repo");
        let bare_info = bare_git.info().expect("failed to get bare info");
        let bare_paths = LatticePaths::from_repo_info(&bare_info);

        let wt_git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let wt_info = wt_git.info().expect("failed to get worktree info");
        let wt_paths = LatticePaths::from_repo_info(&wt_info);

        // Config paths should be the same (both use common_dir)
        assert_eq!(
            bare_paths.repo_config_path(),
            wt_paths.repo_config_path(),
            "Bare and worktree should share config path"
        );

        // Write config from bare repo
        let config = RepoConfig {
            trunk: Some("main".to_string()),
            ..Default::default()
        };
        let config_path = bare_paths.repo_config_path();
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("failed to create dir");
        std::fs::write(&config_path, toml::to_string(&config).unwrap()).expect("failed to write");

        // Read from worktree - should see the same config
        let wt_config_path = wt_paths.repo_config_path();
        assert!(
            wt_config_path.exists(),
            "Config written from bare should be visible from worktree"
        );
        let content = std::fs::read_to_string(&wt_config_path).expect("failed to read");
        let loaded: RepoConfig = toml::from_str(&content).expect("failed to parse");
        assert_eq!(loaded.trunk, Some("main".to_string()));
    }

    #[test]
    fn init_works_in_bare_repo() {
        let (_source, bare) = create_bare_repo();

        let git = Git::open(bare.path()).expect("failed to open bare repo");
        let info = git.info().expect("failed to get bare info");
        let paths = LatticePaths::from_repo_info(&info);

        // Simulate what init does: write config with trunk
        // For now, directly use Config::write_repo_at_path (which we'll add)
        // or test that the paths are correct

        // The key assertion: config path should NOT contain ".git" for a bare repo
        let config_path = paths.repo_config_path();
        let path_str = config_path.to_string_lossy();

        // In a bare repo, the path should be <bare_repo>/lattice/config.toml
        // NOT <bare_repo>/.git/lattice/config.toml
        assert!(
            !path_str.contains(".git/lattice") && !path_str.contains(".git\\lattice"),
            "Bare repo config path should not contain .git: {:?}",
            config_path
        );
    }

    #[test]
    fn workflow_clone_bare_add_worktree_init_create() {
        // This is the full scenario:
        // 1. Clone bare
        // 2. Create worktree
        // 3. lt init (from worktree)
        // 4. lt create (from worktree)

        let (_source, bare) = create_bare_repo();

        // Rename default branch to main for consistency
        run_git_ok(bare.path(), &["branch", "-M", "main"]);

        // Create worktree
        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        run_git_ok(bare.path(), &["branch", "my-feature"]);
        run_git_ok(
            bare.path(),
            &[
                "worktree",
                "add",
                worktree_dir.path().to_str().unwrap(),
                "my-feature",
            ],
        );

        // Open from worktree
        let git = Git::open(worktree_dir.path()).expect("failed to open worktree");
        let info = git.info().expect("failed to get info");
        let paths = LatticePaths::from_repo_info(&info);

        // Verify we're in a worktree context
        assert!(
            matches!(info.context, RepoContext::Worktree),
            "Should be in worktree context"
        );
        assert!(info.work_dir.is_some(), "Worktree should have work_dir");

        // Write config (simulating lt init)
        let config = RepoConfig {
            trunk: Some("main".to_string()),
            ..Default::default()
        };
        let config_path = paths.repo_config_path();
        std::fs::create_dir_all(config_path.parent().unwrap()).expect("failed to create dir");
        std::fs::write(&config_path, toml::to_string(&config).unwrap()).expect("failed to write");

        // Verify config is readable
        assert!(config_path.exists(), "Config should exist after init");

        // Now the worktree should be ready for lt create
        // (We're not testing the full create command here, just that config is set up correctly)

        // Verify bare repo can also see the config
        let bare_git = Git::open(bare.path()).expect("failed to open bare");
        let bare_info = bare_git.info().expect("failed to get bare info");
        let bare_paths = LatticePaths::from_repo_info(&bare_info);

        assert!(
            bare_paths.repo_config_path().exists(),
            "Bare repo should see config written from worktree"
        );
    }

    /// This test verifies the complete worktree-from-bare workflow using the Config API:
    /// 1. Clone bare (simulated)
    /// 2. Add worktree
    /// 3. Config::write_repo from worktree (simulating lt init)
    /// 4. Config::load from worktree (should find the config)
    /// 5. Config::load from bare (should also find the same config)
    #[test]
    fn config_api_worktree_from_bare_workflow() {
        let (_source, bare) = create_bare_repo();

        // Create a worktree from the bare repo
        let worktree_dir = TempDir::new().expect("failed to create worktree temp dir");
        run_git_ok(bare.path(), &["branch", "my-feature"]);
        run_git_ok(
            bare.path(),
            &[
                "worktree",
                "add",
                worktree_dir.path().to_str().unwrap(),
                "my-feature",
            ],
        );

        // Step 3: Use Config::write_repo from the worktree (this simulates lt init)
        let repo_config = RepoConfig {
            trunk: Some("main".to_string()),
            remote: Some("origin".to_string()),
            ..Default::default()
        };

        let write_path = Config::write_repo(worktree_dir.path(), &repo_config)
            .expect("Config::write_repo should succeed from worktree");

        // Verify the path is in the shared location (common_dir), not a per-worktree location
        // For a worktree, config should be in <bare_repo>/lattice/config.toml
        assert!(
            write_path.exists(),
            "Config file should exist at {:?}",
            write_path
        );

        // Step 4: Config::load from worktree should find it
        let loaded_from_wt = Config::load(Some(worktree_dir.path()))
            .expect("Config::load should succeed from worktree");
        assert_eq!(
            loaded_from_wt.config.trunk(),
            Some("main"),
            "Config::load from worktree should return correct trunk"
        );
        assert_eq!(
            loaded_from_wt.config.remote(),
            "origin",
            "Config::load from worktree should return correct remote"
        );

        // Step 5: Config::load from bare repo should find the same config
        let loaded_from_bare =
            Config::load(Some(bare.path())).expect("Config::load should succeed from bare repo");
        assert_eq!(
            loaded_from_bare.config.trunk(),
            Some("main"),
            "Config::load from bare repo should return same trunk"
        );
        assert_eq!(
            loaded_from_bare.config.remote(),
            "origin",
            "Config::load from bare repo should return same remote"
        );
    }
}
