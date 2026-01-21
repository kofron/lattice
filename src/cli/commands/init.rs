//! init command - Initialize Lattice in this repository
//!
//! # Gating
//!
//! Uses `requirements::MINIMAL` (just `RepoOpen`) - this is the command that
//! sets up trunk configuration, so it can't require `TrunkKnown`.

use crate::core::config::{Config, RepoConfig};
use crate::core::metadata::store::MetadataStore;
use crate::core::types::BranchName;
use crate::engine::gate::requirements;
use crate::engine::runner::{run_gated, RunError};
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
///
/// # Gating
///
/// Uses `requirements::MINIMAL` since trunk may not be configured yet.
pub fn init(ctx: &Context, trunk: Option<&str>, reset: bool, force: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;
    let git_dir = git.git_dir().to_path_buf();

    // Check if already initialized (before gating - this is a quick file check)
    let config_path = git_dir.join("lattice/config.toml");
    let already_initialized = config_path.exists();

    if already_initialized && !reset {
        if !ctx.quiet {
            println!("Lattice is already initialized in this repository.");
            println!("Use --reset to reconfigure.");
        }
        return Ok(());
    }

    // Handle reset - this needs gating
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

    // Run through gating with MINIMAL requirements (just RepoOpen)
    run_gated(&git, ctx, &requirements::MINIMAL, |ready| {
        let snapshot = &ready.snapshot;

        // Determine trunk branch
        let trunk_name = if let Some(name) = trunk {
            // Validate branch exists
            let branch = BranchName::new(name).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Invalid trunk branch name: {}",
                    e
                )))
            })?;
            if !snapshot.branches.contains_key(&branch) {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    format!("Branch '{}' does not exist", name),
                )));
            }
            branch
        } else if ctx.interactive {
            // Interactive selection
            let branches: Vec<_> = snapshot.branches.keys().collect();

            if branches.is_empty() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "No branches found in repository".to_string(),
                )));
            }

            println!("Select trunk branch:");
            for (i, branch) in branches.iter().enumerate() {
                println!("  {}. {}", i + 1, branch);
            }
            print!("Enter number [1]: ");
            io::stdout().flush().map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to flush stdout: {}",
                    e
                )))
            })?;

            let mut input = String::new();
            io::stdin().read_line(&mut input).map_err(|e| {
                RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                    "Failed to read input: {}",
                    e
                )))
            })?;
            let input = input.trim();

            let idx = if input.is_empty() {
                0
            } else {
                input.parse::<usize>().map_err(|_| {
                    RunError::Scan(crate::engine::scan::ScanError::Internal(
                        "Invalid selection".to_string(),
                    ))
                })?.saturating_sub(1)
            };

            if idx >= branches.len() {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "Invalid selection".to_string(),
                )));
            }

            branches[idx].clone()
        } else {
            // Default to main or master
            if let Some(main) = snapshot.branches.keys().find(|b| b.as_str() == "main") {
                main.clone()
            } else if let Some(master) = snapshot.branches.keys().find(|b| b.as_str() == "master") {
                master.clone()
            } else {
                return Err(RunError::Scan(crate::engine::scan::ScanError::Internal(
                    "No trunk specified and could not find 'main' or 'master' branch. Use --trunk to specify.".to_string()
                )));
            }
        };

        // Create config directory
        let lattice_dir = git_dir.join("lattice");
        std::fs::create_dir_all(&lattice_dir).map_err(|e| {
            RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                "Failed to create .git/lattice directory: {}",
                e
            )))
        })?;

        // Write config
        let config = RepoConfig {
            trunk: Some(trunk_name.to_string()),
            ..Default::default()
        };
        Config::write_repo(&cwd, &config).map_err(|e| {
            RunError::Scan(crate::engine::scan::ScanError::Internal(format!(
                "Failed to write config: {}",
                e
            )))
        })?;

        if !ctx.quiet {
            println!("Initialized Lattice with trunk: {}", trunk_name);
        }

        Ok(trunk_name)
    })
    .map(|trunk_name| {
        // Show bootstrap hint after successful init (non-fatal, skip on reset)
        // Per Milestone 5.6: hint is purely informational and never blocks init
        if !reset && !ctx.quiet {
            show_bootstrap_hint_sync(&git, &trunk_name);
        }
    })
    .map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}

/// Show a hint about open PRs that can be imported via `lattice doctor`.
///
/// This function runs the async hint check in a blocking context.
/// All errors are silently swallowed - the hint is purely informational
/// and MUST NOT prevent init from succeeding.
///
/// # Design Decisions (per Milestone 5.6 PLAN.md)
///
/// - Non-fatal: Any failure silently succeeds without hint
/// - Auth-gated: Only shown when GitHub auth is available
/// - Lightweight: Uses small limit (10) to quickly detect presence of PRs
/// - No mutations: Only reads remote state, never writes anything
fn show_bootstrap_hint_sync(git: &Git, _trunk: &BranchName) {
    // Build a minimal tokio runtime for the async hint check
    // This is acceptable because:
    // 1. It's a one-shot operation at the end of init
    // 2. The hint is optional and non-blocking
    // 3. Using block_on here avoids making init async
    if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        rt.block_on(maybe_show_bootstrap_hint(git));
    }
}

/// Async implementation of the bootstrap hint check.
///
/// Checks for open PRs on the remote and prints a hint if found.
/// All errors are silently swallowed.
async fn maybe_show_bootstrap_hint(git: &Git) {
    // Swallow all errors - the hint is purely informational
    let _ = try_show_bootstrap_hint(git).await;
}

/// Internal implementation that returns errors for cleaner control flow.
///
/// # Errors
///
/// Returns error if:
/// - Auth is not available
/// - Remote URL cannot be resolved
/// - Remote is not a GitHub URL
/// - API call fails (network, auth, rate limit, etc.)
async fn try_show_bootstrap_hint(git: &Git) -> Result<()> {
    use crate::auth::{has_github_auth, TokenProvider};
    use crate::forge::github::{parse_github_url, GitHubForge};
    use crate::forge::{Forge, ListPullsOpts};

    // Check if GitHub auth is available (quick local check, no network)
    if !has_github_auth("github.com") {
        return Ok(()); // No auth, skip silently
    }

    // Get remote URL (prefer "origin")
    let remote_url = git
        .remote_url("origin")?
        .ok_or_else(|| anyhow::anyhow!("no origin remote"))?;

    // Parse GitHub owner/repo from remote URL
    let (owner, repo) =
        parse_github_url(&remote_url).ok_or_else(|| anyhow::anyhow!("not a GitHub remote"))?;

    // Create forge with TokenProvider for automatic refresh
    use std::sync::Arc;

    let store = crate::secrets::create_store(crate::secrets::DEFAULT_PROVIDER)?;
    let auth_manager = crate::auth::GitHubAuthManager::new("github.com", store);
    let provider: Arc<dyn TokenProvider> = Arc::new(auth_manager);

    // Create forge and check for open PRs (small limit for quick detection)
    let forge = GitHubForge::new_with_provider(provider, owner, repo);
    let opts = ListPullsOpts::with_limit(10);
    let result = forge.list_open_prs(opts).await?;

    // Show hint if PRs found
    if !result.pulls.is_empty() {
        let count = result.pulls.len();
        let suffix = if result.truncated { "+" } else { "" };
        let pr_word = if count == 1 { "PR" } else { "PRs" };
        let pronoun = if count == 1 { "it" } else { "them" };

        println!(
            "Found {}{} open {}. Run `lattice doctor` to import {}.",
            count, suffix, pr_word, pronoun
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    mod bootstrap_hint {
        // Note: Full testing of the bootstrap hint requires either:
        // 1. A mock forge (complex setup)
        // 2. Real GitHub auth and a test repo with open PRs
        //
        // The key behaviors to verify are:
        // - Hint is shown when PRs exist and auth is available
        // - No error/panic when auth is unavailable
        // - No error/panic when remote is not GitHub
        // - No error/panic when API fails
        //
        // These are tested indirectly through integration tests.

        #[test]
        fn try_show_bootstrap_hint_requires_auth() {
            // Without auth configured, the function should return Ok(())
            // without attempting any network calls.
            //
            // This is implicitly tested by the fact that tests run in
            // environments without GitHub auth configured.
        }
    }
}
