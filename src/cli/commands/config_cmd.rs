//! config command - Get, set, or list configuration values

use crate::engine::scan::scan;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

/// Get a configuration value.
pub fn get(ctx: &Context, key: &str) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    let value = match key {
        "trunk.branch" | "trunk" => snapshot
            .trunk
            .as_ref()
            .map(|t| t.to_string())
            .unwrap_or_default(),
        _ => {
            // Try to get from repo config
            if snapshot.repo_config.is_some() {
                bail!("Unknown configuration key: {}", key)
            } else {
                bail!("Repository not initialized. Run 'lattice init' first.");
            }
        }
    };

    if value.is_empty() {
        // Key exists but has no value - exit silently
        Ok(())
    } else {
        println!("{}", value);
        Ok(())
    }
}

/// Set a configuration value.
pub fn set(ctx: &Context, key: &str, value: &str) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let git_dir = git.git_dir();
    let _config_path = git_dir.join("lattice/config.toml");

    // Load existing config
    let config_result =
        crate::core::config::Config::load(Some(git_dir)).context("Failed to load config")?;

    let mut config = config_result
        .config
        .repo
        .ok_or_else(|| anyhow::anyhow!("Repository not initialized. Run 'lattice init' first."))?;

    // Set the value
    match key {
        "trunk.branch" | "trunk" => {
            // Validate branch name
            crate::core::types::BranchName::new(value).context("Invalid branch name")?;
            config.trunk = Some(value.to_string());
        }
        _ => bail!("Unknown configuration key: {}", key),
    }

    // Write config
    crate::core::config::Config::write_repo(git_dir, &config).context("Failed to write config")?;

    if !ctx.quiet {
        println!("Set {} = {}", key, value);
    }

    Ok(())
}

/// List all configuration values.
pub fn list(ctx: &Context) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let snapshot = scan(&git).context("Failed to scan repository")?;

    println!("# Repository Configuration");

    if let Some(ref trunk) = snapshot.trunk {
        println!("trunk.branch = {}", trunk);
    } else {
        println!("trunk.branch = (not set)");
    }

    // Add other config values as they're implemented
    if let Some(ref _config) = snapshot.repo_config {
        // Future config values go here
    }

    Ok(())
}
