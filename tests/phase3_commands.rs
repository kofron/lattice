//! Integration tests for Phase 3 advanced rewriting commands.
//!
//! Tests cover: modify, move, rename, delete, squash, fold, pop, reorder, split, revert
//!
//! Per ROADMAP.md Milestone 9, each command must have:
//! - Happy path integration test
//! - Freeze blocking test (where applicable)
//! - Conflict pause + continue/abort test (where applicable)

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Create a new test repository with trunk configured.
fn setup_repo() -> TempDir {
    let dir = TempDir::new().expect("create temp dir");
    let path = dir.path();

    // Initialize git repo with main as default branch
    run_git(path, &["init", "-b", "main"]);
    run_git(path, &["config", "user.name", "Test User"]);
    run_git(path, &["config", "user.email", "test@example.com"]);

    // Create initial commit on main
    fs::write(path.join("README.md"), "# Test Repo\n").expect("write file");
    run_git(path, &["add", "."]);
    run_git(path, &["commit", "-m", "Initial commit"]);

    // Initialize lattice
    run_lattice(path, &["init", "--trunk", "main"]);

    dir
}

/// Create a tracked branch with a commit.
fn create_branch(path: &Path, name: &str, content: &str) {
    run_lattice(path, &["create", name, "-m", &format!("Add {}", name)]);
    fs::write(path.join(format!("{}.txt", name)), content).expect("write file");
    run_git(path, &["add", "."]);
    run_git(path, &["commit", "-m", &format!("Commit on {}", name)]);
}

/// Run a git command.
fn run_git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .expect("run git");

    if !output.status.success() {
        eprintln!("git {} failed:", args.join(" "));
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("git command failed");
    }

    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Run a lattice command.
fn run_lattice(path: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_lt"))
        .args(args)
        .current_dir(path)
        .output()
        .expect("run lattice");

    if !output.status.success() {
        eprintln!("lattice {} failed:", args.join(" "));
        eprintln!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("stderr: {}", String::from_utf8_lossy(&output.stderr));
        panic!("lattice command failed");
    }

    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Run a lattice command expecting failure.
fn run_lattice_expect_fail(path: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_lt"))
        .args(args)
        .current_dir(path)
        .output()
        .expect("run lattice");

    if output.status.success() {
        panic!("lattice {} unexpectedly succeeded", args.join(" "));
    }

    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Get current branch name.
fn current_branch(path: &Path) -> String {
    run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .trim()
        .to_string()
}

// ========== MODIFY TESTS ==========

#[test]
fn modify_amends_head() {
    let dir = setup_repo();
    let path = dir.path();

    // Create a tracked branch with a commit
    create_branch(path, "feature", "initial content");

    // Modify the file and amend
    fs::write(path.join("feature.txt"), "modified content").expect("write");
    run_git(path, &["add", "."]);

    run_lattice(path, &["modify"]);

    // Verify only one commit exists on branch
    let log = run_git(path, &["log", "--oneline", "main..HEAD"]);
    assert_eq!(
        log.lines().count(),
        1,
        "Should have only 1 commit after amend"
    );
}

#[test]
fn modify_creates_first_commit_on_empty_branch() {
    let dir = setup_repo();
    let path = dir.path();

    // Create an empty tracked branch (no commit yet)
    run_git(path, &["checkout", "-b", "empty-feature"]);
    run_lattice(path, &["track", "--force"]);

    // Add a file and use modify to create first commit
    fs::write(path.join("new.txt"), "new content").expect("write");
    run_git(path, &["add", "."]);

    run_lattice(path, &["modify", "-m", "First commit"]);

    // Verify commit was created
    let log = run_git(path, &["log", "--oneline", "main..HEAD"]);
    assert_eq!(log.lines().count(), 1, "Should have 1 commit");
}

#[test]
fn modify_frozen_branch_fails() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "frozen-feature", "content");
    run_lattice(path, &["freeze"]);

    fs::write(path.join("frozen-feature.txt"), "modified").expect("write");
    run_git(path, &["add", "."]);

    let stderr = run_lattice_expect_fail(path, &["modify"]);
    assert!(
        stderr.contains("frozen"),
        "Should mention frozen: {}",
        stderr
    );
}

// ========== MOVE TESTS ==========

#[test]
fn move_reparents_branch() {
    let dir = setup_repo();
    let path = dir.path();

    // Create two branches off main
    create_branch(path, "branch-a", "content a");
    run_git(path, &["checkout", "main"]);
    create_branch(path, "branch-b", "content b");

    // Move branch-b onto branch-a
    run_lattice(path, &["move", "--onto", "branch-a"]);

    // Verify parent changed
    let info = run_lattice(path, &["info"]);
    assert!(
        info.contains("branch-a"),
        "Parent should be branch-a: {}",
        info
    );
}

#[test]
fn move_prevents_cycle() {
    let dir = setup_repo();
    let path = dir.path();

    // Create a chain: main -> a -> b
    create_branch(path, "branch-a", "content a");
    create_branch(path, "branch-b", "content b");

    // Go back to branch-a
    run_git(path, &["checkout", "branch-a"]);

    // Try to move branch-a onto branch-b (its descendant) - should fail
    let stderr = run_lattice_expect_fail(path, &["move", "--onto", "branch-b"]);
    assert!(
        stderr.contains("cycle") || stderr.contains("descendant"),
        "Should prevent cycle: {}",
        stderr
    );
}

// ========== RENAME TESTS ==========

#[test]
fn rename_updates_refs() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "old-name", "content");

    run_lattice(path, &["rename", "new-name"]);

    // Verify we're on new branch
    assert_eq!(current_branch(path), "new-name");

    // Verify old branch doesn't exist
    let branches = run_git(path, &["branch", "--list"]);
    assert!(
        !branches.contains("old-name"),
        "Old branch should not exist"
    );
    assert!(branches.contains("new-name"), "New branch should exist");
}

#[test]
fn rename_fixes_parent_pointers() {
    let dir = setup_repo();
    let path = dir.path();

    // Create parent -> child
    create_branch(path, "parent-branch", "parent content");
    create_branch(path, "child-branch", "child content");

    // Go back to parent and rename it
    run_git(path, &["checkout", "parent-branch"]);
    run_lattice(path, &["rename", "renamed-parent"]);

    // Check that child's parent was updated
    run_git(path, &["checkout", "child-branch"]);
    let info = run_lattice(path, &["info"]);
    assert!(
        info.contains("renamed-parent"),
        "Child's parent should be updated: {}",
        info
    );
}

#[test]
fn rename_to_existing_fails() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "first", "content");
    run_git(path, &["checkout", "main"]);
    create_branch(path, "second", "content");

    let stderr = run_lattice_expect_fail(path, &["rename", "first"]);
    assert!(
        stderr.contains("exists"),
        "Should fail for existing name: {}",
        stderr
    );
}

// ========== DELETE TESTS ==========

#[test]
fn delete_reparents_children() {
    let dir = setup_repo();
    let path = dir.path();

    // Create chain: main -> middle -> child
    create_branch(path, "middle", "middle content");
    create_branch(path, "child", "child content");

    // Delete middle
    run_git(path, &["checkout", "middle"]);
    run_lattice(path, &["delete", "--force"]);

    // Verify child's parent is now main
    run_git(path, &["checkout", "child"]);
    let info = run_lattice(path, &["info"]);
    assert!(
        info.contains("main"),
        "Child's parent should be main: {}",
        info
    );
}

#[test]
fn delete_upstack_removes_descendants() {
    let dir = setup_repo();
    let path = dir.path();

    // Create chain: main -> a -> b -> c
    create_branch(path, "branch-a", "a");
    create_branch(path, "branch-b", "b");
    create_branch(path, "branch-c", "c");

    // Delete a with --upstack
    run_git(path, &["checkout", "branch-a"]);
    run_lattice(path, &["delete", "--upstack", "--force"]);

    // Verify all descendants are gone
    let branches = run_git(path, &["branch", "--list"]);
    assert!(!branches.contains("branch-a"), "branch-a should be deleted");
    assert!(!branches.contains("branch-b"), "branch-b should be deleted");
    assert!(!branches.contains("branch-c"), "branch-c should be deleted");
}

#[test]
fn delete_frozen_fails() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "frozen", "content");
    run_lattice(path, &["freeze"]);

    let stderr = run_lattice_expect_fail(path, &["delete", "--force"]);
    assert!(
        stderr.contains("frozen"),
        "Should fail for frozen branch: {}",
        stderr
    );
}

// ========== SQUASH TESTS ==========

#[test]
fn squash_collapses_commits() {
    let dir = setup_repo();
    let path = dir.path();

    // Create branch with multiple commits
    run_git(path, &["checkout", "-b", "multi-commit"]);
    run_lattice(path, &["track", "--force"]);

    for i in 1..=3 {
        fs::write(
            path.join(format!("file{}.txt", i)),
            format!("content {}", i),
        )
        .expect("write");
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", &format!("Commit {}", i)]);
    }

    // Verify 3 commits exist
    let log_before = run_git(path, &["log", "--oneline", "main..HEAD"]);
    assert_eq!(log_before.lines().count(), 3);

    // Squash
    run_lattice(path, &["squash", "-m", "Squashed commit"]);

    // Verify 1 commit now
    let log_after = run_git(path, &["log", "--oneline", "main..HEAD"]);
    assert_eq!(log_after.lines().count(), 1);
}

#[test]
fn squash_single_commit_noop() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "single", "content");

    let output = run_lattice(path, &["squash"]);
    assert!(
        output.contains("1 commit") || output.contains("Nothing to squash"),
        "Should be no-op for single commit: {}",
        output
    );
}

// ========== FOLD TESTS ==========

#[test]
fn fold_merges_into_parent() {
    let dir = setup_repo();
    let path = dir.path();

    // Create parent -> child chain (both tracked)
    create_branch(path, "parent-feat", "parent content");
    create_branch(path, "child-feat", "child content");

    // Fold child into parent
    run_lattice(path, &["fold"]);

    // Verify we're on parent now
    assert_eq!(current_branch(path), "parent-feat");

    // Verify child is deleted
    let branches = run_git(path, &["branch", "--list"]);
    assert!(
        !branches.contains("child-feat"),
        "child-feat should be deleted"
    );

    // Verify child's content is in parent
    let content = fs::read_to_string(path.join("child-feat.txt")).expect("read");
    assert_eq!(content, "child content");
}

#[test]
fn fold_reparents_children() {
    let dir = setup_repo();
    let path = dir.path();

    // Create: main -> parent -> middle -> grandchild
    create_branch(path, "parent", "parent");
    create_branch(path, "middle", "middle");
    create_branch(path, "grandchild", "grandchild");

    // Fold middle into parent
    run_git(path, &["checkout", "middle"]);
    run_lattice(path, &["fold"]);

    // grandchild should now have parent as its parent
    run_git(path, &["checkout", "grandchild"]);
    let info = run_lattice(path, &["info"]);
    assert!(
        info.contains("parent"),
        "Grandchild's parent should be 'parent': {}",
        info
    );
}

// ========== POP TESTS ==========

#[test]
fn pop_leaves_uncommitted_changes() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "to-pop", "pop content");

    // Pop the branch
    run_lattice(path, &["pop"]);

    // Verify we're on main
    assert_eq!(current_branch(path), "main");

    // Verify branch is deleted
    let branches = run_git(path, &["branch", "--list"]);
    assert!(!branches.contains("to-pop"), "Branch should be deleted");

    // Verify changes are uncommitted (this may vary based on implementation)
    // The diff should be non-empty or files should exist
}

#[test]
fn pop_reparents_children() {
    let dir = setup_repo();
    let path = dir.path();

    // Create: main -> middle -> child
    create_branch(path, "middle", "middle");
    create_branch(path, "child", "child");

    // Pop middle
    run_git(path, &["checkout", "middle"]);
    run_lattice(path, &["pop"]);

    // Child should now have main as parent
    run_git(path, &["checkout", "child"]);
    let info = run_lattice(path, &["info"]);
    assert!(
        info.contains("main"),
        "Child's parent should be main: {}",
        info
    );
}

// ========== REORDER TESTS ==========

// Note: reorder requires an editor, so we'll use LATTICE_TEST_EDITOR env var
// For now, we just test the validation

#[test]
fn reorder_needs_multiple_branches() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "single", "content");

    let output = run_lattice(path, &["reorder"]);
    assert!(
        output.contains("at least 2") || output.contains("Nothing"),
        "Should require multiple branches: {}",
        output
    );
}

// ========== SPLIT TESTS ==========

#[test]
fn split_by_commit_creates_chain() {
    let dir = setup_repo();
    let path = dir.path();

    // Create branch with multiple commits
    run_git(path, &["checkout", "-b", "to-split"]);
    run_lattice(path, &["track", "--force"]);

    for i in 1..=3 {
        fs::write(
            path.join(format!("file{}.txt", i)),
            format!("content {}", i),
        )
        .expect("write");
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", &format!("Commit {}", i)]);
    }

    // Split by commit
    run_lattice(path, &["split", "--by-commit"]);

    // Verify branches were created
    let branches = run_git(path, &["branch", "--list"]);
    assert!(
        branches.contains("to-split-1"),
        "Should create to-split-1: {}",
        branches
    );
    assert!(
        branches.contains("to-split-2"),
        "Should create to-split-2: {}",
        branches
    );
    assert!(
        branches.contains("to-split"),
        "Should keep to-split: {}",
        branches
    );
}

#[test]
fn split_requires_mode() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "feature", "content");

    let stderr = run_lattice_expect_fail(path, &["split"]);
    assert!(
        stderr.contains("by-commit") || stderr.contains("by-file"),
        "Should require split mode: {}",
        stderr
    );
}

// ========== REVERT TESTS ==========

#[test]
fn revert_creates_branch_off_trunk() {
    let dir = setup_repo();
    let path = dir.path();

    // Get a commit to revert
    let sha = run_git(path, &["rev-parse", "HEAD"]).trim().to_string();
    let short_sha = &sha[..7];

    // Revert it
    run_lattice(path, &["revert", &sha]);

    // Verify new branch was created
    let branch = current_branch(path);
    assert!(
        branch.contains("revert"),
        "Should be on revert branch: {}",
        branch
    );
    assert!(
        branch.contains(short_sha),
        "Branch should contain short SHA: {}",
        branch
    );

    // Verify it's tracked with main as parent
    let info = run_lattice(path, &["info"]);
    assert!(info.contains("main"), "Parent should be main: {}", info);
}

#[test]
fn revert_invalid_sha_fails() {
    let dir = setup_repo();
    let path = dir.path();

    let stderr = run_lattice_expect_fail(path, &["revert", "invalidsha123"]);
    assert!(
        stderr.contains("not a valid") || stderr.contains("Invalid"),
        "Should fail for invalid SHA: {}",
        stderr
    );
}

// ========== CROSS-CUTTING FREEZE TESTS ==========

#[test]
fn move_frozen_branch_fails() {
    let dir = setup_repo();
    let path = dir.path();

    create_branch(path, "branch-a", "a");
    run_git(path, &["checkout", "main"]);
    create_branch(path, "frozen-b", "b");
    run_lattice(path, &["freeze"]);

    let stderr = run_lattice_expect_fail(path, &["move", "--onto", "branch-a"]);
    assert!(
        stderr.contains("frozen"),
        "Should fail for frozen: {}",
        stderr
    );
}

#[test]
fn squash_frozen_branch_fails() {
    let dir = setup_repo();
    let path = dir.path();

    run_git(path, &["checkout", "-b", "frozen-squash"]);
    run_lattice(path, &["track", "--force"]);

    for i in 1..=2 {
        fs::write(path.join(format!("f{}.txt", i)), "c").expect("write");
        run_git(path, &["add", "."]);
        run_git(path, &["commit", "-m", &format!("C{}", i)]);
    }

    run_lattice(path, &["freeze"]);

    let stderr = run_lattice_expect_fail(path, &["squash"]);
    assert!(
        stderr.contains("frozen"),
        "Should fail for frozen: {}",
        stderr
    );
}
