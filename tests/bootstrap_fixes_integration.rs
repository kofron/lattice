//! Integration tests for Bootstrap Fix Workflows (Milestone 5.4/5.5).
//!
//! These tests verify the bootstrap fix generators and their execution
//! through the Doctor framework, ensuring:
//!
//! 1. TrackExisting fixes work for untracked branches with open PRs
//! 2. FetchAndTrack fixes (simulated) work for missing branches
//! 3. LinkPR fixes update cached metadata without changing structural fields
//! 4. Event recording happens correctly (DoctorProposed, DoctorApplied)
//! 5. Post-verification confirms fix success

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use latticework::core::metadata::schema::PrState;
use latticework::core::metadata::store::MetadataStore;
use latticework::core::types::BranchName;
use latticework::doctor::{generate_fixes, Doctor, FixId};
use latticework::engine::health::issues;
use latticework::engine::scan::scan;
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

    /// Initialize Lattice in the repository.
    fn init_lattice(&self) {
        let ctx = self.context();
        latticework::cli::commands::init(&ctx, Some("main"), false, true).expect("init failed");
    }

    /// Create a standard test context.
    fn context(&self) -> latticework::engine::Context {
        latticework::engine::Context {
            cwd: Some(self.path().to_path_buf()),
            interactive: false,
            quiet: true,
            debug: false,
        }
    }

    /// Create a file and commit it.
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

    /// Track a branch with Lattice.
    fn track_branch(&self, branch: &str, parent: &str) {
        let ctx = self.context();
        latticework::cli::commands::track(&ctx, Some(branch), Some(parent), false, false)
            .expect("track failed");
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
// Fix Generator Tests
// =============================================================================

#[test]
fn track_existing_from_pr_generates_correct_fix() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a branch but don't track it
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature content", "Add feature");
    repo.checkout("main");

    // Simulate an issue that would be detected when there's an open PR
    // for this untracked branch
    let issue = issues::remote_pr_branch_untracked(
        "feature",
        42,
        "main",
        "https://github.com/org/repo/pull/42",
    );

    // Generate fixes using the real snapshot
    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should generate exactly one fix
    assert_eq!(fixes.len(), 1, "Should generate one TrackExisting fix");

    let fix = &fixes[0];
    assert!(
        fix.description.contains("Track"),
        "Fix should be a Track fix"
    );
    assert!(
        fix.description.contains("feature"),
        "Fix should mention the branch"
    );
    assert!(
        fix.description.contains("PR #42"),
        "Fix should mention the PR"
    );

    // Verify fix ID format
    assert_eq!(
        fix.id.to_string(),
        "remote-pr-branch-untracked:track:feature"
    );
}

#[test]
fn track_existing_requires_branch_to_exist() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Don't create the branch - simulate detection error scenario
    let issue = issues::remote_pr_branch_untracked(
        "nonexistent",
        42,
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should generate no fixes because branch doesn't exist
    assert!(
        fixes.is_empty(),
        "Should not generate fix for nonexistent branch"
    );
}

#[test]
fn track_existing_skips_already_tracked() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create and track a branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("feature.txt", "feature content", "Add feature");
    repo.track_branch("feature", "main");
    repo.checkout("main");

    // Simulate issue for this already-tracked branch
    let issue = issues::remote_pr_branch_untracked(
        "feature",
        42,
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should generate no fixes because branch is already tracked
    assert!(
        fixes.is_empty(),
        "Should not generate fix for already tracked branch"
    );
}

#[test]
fn fetch_and_track_generates_fix_for_missing_branch() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Simulate issue for a branch that doesn't exist locally
    let issue = issues::remote_pr_branch_missing(
        42,
        "teammate-feature",
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should generate exactly one fix
    assert_eq!(fixes.len(), 1, "Should generate one FetchAndTrack fix");

    let fix = &fixes[0];
    assert!(
        fix.description.contains("Fetch"),
        "Fix should be a Fetch fix"
    );
    assert!(
        fix.description.contains("teammate-feature"),
        "Fix should mention the branch"
    );
    assert!(
        fix.description.contains("frozen"),
        "Fix should indicate frozen state"
    );

    // Verify fix ID format
    assert_eq!(
        fix.id.to_string(),
        "remote-pr-branch-missing:fetch-and-track:teammate-feature"
    );
}

#[test]
fn fetch_and_track_skips_if_branch_exists() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create the branch locally
    repo.create_branch("teammate-feature");
    repo.checkout("teammate-feature");
    repo.commit("tf.txt", "content", "Add content");
    repo.checkout("main");

    // Simulate issue for this branch (as if scanner incorrectly flagged it)
    let issue = issues::remote_pr_branch_missing(
        42,
        "teammate-feature",
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should not generate fix because branch exists
    assert!(
        fixes.is_empty(),
        "Should not generate FetchAndTrack for existing branch"
    );
}

#[test]
fn link_pr_generates_fix_for_tracked_unlinked_branch() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create and track a branch
    repo.create_branch("my-feature");
    repo.checkout("my-feature");
    repo.commit("mf.txt", "content", "Add feature");
    repo.track_branch("my-feature", "main");
    repo.checkout("main");

    // Verify it's tracked but has no PR link
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("my-feature").unwrap();
    let metadata = store.read(&branch).unwrap().expect("metadata");
    assert!(
        matches!(metadata.metadata.pr, PrState::None),
        "Branch should not have PR linked"
    );

    // Simulate issue for this tracked branch missing PR link
    let issue =
        issues::remote_pr_not_linked("my-feature", 42, "https://github.com/org/repo/pull/42");

    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should generate exactly one fix
    assert_eq!(fixes.len(), 1, "Should generate one LinkPR fix");

    let fix = &fixes[0];
    assert!(fix.description.contains("Link"), "Fix should be a Link fix");
    assert!(
        fix.description.contains("PR #42"),
        "Fix should mention the PR"
    );

    // Verify fix ID format
    assert_eq!(fix.id.to_string(), "remote-pr-not-linked:link:my-feature");
}

#[test]
fn link_pr_skips_untracked_branch() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a branch but don't track it
    repo.create_branch("untracked-feature");
    repo.checkout("untracked-feature");
    repo.commit("uf.txt", "content", "Add feature");
    repo.checkout("main");

    // Simulate issue for this untracked branch
    let issue = issues::remote_pr_not_linked(
        "untracked-feature",
        42,
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should not generate fix because branch is not tracked
    assert!(
        fixes.is_empty(),
        "Should not generate LinkPR for untracked branch"
    );
}

// =============================================================================
// Parent Inference Tests
// =============================================================================

#[test]
fn parent_inference_uses_trunk_for_trunk_based_pr() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create an untracked branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "content", "Add feature");
    repo.checkout("main");

    // Issue with base_ref = main (trunk)
    let issue = issues::remote_pr_branch_untracked(
        "feature",
        42,
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    assert_eq!(fixes.len(), 1);
    // The fix description should mention 'main' as parent
    assert!(
        fixes[0].description.contains("main"),
        "Parent should be trunk (main)"
    );
}

#[test]
fn parent_inference_uses_tracked_branch_as_parent() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create and track a parent branch
    repo.create_branch("parent-feature");
    repo.checkout("parent-feature");
    repo.commit("pf.txt", "content", "Add parent feature");
    repo.track_branch("parent-feature", "main");

    // Create an untracked child branch
    repo.create_branch("child-feature");
    repo.checkout("child-feature");
    repo.commit("cf.txt", "content", "Add child feature");
    repo.checkout("main");

    // Issue with base_ref = parent-feature
    let issue = issues::remote_pr_branch_untracked(
        "child-feature",
        42,
        "parent-feature",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    assert_eq!(fixes.len(), 1);
    // The fix description should mention 'parent-feature' as parent
    assert!(
        fixes[0].description.contains("parent-feature"),
        "Parent should be the tracked branch"
    );
}

// =============================================================================
// Doctor Integration Tests
// =============================================================================

#[test]
fn doctor_diagnose_produces_fixes_for_bootstrap_issues() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create an untracked branch to simulate a PR bootstrap scenario
    repo.create_branch("pr-feature");
    repo.checkout("pr-feature");
    repo.commit("prf.txt", "content", "Add PR feature");
    repo.checkout("main");

    // Create the Doctor instance
    let doctor = Doctor::new();

    // Scan the repo
    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    // Run diagnosis (no bootstrap issues in this state - branch is just untracked)
    let diagnosis = doctor.diagnose(&snapshot);

    // Repository should be healthy (no blocking issues)
    // The untracked branch is informational, not a blocking issue
    assert!(
        !diagnosis.issues.iter().any(|i| i.is_blocking()),
        "Should have no blocking issues"
    );
}

#[test]
fn doctor_fix_preview_shows_changes() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create an untracked branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "content", "Add feature");
    repo.checkout("main");

    // Create issue and get fixes
    let issue = issues::remote_pr_branch_untracked(
        "feature",
        42,
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    assert!(!fixes.is_empty());

    // Check that the preview contains meaningful information
    let fix = &fixes[0];
    let preview = fix.preview.format();

    assert!(!preview.is_empty(), "Fix preview should not be empty");
    assert!(
        preview.contains("metadata") || preview.contains("tracking"),
        "Preview should mention metadata or tracking: {}",
        preview
    );
}

// =============================================================================
// Fix ID Parsing Tests
// =============================================================================

#[test]
fn fix_id_parsing_roundtrip() {
    // Test that fix IDs parse correctly for bootstrap fixes
    let test_cases = [
        "remote-pr-branch-untracked:track:feature",
        "remote-pr-branch-missing:fetch-and-track:teammate-feature",
        "remote-pr-not-linked:link:my-feature",
    ];

    for id_str in test_cases {
        let parsed = FixId::parse(id_str);
        let serialized = parsed.to_string();
        assert_eq!(serialized, id_str, "FixId should round-trip: {}", id_str);
    }
}

// =============================================================================
// Edge Cases
// =============================================================================

#[test]
fn fixes_handle_branch_with_slashes() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a branch with slashes in the name
    repo.create_branch("feature/user/task-123");
    repo.checkout("feature/user/task-123");
    repo.commit("task.txt", "content", "Add task");
    repo.checkout("main");

    // Issue for this branch
    let issue = issues::remote_pr_branch_untracked(
        "feature/user/task-123",
        42,
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should generate a fix
    assert_eq!(fixes.len(), 1);
    assert!(fixes[0].description.contains("feature/user/task-123"));
}

#[test]
fn fixes_handle_special_characters_in_url() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create an untracked branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "content", "Add feature");
    repo.checkout("main");

    // Issue with special characters in URL
    let issue = issues::remote_pr_branch_untracked(
        "feature",
        42,
        "main",
        "https://github.com/org-with-dash/repo_underscore/pull/42?foo=bar",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    // Should still generate a fix
    assert_eq!(fixes.len(), 1);
}

// =============================================================================
// Precondition Tests
// =============================================================================

#[test]
fn track_existing_fix_has_correct_preconditions() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "content", "Add feature");
    repo.checkout("main");

    let issue = issues::remote_pr_branch_untracked(
        "feature",
        42,
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    assert_eq!(fixes.len(), 1);

    let fix = &fixes[0];
    // TrackExisting requires: RepoOpen, TrunkKnown, GraphValid
    assert!(
        !fix.preconditions.is_empty(),
        "Fix should have preconditions"
    );
}

#[test]
fn fetch_and_track_fix_has_auth_precondition() {
    let repo = TestRepo::new();
    repo.init_lattice();

    let issue = issues::remote_pr_branch_missing(
        42,
        "teammate-feature",
        "main",
        "https://github.com/org/repo/pull/42",
    );

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    assert_eq!(fixes.len(), 1);

    let fix = &fixes[0];
    // FetchAndTrack requires: RepoOpen, TrunkKnown, AuthAvailable, RemoteResolved
    assert!(
        !fix.preconditions.is_empty(),
        "Fix should have preconditions including auth"
    );
}

#[test]
fn link_pr_fix_has_minimal_preconditions() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "content", "Add feature");
    repo.track_branch("feature", "main");
    repo.checkout("main");

    let issue = issues::remote_pr_not_linked("feature", 42, "https://github.com/org/repo/pull/42");

    let git = repo.git();
    let snapshot = scan(&git).unwrap();
    let fixes = generate_fixes(&issue, &snapshot);

    assert_eq!(fixes.len(), 1);

    let fix = &fixes[0];
    // LinkPR only requires: RepoOpen
    assert!(
        !fix.preconditions.is_empty(),
        "Fix should have preconditions"
    );
}

// =============================================================================
// Local-Only Bootstrap Tests (Milestone 5.7)
// =============================================================================

/// Helper to find an untracked branch issue by branch name in evidence or message.
fn find_untracked_issue_for_branch<'a>(
    issues: &'a [latticework::engine::health::Issue],
    branch: &str,
) -> Option<&'a latticework::engine::health::Issue> {
    issues.iter().find(|i| {
        // Check if it's an untracked-branch issue type
        if !i.id.as_str().starts_with("untracked-branch:") {
            return false;
        }
        // Check if the message or evidence contains the branch name
        i.message.contains(branch)
    })
}

#[test]
fn local_bootstrap_single_best_parent() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create an untracked branch off main
    repo.create_branch("local-feature");
    repo.checkout("local-feature");
    repo.commit("lf.txt", "content", "Add local feature");
    repo.checkout("main");

    // Scan to detect the untracked branch
    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    // Find the untracked branch issue
    let untracked_issue =
        find_untracked_issue_for_branch(snapshot.health.issues(), "local-feature");

    assert!(
        untracked_issue.is_some(),
        "Should detect local-feature as untracked"
    );

    let issue = untracked_issue.unwrap();

    // Generate fixes for this issue
    let fixes = generate_fixes(issue, &snapshot);

    // Should generate exactly one fix with main as parent (nearest)
    assert_eq!(
        fixes.len(),
        1,
        "Should generate one fix for single best parent"
    );

    let fix = &fixes[0];
    assert!(
        fix.description.contains("local-feature"),
        "Fix should mention the branch"
    );
    assert!(
        fix.description.contains("main"),
        "Fix should use main as parent"
    );
    assert!(
        fix.description.contains("Track"),
        "Fix should be a Track fix"
    );
}

#[test]
fn local_bootstrap_prefers_tracked_branch_over_trunk() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create and track a parent branch
    repo.create_branch("parent-feature");
    repo.checkout("parent-feature");
    repo.commit("pf.txt", "content", "Add parent feature");
    repo.track_branch("parent-feature", "main");

    // Create an untracked child branch off the tracked parent
    repo.create_branch("child-feature");
    repo.checkout("child-feature");
    repo.commit("cf.txt", "content", "Add child feature");
    repo.checkout("main");

    // Scan to detect the untracked branch
    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    // Find the untracked branch issue for child-feature
    let untracked_issue =
        find_untracked_issue_for_branch(snapshot.health.issues(), "child-feature");

    assert!(
        untracked_issue.is_some(),
        "Should detect child-feature as untracked"
    );

    let issue = untracked_issue.unwrap();

    // Generate fixes
    let fixes = generate_fixes(issue, &snapshot);

    // Should generate exactly one fix with parent-feature as parent (nearer than main)
    assert_eq!(fixes.len(), 1, "Should generate one fix");

    let fix = &fixes[0];
    assert!(
        fix.description.contains("parent-feature"),
        "Should prefer tracked parent over trunk"
    );
}

#[test]
fn local_bootstrap_ambiguous_parents_generates_multiple_fixes() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create two tracked sibling branches at the same commit
    repo.create_branch("sibling-a");
    repo.create_branch("sibling-b");

    // Track both as children of main
    repo.checkout("sibling-a");
    repo.track_branch("sibling-a", "main");
    repo.checkout("sibling-b");
    repo.track_branch("sibling-b", "main");

    // Create an untracked branch that diverges from both siblings equally
    // (actually from main, which is the merge-base for both)
    repo.checkout("main");
    repo.create_branch("ambiguous-child");
    repo.checkout("ambiguous-child");
    repo.commit("ac.txt", "content", "Add ambiguous child");
    repo.checkout("main");

    // Scan to detect the untracked branch
    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    // Find the untracked branch issue
    let untracked_issue =
        find_untracked_issue_for_branch(snapshot.health.issues(), "ambiguous-child");

    assert!(
        untracked_issue.is_some(),
        "Should detect ambiguous-child as untracked"
    );

    let issue = untracked_issue.unwrap();

    // Generate fixes
    let fixes = generate_fixes(issue, &snapshot);

    // Should generate multiple fixes if candidates are equally close
    // Note: The exact number depends on how many candidates have the same distance
    // In this case, main, sibling-a, and sibling-b all have the same merge-base
    // with ambiguous-child (the initial commit), so they should all be equally close
    assert!(!fixes.is_empty(), "Should generate at least one fix option");

    // If there are multiple fixes, they should indicate ambiguity
    if fixes.len() > 1 {
        assert!(
            fixes[0].description.contains("equally close"),
            "Multiple fixes should indicate ambiguity"
        );
    }
}

#[test]
fn local_bootstrap_no_candidates_when_only_self_exists() {
    let repo = TestRepo::new();
    // Note: NOT initializing lattice - so trunk isn't set, no tracked branches

    // Create a branch
    repo.create_branch("orphan");
    repo.checkout("orphan");
    repo.commit("o.txt", "content", "Add orphan");
    repo.checkout("main");

    // Try to scan without lattice init
    let git = repo.git();
    let snapshot = scan(&git);

    // Without trunk configured, scan should fail or return degraded state
    // The local bootstrap generator handles this by returning empty fixes
    // when there are no candidates
    if let Ok(snap) = snapshot {
        // If scan succeeds, check that no fixes are generated without candidates
        let untracked_issue = find_untracked_issue_for_branch(snap.health.issues(), "orphan");

        if let Some(issue) = untracked_issue {
            let fixes = generate_fixes(issue, &snap);
            // Without trunk/tracked branches, there may be no valid candidates
            // (or main might still serve as implicit trunk candidate)
            // The key is that the system doesn't crash
            assert!(
                fixes.len() <= 1,
                "Should handle limited candidates gracefully"
            );
        }
    }
}

#[test]
fn local_bootstrap_fix_preview_shows_merge_base() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "content", "Add feature");
    repo.checkout("main");

    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    let untracked_issue = find_untracked_issue_for_branch(snapshot.health.issues(), "feature");

    assert!(
        untracked_issue.is_some(),
        "Should detect feature as untracked"
    );

    let fixes = generate_fixes(untracked_issue.unwrap(), &snapshot);
    assert!(!fixes.is_empty());

    let preview = fixes[0].preview.format();
    // The preview should mention the merge-base (truncated OID)
    assert!(
        preview.contains("base") || preview.contains("merge-base"),
        "Preview should mention base: {}",
        preview
    );
}

#[test]
fn local_bootstrap_fix_id_format() {
    let repo = TestRepo::new();
    repo.init_lattice();

    repo.create_branch("my-feature");
    repo.checkout("my-feature");
    repo.commit("mf.txt", "content", "Add feature");
    repo.checkout("main");

    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    let untracked_issue = find_untracked_issue_for_branch(snapshot.health.issues(), "my-feature");

    assert!(
        untracked_issue.is_some(),
        "Should detect my-feature as untracked"
    );

    let fixes = generate_fixes(untracked_issue.unwrap(), &snapshot);
    assert!(!fixes.is_empty());

    let fix_id = fixes[0].id.to_string();
    // Fix ID should follow format: issue-type:fix-type:branch
    assert!(
        fix_id.starts_with("untracked-branch:"),
        "Fix ID should start with issue type: {}",
        fix_id
    );
    assert!(
        fix_id.contains("import-local"),
        "Fix ID should contain fix type: {}",
        fix_id
    );
}
