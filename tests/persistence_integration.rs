//! Integration tests for the persistence layer.
//!
//! These tests exercise the MetadataStore, RepoLock, and Journal
//! against real Git repositories created with tempfile.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use lattice::core::metadata::schema::{BranchMetadataV1, FreezeScope, FreezeState, PrState};
use lattice::core::metadata::store::{MetadataStore, StoreError};
use lattice::core::ops::journal::{Journal, OpPhase, OpState, StepKind};
use lattice::core::ops::lock::{LockError, RepoLock};
use lattice::core::types::{BranchName, Oid};
use lattice::git::Git;

// =============================================================================
// Test Helpers
// =============================================================================

/// Create a temporary Git repository for testing.
struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let dir = TempDir::new().expect("create temp dir");

        // Initialize git repo
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(dir.path())
            .output()
            .expect("git init");

        // Configure git
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .output()
            .expect("git config email");

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir.path())
            .output()
            .expect("git config name");

        // Create initial commit
        std::fs::write(dir.path().join("README.md"), "# Test\n").expect("write readme");

        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .expect("git add");

        Command::new("git")
            .args(["commit", "-m", "Initial commit"])
            .current_dir(dir.path())
            .output()
            .expect("git commit");

        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git_dir(&self) -> std::path::PathBuf {
        self.dir.path().join(".git")
    }

    fn git(&self) -> Git {
        Git::open(self.path()).expect("open git repo")
    }

    #[allow(dead_code)]
    fn commit_file(&self, filename: &str, content: &str, message: &str) -> Oid {
        std::fs::write(self.path().join(filename), content).expect("write file");

        Command::new("git")
            .args(["add", filename])
            .current_dir(self.path())
            .output()
            .expect("git add");

        Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(self.path())
            .output()
            .expect("git commit");

        self.git().head_oid().expect("head oid")
    }

    #[allow(dead_code)]
    fn create_branch(&self, name: &str) {
        Command::new("git")
            .args(["branch", name])
            .current_dir(self.path())
            .output()
            .expect("git branch");
    }
}

fn sample_oid() -> Oid {
    Oid::new("abc123def4567890abc123def4567890abc12345").expect("valid oid")
}

// =============================================================================
// MetadataStore Tests
// =============================================================================

mod metadata_store {
    use super::*;

    #[test]
    fn read_nonexistent_returns_none() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("nonexistent").unwrap();
        let result = store.read(&branch).expect("read");

        assert!(result.is_none());
    }

    #[test]
    fn write_cas_create() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        // Create (expected_old = None)
        let new_oid = store
            .write_cas(&branch, None, &meta)
            .expect("write_cas create");

        // Verify it was written
        let entry = store.read(&branch).expect("read").expect("should exist");
        assert_eq!(entry.ref_oid, new_oid);
        assert_eq!(entry.metadata.branch.name, "feature");
    }

    #[test]
    fn write_cas_update() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let mut meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        // Create
        let first_oid = store.write_cas(&branch, None, &meta).expect("create");

        // Update
        meta.touch();
        let second_oid = store
            .write_cas(&branch, Some(&first_oid), &meta)
            .expect("update");

        assert_ne!(first_oid, second_oid);

        // Verify update
        let entry = store.read(&branch).expect("read").expect("should exist");
        assert_eq!(entry.ref_oid, second_oid);
    }

    #[test]
    fn write_cas_fails_on_mismatch() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        // Create
        store.write_cas(&branch, None, &meta).expect("create");

        // Try to update with wrong expected_old
        let wrong_oid = Oid::new("0000000000000000000000000000000000000000").unwrap();
        let result = store.write_cas(&branch, Some(&wrong_oid), &meta);

        assert!(matches!(result, Err(StoreError::CasFailed { .. })));
    }

    #[test]
    fn write_cas_fails_on_unexpected_existence() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        // Create
        store.write_cas(&branch, None, &meta).expect("create");

        // Try to create again (expected_old = None should fail)
        let result = store.write_cas(&branch, None, &meta);

        assert!(matches!(result, Err(StoreError::CasFailed { .. })));
    }

    #[test]
    fn delete_cas_success() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        // Create
        let oid = store.write_cas(&branch, None, &meta).expect("create");

        // Delete
        store.delete_cas(&branch, &oid).expect("delete");

        // Verify deleted
        let result = store.read(&branch).expect("read");
        assert!(result.is_none());
    }

    #[test]
    fn delete_cas_fails_on_mismatch() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        // Create
        store.write_cas(&branch, None, &meta).expect("create");

        // Try to delete with wrong expected_old
        let wrong_oid = Oid::new("0000000000000000000000000000000000000000").unwrap();
        let result = store.delete_cas(&branch, &wrong_oid);

        assert!(matches!(result, Err(StoreError::CasFailed { .. })));
    }

    #[test]
    fn delete_cas_fails_on_nonexistent() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("nonexistent").unwrap();
        let oid = Oid::new("0000000000000000000000000000000000000000").unwrap();

        let result = store.delete_cas(&branch, &oid);

        assert!(matches!(result, Err(StoreError::NotFound(_))));
    }

    #[test]
    fn list_empty() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branches = store.list().expect("list");
        assert!(branches.is_empty());
    }

    #[test]
    fn list_multiple() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let main = BranchName::new("main").unwrap();

        // Create metadata for several branches
        for name in ["feature-a", "feature-b", "feature-c"] {
            let branch = BranchName::new(name).unwrap();
            let meta = BranchMetadataV1::new(branch.clone(), main.clone(), sample_oid());
            store.write_cas(&branch, None, &meta).expect("create");
        }

        let branches = store.list().expect("list");
        assert_eq!(branches.len(), 3);

        let names: Vec<_> = branches.iter().map(|b| b.as_str()).collect();
        assert!(names.contains(&"feature-a"));
        assert!(names.contains(&"feature-b"));
        assert!(names.contains(&"feature-c"));
    }

    #[test]
    fn exists_check() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();

        assert!(!store.exists(&branch).expect("exists before"));

        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());
        store.write_cas(&branch, None, &meta).expect("create");

        assert!(store.exists(&branch).expect("exists after"));
    }

    #[test]
    fn metadata_with_freeze_state_roundtrip() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();

        let meta = BranchMetadataV1::builder(branch.clone(), parent, sample_oid())
            .freeze_state(FreezeState::frozen(
                FreezeScope::DownstackInclusive,
                Some("teammate branch".into()),
            ))
            .build();

        store.write_cas(&branch, None, &meta).expect("create");

        let entry = store.read(&branch).expect("read").expect("should exist");
        assert!(entry.metadata.freeze.is_frozen());
    }

    #[test]
    fn metadata_with_pr_state_roundtrip() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();

        let meta = BranchMetadataV1::builder(branch.clone(), parent, sample_oid())
            .pr_state(PrState::linked(
                "github",
                42,
                "https://github.com/org/repo/pull/42",
            ))
            .build();

        store.write_cas(&branch, None, &meta).expect("create");

        let entry = store.read(&branch).expect("read").expect("should exist");
        assert!(entry.metadata.pr.is_linked());
        assert_eq!(entry.metadata.pr.number(), Some(42));
    }

    #[test]
    fn list_with_oids() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        let expected_oid = store.write_cas(&branch, None, &meta).expect("create");

        let entries = store.list_with_oids().expect("list_with_oids");
        assert_eq!(entries.len(), 1);

        let (name, oid) = &entries[0];
        assert_eq!(name.as_str(), "feature");
        assert_eq!(oid, &expected_oid);
    }
}

// =============================================================================
// RepoLock Tests
// =============================================================================

mod repo_lock {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let repo = TestRepo::new();

        let lock = RepoLock::acquire(&repo.git_dir()).expect("acquire");
        assert!(lock.is_held());

        // Lock file should exist
        assert!(lock.path().exists());
    }

    #[test]
    fn prevents_concurrent_acquire() {
        let repo = TestRepo::new();

        let lock1 = RepoLock::acquire(&repo.git_dir()).expect("first acquire");
        assert!(lock1.is_held());

        let result = RepoLock::acquire(&repo.git_dir());
        assert!(matches!(result, Err(LockError::AlreadyLocked)));
    }

    #[test]
    fn released_on_drop() {
        let repo = TestRepo::new();

        {
            let lock = RepoLock::acquire(&repo.git_dir()).expect("acquire");
            assert!(lock.is_held());
        }

        // Should be able to acquire again
        let lock2 = RepoLock::acquire(&repo.git_dir()).expect("reacquire");
        assert!(lock2.is_held());
    }

    #[test]
    fn try_acquire_returns_none_when_locked() {
        let repo = TestRepo::new();

        let _lock1 = RepoLock::acquire(&repo.git_dir()).expect("first acquire");

        let result = RepoLock::try_acquire(&repo.git_dir()).expect("try_acquire");
        assert!(result.is_none());
    }

    #[test]
    fn creates_lattice_directory() {
        let repo = TestRepo::new();
        let lattice_dir = repo.git_dir().join("lattice");

        assert!(!lattice_dir.exists());

        let _lock = RepoLock::acquire(&repo.git_dir()).expect("acquire");

        assert!(lattice_dir.exists());
    }
}

// =============================================================================
// Journal Tests
// =============================================================================

mod journal {
    use super::*;

    #[test]
    fn write_and_read_roundtrip() {
        let repo = TestRepo::new();
        let git_dir = repo.git_dir();

        let mut journal = Journal::new("restack");
        journal.record_ref_update("refs/heads/feature", None, "abc123");
        journal.record_metadata_write("feature", None, "meta-oid");
        journal.record_checkpoint("midpoint");
        journal.commit();

        journal.write(&git_dir).expect("write");

        let loaded = Journal::read(&git_dir, &journal.op_id).expect("read");

        assert_eq!(loaded.op_id, journal.op_id);
        assert_eq!(loaded.command, "restack");
        assert_eq!(loaded.phase, OpPhase::Committed);
        assert_eq!(loaded.steps.len(), 3);
    }

    #[test]
    fn list_and_most_recent() {
        let repo = TestRepo::new();
        let git_dir = repo.git_dir();

        let journal1 = Journal::new("first");
        journal1.write(&git_dir).expect("write 1");

        std::thread::sleep(std::time::Duration::from_millis(10));

        let journal2 = Journal::new("second");
        journal2.write(&git_dir).expect("write 2");

        // List should return both
        let ids = Journal::list(&git_dir).expect("list");
        assert_eq!(ids.len(), 2);

        // Most recent should be second
        let recent = Journal::most_recent(&git_dir)
            .expect("most_recent")
            .expect("should exist");
        assert_eq!(recent.command, "second");
    }

    #[test]
    fn delete_journal() {
        let repo = TestRepo::new();
        let git_dir = repo.git_dir();

        let journal = Journal::new("test");
        journal.write(&git_dir).expect("write");

        let path = journal.file_path(&git_dir);
        assert!(path.exists());

        journal.delete(&git_dir).expect("delete");
        assert!(!path.exists());
    }

    #[test]
    fn ref_updates_for_rollback_ordering() {
        let mut journal = Journal::new("test");

        journal.record_ref_update("refs/heads/a", None, "oid1");
        journal.record_checkpoint("checkpoint");
        journal.record_ref_update("refs/heads/b", Some("old".into()), "oid2");
        journal.record_metadata_write("branch", None, "meta-oid");
        journal.record_git_process(vec!["rebase".into()], "rebase");
        journal.record_metadata_delete("deleted", "del-oid");

        let updates = journal.ref_updates_for_rollback();

        // Should be in reverse order, excluding non-ref-update steps
        assert_eq!(updates.len(), 4);

        // Verify order: most recent first
        assert!(
            matches!(updates[0], StepKind::MetadataDelete { branch, .. } if branch == "deleted")
        );
        assert!(matches!(updates[1], StepKind::MetadataWrite { branch, .. } if branch == "branch"));
        assert!(
            matches!(updates[2], StepKind::RefUpdate { refname, .. } if refname == "refs/heads/b")
        );
        assert!(
            matches!(updates[3], StepKind::RefUpdate { refname, .. } if refname == "refs/heads/a")
        );
    }
}

// =============================================================================
// OpState Tests
// =============================================================================

mod op_state {
    use super::*;

    #[test]
    fn from_journal_and_roundtrip() {
        let repo = TestRepo::new();
        let git_dir = repo.git_dir();

        let journal = Journal::new("test-cmd");
        let state = OpState::from_journal(&journal);

        state.write(&git_dir).expect("write");

        let loaded = OpState::read(&git_dir)
            .expect("read")
            .expect("should exist");

        assert_eq!(loaded.op_id, journal.op_id);
        assert_eq!(loaded.command, "test-cmd");
        assert_eq!(loaded.phase, OpPhase::InProgress);
    }

    #[test]
    fn exists_and_remove() {
        let repo = TestRepo::new();
        let git_dir = repo.git_dir();

        assert!(!OpState::exists(&git_dir));

        let journal = Journal::new("test");
        let state = OpState::from_journal(&journal);
        state.write(&git_dir).expect("write");

        assert!(OpState::exists(&git_dir));

        OpState::remove(&git_dir).expect("remove");

        assert!(!OpState::exists(&git_dir));
    }

    #[test]
    fn update_phase() {
        let repo = TestRepo::new();
        let git_dir = repo.git_dir();

        let journal = Journal::new("test");
        let mut state = OpState::from_journal(&journal);
        state.write(&git_dir).expect("write");

        state
            .update_phase(OpPhase::Paused, &git_dir)
            .expect("update");

        let loaded = OpState::read(&git_dir)
            .expect("read")
            .expect("should exist");
        assert_eq!(loaded.phase, OpPhase::Paused);
    }
}

// =============================================================================
// Integration: Lock + Metadata + Journal
// =============================================================================

mod integration {
    use super::*;

    #[test]
    fn full_operation_lifecycle() {
        let repo = TestRepo::new();
        let git = repo.git();
        let git_dir = repo.git_dir();

        // 1. Acquire lock
        let _lock = RepoLock::acquire(&git_dir).expect("acquire lock");

        // 2. Create journal
        let mut journal = Journal::new("track");

        // 3. Write op-state
        let op_state = OpState::from_journal(&journal);
        op_state.write(&git_dir).expect("write op-state");

        // 4. Write metadata
        let store = MetadataStore::new(&git);
        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        let meta_oid = store
            .write_cas(&branch, None, &meta)
            .expect("write metadata");

        // 5. Record in journal
        journal.record_metadata_write(branch.to_string(), None, meta_oid.to_string());
        journal.write(&git_dir).expect("write journal step");

        // 6. Commit
        journal.commit();
        journal.write(&git_dir).expect("write journal commit");
        OpState::remove(&git_dir).expect("remove op-state");

        // Verify final state
        assert!(!OpState::exists(&git_dir));
        assert!(store.exists(&branch).expect("branch exists"));
    }

    #[test]
    fn simulated_crash_recovery() {
        let repo = TestRepo::new();
        let _git = repo.git();
        let git_dir = repo.git_dir();

        // Simulate an interrupted operation
        let journal = Journal::new("restack");
        let op_state = OpState::from_journal(&journal);

        // Write op-state but don't complete the operation
        op_state.write(&git_dir).expect("write op-state");
        journal.write(&git_dir).expect("write journal");

        // Simulate "next invocation"
        // Should detect op-state exists
        assert!(OpState::exists(&git_dir));

        // Read the op-state and journal
        let loaded_state = OpState::read(&git_dir)
            .expect("read")
            .expect("should exist");
        assert_eq!(loaded_state.phase, OpPhase::InProgress);

        let loaded_journal = Journal::read(&git_dir, &loaded_state.op_id).expect("read journal");
        assert_eq!(loaded_journal.command, "restack");

        // Cleanup (simulating abort)
        OpState::remove(&git_dir).expect("remove op-state");
    }

    #[test]
    fn metadata_visible_via_git_refs() {
        let repo = TestRepo::new();
        let git = repo.git();
        let store = MetadataStore::new(&git);

        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let meta = BranchMetadataV1::new(branch.clone(), parent, sample_oid());

        store.write_cas(&branch, None, &meta).expect("write");

        // Verify the ref exists using git directly
        // Note: We use try_resolve_ref_to_object because metadata refs point to blobs
        let refname = MetadataStore::ref_name(&branch);
        let resolved = git
            .try_resolve_ref_to_object(refname.as_str())
            .expect("resolve");
        assert!(resolved.is_some());

        // List metadata refs
        let refs = git.list_metadata_refs().expect("list_metadata_refs");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].0.as_str(), "feature");
    }
}
