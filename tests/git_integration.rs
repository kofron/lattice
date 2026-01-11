//! Integration tests for the Git interface.
//!
//! These tests use real git repositories created via tempfile to verify
//! that the Git interface works correctly with actual git operations.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use latticework::core::types::Oid;
use latticework::git::{Git, GitError, GitState};

/// Test fixture that creates a real git repository.
struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    /// Create a new test repository with an initial commit.
    fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");

        // Initialize git repo
        run_git(dir.path(), &["init"]);
        run_git(dir.path(), &["config", "user.email", "test@example.com"]);
        run_git(dir.path(), &["config", "user.name", "Test User"]);

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

    /// Create a file and commit it, returning the new commit OID.
    fn commit_file(&self, path: &str, content: &str, message: &str) -> Oid {
        std::fs::write(self.dir.path().join(path), content).unwrap();
        run_git(self.path(), &["add", path]);
        run_git(self.path(), &["commit", "-m", message]);

        // Get the new HEAD
        self.git().head_oid().unwrap()
    }

    /// Create a branch at the current HEAD.
    fn create_branch(&self, name: &str) {
        run_git(self.path(), &["branch", name]);
    }

    /// Checkout a branch.
    fn checkout(&self, name: &str) {
        run_git(self.path(), &["checkout", name]);
    }

    /// Get HEAD OID using git directly.
    fn head_oid_raw(&self) -> String {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(self.path())
            .output()
            .expect("git rev-parse failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
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
// Repository Opening Tests
// =============================================================================

#[test]
fn open_valid_repository() {
    let repo = TestRepo::new();
    let git = Git::open(repo.path());
    assert!(git.is_ok());
}

#[test]
fn open_from_subdirectory() {
    let repo = TestRepo::new();
    let subdir = repo.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();

    let git = Git::open(&subdir);
    assert!(git.is_ok());
}

#[test]
fn open_non_repository_fails() {
    let dir = TempDir::new().unwrap();
    let git = Git::open(dir.path());
    assert!(matches!(git, Err(GitError::NotARepo { .. })));
}

#[test]
fn repo_info() {
    let repo = TestRepo::new();
    let git = repo.git();
    let info = git.info().unwrap();

    assert!(info.git_dir.ends_with(".git"));
    // For normal repos, common_dir should equal git_dir
    assert_eq!(info.git_dir, info.common_dir);
    // Context should be Normal for a regular repo
    assert_eq!(info.context, latticework::git::RepoContext::Normal);
    // Use canonicalize to handle macOS /var -> /private/var symlink
    let expected = repo.path().canonicalize().unwrap();
    let actual = info
        .work_dir
        .expect("normal repo should have work_dir")
        .canonicalize()
        .unwrap();
    assert_eq!(actual, expected);
}

// =============================================================================
// Ref Resolution Tests
// =============================================================================

#[test]
fn resolve_ref_head() {
    let repo = TestRepo::new();
    let git = repo.git();

    let oid = git.resolve_ref("HEAD").unwrap();
    assert_eq!(oid.as_str().len(), 40);
}

#[test]
fn resolve_ref_branch() {
    let repo = TestRepo::new();
    let git = repo.git();

    // Default branch after init might be main or master
    let branches = git.list_branches().unwrap();
    assert!(!branches.is_empty());

    let branch_ref = format!("refs/heads/{}", branches[0].as_str());
    let oid = git.resolve_ref(&branch_ref).unwrap();
    assert_eq!(oid.as_str().len(), 40);
}

#[test]
fn resolve_ref_not_found() {
    let repo = TestRepo::new();
    let git = repo.git();

    let result = git.resolve_ref("refs/heads/nonexistent");
    assert!(matches!(result, Err(GitError::RefNotFound { .. })));
}

#[test]
fn try_resolve_ref_returns_none_for_missing() {
    let repo = TestRepo::new();
    let git = repo.git();

    let result = git.try_resolve_ref("refs/heads/nonexistent").unwrap();
    assert!(result.is_none());
}

#[test]
fn try_resolve_ref_returns_some_for_existing() {
    let repo = TestRepo::new();
    let git = repo.git();

    let result = git.try_resolve_ref("HEAD").unwrap();
    assert!(result.is_some());
}

#[test]
fn head_oid_matches_git() {
    let repo = TestRepo::new();
    let git = repo.git();

    let our_oid = git.head_oid().unwrap();
    let git_oid = repo.head_oid_raw();

    assert_eq!(our_oid.as_str(), git_oid);
}

#[test]
fn ref_exists_true_for_existing() {
    let repo = TestRepo::new();
    let git = repo.git();

    assert!(git.ref_exists("HEAD"));
}

#[test]
fn ref_exists_false_for_missing() {
    let repo = TestRepo::new();
    let git = repo.git();

    assert!(!git.ref_exists("refs/heads/nonexistent"));
}

// =============================================================================
// Branch Operations Tests
// =============================================================================

#[test]
fn list_branches_includes_default() {
    let repo = TestRepo::new();
    let git = repo.git();

    let branches = git.list_branches().unwrap();
    assert!(!branches.is_empty());
}

#[test]
fn list_branches_includes_created() {
    let repo = TestRepo::new();
    repo.create_branch("feature");

    let git = repo.git();
    let branches = git.list_branches().unwrap();

    let names: Vec<_> = branches.iter().map(|b| b.as_str()).collect();
    assert!(names.contains(&"feature"));
}

#[test]
fn current_branch_returns_checked_out() {
    let repo = TestRepo::new();
    repo.create_branch("feature");
    repo.checkout("feature");

    let git = repo.git();
    let current = git.current_branch().unwrap();

    assert_eq!(current.unwrap().as_str(), "feature");
}

// =============================================================================
// CAS Ref Operations Tests
// =============================================================================

#[test]
fn update_ref_cas_create_new() {
    let repo = TestRepo::new();
    let git = repo.git();
    let head_oid = git.head_oid().unwrap();

    // Create a new metadata ref
    let result = git.update_ref_cas(
        "refs/branch-metadata/test",
        &head_oid,
        None, // Must not exist
        "test: create metadata",
    );

    assert!(result.is_ok());
    assert!(git.ref_exists("refs/branch-metadata/test"));
}

#[test]
fn update_ref_cas_fails_if_exists_when_creating() {
    let repo = TestRepo::new();
    let git = repo.git();
    let head_oid = git.head_oid().unwrap();

    // Create the ref first
    git.update_ref_cas("refs/branch-metadata/test", &head_oid, None, "first")
        .unwrap();

    // Try to create again (should fail)
    let result = git.update_ref_cas("refs/branch-metadata/test", &head_oid, None, "second");

    assert!(matches!(result, Err(GitError::CasFailed { .. })));
}

#[test]
fn update_ref_cas_update_existing() {
    let repo = TestRepo::new();
    let git = repo.git();

    // Create initial state
    let old_oid = git.head_oid().unwrap();
    git.update_ref_cas("refs/branch-metadata/test", &old_oid, None, "create")
        .unwrap();

    // Add a commit
    let new_oid = repo.commit_file("file.txt", "content", "New commit");

    // Update with correct expected value
    let result = git.update_ref_cas(
        "refs/branch-metadata/test",
        &new_oid,
        Some(&old_oid),
        "update",
    );

    assert!(result.is_ok());

    // Verify update
    let current = git.resolve_ref("refs/branch-metadata/test").unwrap();
    assert_eq!(current, new_oid);
}

#[test]
fn update_ref_cas_fails_on_wrong_expected() {
    let repo = TestRepo::new();
    let git = repo.git();

    // Create initial state
    let old_oid = git.head_oid().unwrap();
    git.update_ref_cas("refs/branch-metadata/test", &old_oid, None, "create")
        .unwrap();

    // Add commits
    let new_oid = repo.commit_file("file.txt", "content", "New commit");
    let wrong_oid = repo.commit_file("file2.txt", "content", "Another commit");

    // Try to update with wrong expected value
    let result = git.update_ref_cas(
        "refs/branch-metadata/test",
        &new_oid,
        Some(&wrong_oid), // Wrong!
        "update",
    );

    assert!(matches!(result, Err(GitError::CasFailed { .. })));
}

#[test]
fn delete_ref_cas_success() {
    let repo = TestRepo::new();
    let git = repo.git();

    // Create a ref
    let oid = git.head_oid().unwrap();
    git.update_ref_cas("refs/branch-metadata/test", &oid, None, "create")
        .unwrap();

    // Delete with correct expected
    let result = git.delete_ref_cas("refs/branch-metadata/test", &oid);
    assert!(result.is_ok());
    assert!(!git.ref_exists("refs/branch-metadata/test"));
}

#[test]
fn delete_ref_cas_fails_on_wrong_expected() {
    let repo = TestRepo::new();
    let git = repo.git();

    // Create a ref
    let oid = git.head_oid().unwrap();
    git.update_ref_cas("refs/branch-metadata/test", &oid, None, "create")
        .unwrap();

    // Create a different oid
    let wrong_oid = repo.commit_file("file.txt", "content", "commit");

    // Try to delete with wrong expected
    let result = git.delete_ref_cas("refs/branch-metadata/test", &wrong_oid);
    assert!(matches!(result, Err(GitError::CasFailed { .. })));

    // Ref should still exist
    assert!(git.ref_exists("refs/branch-metadata/test"));
}

// =============================================================================
// Ref Enumeration Tests
// =============================================================================

#[test]
fn list_refs_by_prefix_empty_namespace() {
    let repo = TestRepo::new();
    let git = repo.git();

    let refs = git.list_refs_by_prefix("refs/branch-metadata/").unwrap();
    assert!(refs.is_empty());
}

#[test]
fn list_refs_by_prefix_finds_refs() {
    let repo = TestRepo::new();
    let git = repo.git();

    // Create some metadata refs
    let oid = git.head_oid().unwrap();
    git.update_ref_cas("refs/branch-metadata/foo", &oid, None, "create")
        .unwrap();
    git.update_ref_cas("refs/branch-metadata/bar", &oid, None, "create")
        .unwrap();

    let refs = git.list_refs_by_prefix("refs/branch-metadata/").unwrap();
    assert_eq!(refs.len(), 2);

    let names: Vec<_> = refs.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"refs/branch-metadata/foo"));
    assert!(names.contains(&"refs/branch-metadata/bar"));
}

#[test]
fn list_metadata_refs_extracts_branch_names() {
    let repo = TestRepo::new();
    let git = repo.git();

    // Create metadata refs
    let oid = git.head_oid().unwrap();
    git.update_ref_cas("refs/branch-metadata/feature-a", &oid, None, "create")
        .unwrap();
    git.update_ref_cas("refs/branch-metadata/feature-b", &oid, None, "create")
        .unwrap();

    let refs = git.list_metadata_refs().unwrap();
    assert_eq!(refs.len(), 2);

    let names: Vec<_> = refs.iter().map(|(b, _)| b.as_str()).collect();
    assert!(names.contains(&"feature-a"));
    assert!(names.contains(&"feature-b"));
}

// =============================================================================
// Ancestry Tests
// =============================================================================

#[test]
fn is_ancestor_true_for_parent() {
    let repo = TestRepo::new();
    let git = repo.git();

    let parent = git.head_oid().unwrap();
    let child = repo.commit_file("file.txt", "content", "child commit");

    assert!(git.is_ancestor(&parent, &child).unwrap());
}

#[test]
fn is_ancestor_false_for_unrelated() {
    let repo = TestRepo::new();
    let git = repo.git();

    let _first = git.head_oid().unwrap();

    // Create a branch and add a commit there
    repo.create_branch("other");
    repo.checkout("other");
    let other = repo.commit_file("other.txt", "content", "other commit");

    // Go back and create divergent commit
    let branches = git.list_branches().unwrap();
    let default_branch = branches.iter().find(|b| b.as_str() != "other").unwrap();
    repo.checkout(default_branch.as_str());
    let divergent = repo.commit_file("divergent.txt", "content", "divergent commit");

    // divergent is not an ancestor of other (they diverged from first)
    // But actually they share the initial commit as ancestor, so we need
    // to test that other is not an ancestor of divergent
    assert!(!git.is_ancestor(&other, &divergent).unwrap());
}

#[test]
fn is_ancestor_true_for_same_commit() {
    let repo = TestRepo::new();
    let git = repo.git();

    let oid = git.head_oid().unwrap();
    assert!(git.is_ancestor(&oid, &oid).unwrap());
}

#[test]
fn merge_base_finds_common_ancestor() {
    let repo = TestRepo::new();
    let git = repo.git();

    let base = git.head_oid().unwrap();

    // Create two branches with commits
    repo.create_branch("branch-a");
    repo.create_branch("branch-b");

    repo.checkout("branch-a");
    let commit_a = repo.commit_file("a.txt", "a", "commit a");

    repo.checkout("branch-b");
    let commit_b = repo.commit_file("b.txt", "b", "commit b");

    // Find merge base
    let merge_base = git.merge_base(&commit_a, &commit_b).unwrap();
    assert_eq!(merge_base, Some(base));
}

#[test]
fn commit_count_linear_history() {
    let repo = TestRepo::new();
    let git = repo.git();

    let base = git.head_oid().unwrap();
    repo.commit_file("1.txt", "1", "commit 1");
    repo.commit_file("2.txt", "2", "commit 2");
    let tip = repo.commit_file("3.txt", "3", "commit 3");

    let count = git.commit_count(&base, &tip).unwrap();
    assert_eq!(count, 3);
}

#[test]
fn commit_count_same_commit_is_zero() {
    let repo = TestRepo::new();
    let git = repo.git();

    let oid = git.head_oid().unwrap();
    let count = git.commit_count(&oid, &oid).unwrap();
    assert_eq!(count, 0);
}

// =============================================================================
// Blob Operations Tests
// =============================================================================

#[test]
fn write_and_read_blob() {
    let repo = TestRepo::new();
    let git = repo.git();

    let content = b"Hello, World!";
    let oid = git.write_blob(content).unwrap();

    let read_back = git.read_blob(&oid).unwrap();
    assert_eq!(read_back, content);
}

#[test]
fn read_blob_as_string() {
    let repo = TestRepo::new();
    let git = repo.git();

    let content = "Hello, UTF-8!";
    let oid = git.write_blob(content.as_bytes()).unwrap();

    let read_back = git.read_blob_as_string(&oid).unwrap();
    assert_eq!(read_back, content);
}

#[test]
fn read_nonexistent_blob_fails() {
    let repo = TestRepo::new();
    let git = repo.git();

    let fake_oid = Oid::new("0000000000000000000000000000000000000000").unwrap();
    let result = git.read_blob(&fake_oid);
    assert!(matches!(result, Err(GitError::ObjectNotFound { .. })));
}

// =============================================================================
// State Detection Tests
// =============================================================================

#[test]
fn clean_state_when_no_operation() {
    let repo = TestRepo::new();
    let git = repo.git();

    let state = git.state();
    assert_eq!(state, GitState::Clean);
    assert!(!state.is_in_progress());
}

#[test]
fn has_conflicts_false_normally() {
    let repo = TestRepo::new();
    let git = repo.git();

    assert!(!git.has_conflicts().unwrap());
}

// =============================================================================
// Working Tree Status Tests
// =============================================================================

#[test]
fn worktree_status_clean() {
    let repo = TestRepo::new();
    let git = repo.git();

    let status = git.worktree_status(false).unwrap();
    assert!(status.is_clean());
    assert!(!status.is_dirty());
    assert!(!status.is_unavailable());
}

#[test]
fn worktree_status_staged_changes() {
    let repo = TestRepo::new();

    // Stage a change
    std::fs::write(repo.path().join("new.txt"), "content").unwrap();
    run_git(repo.path(), &["add", "new.txt"]);

    let git = repo.git();
    let status = git.worktree_status(false).unwrap();

    assert!(!status.is_clean());
    assert!(status.is_dirty());
    assert!(status.has_staged());
}

#[test]
fn worktree_status_unstaged_changes() {
    let repo = TestRepo::new();

    // Modify an existing tracked file
    std::fs::write(repo.path().join("README.md"), "Modified content").unwrap();

    let git = repo.git();
    let status = git.worktree_status(false).unwrap();

    assert!(!status.is_clean());
    assert!(status.is_dirty());
}

#[test]
fn worktree_status_untracked_not_counted() {
    let repo = TestRepo::new();

    // Create an untracked file - should not affect clean status
    // per SPEC.md §4.6.9, untracked files are not part of WorktreeStatus
    std::fs::write(repo.path().join("untracked.txt"), "content").unwrap();

    let git = repo.git();

    // Untracked files don't make the worktree dirty
    let status = git.worktree_status(false).unwrap();
    assert!(status.is_clean());
}

#[test]
fn is_worktree_clean_true_when_clean() {
    let repo = TestRepo::new();
    let git = repo.git();

    assert!(git.is_worktree_clean().unwrap());
}

#[test]
fn is_worktree_clean_false_with_changes() {
    let repo = TestRepo::new();
    std::fs::write(repo.path().join("README.md"), "Modified").unwrap();

    let git = repo.git();
    assert!(!git.is_worktree_clean().unwrap());
}

// =============================================================================
// Commit Information Tests
// =============================================================================

#[test]
fn commit_info_returns_correct_data() {
    let repo = TestRepo::new();
    let git = repo.git();

    let oid = git.head_oid().unwrap();
    let info = git.commit_info(&oid).unwrap();

    assert_eq!(info.oid, oid);
    assert_eq!(info.summary, "Initial commit");
    assert_eq!(info.author_name, "Test User");
    assert_eq!(info.author_email, "test@example.com");
}

#[test]
fn commit_parents_returns_empty_for_root() {
    let repo = TestRepo::new();
    let git = repo.git();

    let oid = git.head_oid().unwrap();
    let parents = git.commit_parents(&oid).unwrap();

    assert!(parents.is_empty());
}

#[test]
fn commit_parents_returns_parent() {
    let repo = TestRepo::new();
    let git = repo.git();

    let parent = git.head_oid().unwrap();
    let child = repo.commit_file("file.txt", "content", "child");

    let parents = git.commit_parents(&child).unwrap();
    assert_eq!(parents.len(), 1);
    assert_eq!(parents[0], parent);
}

// =============================================================================
// Remote Operations Tests
// =============================================================================

#[test]
fn remote_url_none_when_no_remotes() {
    let repo = TestRepo::new();
    let git = repo.git();

    let url = git.remote_url("origin").unwrap();
    assert!(url.is_none());
}

#[test]
fn default_remote_none_when_no_remotes() {
    let repo = TestRepo::new();
    let git = repo.git();

    let remote = git.default_remote().unwrap();
    assert!(remote.is_none());
}

#[test]
fn remote_url_returns_configured_url() {
    let repo = TestRepo::new();
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/test/repo.git",
        ],
    );

    let git = repo.git();
    let url = git.remote_url("origin").unwrap();
    assert_eq!(url, Some("https://github.com/test/repo.git".to_string()));
}

#[test]
fn default_remote_prefers_origin() {
    let repo = TestRepo::new();
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/upstream/repo.git",
        ],
    );
    run_git(
        repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/origin/repo.git",
        ],
    );

    let git = repo.git();
    let remote = git.default_remote().unwrap();
    assert_eq!(remote, Some("origin".to_string()));
}
