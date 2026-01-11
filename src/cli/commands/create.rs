//! create command - Create a new tracked branch

use crate::core::metadata::schema::{
    BaseInfo, BranchInfo, BranchMetadataV1, FreezeState, ParentInfo, PrState, Timestamps,
    METADATA_KIND, SCHEMA_VERSION,
};
use crate::core::metadata::store::MetadataStore;
use crate::core::ops::journal::OpState;
use crate::core::paths::LatticePaths;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};
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

    // Check for in-progress operation
    if let Some(op_state) = OpState::read(&paths)? {
        bail!(
            "Another operation is in progress: {} ({}). Use 'lattice continue' or 'lattice abort'.",
            op_state.command,
            op_state.op_id
        );
    }

    let snapshot = scan(&git).context("Failed to scan repository")?;

    // Ensure trunk is configured
    let trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured. Run 'lattice init' first."))?;

    // Get current branch (will be the parent)
    let parent = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on any branch"))?
        .clone();

    // Determine branch name
    let branch_name = if let Some(n) = name {
        BranchName::new(n).context("Invalid branch name")?
    } else if let Some(msg) = message {
        // Derive from message
        let slug = slugify(msg);
        BranchName::new(&slug).context("Could not derive valid branch name from message")?
    } else if ctx.interactive {
        // Prompt for name
        print!("Branch name: ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();
        if input.is_empty() {
            bail!("Branch name required");
        }
        BranchName::new(input).context("Invalid branch name")?
    } else {
        bail!("Branch name required. Use --message to derive from commit message.");
    };

    // Check if branch already exists
    if snapshot.branches.contains_key(&branch_name) {
        bail!("Branch '{}' already exists", branch_name);
    }

    // Handle insert mode
    let child_to_reparent = if insert {
        // Find child of current branch
        let children = snapshot.graph.children(&parent);
        match children {
            None => {
                bail!("No child branch to insert before");
            }
            Some(kids) if kids.is_empty() => {
                bail!("No child branch to insert before");
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
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                let idx = input
                    .trim()
                    .parse::<usize>()
                    .context("Invalid selection")?
                    .saturating_sub(1);

                if idx >= kids_vec.len() {
                    bail!("Invalid selection");
                }

                Some(kids_vec[idx].clone())
            }
            Some(kids) => {
                bail!(
                    "Multiple children. Run interactively to select: {}",
                    kids.iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
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
            .context("Failed to run git add")?;

        if !status.success() {
            bail!("git add failed");
        }
    } else if update {
        let status = Command::new("git")
            .args(["add", "-u"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add")?;

        if !status.success() {
            bail!("git add failed");
        }
    } else if patch {
        let status = Command::new("git")
            .args(["add", "-p"])
            .current_dir(&cwd)
            .status()
            .context("Failed to run git add")?;

        if !status.success() {
            bail!("git add failed");
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
        .context("Failed to create branch")?;

    if !status.success() {
        bail!("git checkout -b failed");
    }

    // Create commit if we have staged changes and a message
    if has_staged {
        if let Some(msg) = message {
            let status = Command::new("git")
                .args(["commit", "-m", msg])
                .current_dir(&cwd)
                .status()
                .context("Failed to create commit")?;

            if !status.success() {
                bail!("git commit failed");
            }
        } else if ctx.interactive {
            // Open editor for commit message
            let status = Command::new("git")
                .args(["commit"])
                .current_dir(&cwd)
                .status()
                .context("Failed to create commit")?;

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
    let parent_oid = snapshot
        .branches
        .get(&parent)
        .ok_or_else(|| anyhow::anyhow!("Parent branch '{}' not found", parent))?
        .clone();

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
        .context("Failed to write metadata")?;

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
        let child_scanned = snapshot
            .metadata
            .get(&child)
            .ok_or_else(|| anyhow::anyhow!("Child '{}' metadata not found", child))?;

        // Get new branch tip as new base for child
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&cwd)
            .output()
            .context("Failed to get HEAD")?;

        let new_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let new_base_oid =
            crate::core::types::Oid::new(&new_base).context("Invalid OID from HEAD")?;

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
            .with_context(|| format!("Failed to update child '{}' metadata", child))?;

        if !ctx.quiet {
            println!("Reparented '{}' under '{}'", child, branch_name);
        }
    }

    Ok(())
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
