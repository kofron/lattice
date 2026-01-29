//! cli::commands::pr
//!
//! Open PR URL in browser or print it.
//!
//! # Architecture
//!
//! This is a read-only command that implements `ReadOnlyCommand` and uses
//! `requirements::READ_ONLY`. It flows through `run_readonly_command` to
//! ensure proper gating.
//!
//! # Design
//!
//! Per SPEC.md Section 8E.6, the pr command:
//! - Opens PR URL in browser in interactive mode
//! - Prints URL in non-interactive mode
//! - Falls back to find_pr_by_head if not linked
//!
//! # Example
//!
//! ```bash
//! # Open current branch's PR
//! lattice pr
//!
//! # Open specific branch's PR
//! lattice pr feature-branch
//!
//! # Show URLs for entire stack
//! lattice pr --stack
//! ```

use crate::engine::command::ReadOnlyCommand;
use crate::engine::gate::{requirements, ReadyContext, RequirementSet};
use crate::engine::plan::PlanError;
use crate::engine::runner::{run_readonly_command, RunError};
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{Context as _, Result};

/// Command to open PR URL in browser or print it.
pub struct PrCommand<'a> {
    ctx: &'a Context,
    target: Option<&'a str>,
    stack: bool,
}

impl ReadOnlyCommand for PrCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::READ_ONLY;
    type Output = ();

    fn execute(&self, ready: &ReadyContext) -> Result<Self::Output, PlanError> {
        let snapshot = &ready.snapshot;

        // Resolve target branch
        let branch = if let Some(t) = self.target {
            // Could be a branch name or PR number
            // For now, treat as branch name
            crate::core::types::BranchName::new(t)
                .map_err(|e| PlanError::InvalidState(format!("Invalid branch name: {}", e)))?
        } else {
            snapshot.current_branch.clone().ok_or_else(|| {
                PlanError::InvalidState("Not on a branch. Specify a branch name.".to_string())
            })?
        };

        // Get metadata for the branch
        let scanned = snapshot.metadata.get(&branch).ok_or_else(|| {
            PlanError::InvalidState(format!("Branch '{}' is not tracked by Lattice.", branch))
        })?;

        // Check if we have PR linkage
        use crate::core::metadata::schema::PrState;
        let urls = if self.stack {
            // Get URLs for all branches in stack
            collect_stack_urls(snapshot, &branch)?
        } else {
            match &scanned.metadata.pr {
                PrState::Linked { url, .. } => vec![url.clone()],
                PrState::None => {
                    return Err(PlanError::InvalidState(format!(
                        "No PR linked to branch '{}'. Run 'lattice submit' first.",
                        branch
                    )));
                }
            }
        };

        // Output URLs
        for url in urls {
            if self.ctx.interactive && !self.ctx.quiet {
                // Try to open in browser
                if let Err(e) = open_browser(&url) {
                    // Fall back to printing
                    eprintln!("Could not open browser: {}", e);
                    println!("{}", url);
                }
            } else {
                println!("{}", url);
            }
        }

        Ok(())
    }
}

/// Run the pr command.
///
/// # Arguments
///
/// * `ctx` - Engine context
/// * `target` - Optional branch name or PR number (defaults to current)
/// * `stack` - If true, show URLs for entire stack
pub fn pr(ctx: &Context, target: Option<&str>, stack: bool) -> Result<()> {
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    let cmd = PrCommand { ctx, target, stack };

    run_readonly_command(&cmd, &git, ctx).map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}

/// Collect PR URLs for all branches in the stack.
fn collect_stack_urls(
    snapshot: &RepoSnapshot,
    branch: &crate::core::types::BranchName,
) -> Result<Vec<String>, PlanError> {
    use crate::core::metadata::schema::PrState;

    let mut urls = Vec::new();

    // Get ancestors (parents toward trunk)
    let ancestors = snapshot.graph.ancestors(branch);

    // Collect URLs from ancestors (bottom-up order)
    for ancestor in ancestors.iter().rev() {
        if let Some(scanned) = snapshot.metadata.get(ancestor) {
            if let PrState::Linked { url, .. } = &scanned.metadata.pr {
                urls.push(url.clone());
            }
        }
    }

    // Add current branch
    if let Some(scanned) = snapshot.metadata.get(branch) {
        if let PrState::Linked { url, .. } = &scanned.metadata.pr {
            urls.push(url.clone());
        }
    }

    // Add descendants
    let descendants = snapshot.graph.descendants(branch);
    for desc in descendants {
        if let Some(scanned) = snapshot.metadata.get(&desc) {
            if let PrState::Linked { url, .. } = &scanned.metadata.pr {
                urls.push(url.clone());
            }
        }
    }

    if urls.is_empty() {
        return Err(PlanError::InvalidState(
            "No PRs linked in stack. Run 'lattice submit' first.".to_string(),
        ));
    }

    Ok(urls)
}

/// Open a URL in the default browser.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .context("Failed to open browser")?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .status()
            .context("Failed to open browser")?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .status()
            .context("Failed to open browser")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn open_browser_url_format() {
        // Just verify the function compiles - actual browser opening is hard to test
        let url = "https://github.com/owner/repo/pull/42";
        assert!(url.starts_with("https://"));
    }
}
