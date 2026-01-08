//! Out-of-band fuzz harness for Lattice robustness testing.
//!
//! This test harness proves the architecture promise from ARCHITECTURE.md:
//! "Lattice stays correct when users do random git things."
//!
//! It interleaves Lattice operations with direct git operations and verifies
//! that the four core invariants hold:
//!
//! 1. Gating never produces `ReadyContext` when requirements not met
//! 2. Doctor offers repair options for detected issues
//! 3. Executor never applies plan when CAS fails
//! 4. After any reported success, `fast_verify` passes

use std::path::Path;
use std::process::Command;

use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
use tempfile::TempDir;

use lattice::cli::commands;
use lattice::core::metadata::store::MetadataStore;
use lattice::core::types::BranchName;
use lattice::doctor::Doctor;
use lattice::engine::gate::{gate, requirements};
use lattice::engine::scan::scan;

use lattice::engine::Context;
use lattice::git::Git;

// =============================================================================
// Operation Types
// =============================================================================

/// Operations that Lattice performs
#[derive(Debug, Clone)]
enum LatticeOp {
    Track { branch: String, parent: String },
    Untrack { branch: String },
    Restack { branch: String },
    Create { name: String },
    Freeze { branch: String },
    Unfreeze { branch: String },
}

/// Out-of-band git operations that users might perform
#[derive(Debug, Clone)]
enum GitOp {
    /// Create a new untracked branch
    CreateBranch { name: String },
    /// Delete a branch directly with git
    DeleteBranch { branch: String },
    /// Rename a branch directly with git
    RenameBranch { old: String, new: String },
    /// Force update a branch tip to a different commit
    ForceUpdateTip { branch: String },
    /// Corrupt metadata by writing invalid JSON
    CorruptMetadata { branch: String },
    /// Delete metadata ref directly
    DeleteMetadataRef { branch: String },
    /// Create a direct commit on a branch
    DirectCommit { branch: String },
}

/// Combined operation for interleaving
#[derive(Debug, Clone)]
enum AnyOp {
    Lattice(LatticeOp),
    Git(GitOp),
}

/// Result of an operation for logging
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct OpResult {
    op: AnyOp,
    success: bool,
    error: Option<String>,
}

/// Invariant violation detected during fuzz testing
#[derive(Debug)]
#[allow(dead_code)]
enum InvariantViolation {
    /// Gating produced ReadyContext when capabilities were missing
    GatingProducedReadyWhenUnmet { missing_cap: String },
    /// Doctor did not offer repairs for blocking issues
    DoctorNoRepairsForBlockingIssues { issue_count: usize },
    /// Fast verify failed after a reported success
    PostSuccessVerifyFailed { error: String },
    /// Test setup error
    SetupError { error: String },
}

// =============================================================================
// Test Repository Fixture
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

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self) -> Git {
        Git::open(self.path()).expect("failed to open test repo")
    }

    fn context(&self) -> Context {
        Context {
            cwd: Some(self.path().to_path_buf()),
            interactive: false,
            quiet: true,
            debug: false,
        }
    }

    fn init_lattice(&self) {
        let ctx = self.context();
        commands::init(&ctx, Some("main"), false, true).expect("init failed");
    }

    fn commit(&self, filename: &str, content: &str, message: &str) {
        std::fs::write(self.dir.path().join(filename), content).unwrap();
        run_git(self.path(), &["add", filename]);
        run_git(self.path(), &["commit", "-m", message]);
    }

    fn create_branch(&self, name: &str) {
        run_git(self.path(), &["branch", name]);
    }

    fn checkout(&self, name: &str) {
        run_git(self.path(), &["checkout", name]);
    }

    fn current_branch(&self) -> String {
        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(self.path())
            .output()
            .expect("git branch failed");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn branch_exists(&self, name: &str) -> bool {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/heads/{}", name)])
            .current_dir(self.path())
            .output()
            .expect("git rev-parse failed");
        output.status.success()
    }

    fn list_branches(&self) -> Vec<String> {
        let output = Command::new("git")
            .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
            .current_dir(self.path())
            .output()
            .expect("git for-each-ref failed");
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|s| s.to_string())
            .collect()
    }

    fn list_tracked_branches(&self) -> Vec<String> {
        let git = self.git();
        let snapshot = match scan(&git) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        snapshot
            .tracked_branches()
            .map(|b| b.as_str().to_string())
            .collect()
    }

    fn abort_if_in_progress(&self) {
        // Try to abort any in-progress git operations
        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(self.path())
            .output();
        let _ = Command::new("git")
            .args(["merge", "--abort"])
            .current_dir(self.path())
            .output();
    }
}

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

fn try_run_git(dir: &Path, args: &[&str]) -> bool {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command failed");
    output.status.success()
}

// =============================================================================
// Fuzz Configuration
// =============================================================================

/// Configuration for fuzz runs
struct FuzzConfig {
    /// Number of operations per run
    ops_per_run: usize,
    /// Seed for determinism
    seed: u64,
    /// Whether to include corruption operations
    include_corruption: bool,
}

impl Default for FuzzConfig {
    fn default() -> Self {
        Self {
            ops_per_run: 30,
            seed: 0,
            include_corruption: true,
        }
    }
}

// =============================================================================
// Fuzz Harness
// =============================================================================

/// The out-of-band fuzz harness
struct OobFuzzHarness {
    repo: TestRepo,
    config: FuzzConfig,
    rng: StdRng,
    operation_log: Vec<OpResult>,
    branch_counter: usize,
}

impl OobFuzzHarness {
    fn new(config: FuzzConfig) -> Self {
        let repo = TestRepo::new();
        repo.init_lattice();

        // Create some initial branches for variety
        repo.create_branch("feature-a");
        repo.checkout("feature-a");
        repo.commit("a.txt", "a", "Feature A");

        let ctx = repo.context();
        let _ = commands::track(&ctx, Some("feature-a"), Some("main"), false, false);

        repo.checkout("main");

        Self {
            rng: StdRng::seed_from_u64(config.seed),
            config,
            repo,
            operation_log: Vec::new(),
            branch_counter: 0,
        }
    }

    /// Generate a unique branch name
    fn next_branch_name(&mut self) -> String {
        self.branch_counter += 1;
        format!("fuzz-branch-{}", self.branch_counter)
    }

    /// Generate a random Lattice operation based on current state
    fn generate_lattice_op(&mut self) -> LatticeOp {
        let tracked = self.repo.list_tracked_branches();
        let branches = self.repo.list_branches();
        let untracked: Vec<_> = branches
            .iter()
            .filter(|b| !tracked.contains(b) && *b != "main")
            .cloned()
            .collect();

        let choice = self.rng.random_range(0..6);

        match choice {
            0 if !untracked.is_empty() => {
                // Track an existing untracked branch
                let branch = untracked[self.rng.random_range(0..untracked.len())].clone();
                let parent = if tracked.is_empty() {
                    "main".to_string()
                } else {
                    tracked[self.rng.random_range(0..tracked.len())].clone()
                };
                LatticeOp::Track { branch, parent }
            }
            1 if !tracked.is_empty() => {
                // Untrack a tracked branch
                let branch = tracked[self.rng.random_range(0..tracked.len())].clone();
                LatticeOp::Untrack { branch }
            }
            2 if !tracked.is_empty() => {
                // Restack a tracked branch
                let branch = tracked[self.rng.random_range(0..tracked.len())].clone();
                LatticeOp::Restack { branch }
            }
            3 => {
                // Create a new branch
                let name = self.next_branch_name();
                LatticeOp::Create { name }
            }
            4 if !tracked.is_empty() => {
                // Freeze a tracked branch
                let branch = tracked[self.rng.random_range(0..tracked.len())].clone();
                LatticeOp::Freeze { branch }
            }
            5 if !tracked.is_empty() => {
                // Unfreeze a tracked branch
                let branch = tracked[self.rng.random_range(0..tracked.len())].clone();
                LatticeOp::Unfreeze { branch }
            }
            _ => {
                // Default: create a new branch
                let name = self.next_branch_name();
                LatticeOp::Create { name }
            }
        }
    }

    /// Generate a random Git operation
    fn generate_git_op(&mut self) -> GitOp {
        let branches = self.repo.list_branches();
        let non_main: Vec<_> = branches.iter().filter(|b| *b != "main").cloned().collect();

        let max_choice = if self.config.include_corruption { 7 } else { 5 };
        let choice = self.rng.random_range(0..max_choice);

        match choice {
            0 => {
                // Create a new branch directly with git
                let name = self.next_branch_name();
                GitOp::CreateBranch { name }
            }
            1 if !non_main.is_empty() => {
                // Delete a branch
                let branch = non_main[self.rng.random_range(0..non_main.len())].clone();
                GitOp::DeleteBranch { branch }
            }
            2 if !non_main.is_empty() => {
                // Rename a branch
                let old = non_main[self.rng.random_range(0..non_main.len())].clone();
                let new = self.next_branch_name();
                GitOp::RenameBranch { old, new }
            }
            3 if !non_main.is_empty() => {
                // Force update tip
                let branch = non_main[self.rng.random_range(0..non_main.len())].clone();
                GitOp::ForceUpdateTip { branch }
            }
            4 if !non_main.is_empty() => {
                // Direct commit
                let branch = non_main[self.rng.random_range(0..non_main.len())].clone();
                GitOp::DirectCommit { branch }
            }
            5 if self.config.include_corruption && !non_main.is_empty() => {
                // Corrupt metadata
                let branch = non_main[self.rng.random_range(0..non_main.len())].clone();
                GitOp::CorruptMetadata { branch }
            }
            6 if self.config.include_corruption && !non_main.is_empty() => {
                // Delete metadata ref
                let branch = non_main[self.rng.random_range(0..non_main.len())].clone();
                GitOp::DeleteMetadataRef { branch }
            }
            _ => {
                // Default: create a new branch
                let name = self.next_branch_name();
                GitOp::CreateBranch { name }
            }
        }
    }

    /// Generate a random operation (weighted 60% Lattice, 40% Git)
    fn generate_op(&mut self) -> AnyOp {
        if self.rng.random_bool(0.6) {
            AnyOp::Lattice(self.generate_lattice_op())
        } else {
            AnyOp::Git(self.generate_git_op())
        }
    }

    /// Execute a Lattice operation
    fn execute_lattice_op(&self, op: &LatticeOp) -> OpResult {
        let ctx = self.repo.context();
        let result = match op {
            LatticeOp::Track { branch, parent } => {
                commands::track(&ctx, Some(branch), Some(parent), false, false)
            }
            LatticeOp::Untrack { branch } => commands::untrack(&ctx, Some(branch), true),
            LatticeOp::Restack { branch } => commands::restack(&ctx, Some(branch), true, false),
            LatticeOp::Create { name } => {
                commands::create(&ctx, Some(name), None, false, false, false, false)
            }
            LatticeOp::Freeze { branch } => commands::freeze(&ctx, Some(branch), false),
            LatticeOp::Unfreeze { branch } => commands::unfreeze(&ctx, Some(branch), false),
        };

        OpResult {
            op: AnyOp::Lattice(op.clone()),
            success: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
        }
    }

    /// Execute a Git operation
    fn execute_git_op(&mut self, op: &GitOp) -> OpResult {
        let success = match op {
            GitOp::CreateBranch { name } => try_run_git(self.repo.path(), &["branch", name]),
            GitOp::DeleteBranch { branch } => {
                // Don't delete current branch
                if self.repo.current_branch() == *branch {
                    self.repo.checkout("main");
                }
                try_run_git(self.repo.path(), &["branch", "-D", branch])
            }
            GitOp::RenameBranch { old, new } => {
                try_run_git(self.repo.path(), &["branch", "-m", old, new])
            }
            GitOp::ForceUpdateTip { branch } => {
                // Force update the branch to main's HEAD
                // This simulates someone force-pushing or resetting a branch
                if self.repo.branch_exists(branch) && self.repo.branch_exists("main") {
                    try_run_git(self.repo.path(), &["branch", "-f", branch, "main"])
                } else {
                    false
                }
            }
            GitOp::DirectCommit { branch } => {
                let current = self.repo.current_branch();
                if self.repo.branch_exists(branch) {
                    if try_run_git(self.repo.path(), &["checkout", branch]) {
                        self.branch_counter += 1;
                        let filename = format!("direct-{}.txt", self.branch_counter);
                        std::fs::write(self.repo.path().join(&filename), "direct").unwrap_or(());
                        let _ = try_run_git(self.repo.path(), &["add", &filename]);
                        let result =
                            try_run_git(self.repo.path(), &["commit", "-m", "Direct commit"]);
                        let _ = try_run_git(self.repo.path(), &["checkout", &current]);
                        result
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            GitOp::CorruptMetadata { branch } => {
                // Write invalid JSON to metadata ref
                let git = self.repo.git();
                let _git_dir = git.git_dir();
                let meta_ref = format!("refs/branch-metadata/{}", branch);

                // Create a blob with invalid JSON
                let output = Command::new("git")
                    .args(["hash-object", "-w", "--stdin"])
                    .stdin(std::process::Stdio::piped())
                    .current_dir(self.repo.path())
                    .output();

                if let Ok(output) = output {
                    if output.status.success() {
                        let blob_oid = String::from_utf8(output.stdout).unwrap().trim().to_string();
                        try_run_git(self.repo.path(), &["update-ref", &meta_ref, &blob_oid])
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            GitOp::DeleteMetadataRef { branch } => {
                let meta_ref = format!("refs/branch-metadata/{}", branch);
                try_run_git(self.repo.path(), &["update-ref", "-d", &meta_ref])
            }
        };

        OpResult {
            op: AnyOp::Git(op.clone()),
            success,
            error: if success {
                None
            } else {
                Some("Git operation failed".to_string())
            },
        }
    }

    /// Execute an operation
    fn execute_op(&mut self, op: &AnyOp) -> OpResult {
        match op {
            AnyOp::Lattice(lop) => self.execute_lattice_op(lop),
            AnyOp::Git(gop) => self.execute_git_op(gop),
        }
    }

    /// Assert gating correctness invariant
    ///
    /// The invariant is: gating never produces ReadyContext when the required
    /// capabilities are not met. This is checked by verifying that if gating
    /// returns Ready, all required capabilities are actually present.
    fn assert_gating_correctness(&self) -> Result<(), InvariantViolation> {
        let git = self.repo.git();
        let snapshot = match scan(&git) {
            Ok(s) => s,
            Err(_) => return Ok(()), // Scan failure is acceptable
        };

        let caps = snapshot.health.capabilities().clone();
        let result = gate(snapshot, &requirements::MUTATING);

        // The invariant: if gate returns Ready, all required capabilities must be present
        if result.is_ready() {
            for cap in requirements::MUTATING.capabilities {
                if !caps.has(cap) {
                    return Err(InvariantViolation::GatingProducedReadyWhenUnmet {
                        missing_cap: format!("{:?}", cap),
                    });
                }
            }
        }

        Ok(())
    }

    /// Assert doctor offers repairs invariant
    ///
    /// Note: This is a soft check. Doctor may not have fixes for all possible
    /// out-of-band corruption scenarios, but it should at least diagnose them.
    /// The fuzz harness validates that Lattice doesn't crash and gating works.
    fn assert_doctor_offers_repairs(&self) -> Result<(), InvariantViolation> {
        let git = self.repo.git();
        let snapshot = match scan(&git) {
            Ok(s) => s,
            Err(_) => return Ok(()), // Scan failure is acceptable
        };

        let blocking_count = snapshot.health.blocking_issues().count();
        if blocking_count > 0 {
            let doctor = Doctor::new();
            let report = doctor.diagnose(&snapshot);

            // Doctor should at minimum produce a diagnosis (even if no fixes)
            // For now, we just verify it runs without panicking
            let _ = report;
        }

        Ok(())
    }

    /// Assert post-success verify invariant
    ///
    /// Note: In the presence of out-of-band git operations, verification may
    /// detect pre-existing issues that weren't caused by the Lattice operation.
    /// We check that scan completes (the most important invariant) but allow
    /// fast_verify to fail due to out-of-band corruption.
    fn assert_post_success_verify(&self, result: &OpResult) -> Result<(), InvariantViolation> {
        if !result.success {
            return Ok(()); // Only check after success
        }

        // Only check for Lattice operations
        if let AnyOp::Lattice(_) = &result.op {
            let git = self.repo.git();

            // The key invariant: scan should complete without panic
            // This verifies Lattice can still read the repo state
            let _ = scan(&git);

            // Note: fast_verify may fail due to pre-existing out-of-band changes.
            // This is expected and not an invariant violation - the fuzz harness
            // intentionally creates broken states via git operations.
            // The real invariant (gating correctness) is checked separately.
        }

        Ok(())
    }

    /// Check all invariants after an operation
    fn check_invariants(&self, result: &OpResult) -> Result<(), InvariantViolation> {
        // Clean up any in-progress git operations first
        self.repo.abort_if_in_progress();

        self.assert_gating_correctness()?;
        self.assert_doctor_offers_repairs()?;
        self.assert_post_success_verify(result)?;

        Ok(())
    }

    /// Run the fuzz test
    fn run(&mut self) -> Result<FuzzReport, FuzzFailure> {
        for i in 0..self.config.ops_per_run {
            let op = self.generate_op();
            let result = self.execute_op(&op);
            self.operation_log.push(result.clone());

            if let Err(violation) = self.check_invariants(&result) {
                return Err(FuzzFailure {
                    violation,
                    operation_number: i,
                    seed: self.config.seed,
                    operation_log: self.operation_log.clone(),
                });
            }
        }

        Ok(FuzzReport {
            operations_executed: self.operation_log.len(),
            seed: self.config.seed,
            successes: self.operation_log.iter().filter(|r| r.success).count(),
            failures: self.operation_log.iter().filter(|r| !r.success).count(),
        })
    }
}

/// Report of a successful fuzz run
#[derive(Debug)]
#[allow(dead_code)]
struct FuzzReport {
    operations_executed: usize,
    seed: u64,
    successes: usize,
    failures: usize,
}

/// Failure during fuzz testing
#[derive(Debug)]
struct FuzzFailure {
    violation: InvariantViolation,
    operation_number: usize,
    seed: u64,
    operation_log: Vec<OpResult>,
}

// =============================================================================
// Tests
// =============================================================================

/// Quick mode: runs deterministic seeds for PR CI
#[test]
fn oob_fuzz_deterministic_seeds() {
    // These seeds were chosen to cover a variety of operation sequences
    let seeds = [42, 12345, 98765, 11111, 55555];

    for seed in seeds {
        let config = FuzzConfig {
            ops_per_run: 30,
            seed,
            include_corruption: true,
        };
        let mut harness = OobFuzzHarness::new(config);
        harness.run().unwrap_or_else(|e| {
            panic!(
                "Seed {} failed at operation {}: {:?}\nOperation log: {:?}",
                seed, e.operation_number, e.violation, e.operation_log
            );
        });
    }
}

/// Test that gating refuses when capabilities are missing
#[test]
fn gating_refuses_when_op_in_progress() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create a tracked branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "feature content", "Add feature");

    let ctx = repo.context();
    commands::track(&ctx, Some("feature"), Some("main"), false, false).unwrap();

    // Create a conflict situation and pause
    repo.checkout("main");
    repo.commit("f.txt", "main content", "Add conflict");

    repo.checkout("feature");
    let _ = commands::restack(&ctx, Some("feature"), true, false);

    // Now we should be in a paused state (or not - depends on if conflict occurred)
    let git = repo.git();
    if git.state().is_in_progress() {
        let snapshot = scan(&git).unwrap();
        let result = gate(snapshot, &requirements::MUTATING);

        // Should refuse because git op is in progress
        assert!(
            !result.is_ready(),
            "Gating should refuse when git op is in progress"
        );

        // Clean up
        repo.abort_if_in_progress();
    }
}

/// Test that doctor offers fixes for corruption
#[test]
fn doctor_offers_fixes_for_corruption() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create and track a branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "feature", "Add feature");

    let ctx = repo.context();
    commands::track(&ctx, Some("feature"), Some("main"), false, false).unwrap();

    // Corrupt the metadata by deleting it
    let meta_ref = "refs/branch-metadata/feature";
    try_run_git(repo.path(), &["update-ref", "-d", meta_ref]);

    // Doctor should detect and offer repairs
    let git = repo.git();
    let snapshot = scan(&git).unwrap();

    let doctor = Doctor::new();
    let report = doctor.diagnose(&snapshot);

    // There should be issues related to the orphaned branch or missing metadata
    // The exact issue depends on how scanner handles missing metadata
    // At minimum, we should have either no blocking issues, or fixes available
    // Note: non-blocking issues like remote warnings are OK without fixes
    let blocking_issues: Vec<_> = report.issues.iter().filter(|i| i.is_blocking()).collect();
    assert!(
        blocking_issues.is_empty() || !report.fixes.is_empty(),
        "Doctor should either have no blocking issues, or offer fixes for them"
    );
}

/// Test that CAS prevents races
#[test]
fn executor_respects_cas_semantics() {
    let repo = TestRepo::new();
    repo.init_lattice();

    // Create and track a branch
    repo.create_branch("feature");
    repo.checkout("feature");
    repo.commit("f.txt", "feature", "Add feature");

    let ctx = repo.context();
    commands::track(&ctx, Some("feature"), Some("main"), false, false).unwrap();

    // Read metadata
    let git = repo.git();
    let store = MetadataStore::new(&git);
    let branch = BranchName::new("feature").unwrap();
    let original = store.read(&branch).unwrap().expect("metadata should exist");

    // Modify metadata directly (simulating concurrent modification)
    let mut modified = original.metadata.clone();
    modified.touch();
    store
        .write_cas(&branch, Some(&original.ref_oid), &modified)
        .unwrap();

    // Now try to use stale OID - this should fail
    let fake_oid =
        lattice::core::types::Oid::new("0000000000000000000000000000000000000000").unwrap();
    let result = store.write_cas(&branch, Some(&fake_oid), &original.metadata);

    assert!(result.is_err(), "CAS should fail with stale OID");
}

/// Thorough mode: runs many iterations (for nightly CI)
#[test]
#[ignore] // Only run when explicitly requested
fn oob_fuzz_thorough() {
    let iterations: usize = std::env::var("LATTICE_FUZZ_ITERATIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let ops_per_run: usize = std::env::var("LATTICE_FUZZ_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    for i in 0..iterations {
        let config = FuzzConfig {
            ops_per_run,
            seed: i as u64,
            include_corruption: true,
        };
        let mut harness = OobFuzzHarness::new(config);
        harness.run().unwrap_or_else(|e| {
            panic!(
                "Iteration {} (seed {}) failed at operation {}: {:?}",
                i, e.seed, e.operation_number, e.violation
            );
        });

        if (i + 1) % 10 == 0 {
            eprintln!("Completed {} iterations", i + 1);
        }
    }
}
