// Legacy journal API - these commands will be migrated to executor pattern
#![allow(deprecated)]

//! split command - Split current branch
//!
//! Per SPEC.md 8D.6:
//!
//! - `--by-commit`: split each commit into its own branch
//! - `--by-file <paths>`: extract changes to specified files into new branch
//! - `--by-hunk`: deferred to v2 (returns "not implemented")
//! - Sum-of-diffs invariant: combined diff across resulting stack equals original
//!
//! # Integrity Contract
//!
//! - Must never split frozen branches
//! - Must preserve all changes (no data loss)
//! - Metadata updated only after refs succeed

use std::process::Command;

use anyhow::{Context as _, Result};

use crate::cli::commands::phase3_helpers::{check_freeze, get_commits_in_range};
use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeState, ParentInfo, PrState, Timestamps,
    METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::{Journal, OpState};
use crate::core::ops::lock::RepoLock;
use crate::core::paths::LatticePaths;
use crate::core::types::{BranchName, Oid, UtcTimestamp};
use crate::engine::gate::requirements;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;

/// Split current branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `by_commit` - Split each commit into its own branch
/// * `by_file` - Extract changes to specified files into new branch
pub fn split(ctx: &Context, by_commit: bool, by_file: Vec<String>) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Check for in-progress operation
    if let Some(op_state) = OpState::read(&paths)? {
        anyhow::bail!(
            "Another operation is in progress: {} ({}). Use 'lattice continue' or 'lattice abort'.",
            op_state.command,
            op_state.op_id
        );
    }

    // Validate flags
    if !by_commit && by_file.is_empty() {
        anyhow::bail!("Must specify --by-commit or --by-file <paths>");
    }

    if by_commit && !by_file.is_empty() {
        anyhow::bail!("Cannot use both --by-commit and --by-file");
    }

    // Pre-flight gating check
    crate::engine::runner::check_requirements(&git, &requirements::MUTATING)
        .map_err(|bundle| anyhow::anyhow!("Repository needs repair: {}", bundle))?;

    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Ensure trunk is configured
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    // Check if tracked
    if !snapshot.metadata.contains_key(&current) {
        anyhow::bail!(
            "Branch '{}' is not tracked. Use 'lattice track' first.",
            current
        );
    }

    // Check freeze policy
    check_freeze(&current, &snapshot)?;

    // Get current metadata
    let current_meta = snapshot
        .metadata
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    let base_oid = Oid::new(&current_meta.metadata.base.oid).context("Invalid base OID")?;
    let current_tip = snapshot
        .branches
        .get(&current)
        .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found", current))?;

    // Dispatch to appropriate split mode
    if by_commit {
        split_by_commit(
            ctx,
            &git,
            &cwd,
            &paths,
            &snapshot,
            &current,
            &base_oid,
            current_tip,
            trunk,
        )
    } else {
        split_by_file(
            ctx,
            &git,
            &cwd,
            &paths,
            &snapshot,
            &current,
            &base_oid,
            current_tip,
            trunk,
            &by_file,
        )
    }
}

/// Split branch so each commit becomes its own branch.
#[allow(clippy::too_many_arguments)]
fn split_by_commit(
    ctx: &Context,
    git: &Git,
    cwd: &std::path::Path,
    paths: &LatticePaths,
    snapshot: &crate::engine::scan::RepoSnapshot,
    current: &BranchName,
    base_oid: &Oid,
    current_tip: &Oid,
    trunk: &BranchName,
) -> Result<()> {
    // Get commits in range
    let commits = get_commits_in_range(cwd, base_oid, current_tip)?;

    if commits.is_empty() {
        if !ctx.quiet {
            println!("Branch '{}' has no commits to split.", current);
        }
        return Ok(());
    }

    if commits.len() == 1 {
        if !ctx.quiet {
            println!("Branch '{}' has only 1 commit. Nothing to split.", current);
        }
        return Ok(());
    }

    if !ctx.quiet {
        println!("Splitting '{}' into {} branches...", current, commits.len());
    }

    // Acquire lock
    let _lock = RepoLock::acquire(paths).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("split");

    // Write op-state (legacy: split doesn't use executor pattern yet)
    #[allow(deprecated)]
    let op_state = OpState::from_journal_legacy(&journal, paths, None);
    op_state.write(paths)?;

    let store = MetadataStore::new(git);

    // Get current metadata for parent info
    let current_meta = snapshot
        .metadata
        .get(current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    let original_parent = current_meta.metadata.parent.clone();

    // Create a branch for each commit
    let mut prev_branch = if current_meta.metadata.parent.is_trunk() {
        trunk.clone()
    } else {
        BranchName::new(current_meta.metadata.parent.name())?
    };

    let mut prev_tip = base_oid.clone();
    let mut created_branches = Vec::new();

    // Detach HEAD so we can force-update the current branch
    let status = Command::new("git")
        .args(["checkout", "--detach"])
        .current_dir(cwd)
        .status()
        .context("Failed to detach HEAD")?;

    if !status.success() {
        OpState::remove(paths)?;
        anyhow::bail!("git checkout --detach failed");
    }

    for (i, commit) in commits.iter().enumerate() {
        let branch_name = if i == commits.len() - 1 {
            // Last commit keeps original branch name
            current.clone()
        } else {
            // Other commits get numbered names
            BranchName::new(format!("{}-{}", current, i + 1))?
        };

        if !ctx.quiet {
            println!(
                "  Creating '{}' with commit {}...",
                branch_name,
                &commit.as_str()[..7]
            );
        }

        // Create branch at this commit
        let status = Command::new("git")
            .args(["branch", "-f", branch_name.as_str(), commit.as_str()])
            .current_dir(cwd)
            .status()
            .with_context(|| format!("Failed to create branch '{}'", branch_name))?;

        if !status.success() {
            OpState::remove(paths)?;
            anyhow::bail!("git branch -f failed for '{}'", branch_name);
        }

        journal.record_ref_update(
            format!("refs/heads/{}", branch_name),
            if &branch_name == current {
                Some(current_tip.to_string())
            } else {
                None
            },
            commit.to_string(),
        );

        // Create/update metadata
        let parent_ref = if &prev_branch == trunk {
            ParentInfo::Trunk {
                name: prev_branch.to_string(),
            }
        } else {
            ParentInfo::Branch {
                name: prev_branch.to_string(),
            }
        };

        let now = UtcTimestamp::now();
        let metadata = BranchMetadataV1 {
            kind: METADATA_KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            branch: BranchInfo {
                name: branch_name.to_string(),
            },
            parent: parent_ref,
            base: BaseInfo {
                oid: prev_tip.to_string(),
            },
            freeze: FreezeState::Unfrozen,
            pr: PrState::None,
            timestamps: Timestamps {
                created_at: now.clone(),
                updated_at: now,
            },
        };

        if &branch_name == current {
            // Update existing metadata
            let new_ref_oid =
                store.write_cas(&branch_name, Some(&current_meta.ref_oid), &metadata)?;
            journal.record_metadata_write(
                branch_name.as_str(),
                Some(current_meta.ref_oid.to_string()),
                new_ref_oid,
            );
        } else {
            // Create new metadata
            let new_ref_oid = store.write_cas(&branch_name, None, &metadata)?;
            journal.record_metadata_write(branch_name.as_str(), None, new_ref_oid);
        }

        created_branches.push(branch_name.clone());
        prev_branch = branch_name;
        prev_tip = commit.clone();
    }

    // Checkout the last branch (original name)
    let status = Command::new("git")
        .args(["checkout", current.as_str()])
        .current_dir(cwd)
        .status()
        .context("Failed to checkout branch")?;

    if !status.success() {
        eprintln!("Warning: Failed to checkout '{}'", current);
    }

    // Reparent any children of original branch to point to last created branch
    // (which is the original branch name, so children don't need updating)

    // Commit journal
    journal.commit();
    journal.write(paths)?;

    // Clear op-state
    OpState::remove(paths)?;

    if !ctx.quiet {
        println!(
            "Split complete. Created {} branches:",
            created_branches.len()
        );
        for (i, branch) in created_branches.iter().enumerate() {
            let parent = if i == 0 {
                if original_parent.is_trunk() {
                    trunk.to_string()
                } else {
                    original_parent.name().to_string()
                }
            } else {
                created_branches[i - 1].to_string()
            };
            println!("  {} (parent: {})", branch, parent);
        }
    }

    Ok(())
}

/// Split branch by extracting changes to specified files into a new branch.
#[allow(clippy::too_many_arguments)]
fn split_by_file(
    ctx: &Context,
    git: &Git,
    cwd: &std::path::Path,
    paths: &LatticePaths,
    snapshot: &crate::engine::scan::RepoSnapshot,
    current: &BranchName,
    base_oid: &Oid,
    current_tip: &Oid,
    trunk: &BranchName,
    files: &[String],
) -> Result<()> {
    if !ctx.quiet {
        println!("Splitting '{}' by files: {:?}...", current, files);
    }

    // Get current metadata
    let current_meta = snapshot
        .metadata
        .get(current)
        .ok_or_else(|| anyhow::anyhow!("Metadata not found for '{}'", current))?;

    // Get diff for specified files
    let mut diff_args = vec!["diff", base_oid.as_str(), current_tip.as_str(), "--"];
    diff_args.extend(files.iter().map(|s| s.as_str()));

    let output = Command::new("git")
        .args(&diff_args)
        .current_dir(cwd)
        .output()
        .context("Failed to get file diff")?;

    if !output.status.success() {
        anyhow::bail!("git diff failed");
    }

    let file_diff = String::from_utf8_lossy(&output.stdout).to_string();

    if file_diff.trim().is_empty() {
        anyhow::bail!("No changes to specified files in this branch");
    }

    // Get diff for remaining files
    let mut remaining_args = vec![
        "diff".to_string(),
        base_oid.to_string(),
        current_tip.to_string(),
        "--".to_string(),
    ];
    // Add pathspec to exclude the specified files
    for file in files {
        remaining_args.push(format!(":!{}", file));
    }

    let output = Command::new("git")
        .args(&remaining_args)
        .current_dir(cwd)
        .output()
        .context("Failed to get remaining diff")?;

    let remaining_diff = String::from_utf8_lossy(&output.stdout).to_string();

    // Acquire lock
    let _lock = RepoLock::acquire(paths).context("Failed to acquire repository lock")?;

    // Create journal
    let mut journal = Journal::new("split");

    // Write op-state (legacy: split doesn't use executor pattern yet)
    #[allow(deprecated)]
    let op_state = OpState::from_journal_legacy(&journal, paths, None);
    op_state.write(paths)?;

    let store = MetadataStore::new(git);

    // Name for the new branch containing extracted files
    let new_branch_name = BranchName::new(format!("{}-files", current))?;

    // Check if new branch name already exists
    if snapshot.branches.contains_key(&new_branch_name) {
        OpState::remove(paths)?;
        anyhow::bail!("Branch '{}' already exists", new_branch_name);
    }

    // Get parent info
    let parent_name = if current_meta.metadata.parent.is_trunk() {
        trunk.clone()
    } else {
        BranchName::new(current_meta.metadata.parent.name())?
    };

    // Create new branch at base
    let status = Command::new("git")
        .args([
            "checkout",
            "-b",
            new_branch_name.as_str(),
            base_oid.as_str(),
        ])
        .current_dir(cwd)
        .status()
        .context("Failed to create new branch")?;

    if !status.success() {
        OpState::remove(paths)?;
        anyhow::bail!("git checkout -b failed");
    }

    journal.record_ref_update(
        format!("refs/heads/{}", new_branch_name),
        None,
        base_oid.to_string(),
    );

    // Apply file diff
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("git")
        .args(["apply", "--index"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn git apply")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(file_diff.as_bytes())
            .context("Failed to write diff")?;
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Restore state
        let _ = Command::new("git")
            .args(["checkout", current.as_str()])
            .current_dir(cwd)
            .status();
        let _ = Command::new("git")
            .args(["branch", "-D", new_branch_name.as_str()])
            .current_dir(cwd)
            .status();
        OpState::remove(paths)?;
        anyhow::bail!("Failed to apply file changes: {}", stderr);
    }

    // Commit the changes
    let commit_msg = format!("Split from '{}': changes to {:?}", current, files);
    let mut commit_args = vec!["commit"];
    if !ctx.verify {
        commit_args.push("--no-verify");
    }
    commit_args.extend(["-m", &commit_msg]);
    let status = Command::new("git")
        .args(&commit_args)
        .current_dir(cwd)
        .status()
        .context("Failed to commit")?;

    if !status.success() {
        OpState::remove(paths)?;
        anyhow::bail!("git commit failed");
    }

    // Get new branch tip
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()?;

    let new_branch_tip = String::from_utf8_lossy(&output.stdout).trim().to_string();

    journal.record_ref_update(
        format!("refs/heads/{}", new_branch_name),
        Some(base_oid.to_string()),
        new_branch_tip.clone(),
    );

    // Create metadata for new branch
    let parent_ref = if &parent_name == trunk {
        ParentInfo::Trunk {
            name: parent_name.to_string(),
        }
    } else {
        ParentInfo::Branch {
            name: parent_name.to_string(),
        }
    };

    let now = UtcTimestamp::now();
    let new_metadata = BranchMetadataV1 {
        kind: METADATA_KIND.to_string(),
        schema_version: SCHEMA_VERSION,
        branch: BranchInfo {
            name: new_branch_name.to_string(),
        },
        parent: parent_ref,
        base: BaseInfo {
            oid: base_oid.to_string(),
        },
        freeze: FreezeState::Unfrozen,
        pr: PrState::None,
        timestamps: Timestamps {
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    };

    let new_ref_oid = store.write_cas(&new_branch_name, None, &new_metadata)?;
    journal.record_metadata_write(new_branch_name.as_str(), None, new_ref_oid);

    // Now update original branch to have remaining changes only
    // First, reset original branch to base
    let status = Command::new("git")
        .args(["checkout", current.as_str()])
        .current_dir(cwd)
        .status()?;

    if !status.success() {
        anyhow::bail!("Failed to checkout original branch");
    }

    // Reset to base
    let status = Command::new("git")
        .args(["reset", "--hard", base_oid.as_str()])
        .current_dir(cwd)
        .status()?;

    if !status.success() {
        anyhow::bail!("git reset failed");
    }

    // Apply remaining diff if any
    if !remaining_diff.trim().is_empty() {
        let mut child = Command::new("git")
            .args(["apply", "--index"])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(remaining_diff.as_bytes())?;
        }

        let output = child.wait_with_output()?;

        if output.status.success() {
            // Commit remaining changes
            let mut commit_args = vec!["commit"];
            if !ctx.verify {
                commit_args.push("--no-verify");
            }
            commit_args.extend(["-m", "Remaining changes after split"]);
            let status = Command::new("git")
                .args(&commit_args)
                .current_dir(cwd)
                .status()?;

            if !status.success() {
                eprintln!("Warning: Failed to commit remaining changes");
            }
        }
    }

    // Get updated tip
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(cwd)
        .output()?;

    let updated_tip = String::from_utf8_lossy(&output.stdout).trim().to_string();

    journal.record_ref_update(
        format!("refs/heads/{}", current),
        Some(current_tip.to_string()),
        updated_tip,
    );

    // Update original branch metadata to have new branch as parent
    let mut updated_meta = current_meta.metadata.clone();
    updated_meta.parent = ParentInfo::Branch {
        name: new_branch_name.to_string(),
    };
    updated_meta.base = BaseInfo {
        oid: new_branch_tip,
    };
    updated_meta.timestamps.updated_at = now;

    let updated_ref_oid = store.write_cas(current, Some(&current_meta.ref_oid), &updated_meta)?;
    journal.record_metadata_write(
        current.as_str(),
        Some(current_meta.ref_oid.to_string()),
        updated_ref_oid,
    );

    // Commit journal
    journal.commit();
    journal.write(paths)?;

    // Clear op-state
    OpState::remove(paths)?;

    if !ctx.quiet {
        println!("Split complete.");
        println!(
            "  Created '{}' with changes to {:?}",
            new_branch_name, files
        );
        println!("  Updated '{}' with remaining changes", current);
        println!(
            "  Stack: {} -> {} -> {}",
            parent_name, new_branch_name, current
        );
    }

    Ok(())
}
