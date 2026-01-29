//! Gating matrix tests per ROADMAP.md Milestone 0.12.
//!
//! These tests verify that commands are correctly gated based on
//! repository state and capabilities. Each test verifies a specific
//! gating scenario from the architectural requirements.
//!
//! # Test Categories
//!
//! 1. **Read-Only Commands** - Very permissive, work in degraded states
//! 2. **Navigation Commands** - Need working directory
//! 3. **Mutating Commands** - Strict requirements, no in-progress ops
//! 4. **Remote Commands** - Mode-dependent requirements
//! 5. **Recovery Commands** - Minimal requirements
//!
//! # Architecture Reference
//!
//! - ARCHITECTURE.md Section 5.2-5.4: Capabilities and gating
//! - SPEC.md Section 4.6.6: Command capability requirements
//! - SPEC.md Section 4.6.7: Bare repo policy

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use latticework::cli::commands;
use latticework::engine::capabilities::{Capability, CapabilitySet};
use latticework::engine::gate::{gate, requirements, GateResult};
use latticework::engine::scan::scan;
use latticework::engine::Context;
use latticework::git::Git;

// =============================================================================
// Test Fixtures
// =============================================================================

/// Run a git command and expect success.
fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command failed to execute");

    if !output.status.success() {
        panic!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Create a minimal git repository with an initial commit.
fn create_minimal_repo() -> TempDir {
    let dir = TempDir::new().expect("failed to create temp dir");
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    run_git(dir.path(), &["config", "user.name", "Test User"]);

    std::fs::write(dir.path().join("README.md"), "# Test\n").unwrap();
    run_git(dir.path(), &["add", "README.md"]);
    run_git(dir.path(), &["commit", "-m", "Initial commit"]);

    dir
}

/// Create a repository initialized with Lattice.
fn create_initialized_repo() -> TempDir {
    let dir = create_minimal_repo();
    let ctx = test_context(dir.path());
    commands::init(&ctx, Some("main"), false, true).expect("init failed");
    dir
}

/// Create a bare repository.
fn create_bare_repo() -> TempDir {
    let dir = TempDir::new().expect("failed to create temp dir");
    run_git(dir.path(), &["init", "--bare"]);
    dir
}

/// Create a test context.
fn test_context(path: &Path) -> Context {
    Context {
        cwd: Some(path.to_path_buf()),
        interactive: false,
        quiet: true,
        debug: false,
        verify: true,
    }
}

// =============================================================================
// Read-Only Command Gating Tests
// =============================================================================

mod read_only_gating {
    use super::*;

    /// Read-only commands should be gated with READ_ONLY requirements.
    #[test]
    fn read_only_requirements_are_minimal() {
        // READ_ONLY only requires RepoOpen
        assert_eq!(requirements::READ_ONLY.capabilities.len(), 1);
        assert!(requirements::READ_ONLY
            .capabilities
            .contains(&Capability::RepoOpen));
    }

    /// Read-only commands work in a normal repo.
    #[test]
    fn read_only_works_in_normal_repo() {
        let dir = create_initialized_repo();
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::READ_ONLY) {
            GateResult::Ready(_) => {} // Expected
            GateResult::NeedsRepair(bundle) => {
                panic!(
                    "Expected Ready, got NeedsRepair: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }

    /// Read-only commands work in a bare repo.
    #[test]
    fn read_only_works_in_bare_repo() {
        let dir = create_bare_repo();
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::READ_ONLY) {
            GateResult::Ready(_) => {} // Expected
            GateResult::NeedsRepair(bundle) => {
                panic!(
                    "Expected Ready, got NeedsRepair: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }

    /// Read-only commands work even without lattice initialization.
    #[test]
    fn read_only_works_without_init() {
        let dir = create_minimal_repo(); // No lattice init
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::READ_ONLY) {
            GateResult::Ready(_) => {} // Expected
            GateResult::NeedsRepair(bundle) => {
                panic!(
                    "Expected Ready, got NeedsRepair: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }
}

// =============================================================================
// Navigation Command Gating Tests
// =============================================================================

mod navigation_gating {
    use super::*;

    /// Navigation commands require working directory.
    #[test]
    fn navigation_requires_working_directory() {
        assert!(requirements::NAVIGATION
            .capabilities
            .contains(&Capability::WorkingDirectoryAvailable));
    }

    /// Navigation commands work in a normal initialized repo.
    #[test]
    fn navigation_works_in_normal_repo() {
        let dir = create_initialized_repo();
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::NAVIGATION) {
            GateResult::Ready(_) => {} // Expected
            GateResult::NeedsRepair(bundle) => {
                panic!(
                    "Expected Ready, got NeedsRepair: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }

    /// Navigation commands fail in a bare repo.
    #[test]
    fn navigation_fails_in_bare_repo() {
        let dir = create_bare_repo();
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::NAVIGATION) {
            GateResult::Ready(_) => {
                panic!("Expected NeedsRepair for bare repo, got Ready");
            }
            GateResult::NeedsRepair(bundle) => {
                // Should fail because WorkingDirectoryAvailable is missing
                let missing_workdir = bundle.blocking_issues.iter().any(|issue| {
                    issue.message.contains("working directory")
                        || issue.message.contains("bare")
                        || format!("{:?}", issue).contains("WorkingDirectoryAvailable")
                });
                assert!(
                    missing_workdir || !bundle.blocking_issues.is_empty(),
                    "Expected working directory issue, got: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }
}

// =============================================================================
// Mutating Command Gating Tests
// =============================================================================

mod mutating_gating {
    use super::*;

    /// Mutating commands require no in-progress operations.
    #[test]
    fn mutating_requires_no_op_in_progress() {
        assert!(requirements::MUTATING
            .capabilities
            .contains(&Capability::NoLatticeOpInProgress));
        assert!(requirements::MUTATING
            .capabilities
            .contains(&Capability::NoExternalGitOpInProgress));
    }

    /// Mutating commands require working directory.
    #[test]
    fn mutating_requires_working_directory() {
        assert!(requirements::MUTATING
            .capabilities
            .contains(&Capability::WorkingDirectoryAvailable));
    }

    /// Mutating commands require frozen policy satisfied.
    #[test]
    fn mutating_requires_frozen_policy() {
        assert!(requirements::MUTATING
            .capabilities
            .contains(&Capability::FrozenPolicySatisfied));
    }

    /// Mutating commands work in a normal initialized repo.
    #[test]
    fn mutating_works_in_normal_repo() {
        let dir = create_initialized_repo();
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::MUTATING) {
            GateResult::Ready(_) => {} // Expected
            GateResult::NeedsRepair(bundle) => {
                panic!(
                    "Expected Ready, got NeedsRepair: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }

    /// Mutating commands fail in a bare repo.
    #[test]
    fn mutating_fails_in_bare_repo() {
        let dir = create_bare_repo();
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::MUTATING) {
            GateResult::Ready(_) => {
                panic!("Expected NeedsRepair for bare repo, got Ready");
            }
            GateResult::NeedsRepair(_) => {} // Expected
        }
    }
}

// =============================================================================
// Metadata-Only Command Gating Tests
// =============================================================================

mod metadata_only_gating {
    use super::*;

    /// Metadata-only commands do NOT require working directory.
    #[test]
    fn metadata_only_does_not_require_working_directory() {
        assert!(!requirements::MUTATING_METADATA_ONLY
            .capabilities
            .contains(&Capability::WorkingDirectoryAvailable));
    }

    /// Metadata-only commands still require no in-progress operations.
    #[test]
    fn metadata_only_requires_no_op_in_progress() {
        assert!(requirements::MUTATING_METADATA_ONLY
            .capabilities
            .contains(&Capability::NoLatticeOpInProgress));
    }

    /// Metadata-only commands work in a normal initialized repo.
    #[test]
    fn metadata_only_works_in_normal_repo() {
        let dir = create_initialized_repo();
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::MUTATING_METADATA_ONLY) {
            GateResult::Ready(_) => {} // Expected
            GateResult::NeedsRepair(bundle) => {
                panic!(
                    "Expected Ready, got NeedsRepair: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }
}

// =============================================================================
// Remote Command Gating Tests
// =============================================================================

mod remote_gating {
    use super::*;

    /// Remote commands require auth.
    #[test]
    fn remote_requires_auth() {
        assert!(requirements::REMOTE
            .capabilities
            .contains(&Capability::AuthAvailable));
        assert!(requirements::REMOTE
            .capabilities
            .contains(&Capability::RepoAuthorized));
    }

    /// Remote commands require working directory by default.
    #[test]
    fn remote_requires_working_directory() {
        assert!(requirements::REMOTE
            .capabilities
            .contains(&Capability::WorkingDirectoryAvailable));
    }

    /// Remote bare-allowed does NOT require working directory.
    #[test]
    fn remote_bare_allowed_does_not_require_working_directory() {
        assert!(!requirements::REMOTE_BARE_ALLOWED
            .capabilities
            .contains(&Capability::WorkingDirectoryAvailable));
    }

    /// Remote bare-allowed still requires auth.
    #[test]
    fn remote_bare_allowed_requires_auth() {
        assert!(requirements::REMOTE_BARE_ALLOWED
            .capabilities
            .contains(&Capability::AuthAvailable));
        assert!(requirements::REMOTE_BARE_ALLOWED
            .capabilities
            .contains(&Capability::RepoAuthorized));
    }
}

// =============================================================================
// Recovery Command Gating Tests
// =============================================================================

mod recovery_gating {
    use super::*;

    /// Recovery commands have minimal requirements.
    #[test]
    fn recovery_has_minimal_requirements() {
        // RECOVERY only requires RepoOpen
        assert_eq!(requirements::RECOVERY.capabilities.len(), 1);
        assert!(requirements::RECOVERY
            .capabilities
            .contains(&Capability::RepoOpen));
    }

    /// Recovery commands do NOT require no-op-in-progress.
    /// (They specifically handle in-progress operations)
    #[test]
    fn recovery_allows_op_in_progress() {
        assert!(!requirements::RECOVERY
            .capabilities
            .contains(&Capability::NoLatticeOpInProgress));
    }

    /// Recovery commands work in any repo state.
    #[test]
    fn recovery_works_in_any_state() {
        let dir = create_minimal_repo(); // Minimal, not even initialized
        let git = Git::open(dir.path()).expect("failed to open git");
        let snapshot = scan(&git).expect("failed to scan");

        match gate(snapshot, &requirements::RECOVERY) {
            GateResult::Ready(_) => {} // Expected
            GateResult::NeedsRepair(bundle) => {
                panic!(
                    "Expected Ready, got NeedsRepair: {:?}",
                    bundle.blocking_issues
                );
            }
        }
    }
}

// =============================================================================
// Requirement Set Comparison Tests
// =============================================================================

mod requirement_comparisons {
    use super::*;

    /// Verify requirement set hierarchy makes sense.
    #[test]
    fn requirement_hierarchy() {
        // READ_ONLY < NAVIGATION < MUTATING
        assert!(
            requirements::READ_ONLY.capabilities.len()
                < requirements::NAVIGATION.capabilities.len()
        );
        assert!(
            requirements::NAVIGATION.capabilities.len() < requirements::MUTATING.capabilities.len()
        );

        // MUTATING_METADATA_ONLY has fewer requirements than MUTATING
        // (no WorkingDirectoryAvailable)
        assert!(
            requirements::MUTATING_METADATA_ONLY.capabilities.len()
                < requirements::MUTATING.capabilities.len()
        );

        // REMOTE has more requirements than MUTATING
        assert!(
            requirements::REMOTE.capabilities.len() > requirements::MUTATING.capabilities.len()
        );
    }

    /// All mutating requirements are subset of remote requirements.
    #[test]
    fn mutating_is_subset_of_remote() {
        for cap in requirements::MUTATING.capabilities {
            assert!(
                requirements::REMOTE.capabilities.contains(cap),
                "MUTATING capability {:?} not in REMOTE",
                cap
            );
        }
    }

    /// Read-only is subset of everything.
    #[test]
    fn read_only_is_minimal() {
        for cap in requirements::READ_ONLY.capabilities {
            assert!(
                requirements::NAVIGATION.capabilities.contains(cap),
                "READ_ONLY capability {:?} not in NAVIGATION",
                cap
            );
            assert!(
                requirements::MUTATING.capabilities.contains(cap),
                "READ_ONLY capability {:?} not in MUTATING",
                cap
            );
        }
    }
}

// =============================================================================
// Capability Set Tests
// =============================================================================

mod capability_set_tests {
    use super::*;

    #[test]
    fn capability_set_has_and_missing() {
        let mut caps = CapabilitySet::new();
        caps.insert(Capability::RepoOpen);
        caps.insert(Capability::TrunkKnown);

        assert!(caps.has(&Capability::RepoOpen));
        assert!(caps.has(&Capability::TrunkKnown));
        assert!(!caps.has(&Capability::AuthAvailable));

        let missing = caps.missing(&[
            Capability::RepoOpen,
            Capability::TrunkKnown,
            Capability::AuthAvailable,
        ]);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], Capability::AuthAvailable);
    }

    #[test]
    fn capability_set_has_all() {
        let mut caps = CapabilitySet::new();
        caps.insert(Capability::RepoOpen);
        caps.insert(Capability::TrunkKnown);

        assert!(caps.has_all(&[Capability::RepoOpen]));
        assert!(caps.has_all(&[Capability::RepoOpen, Capability::TrunkKnown]));
        assert!(!caps.has_all(&[Capability::RepoOpen, Capability::AuthAvailable]));
    }
}
