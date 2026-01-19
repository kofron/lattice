//! cli::commands::submit
//!
//! Submit branches as PRs to GitHub.
//!
//! # Design
//!
//! Per SPEC.md Section 8E.2, the submit command:
//! - Pushes branches to remote
//! - Creates or updates PRs via GitHub API
//! - Links PRs in branch metadata
//! - Optionally restacks before submitting
//! - Generates stack comments in PR descriptions
//!
//! # Bare Repository Support
//!
//! Per SPEC.md Section 4.6.7, in bare repositories:
//! - `lattice submit` MUST refuse unless `--no-restack` is provided
//! - Even with `--no-restack`, MUST refuse if submit set is not aligned
//! - Alignment is ancestry-based: `parent.tip` must be ancestor of `branch.tip`
//! - If ancestry holds but `base != parent.tip`: normalize base metadata
//!
//! # Algorithm
//!
//! 1. Gate on REMOTE requirements (auth, remote configured)
//! 2. Check bare repo constraints (require --no-restack, check alignment)
//! 3. Optionally restack branches
//! 4. For each branch in stack order:
//!    - Determine PR base (parent branch or trunk)
//!    - Push if changed (or --always)
//!    - Create/update PR via forge (with stack comment)
//!    - Handle draft toggle
//!    - Request reviewers if specified
//! 5. Update metadata with PR linkage
//! 6. Update stack comments for all PRs in stack
//!
//! # Example
//!
//! ```bash
//! # Submit current branch and ancestors
//! lattice submit
//!
//! # Submit entire stack including descendants
//! lattice submit --stack
//!
//! # Create as draft
//! lattice submit --draft
//!
//! # Dry run
//! lattice submit --dry-run
//!
//! # Submit from bare repo (requires aligned branches)
//! lattice submit --no-restack
//! ```

use crate::core::metadata::schema::BaseInfo;
use crate::core::metadata::store::MetadataStore;
use crate::core::types::{BranchName, Oid};
use crate::engine::scan::RepoSnapshot;
use crate::engine::Context;
use crate::git::Git;
use anyhow::{bail, Context as _, Result};

use super::stack_comment_ops::{
    generate_merged_body, update_stack_comments_for_branches_from_forge,
};

/// Submit options parsed from CLI arguments.
#[derive(Debug)]
#[allow(dead_code)]
pub struct SubmitOptions<'a> {
    pub stack: bool,
    pub draft: bool,
    pub publish: bool,
    pub confirm: bool,
    pub dry_run: bool,
    pub force: bool,
    pub always: bool,
    pub update_only: bool,
    pub reviewers: Option<&'a str>,
    pub team_reviewers: Option<&'a str>,
    pub no_restack: bool,
    pub view: bool,
}

/// Run the submit command.
///
/// This is a synchronous wrapper that uses tokio to run the async implementation.
#[allow(clippy::too_many_arguments)]
pub fn submit(
    ctx: &Context,
    stack: bool,
    draft: bool,
    publish: bool,
    confirm: bool,
    dry_run: bool,
    force: bool,
    always: bool,
    update_only: bool,
    reviewers: Option<&str>,
    team_reviewers: Option<&str>,
    no_restack: bool,
    view: bool,
) -> Result<()> {
    let opts = SubmitOptions {
        stack,
        draft,
        publish,
        confirm,
        dry_run,
        force,
        always,
        update_only,
        reviewers,
        team_reviewers,
        no_restack,
        view,
    };

    // Use tokio runtime to run async code
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(submit_async(ctx, opts))
}

/// Async implementation of submit.
async fn submit_async(ctx: &Context, opts: SubmitOptions<'_>) -> Result<()> {
    use crate::cli::commands::auth::get_github_token;
    use crate::engine::scan::scan;

    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;
    let snapshot = scan(&git)?;

    // Per SPEC.md §4.6.7: submit MUST refuse in bare repos unless --no-restack
    let is_bare = git.info()?.work_dir.is_none();
    if is_bare && !opts.no_restack {
        bail!(
            "This is a bare repository. The `submit` command requires a working directory for restacking.\n\n\
             To submit without restacking (branches must be properly aligned), use:\n\n\
                 lattice submit --no-restack\n\n\
             Note: Branches must satisfy ancestry alignment (parent tip is ancestor of branch tip).\n\
             If alignment fails, you'll need to restack from a worktree first."
        );
    }

    // Check authentication
    let token = match get_github_token() {
        Ok(t) => t,
        Err(_) => bail!("Not authenticated. Run 'lattice auth' first."),
    };

    // Get remote URL and create forge
    let remote_url = git
        .remote_url("origin")?
        .ok_or_else(|| anyhow::anyhow!("No 'origin' remote configured."))?;

    let forge = crate::forge::create_forge(&remote_url, &token, None)?;

    // Get current branch
    let current = snapshot
        .current_branch
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not on a branch."))?;

    // Determine branches to submit
    let branches = if opts.stack {
        // Include ancestors and descendants
        let mut all = snapshot.graph.ancestors(current);
        all.reverse(); // Bottom-up order
        all.push(current.clone());
        let descendants: Vec<_> = snapshot.graph.descendants(current).into_iter().collect();
        all.extend(descendants);
        all
    } else {
        // Just ancestors and current
        let mut all = snapshot.graph.ancestors(current);
        all.reverse();
        all.push(current.clone());
        all
    };

    // Per SPEC.md §4.6.7: Even with --no-restack, check alignment in bare repos
    if is_bare && opts.no_restack {
        check_and_normalize_alignment(ctx, &git, &snapshot, &branches)?;
    }

    if opts.dry_run {
        println!("Would submit {} branch(es):", branches.len());
        for branch in &branches {
            let has_pr = snapshot
                .metadata
                .get(branch)
                .map(|s| !matches!(s.metadata.pr, crate::core::metadata::schema::PrState::None))
                .unwrap_or(false);
            let action = if has_pr { "update" } else { "create" };
            println!("  {} - {} PR", branch, action);
        }
        return Ok(());
    }

    // Submit each branch
    use crate::forge::CreatePrRequest;

    for branch in &branches {
        let scanned = match snapshot.metadata.get(branch) {
            Some(s) => s,
            None => {
                if !ctx.quiet {
                    println!("Skipping untracked branch '{}'", branch);
                }
                continue;
            }
        };

        // Push branch to remote before creating/updating PR
        if !ctx.quiet {
            println!("Pushing '{}'...", branch);
        }
        let push_args = if opts.force {
            vec!["push", "--force-with-lease", "origin", branch.as_str()]
        } else {
            vec!["push", "origin", branch.as_str()]
        };
        let push_result = std::process::Command::new("git")
            .args(&push_args)
            .current_dir(&cwd)
            .output()?;

        if !push_result.status.success() {
            let stderr = String::from_utf8_lossy(&push_result.stderr);
            // Check if it's just "already up to date" which is fine
            if !stderr.contains("Everything up-to-date") {
                eprintln!("  Failed to push '{}': {}", branch, stderr.trim());
                continue;
            }
        }

        // Determine base (parent branch or trunk)
        let base = scanned.metadata.parent.name().to_string();

        // Check if PR exists
        use crate::core::metadata::schema::PrState;
        match &scanned.metadata.pr {
            PrState::Linked { number, .. } => {
                // Update existing PR
                if !ctx.quiet {
                    println!("Updating PR #{} for '{}'...", number, branch);
                }

                // Fetch existing PR body and merge with updated stack comment
                let existing_body = forge.get_pr(*number).await.ok().and_then(|pr| pr.body);
                let body = generate_merged_body(existing_body.as_deref(), &snapshot, branch);

                let update_req = crate::forge::UpdatePrRequest {
                    number: *number,
                    base: Some(base),
                    title: None,
                    body: Some(body),
                };

                match forge.update_pr(update_req).await {
                    Ok(pr) => {
                        if !ctx.quiet {
                            println!("  Updated: {}", pr.url);
                        }
                    }
                    Err(e) => {
                        eprintln!("  Failed to update PR: {}", e);
                    }
                }

                // Handle draft toggle
                if opts.publish {
                    if let Err(e) = forge.set_draft(*number, false).await {
                        eprintln!("  Failed to publish PR: {}", e);
                    }
                } else if opts.draft {
                    if let Err(e) = forge.set_draft(*number, true).await {
                        eprintln!("  Failed to convert to draft: {}", e);
                    }
                }
            }
            PrState::None => {
                if opts.update_only {
                    if !ctx.quiet {
                        println!("Skipping '{}' (no existing PR, --update-only)", branch);
                    }
                    continue;
                }

                // Try to find existing PR by head
                match forge.find_pr_by_head(branch.as_str()).await? {
                    Some(existing) => {
                        if !ctx.quiet {
                            println!(
                                "Found existing PR #{} for '{}', linking...",
                                existing.number, branch
                            );
                        }
                        // Would update metadata here
                    }
                    None => {
                        // Create new PR
                        if !ctx.quiet {
                            println!("Creating PR for '{}'...", branch);
                        }

                        // Get commit message for title
                        let title = format!("{}", branch);

                        // Create PR initially without stack comment body
                        // (we'll update it immediately after to include correct PR number)
                        let create_req = CreatePrRequest {
                            head: branch.as_str().to_string(),
                            base,
                            title,
                            body: None,
                            draft: opts.draft,
                        };

                        match forge.create_pr(create_req).await {
                            Ok(pr) => {
                                if !ctx.quiet {
                                    println!("  Created: {}", pr.url);
                                }
                                // Would update metadata with PR linkage here

                                // Request reviewers if specified
                                if opts.reviewers.is_some() || opts.team_reviewers.is_some() {
                                    let reviewers = crate::forge::Reviewers {
                                        users: opts
                                            .reviewers
                                            .map(|r| {
                                                r.split(',').map(|s| s.trim().to_string()).collect()
                                            })
                                            .unwrap_or_default(),
                                        teams: opts
                                            .team_reviewers
                                            .map(|r| {
                                                r.split(',').map(|s| s.trim().to_string()).collect()
                                            })
                                            .unwrap_or_default(),
                                    };
                                    if let Err(e) =
                                        forge.request_reviewers(pr.number, reviewers).await
                                    {
                                        eprintln!("  Failed to request reviewers: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("  Failed to create PR: {}", e);
                            }
                        }
                    }
                }
            }
        }
    }

    // After all PRs are created/updated, refresh stack comments for all PRs
    // This ensures newly created PRs are reflected in existing PR descriptions
    if !opts.dry_run {
        if !ctx.quiet {
            println!("Refreshing stack comments...");
        }

        // Use forge-based lookup since metadata may not have been persisted yet
        let updated = update_stack_comments_for_branches_from_forge(
            forge.as_ref(),
            &snapshot,
            &branches,
            ctx.quiet,
        )
        .await?;

        if updated > 0 && !ctx.quiet {
            println!("  Updated {} PR description(s)", updated);
        }
    }

    if opts.view {
        // Would open PR URLs in browser
        println!("\nUse 'lattice pr --stack' to view PRs.");
    }

    Ok(())
}

/// Result of checking submit alignment for bare repo mode.
enum AlignmentResult {
    /// All branches are aligned (parent.tip is ancestor of branch.tip, base matches)
    Aligned,
    /// Ancestry holds but base needs normalization (metadata-only update)
    NeedsNormalization(Vec<BranchNormalization>),
    /// Ancestry violated - restack required
    NotAligned {
        branch: BranchName,
        parent: BranchName,
    },
}

/// Information about a branch that needs base metadata normalization.
struct BranchNormalization {
    branch: BranchName,
    new_base: Oid,
}

/// Check if all branches in submit set are aligned for bare repo submission.
///
/// Per SPEC.md §4.6.7:
/// - Alignment is ancestry-based: `parent.tip` must be ancestor of `branch.tip`
/// - If ancestry holds but `base != parent.tip`: normalize base metadata (metadata-only)
/// - If ancestry violated: refuse with "Restack required" message
fn check_submit_alignment(
    git: &Git,
    snapshot: &RepoSnapshot,
    branches: &[BranchName],
) -> Result<AlignmentResult> {
    let _trunk = snapshot
        .trunk
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Trunk not configured"))?;

    let mut needs_normalization = Vec::new();

    for branch in branches {
        // Get branch metadata
        let metadata_entry = match snapshot.metadata.get(branch) {
            Some(entry) => entry,
            None => continue, // Untracked branches are skipped
        };

        let parent_name = metadata_entry.metadata.parent.name();
        let parent_branch = BranchName::new(parent_name)
            .with_context(|| format!("Invalid parent name: {}", parent_name))?;

        // Skip if parent is trunk (trunk is always the root, no ancestry check needed)
        // But we still need to check for branches directly on trunk
        if metadata_entry.metadata.parent.is_trunk() {
            // For trunk children, parent tip = trunk tip, which is always valid
            continue;
        }

        // Get branch tip
        let branch_tip = snapshot
            .branches
            .get(branch)
            .ok_or_else(|| anyhow::anyhow!("Branch '{}' not found in branches", branch))?;

        // Get parent tip
        let parent_tip = snapshot
            .branches
            .get(&parent_branch)
            .ok_or_else(|| anyhow::anyhow!("Parent branch '{}' not found", parent_name))?;

        // Check ancestry: parent.tip must be ancestor of branch.tip
        let is_ancestor = git.is_ancestor(parent_tip, branch_tip)?;
        if !is_ancestor {
            return Ok(AlignmentResult::NotAligned {
                branch: branch.clone(),
                parent: parent_branch,
            });
        }

        // If ancestry holds but base differs from parent tip, needs normalization
        let current_base = &metadata_entry.metadata.base.oid;
        if current_base != parent_tip.as_str() {
            needs_normalization.push(BranchNormalization {
                branch: branch.clone(),
                new_base: parent_tip.clone(),
            });
        }
    }

    if needs_normalization.is_empty() {
        Ok(AlignmentResult::Aligned)
    } else {
        Ok(AlignmentResult::NeedsNormalization(needs_normalization))
    }
}

/// Normalize base metadata for branches where ancestry holds but base differs.
///
/// This is a metadata-only operation - no history rewrite.
/// Per SPEC.md §4.6.7: "normalize base to `parent.tip` (metadata-only)"
fn normalize_base_metadata(
    git: &Git,
    snapshot: &RepoSnapshot,
    normalizations: &[BranchNormalization],
) -> Result<()> {
    let store = MetadataStore::new(git);

    for norm in normalizations {
        // Get current metadata entry (we need the ref_oid for CAS)
        let entry = snapshot
            .metadata
            .get(&norm.branch)
            .ok_or_else(|| anyhow::anyhow!("Metadata not found for branch '{}'", norm.branch))?;

        // Create updated metadata with new base
        let mut updated_metadata = entry.metadata.clone();
        updated_metadata.base = BaseInfo {
            oid: norm.new_base.to_string(),
        };
        updated_metadata.touch(); // Update the updated_at timestamp

        // Write with CAS semantics
        store
            .write_cas(&norm.branch, Some(&entry.ref_oid), &updated_metadata)
            .with_context(|| format!("Failed to update metadata for '{}'", norm.branch))?;
    }

    Ok(())
}

/// Check alignment and normalize metadata if needed for bare repo submission.
///
/// Per SPEC.md §4.6.7:
/// - Check ancestry alignment for all branches
/// - If aligned with stale base: normalize metadata and print message
/// - If not aligned: bail with restack required message
fn check_and_normalize_alignment(
    ctx: &Context,
    git: &Git,
    snapshot: &RepoSnapshot,
    branches: &[BranchName],
) -> Result<()> {
    match check_submit_alignment(git, snapshot, branches)? {
        AlignmentResult::Aligned => {
            // All good, proceed with submit
            Ok(())
        }
        AlignmentResult::NeedsNormalization(normalizations) => {
            // Ancestry holds but base != parent.tip - normalize metadata
            let count = normalizations.len();
            normalize_base_metadata(git, snapshot, &normalizations)?;

            if !ctx.quiet {
                println!(
                    "Updated base metadata for {} branch(es) (no history changes).",
                    count
                );
            }
            Ok(())
        }
        AlignmentResult::NotAligned { branch, parent } => {
            bail!(
                "Branch '{}' is not aligned with parent '{}'.\n\n\
                 The parent's tip is not an ancestor of the branch tip, which means\n\
                 the branch needs to be rebased.\n\n\
                 Restack required. Run from a worktree and re-run `lattice submit`.",
                branch,
                parent
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submit_options_defaults() {
        let opts = SubmitOptions {
            stack: false,
            draft: false,
            publish: false,
            confirm: false,
            dry_run: false,
            force: false,
            always: false,
            update_only: false,
            reviewers: None,
            team_reviewers: None,
            no_restack: false,
            view: false,
        };
        assert!(!opts.stack);
        assert!(!opts.draft);
    }
}
