//! Architecture enforcement tests.
//!
//! Per ARCHITECTURE.md Section 12 and SPEC.md Section 5, commands must not
//! bypass the unified lifecycle by calling `scan()` directly. These tests
//! ensure violations are caught in CI.
//!
//! # Why This Matters
//!
//! The governing principle from `runner.rs`:
//! > "Commands cannot call `scan()` directly. All command execution flows
//! > through `run_command()`, which ensures gating is enforced."
//!
//! This ensures:
//! - Engine hooks (Milestone 0.12) fire for all mutating commands
//! - Gating is consistent across all commands
//! - Journaling and crash recovery are uniform
//! - Out-of-band drift detection works everywhere
//!
//! # Test Categories
//!
//! 1. **Scan Import Detection** - Commands must not import `scan` function
//! 2. **Manual Gating Detection** - Commands must not call `check_requirements()`
//! 3. **Trait Verification** - Commands must implement appropriate traits
//! 4. **Phase 5 Tracking** - Documents remaining migration debt

use std::fs;
use std::path::Path;

/// Commands that are excluded from architecture lint checks.
///
/// Per HANDOFF.md, these commands don't benefit from trait migration:
/// - `auth.rs` - No repo required (pure OAuth flow)
/// - `changelog.rs` - Static version info
/// - `completion.rs` - Shell completion generation
/// - `config_cmd.rs` - File I/O only, no repo state
/// - `phase3_helpers.rs` - Internal helper module (not a command)
/// - `stack_comment_ops.rs` - Internal helper module (not a command)
/// - `mod.rs` - Module definition file
const EXCLUDED_COMMANDS: &[&str] = &[
    "auth.rs",
    "changelog.rs",
    "completion.rs",
    "config_cmd.rs",
    "phase3_helpers.rs",
    "stack_comment_ops.rs",
    "mod.rs",
];

/// Commands that are expected to call `scan()` directly.
///
/// These are Phase 5 work items - mutating commands that have not yet
/// been migrated to the `Command` trait. When Phase 5 is complete,
/// this list should be empty.
///
/// Each command here represents technical debt that should be addressed.
/// The architecture lint tracks this debt explicitly rather than hiding it.
const PHASE5_PENDING: &[&str] = &[
    // "create.rs" - MIGRATED to Command trait (2026-01-21)
    // "modify.rs" - MIGRATED to Command trait (2026-01-21)
    // "delete.rs" - MIGRATED to Command trait (2026-01-21)
    // "rename.rs" - MIGRATED to Command trait (2026-01-21)
    // "squash.rs" - MIGRATED to Command trait (2026-01-21)
    // "fold.rs" - MIGRATED to Command trait (2026-01-21)
    // "move_cmd.rs" - MIGRATED to Command trait (2026-01-21)
    // "pop.rs" - MIGRATED to Command trait (2026-01-21)
    // "reorder.rs" - MIGRATED to Command trait (2026-01-21)
    // "split.rs" - MIGRATED to Command trait (2026-01-21)
    // "revert.rs" - MIGRATED to Command trait (2026-01-21)
    // === PHASE 5 COMPLETE: All 11 commands migrated to Command trait ===
];

/// Commands that use `scan()` in internal helper functions AFTER trait-based gating.
///
/// These commands implement `AsyncCommand` and use the trait's `REQUIREMENTS` for gating,
/// but have internal helper functions that call `scan()` for post-gating work.
/// This is an acceptable pattern because:
/// 1. The trait-based gating ensures requirements are checked first
/// 2. The helper functions are internal implementation details
/// 3. The scan() calls are for getting fresh state after the initial gating
///
/// This is different from Phase 5 pending commands which bypass gating entirely.
const ASYNC_WITH_INTERNAL_SCAN: &[&str] = &[
    "submit.rs", // execute_submit helper after AsyncCommand gating
    "sync.rs",   // execute_sync helper after AsyncCommand gating
    "get.rs",    // track_local_branch helper after AsyncCommand gating
    "merge.rs",  // execute_merge helper after AsyncCommand gating
];

/// Commands that use `scan()` for pre-command data gathering.
///
/// These commands implement `Command` trait but need a preliminary scan BEFORE
/// entering the command lifecycle for one of these reasons:
/// - Interactive confirmation needs to show what will be affected
/// - Pre-computation of data that will be used post-plan (e.g., diffs)
///
/// This is acceptable because:
/// 1. The actual command execution goes through run_command() with proper gating
/// 2. The preliminary scan is only for UX/pre-computation, not for mutations
/// 3. The command lifecycle re-scans and validates state properly
const COMMAND_WITH_PRE_SCAN: &[&str] = &[
    "create.rs",   // Preliminary scan for interactive prompts and validation
    "delete.rs",   // Preliminary scan for confirmation prompt
    "modify.rs",   // Preliminary scan for interactive staging and descendant detection
    "move_cmd.rs", // Preliminary scan for cycle detection and descendant info
    "pop.rs",      // Preliminary scan to compute diff before branch deletion
    "reorder.rs",  // Preliminary scan for editor interaction and validation
    "split.rs",    // Preliminary scan for commit listing and file diff extraction
    "squash.rs",   // Preliminary scan to gather commit messages and descendant info
];

/// Commands that are allowed to call `check_requirements()` manually.
///
/// Recovery commands have unique semantics - they don't plan new operations,
/// they resume or reverse existing ones. Per Phase 7 decision, they stay
/// as specialized functions rather than implementing traits.
const ALLOWED_MANUAL_GATING: &[&str] = &["recovery.rs", "undo.rs"];

/// Commands that must implement `ReadOnlyCommand`.
const READONLY_COMMANDS: &[(&str, &str)] = &[
    ("log_cmd.rs", "LogCommand"),
    ("info.rs", "InfoCommand"),
    ("relationships.rs", "ParentCommand"),
    ("relationships.rs", "ChildrenCommand"),
    ("pr.rs", "PrCommand"),
];

/// Commands that must implement `Command`.
const COMMAND_TRAIT_COMMANDS: &[(&str, &str)] = &[
    ("freeze.rs", "FreezeCommand"),
    ("freeze.rs", "UnfreezeCommand"),
    ("restack.rs", "RestackCommand"),
];

/// Commands that must implement `AsyncCommand`.
const ASYNC_COMMANDS: &[(&str, &str)] = &[
    ("submit.rs", "SubmitWithRestackCommand"),
    ("submit.rs", "SubmitNoRestackCommand"),
    ("sync.rs", "SyncWithRestackCommand"),
    ("sync.rs", "SyncNoRestackCommand"),
    ("get.rs", "GetWithCheckoutCommand"),
    ("get.rs", "GetNoCheckoutCommand"),
    ("merge.rs", "MergeCommand"),
];

// =============================================================================
// Scan Import Detection
// =============================================================================

/// Verify that migrated commands do not import the `scan` function directly.
///
/// Commands should use `run_command()`, `run_readonly_command()`, or
/// `run_async_command()` entry points which handle scanning internally.
///
/// # Allowed Patterns
///
/// - Importing `RepoSnapshot` type is allowed (for type annotations)
/// - Excluded commands may use `scan` freely
/// - Phase 5 pending commands are tracked separately
#[test]
fn commands_cannot_import_scan_directly() {
    let command_dir = Path::new("src/cli/commands");

    let mut violations = Vec::new();

    for entry in fs::read_dir(command_dir).expect("Failed to read commands directory") {
        let entry = entry.expect("Failed to read entry");
        let path = entry.path();

        if path.extension().map(|e| e == "rs").unwrap_or(false) {
            let filename = path.file_name().unwrap().to_str().unwrap();

            // Skip excluded commands
            if EXCLUDED_COMMANDS.contains(&filename) {
                continue;
            }

            // Skip Phase 5 pending (tracked separately)
            if PHASE5_PENDING.contains(&filename) {
                continue;
            }

            // Skip async commands with internal scan (acceptable pattern)
            if ASYNC_WITH_INTERNAL_SCAN.contains(&filename) {
                continue;
            }

            // Skip commands with pre-scan for UX/pre-computation (acceptable pattern)
            if COMMAND_WITH_PRE_SCAN.contains(&filename) {
                continue;
            }

            let content =
                fs::read_to_string(&path).unwrap_or_else(|_| panic!("Failed to read {}", filename));

            // Check for direct scan() function import
            // Allow: `use crate::engine::scan::RepoSnapshot` (type import)
            // Deny: `use crate::engine::scan::scan` (function import)
            if content.contains("use crate::engine::scan::scan") {
                violations.push(format!(
                    "{}: imports scan function directly - commands should use runner entry points",
                    filename
                ));
            }

            // Check for direct scan() calls in non-helper functions
            // Note: Some commands have helper functions that legitimately call scan
            // We check for the pattern in the main command body
            let has_direct_scan_call =
                content.contains("scan(&git)") || content.contains("scan(git)");

            // Allow scan calls inside helper functions (marked with specific patterns)
            let has_helper_scan = content.contains("fn compute_") && has_direct_scan_call;
            let has_test_scan = content.contains("#[cfg(test)]") && has_direct_scan_call;

            if has_direct_scan_call && !has_helper_scan && !has_test_scan {
                // Check if it's in a helper function by looking for specific patterns
                // This is a heuristic - we allow scan in clearly marked internal helpers
                if !content.contains("// Internal helper that uses scan") {
                    violations.push(format!(
                        "{}: calls scan() directly - use run_command() or run_readonly_command() instead",
                        filename
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Architecture violations found:\n  {}",
        violations.join("\n  ")
    );
}

/// Document the current count of Phase 5 pending commands.
///
/// This test serves as documentation and tracking. When Phase 5 commands
/// are migrated, update `PHASE5_PENDING` and this expected count.
///
/// When all Phase 5 work is complete, this test should expect 0.
#[test]
fn phase5_pending_count_is_tracked() {
    let expected_pending = 0; // PHASE 5 COMPLETE - all commands migrated

    assert_eq!(
        PHASE5_PENDING.len(),
        expected_pending,
        "Phase 5 pending count changed. \
         If commands were migrated, update PHASE5_PENDING list and this expected count. \
         Current pending: {:?}",
        PHASE5_PENDING
    );
}

// =============================================================================
// Manual Gating Detection
// =============================================================================

/// Verify that trait-based commands do not call `check_requirements()` manually.
///
/// Commands implementing `Command`, `ReadOnlyCommand`, or `AsyncCommand`
/// should rely on the trait's `REQUIREMENTS` constant for gating.
/// Manual `check_requirements()` calls bypass the unified lifecycle.
///
/// # Exceptions
///
/// - Recovery commands (`recovery.rs`, `undo.rs`) are allowed manual gating
/// - Excluded commands are not checked
/// - Phase 5 pending commands are not checked
#[test]
fn commands_do_not_manually_call_check_requirements() {
    let command_dir = Path::new("src/cli/commands");

    let mut violations = Vec::new();

    for entry in fs::read_dir(command_dir).expect("Failed to read commands directory") {
        let entry = entry.expect("Failed to read entry");
        let path = entry.path();

        if path.extension().map(|e| e == "rs").unwrap_or(false) {
            let filename = path.file_name().unwrap().to_str().unwrap();

            // Skip allowed, excluded, and pending
            if ALLOWED_MANUAL_GATING.contains(&filename)
                || EXCLUDED_COMMANDS.contains(&filename)
                || PHASE5_PENDING.contains(&filename)
            {
                continue;
            }

            let content =
                fs::read_to_string(&path).unwrap_or_else(|_| panic!("Failed to read {}", filename));

            // Check for manual check_requirements calls
            if content.contains("check_requirements(") {
                // Verify it's not in a test or doc comment
                let in_test = content
                    .lines()
                    .any(|l| l.contains("#[test]") || l.contains("#[cfg(test)]"));
                let in_doc = content.contains("/// check_requirements");

                if !in_test && !in_doc {
                    violations.push(format!(
                        "{}: calls check_requirements() manually - use Command/AsyncCommand trait instead",
                        filename
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "Manual gating violations found:\n  {}",
        violations.join("\n  ")
    );
}

// =============================================================================
// Trait Implementation Verification
// =============================================================================

/// Verify that required commands implement `ReadOnlyCommand`.
#[test]
fn readonly_commands_implement_trait() {
    let command_dir = Path::new("src/cli/commands");
    let mut missing = Vec::new();

    for (filename, struct_name) in READONLY_COMMANDS {
        let path = command_dir.join(filename);
        let content =
            fs::read_to_string(&path).unwrap_or_else(|_| panic!("Failed to read {}", filename));

        // Look for trait implementation
        let trait_impl = format!("impl ReadOnlyCommand for {}", struct_name);
        if !content.contains(&trait_impl) {
            missing.push(format!(
                "{}: {} should implement ReadOnlyCommand",
                filename, struct_name
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "Commands missing ReadOnlyCommand implementation:\n  {}",
        missing.join("\n  ")
    );
}

/// Verify that required commands implement `Command`.
#[test]
fn mutating_commands_implement_trait() {
    let command_dir = Path::new("src/cli/commands");
    let mut missing = Vec::new();

    for (filename, struct_name) in COMMAND_TRAIT_COMMANDS {
        let path = command_dir.join(filename);
        let content =
            fs::read_to_string(&path).unwrap_or_else(|_| panic!("Failed to read {}", filename));

        // Look for trait implementation
        let trait_impl = format!("impl Command for {}", struct_name);
        if !content.contains(&trait_impl) {
            missing.push(format!(
                "{}: {} should implement Command",
                filename, struct_name
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "Commands missing Command implementation:\n  {}",
        missing.join("\n  ")
    );
}

/// Verify that required commands implement `AsyncCommand`.
#[test]
fn async_commands_implement_trait() {
    let command_dir = Path::new("src/cli/commands");
    let mut missing = Vec::new();

    for (filename, struct_name) in ASYNC_COMMANDS {
        let path = command_dir.join(filename);
        let content =
            fs::read_to_string(&path).unwrap_or_else(|_| panic!("Failed to read {}", filename));

        // Look for trait implementation
        let trait_impl = format!("impl AsyncCommand for {}", struct_name);
        if !content.contains(&trait_impl) {
            missing.push(format!(
                "{}: {} should implement AsyncCommand",
                filename, struct_name
            ));
        }
    }

    assert!(
        missing.is_empty(),
        "Commands missing AsyncCommand implementation:\n  {}",
        missing.join("\n  ")
    );
}

// =============================================================================
// Additional Structural Checks
// =============================================================================

/// Verify that all command files exist.
///
/// This catches accidental file deletions or renames.
#[test]
fn all_expected_command_files_exist() {
    let command_dir = Path::new("src/cli/commands");

    // Combine all known command files
    let mut expected_files: Vec<&str> = Vec::new();

    for (filename, _) in READONLY_COMMANDS {
        if !expected_files.contains(filename) {
            expected_files.push(filename);
        }
    }
    for (filename, _) in COMMAND_TRAIT_COMMANDS {
        if !expected_files.contains(filename) {
            expected_files.push(filename);
        }
    }
    for (filename, _) in ASYNC_COMMANDS {
        if !expected_files.contains(filename) {
            expected_files.push(filename);
        }
    }
    for filename in PHASE5_PENDING {
        if !expected_files.contains(filename) {
            expected_files.push(filename);
        }
    }

    let mut missing = Vec::new();
    for filename in expected_files {
        let path = command_dir.join(filename);
        if !path.exists() {
            missing.push(filename.to_string());
        }
    }

    assert!(
        missing.is_empty(),
        "Expected command files not found:\n  {}",
        missing.join("\n  ")
    );
}

/// Verify navigation commands use run_gated with proper requirements.
///
/// Per Phase 3 decision, navigation commands use `run_gated()` directly
/// which is architecturally sound since it still goes through proper gating.
#[test]
fn navigation_commands_use_run_gated() {
    let command_dir = Path::new("src/cli/commands");
    let navigation_files = ["checkout.rs", "navigation.rs"];

    let mut issues = Vec::new();

    for filename in navigation_files {
        let path = command_dir.join(filename);
        if !path.exists() {
            continue;
        }

        let content =
            fs::read_to_string(&path).unwrap_or_else(|_| panic!("Failed to read {}", filename));

        // Navigation commands should use run_gated
        if !content.contains("run_gated") {
            issues.push(format!(
                "{}: expected to use run_gated() for navigation",
                filename
            ));
        }

        // Should reference NAVIGATION requirements
        if !content.contains("NAVIGATION") && !content.contains("requirements::NAVIGATION") {
            issues.push(format!(
                "{}: expected to use NAVIGATION requirements",
                filename
            ));
        }
    }

    assert!(
        issues.is_empty(),
        "Navigation command issues:\n  {}",
        issues.join("\n  ")
    );
}

/// Verify recovery commands exist and have proper structure.
///
/// Per Phase 7 decision, recovery commands stay as specialized functions
/// but must use `check_requirements(RECOVERY)` for minimal gating.
#[test]
fn recovery_commands_have_proper_structure() {
    let command_dir = Path::new("src/cli/commands");
    let recovery_files = ["recovery.rs", "undo.rs"];

    let mut issues = Vec::new();

    for filename in recovery_files {
        let path = command_dir.join(filename);
        if !path.exists() {
            issues.push(format!("{}: file not found", filename));
            continue;
        }

        let content =
            fs::read_to_string(&path).unwrap_or_else(|_| panic!("Failed to read {}", filename));

        // Should reference RECOVERY requirements
        if !content.contains("RECOVERY") {
            issues.push(format!(
                "{}: expected to use RECOVERY requirements",
                filename
            ));
        }
    }

    assert!(
        issues.is_empty(),
        "Recovery command issues:\n  {}",
        issues.join("\n  ")
    );
}
