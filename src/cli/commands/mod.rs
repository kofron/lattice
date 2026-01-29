//! cli::commands
//!
//! Command dispatch and handlers.
//!
//! # Architecture
//!
//! Each command handler:
//! 1. Validates command-specific arguments
//! 2. Calls the engine to execute the command
//! 3. Formats and displays output
//!
//! Handlers do NOT perform repository mutations directly.
//!
//! # Async Commands
//!
//! GitHub integration commands (submit, sync, get, merge) are async
//! because they involve network I/O. The dispatch function uses
//! `tokio::runtime::Handle` to run async commands within the sync context.

mod auth;
mod changelog;
mod checkout;
mod completion;
mod config_cmd;
mod create;
mod delete;
mod fold;
mod freeze;
mod get;
mod info;
mod init;
mod log_cmd;
mod merge;
mod modify;
mod move_cmd;
mod navigation;
mod phase3_helpers;
mod pop;
mod pr;
mod recovery;
mod relationships;
mod rename;
mod reorder;
mod restack;
mod revert;
mod split;
mod squash;
pub mod stack_comment_ops;
mod submit;
mod sync;
mod track;
mod trunk;
mod undo;
mod unlink;
mod untrack;

// Re-export command functions for testing and direct invocation
pub use auth::{auth, get_github_token, has_github_token};
pub use changelog::changelog;
pub use checkout::checkout;
pub use completion::completion;
pub use config_cmd::{get as config_get, list as config_list, set as config_set};
pub use create::create;
pub use delete::delete;
pub use fold::fold;
pub use freeze::{freeze, unfreeze};
pub use get::get;
pub use info::info;
pub use init::init;
pub use log_cmd::log;
pub use merge::merge;
pub use modify::modify;
pub use move_cmd::move_branch;
pub use navigation::{bottom, down, top, up};
pub use pop::pop;
pub use pr::pr;
pub use recovery::{abort, continue_op};
pub use relationships::{children, parent};
pub use rename::rename;
pub use reorder::reorder;
pub use restack::restack;
pub use revert::revert;
pub use split::split;
pub use squash::squash;
pub use submit::submit;
pub use sync::sync;
pub use track::track;
pub use trunk::trunk;
pub use undo::undo;
pub use unlink::unlink;
pub use untrack::untrack;

use crate::cli::args::{Command, ConfigAction};
use crate::doctor::{Doctor, FixId};
use crate::engine::exec::{ExecuteResult, Executor};
use crate::engine::ledger::{Event, EventLedger};
use crate::engine::Context;
use crate::git::Git;
use anyhow::Result;

/// Dispatch a command to its handler.
pub fn dispatch(command: Command, ctx: &Context) -> Result<()> {
    match command {
        Command::Doctor {
            fix_ids,
            dry_run,
            list,
            deep_remote,
        } => doctor(ctx, &fix_ids, dry_run, list, deep_remote),

        // Phase A: Read-Only Commands
        Command::Log {
            short,
            long,
            stack,
            all,
            reverse,
        } => log_cmd::log(ctx, short, long, stack, all, reverse),
        Command::Info {
            branch,
            diff,
            stat,
            patch,
        } => info::info(ctx, branch.as_deref(), diff, stat, patch),
        Command::Parent => relationships::parent(ctx),
        Command::Children => relationships::children(ctx),
        Command::Trunk { set } => trunk::trunk(ctx, set.as_deref()),

        // Phase B: Setup Commands
        Command::Auth {
            no_browser,
            host,
            status,
            logout,
        } => auth::auth(ctx, &host, no_browser, status, logout),
        Command::Init {
            trunk,
            reset,
            force,
        } => init::init(ctx, trunk.as_deref(), reset, force),
        Command::Config { action } => match action {
            ConfigAction::Get { key } => config_cmd::get(ctx, &key),
            ConfigAction::Set { key, value } => config_cmd::set(ctx, &key, &value),
            ConfigAction::List => config_cmd::list(ctx),
        },
        Command::Completion { shell } => completion::completion(shell),
        Command::Changelog => changelog::changelog(),

        // Phase C: Tracking Commands
        Command::Track {
            branch,
            parent,
            force,
            as_frozen,
        } => track::track(ctx, branch.as_deref(), parent.as_deref(), force, as_frozen),
        Command::Untrack { branch, force } => untrack::untrack(ctx, branch.as_deref(), force),
        Command::Freeze { branch, only } => freeze::freeze(ctx, branch.as_deref(), only),
        Command::Unfreeze { branch, only } => freeze::unfreeze(ctx, branch.as_deref(), only),

        // Phase D: Navigation Commands
        Command::Checkout {
            branch,
            trunk,
            stack,
        } => checkout::checkout(ctx, branch.as_deref(), trunk, stack),
        Command::Up { steps } => navigation::up(ctx, steps),
        Command::Down { steps } => navigation::down(ctx, steps),
        Command::Top => navigation::top(ctx),
        Command::Bottom => navigation::bottom(ctx),

        // Phase E: Core Mutating Commands
        Command::Restack {
            branch,
            only,
            downstack,
        } => restack::restack(ctx, branch.as_deref(), only, downstack),
        Command::Continue { all } => recovery::continue_op(ctx, all),
        Command::Abort => recovery::abort(ctx),
        Command::Undo => undo::undo(ctx),
        Command::Create {
            name,
            message,
            all,
            update,
            patch,
            insert,
        } => create::create(
            ctx,
            name.as_deref(),
            message.as_deref(),
            all,
            update,
            patch,
            insert,
        ),

        // Phase 3: Advanced Rewriting Commands
        Command::Modify {
            create,
            all,
            update,
            patch,
            message,
            edit,
        } => modify::modify(ctx, create, all, update, patch, message.as_deref(), edit),
        Command::Move { onto, source } => move_cmd::move_branch(ctx, &onto, source.as_deref()),
        Command::Rename { name } => rename::rename(ctx, &name),
        Command::Delete {
            branch,
            upstack,
            downstack,
            force,
        } => delete::delete(ctx, branch.as_deref(), upstack, downstack, force),
        Command::Squash { message, edit } => squash::squash(ctx, message.as_deref(), edit),
        Command::Fold { keep } => fold::fold(ctx, keep),
        Command::Pop => pop::pop(ctx),
        Command::Reorder => reorder::reorder(ctx),
        Command::Split { by_commit, by_file } => split::split(ctx, by_commit, by_file),
        Command::Revert { sha } => revert::revert(ctx, &sha),

        // Phase F: GitHub Integration Commands
        Command::Submit {
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
        } => submit::submit(
            ctx,
            stack,
            draft,
            publish,
            confirm,
            dry_run,
            force,
            always,
            update_only,
            reviewers.as_deref(),
            team_reviewers.as_deref(),
            no_restack,
            view,
        ),
        Command::Sync {
            force,
            restack,
            no_restack,
        } => sync::sync(ctx, force, restack && !no_restack),
        Command::Get {
            target,
            downstack,
            force,
            restack,
            no_restack,
            unfrozen,
            no_checkout,
        } => get::get(
            ctx,
            &target,
            downstack,
            force,
            restack && !no_restack,
            unfrozen,
            no_checkout,
        ),
        Command::Merge {
            confirm,
            dry_run,
            method,
        } => merge::merge(ctx, confirm, dry_run, method),
        Command::Pr { target, stack } => pr::pr(ctx, target.as_deref(), stack),
        Command::Unlink { branch } => unlink::unlink(ctx, branch.as_deref()),
    }
}

/// Surface divergence information in debug/verbose output.
///
/// Per ARCHITECTURE.md Section 7.2, divergence is not an error but evidence
/// that the repository was modified outside Lattice. This helper surfaces
/// that information when debug mode is enabled.
pub fn surface_divergence_if_debug(ctx: &Context, health: &crate::engine::RepoHealthReport) {
    if ctx.debug {
        if let Some(divergence) = health.divergence() {
            eprintln!(
                "Note: Repository state has changed since last Lattice operation.\n\
                 Prior fingerprint: {}\n\
                 Current fingerprint: {}",
                &divergence.prior_fingerprint[..12.min(divergence.prior_fingerprint.len())],
                &divergence.current_fingerprint[..12.min(divergence.current_fingerprint.len())]
            );
            if !divergence.changed_refs.is_empty() {
                eprintln!("Changed refs:");
                for ref_name in &divergence.changed_refs {
                    eprintln!("  - {}", ref_name);
                }
            }
        }
    }
}

/// Perform Tier 2 deep analysis for synthetic stack heads.
///
/// Queries the forge for closed PRs that targeted each potential synthetic
/// stack head branch, adding evidence to the diagnosis.
fn perform_deep_synthetic_analysis(
    ctx: &Context,
    git: &Git,
    diagnosis: &mut crate::doctor::DiagnosisReport,
) -> Result<()> {
    use crate::doctor::analyze_synthetic_stack_deep;

    // Load config to get budget settings
    let config = crate::core::config::Config::load(ctx.cwd.as_deref()).ok();
    let bootstrap_config = config
        .as_ref()
        .and_then(|c| c.config.global.doctor.as_ref())
        .map(|d| d.bootstrap.clone())
        .unwrap_or_default();

    // Get forge if available
    let forge = match create_forge_for_deep_analysis(git) {
        Some(f) => f,
        None => {
            if ctx.debug {
                eprintln!("Note: --deep-remote requested but forge not available");
            }
            return Ok(());
        }
    };

    // Find synthetic-stack-head issues
    let synthetic_head_issues: Vec<_> = diagnosis
        .issues
        .iter()
        .filter(|i| i.id.as_str().starts_with("synthetic-stack-head:"))
        .cloned()
        .collect();

    if synthetic_head_issues.is_empty() {
        return Ok(());
    }

    // Enforce budget: max_synthetic_heads
    let issues_to_analyze = synthetic_head_issues
        .iter()
        .take(bootstrap_config.max_synthetic_heads);
    let skipped = synthetic_head_issues
        .len()
        .saturating_sub(bootstrap_config.max_synthetic_heads);

    if skipped > 0 && !ctx.quiet {
        println!(
            "Note: Analyzing {} of {} potential synthetic heads (budget: {})",
            bootstrap_config.max_synthetic_heads,
            synthetic_head_issues.len(),
            bootstrap_config.max_synthetic_heads
        );
    }

    // Create runtime for async calls
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to create runtime: {}", e))?;

    // Analyze each synthetic head
    for issue in issues_to_analyze {
        if let Some(evidence) = rt.block_on(analyze_synthetic_stack_deep(
            issue,
            forge.as_ref(),
            &bootstrap_config,
        )) {
            // Find the issue in diagnosis and add evidence
            if let Some(diag_issue) = diagnosis.issues.iter_mut().find(|i| i.id == issue.id) {
                diag_issue.evidence.push(evidence);
            }
        }
    }

    Ok(())
}

/// Create a forge for deep synthetic analysis.
///
/// Returns None if forge cannot be created (no auth, no remote, etc.)
fn create_forge_for_deep_analysis(git: &Git) -> Option<Box<dyn crate::forge::Forge>> {
    use std::sync::Arc;

    use crate::auth::TokenProvider;
    use crate::forge::github::GitHubForge;

    // Get remote URL
    let remote_url = git.remote_url("origin").ok()??;

    // Parse GitHub URL
    let (owner, repo) = crate::forge::github::parse_github_url(&remote_url)?;

    // Get token
    if !has_github_token() {
        return None;
    }

    // Create auth manager with TokenProvider for automatic refresh
    let store = crate::secrets::create_store(crate::secrets::DEFAULT_PROVIDER).ok()?;
    let auth_manager = crate::auth::GitHubAuthManager::new("github.com", store);
    let provider: Arc<dyn TokenProvider> = Arc::new(auth_manager);

    Some(Box::new(GitHubForge::new_with_provider(
        provider, owner, repo,
    )))
}

/// Doctor command - diagnose and repair repository issues.
///
/// Per ARCHITECTURE.md Section 8.3, doctor never applies fixes without
/// explicit confirmation:
/// - Interactive: user selects from presented options
/// - Non-interactive: user provides explicit `--fix` IDs
fn doctor(
    ctx: &Context,
    fix_ids: &[String],
    dry_run: bool,
    list: bool,
    deep_remote: bool,
) -> Result<()> {
    // Initialize git interface
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;

    // Scan the repository (with remote if capabilities allow)
    // Use blocking runtime to call async scan_with_remote
    let snapshot = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create runtime: {}", e))?;
        rt.block_on(crate::engine::scan::scan_with_remote(&git))?
    };

    // Surface divergence info if debug mode (per ARCHITECTURE.md 7.2)
    surface_divergence_if_debug(ctx, &snapshot.health);

    // Create doctor and diagnose
    let doctor = Doctor::new().interactive(!ctx.quiet && fix_ids.is_empty());
    let mut diagnosis = doctor.diagnose(&snapshot);

    // Tier 2: Deep synthetic stack analysis (if --deep-remote enabled)
    if deep_remote {
        perform_deep_synthetic_analysis(ctx, &git, &mut diagnosis)?;
    }

    // If --list, output machine-readable format
    if list {
        println!("# Issues");
        for issue in &diagnosis.issues {
            println!("issue:{}\t{}\t{}", issue.id, issue.severity, issue.message);
        }
        println!("\n# Fixes");
        for fix in &diagnosis.fixes {
            println!("fix:{}\t{}\t{}", fix.id, fix.issue_id, fix.description);
        }
        return Ok(());
    }

    // If no issues, report healthy
    if diagnosis.is_healthy() {
        if !ctx.quiet {
            println!("Repository is healthy - no issues found.");
        }
        return Ok(());
    }

    // If no fixes requested, just show diagnosis
    if fix_ids.is_empty() {
        println!("{}", diagnosis.format());

        // Record DoctorProposed event when fixes are available (per ARCHITECTURE.md 3.4.2)
        if !diagnosis.fixes.is_empty() {
            let issue_ids: Vec<String> =
                diagnosis.issues.iter().map(|i| i.id.to_string()).collect();
            let available_fix_ids: Vec<String> =
                diagnosis.fixes.iter().map(|f| f.id.to_string()).collect();

            let ledger = EventLedger::new(&git);
            let event = Event::doctor_proposed(issue_ids, available_fix_ids);
            if let Err(e) = ledger.append(event) {
                if ctx.debug {
                    eprintln!("Warning: failed to record DoctorProposed event: {}", e);
                }
            }
        }

        return Ok(());
    }

    // Parse fix IDs
    let parsed_fix_ids: Vec<FixId> = fix_ids.iter().map(|s| FixId::parse(s)).collect();

    // If --dry-run, show preview
    if dry_run {
        let preview = doctor.preview_fixes(&parsed_fix_ids, &diagnosis)?;
        println!("{}", preview);
        return Ok(());
    }

    // Generate repair plan
    let plan = doctor.plan_repairs(&parsed_fix_ids, &diagnosis, &snapshot)?;

    if plan.is_empty() {
        println!("No changes needed.");
        return Ok(());
    }

    // Show plan and confirm
    if !ctx.quiet {
        println!("Repair plan:");
        println!("{}", plan.preview());
        println!();
    }

    // Execute the plan through the standard executor (per ARCHITECTURE.md 8.1)
    // Doctor uses the same executor as other commands - no separate repair path.
    let executor = Executor::new(&git);
    let result = executor.execute(&plan, ctx)?;

    match result {
        ExecuteResult::Success { fingerprint } => {
            // Record DoctorApplied event (per ARCHITECTURE.md 8.4)
            let fix_id_strings: Vec<String> =
                parsed_fix_ids.iter().map(|f| f.to_string()).collect();
            let ledger = EventLedger::new(&git);
            let event = Event::doctor_applied(fix_id_strings.clone(), fingerprint.to_string());
            if let Err(e) = ledger.append(event) {
                // Event recording failure is non-fatal but should be reported
                eprintln!("Warning: failed to record DoctorApplied event: {}", e);
            }

            if !ctx.quiet {
                println!("Successfully applied {} fix(es).", parsed_fix_ids.len());
            }

            // Post-verify: re-run diagnosis to confirm issues are resolved
            if !ctx.quiet {
                let new_snapshot = crate::engine::scan::scan(&git)?;
                let new_diagnosis = doctor.diagnose(&new_snapshot);

                // Check if the fixed issues are now resolved
                let fixed_issue_ids: std::collections::HashSet<_> = parsed_fix_ids
                    .iter()
                    .filter_map(|f| diagnosis.fixes.iter().find(|fix| &fix.id == f))
                    .map(|fix| &fix.issue_id)
                    .collect();

                let remaining: Vec<_> = new_diagnosis
                    .issues
                    .iter()
                    .filter(|i| fixed_issue_ids.contains(&i.id))
                    .collect();

                if remaining.is_empty() {
                    println!("All targeted issues resolved.");
                } else {
                    println!(
                        "Warning: {} issue(s) may not be fully resolved. Run 'lattice doctor' to check.",
                        remaining.len()
                    );
                }
            }
        }
        ExecuteResult::Paused {
            branch, git_state, ..
        } => {
            // Conflict during repair - transition to awaiting_user op-state
            // The executor already handles op-state transition
            println!(
                "Repair paused: conflict on branch '{}' ({:?}).",
                branch, git_state
            );
            println!("Resolve conflicts and run 'lattice continue', or 'lattice abort' to cancel.");
        }
        ExecuteResult::Aborted {
            error,
            applied_steps,
        } => {
            // Repair failed - some steps may have been applied
            eprintln!("Repair aborted: {}", error);
            if !applied_steps.is_empty() {
                eprintln!(
                    "Warning: {} step(s) were applied before failure.",
                    applied_steps.len()
                );
                eprintln!("Run 'lattice doctor' to check repository state.");
            }
            return Err(anyhow::anyhow!("Repair failed: {}", error));
        }
    }

    Ok(())
}
