//! init command - Initialize Lattice in this repository

use crate::core::config::{Config, RepoConfig};
use crate::core::metadata::store::MetadataStore;
use crate::core::types::BranchName;
use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};
use std::io::{self, Write};

/// Initialize Lattice in this repository.
///
/// # Arguments
///
/// * `ctx` - Execution context
/// * `trunk` - Set trunk branch
/// * `reset` - Clear all metadata and reconfigure
/// * `force` - Skip confirmation prompts
pub fn init(ctx: &Context, trunk: Option<&str>, reset: bool, force: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let git_dir = git.git_dir();

    // Check if already initialized
    let config_path = git_dir.join("lattice/config.toml");
    let already_initialized = config_path.exists();

    if already_initialized && !reset {
        if !ctx.quiet {
            println!("Lattice is already initialized in this repository.");
            println!("Use --reset to reconfigure.");
        }
        return Ok(());
    }

    // Handle reset
    if reset {
        if !force && ctx.interactive {
            print!("This will delete all branch metadata. Continue? [y/N] ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return Ok(());
            }
        } else if !force {
            bail!("Use --force to reset in non-interactive mode");
        }

        // Delete all metadata refs
        let store = MetadataStore::new(&git);
        let metadata_refs = store.list().unwrap_or_default();
        for branch in metadata_refs {
            // Read existing metadata to get the ref_oid for CAS delete
            match store.read(&branch) {
                Ok(Some(scanned)) => {
                    if let Err(e) = store.delete_cas(&branch, &scanned.ref_oid) {
                        eprintln!("Warning: failed to delete metadata for {}: {}", branch, e);
                    }
                }
                Ok(None) => {
                    // Already deleted
                }
                Err(e) => {
                    eprintln!("Warning: failed to read metadata for {}: {}", branch, e);
                }
            }
        }

        if !ctx.quiet {
            println!("Cleared all branch metadata.");
        }
    }

    // Determine trunk branch
    let trunk_name = if let Some(name) = trunk {
        // Validate branch exists
        let branch = BranchName::new(name).context("Invalid trunk branch name")?;
        let snapshot = scan(&git).context("Failed to scan repository")?;
        if !snapshot.branches.contains_key(&branch) {
            bail!("Branch '{}' does not exist", name);
        }
        branch
    } else if ctx.interactive {
        // Interactive selection
        let snapshot = scan(&git).context("Failed to scan repository")?;
        let branches: Vec<_> = snapshot.branches.keys().collect();

        if branches.is_empty() {
            bail!("No branches found in repository");
        }

        println!("Select trunk branch:");
        for (i, branch) in branches.iter().enumerate() {
            println!("  {}. {}", i + 1, branch);
        }
        print!("Enter number [1]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        let idx = if input.is_empty() {
            0
        } else {
            input
                .parse::<usize>()
                .context("Invalid selection")?
                .saturating_sub(1)
        };

        if idx >= branches.len() {
            bail!("Invalid selection");
        }

        branches[idx].clone()
    } else {
        // Default to main or master
        let snapshot = scan(&git).context("Failed to scan repository")?;
        if let Some(main) = snapshot.branches.keys().find(|b| b.as_str() == "main") {
            main.clone()
        } else if let Some(master) = snapshot.branches.keys().find(|b| b.as_str() == "master") {
            master.clone()
        } else {
            bail!("No trunk specified and could not find 'main' or 'master' branch. Use --trunk to specify.");
        }
    };

    // Create config directory
    let lattice_dir = git_dir.join("lattice");
    std::fs::create_dir_all(&lattice_dir).context("Failed to create .git/lattice directory")?;

    // Write config
    let config = RepoConfig {
        trunk: Some(trunk_name.to_string()),
        ..Default::default()
    };
    Config::write_repo(&cwd, &config).context("Failed to write config")?;

    if !ctx.quiet {
        println!("Initialized Lattice with trunk: {}", trunk_name);
    }

    Ok(())
}
