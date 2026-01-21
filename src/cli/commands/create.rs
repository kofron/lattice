//! create command - Create a new tracked branch
//!
//! # Gating
//!
//! Uses `requirements::MUTATING` - requires working directory, trunk known,
//! no ops in progress, frozen policy satisfied.

use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeState, ParentInfo, PrState, Timestamps,
    METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::OpState;
use crate::core::paths::LatticePaths;
use crate::core::types::BranchName;
use crate::engine::gate::requirements;
use crate::engine::runner::{run_gated, RunError};
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};
use std::io::{self, Write};
use std::process::Command;

/// Create a new tracked branch.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `name` - Name for the new branch
/// * `message` - Commit message (creates a commit with staged changes)
/// * `all` - Stage all changes before committing
/// * `update` - Stage modified tracked files before committing
/// * `patch` - Interactive patch staging
/// * `insert` - Insert between current branch and its child
pub fn create(
    ctx: &Context,
    name: Option<&str>,
    message: Option<&str>,
    all: bool,
    update: bool,
    patch: bool,
    insert: bool,
) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let info = git.info()?;
    let paths = LatticePaths::from_repo_info(&info);

    // Check for in-progress operation (done before gating because it's a special state)
    if let Some(op_state) = OpState::read(&paths)? {
        anyhow::bail!(
            "Another operation is in progress: {} ({}). Use 'lattice continue' or 'lattice abort'.",
            op_state.command,
            op_state.op_id
        );
    }

    run_gated(&git, ctx, &requirements::MUTATING, |ready| {
        let snapshot = &ready.snapshot;

        // Ensure trunk is configured (gating checks TrunkKnown, but be explicit)
        let trunk = snapshot.trunk.as_ref().ok_or_else(|| {
            RunError::Scan(crate::engine::scan::ScanError::Internal(
                "Trunk not configured. Run 'lattice init' first.".to_string(),
            ))
        })?;

        // Get current branch (will be the parent)
        let parent = snapshot
            .current_branch
            .as_ref()
            .ok_or_else(|| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "Not on any branch".to_string(),
                ))
            })?
            .clone();

        // Determine branch name
        let branch_name = if let Some(n) = name {
            BranchName::new(n).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Invalid branch name: {}",
                    e
                )))
            })?
        } else if let Some(msg) = message {
            // Derive from message
            let slug = slugify(msg);
            BranchName::new(&slug).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Could not derive valid branch name from message: {}",
                    e
                )))
            })?
        } else if ctx.interactive {
            // Prompt for name
            print!("Branch name: ");
            io::stdout().flush().map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "IO error: {}",
                    e
                )))
            })?;
            let mut input = String::new();
            io::stdin().read_line(&mut input).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "IO error: {}",
                    e
                )))
            })?;
            let input = input.trim();
            if input.is_empty() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "Branch name required".to_string(),
                )));
            }
            BranchName::new(input).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Invalid branch name: {}",
                    e
                )))
            })?
        } else {
            return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                "Branch name required. Use --message to derive from commit message.".to_string(),
            )));
        };

        // Check if branch already exists
        if snapshot.branches.contains_key(&branch_name) {
            return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                format!("Branch '{}' already exists", branch_name),
            )));
        }

        // Handle insert mode
        let child_to_reparent = if insert {
            // Find child of current branch
            let children = snapshot.graph.children(&parent);
            match children {
                None => {
                    return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                        "No child branch to insert before".to_string(),
                    )));
                }
                Some(kids) if kids.is_empty() => {
                    return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                        "No child branch to insert before".to_string(),
                    )));
                }
                Some(kids) if kids.len() == 1 => Some(kids.iter().next().unwrap().clone()),
                Some(kids) if ctx.interactive => {
                    // Prompt for selection
                    println!("Select child to insert before:");
                    let kids_vec: Vec<_> = kids.iter().collect();
                    for (i, child) in kids_vec.iter().enumerate() {
                        println!("  {}. {}", i + 1, child);
                    }
                    print!("Enter number: ");
                    io::stdout().flush().map_err(|e| {
                        RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                            "IO error: {}",
                            e
                        )))
                    })?;

                    let mut input = String::new();
                    io::stdin().read_line(&mut input).map_err(|e| {
                        RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                            "IO error: {}",
                            e
                        )))
                    })?;
                    let idx = input.trim().parse::<usize>().map_err(|_| {
                        RunError::Scan(crate::engine::scan::ScanError::Internal(
                            "Invalid selection".to_string(),
                        ))
                    })?;
                    let idx = idx.saturating_sub(1);

                    if idx >= kids_vec.len() {
                        return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                            "Invalid selection".to_string(),
                        )));
                    }

                    Some(kids_vec[idx].clone())
                }
                Some(kids) => {
                    return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                        format!(
                            "Multiple children. Run interactively to select: {}",
                            kids.iter()
                                .map(|b| b.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )));
                }
            }
        } else {
            None
        };

        // Stage changes if requested
        if all {
            let status = Command::new("git")
                .args(["add", "-A"])
                .current_dir(&cwd)
                .status()
                .map_err(|e| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                        "Failed to run git add: {}",
                        e
                    )))
                })?;

            if !status.success() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "git add failed".to_string(),
                )));
            }
        } else if update {
            let status = Command::new("git")
                .args(["add", "-u"])
                .current_dir(&cwd)
                .status()
                .map_err(|e| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                        "Failed to run git add: {}",
                        e
                    )))
                })?;

            if !status.success() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "git add failed".to_string(),
                )));
            }
        } else if patch {
            let status = Command::new("git")
                .args(["add", "-p"])
                .current_dir(&cwd)
                .status()
                .map_err(|e| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                        "Failed to run git add: {}",
                        e
                    )))
                })?;

            if !status.success() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "git add failed".to_string(),
                )));
            }
        }

        // Check for staged changes
        let has_staged = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&cwd)
            .status()
            .map(|s| !s.success())
            .unwrap_or(false);

        // Create branch
        let status = Command::new("git")
            .args(["checkout", "-b", branch_name.as_str()])
            .current_dir(&cwd)
            .status()
            .map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to create branch: {}",
                    e
                )))
            })?;

        if !status.success() {
            return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                "git checkout -b failed".to_string(),
            )));
        }

        // Create commit if we have staged changes and a message
        if has_staged {
            if let Some(msg) = message {
                let mut commit_args = vec!["commit"];
                if !ctx.verify {
                    commit_args.push("--no-verify");
                }
                commit_args.extend(["-m", msg]);
                let status = Command::new("git")
                    .args(&commit_args)
                    .current_dir(&cwd)
                    .status()
                    .map_err(|e| {
                        RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                            "Failed to create commit: {}",
                            e
                        )))
                    })?;

                if !status.success() {
                    return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                        "git commit failed".to_string(),
                    )));
                }
            } else if ctx.interactive {
                // Open editor for commit message
                let mut commit_args = vec!["commit"];
                if !ctx.verify {
                    commit_args.push("--no-verify");
                }
                let status = Command::new("git")
                    .args(&commit_args)
                    .current_dir(&cwd)
                    .status()
                    .map_err(|e| {
                        RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                            "Failed to create commit: {}",
                            e
                        )))
                    })?;

                if !status.success() {
                    // User may have aborted
                    if !ctx.quiet {
                        println!("Commit aborted. Branch created but empty.");
                    }
                }
            } else if !ctx.quiet {
                println!("Staged changes exist. Use --message or run interactively to commit.");
            }
        }

        // Get parent tip for base
        let parent_oid = snapshot.branches.get(&parent).ok_or_else(|| {
            RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                "Parent branch '{}' not found",
                parent
            )))
        })?;

        // Create parent ref
        let parent_ref = if &parent == trunk {
            ParentInfo::Trunk {
                name: parent.to_string(),
            }
        } else {
            ParentInfo::Branch {
                name: parent.to_string(),
            }
        };

        // Create metadata
        let now = crate::core::types::UtcTimestamp::now();
        let metadata = BranchMetadataV1 {
            kind: METADATA_KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            branch: BranchInfo {
                name: branch_name.to_string(),
            },
            parent: parent_ref,
            base: BaseInfo {
                oid: parent_oid.to_string(),
            },
            freeze: FreezeState::Unfrozen,
            pr: PrState::None,
            timestamps: Timestamps {
                created_at: now.clone(),
                updated_at: now,
            },
        };

        // Write metadata (create new - no old ref expected)
        let store = MetadataStore::new(&git);
        store
            .write_cas(&branch_name, None, &metadata)
            .map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to write metadata: {}",
                    e
                )))
            })?;

        if !ctx.quiet {
            println!(
                "Created '{}' with parent '{}' (base: {})",
                branch_name,
                parent,
                &parent_oid.as_str()[..7]
            );
        }

        // Handle insert mode - reparent child
        if let Some(child) = child_to_reparent {
            // Get child's current metadata
            let child_scanned = snapshot.metadata.get(&child).ok_or_else(|| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Child '{}' metadata not found",
                    child
                )))
            })?;

            // Get new branch tip as new base for child
            let output = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&cwd)
                .output()
                .map_err(|e| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                        "Failed to get HEAD: {}",
                        e
                    )))
                })?;

            let new_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let new_base_oid = crate::core::types::Oid::new(&new_base).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Invalid OID from HEAD: {}",
                    e
                )))
            })?;

            // Update child's metadata
            let mut updated_child = child_scanned.metadata.clone();
            updated_child.parent = ParentInfo::Branch {
                name: branch_name.to_string(),
            };
            updated_child.base = BaseInfo {
                oid: new_base_oid.to_string(),
            };
            updated_child.timestamps.updated_at = crate::core::types::UtcTimestamp::now();

            store
                .write_cas(&child, Some(&child_scanned.ref_oid), &updated_child)
                .map_err(|e| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                        "Failed to update child '{}' metadata: {}",
                        child, e
                    )))
                })?;

            if !ctx.quiet {
                println!("Reparented '{}' under '{}'", child, branch_name);
            }
        }

        Ok(())
    })
    .map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}

/// Convert a string to a branch-name-safe slug.
pub fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .take(5) // Limit to first 5 words
        .collect::<Vec<_>>()
        .join("-")
}
