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
use crate::engine::{self, Context};
use crate::git::Git;
use anyhow::Result;

/// Dispatch a command to its handler.
pub fn dispatch(command: Command, ctx: &Context) -> Result<()> {
    match command {
        Command::Hello => hello(ctx),
        Command::Doctor {
            fix_ids,
            dry_run,
            list,
        } => doctor(ctx, &fix_ids, dry_run, list),

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
            token,
            host,
            status,
            logout,
        } => auth::auth(ctx, token.as_deref(), &host, status, logout),
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
        } => get::get(
            ctx,
            &target,
            downstack,
            force,
            restack && !no_restack,
            unfrozen,
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

/// Bootstrap validation command.
///
/// This command exists only to validate that the engine lifecycle works.
/// It runs through Scan → Gate → Plan → Execute → Verify with the full engine lifecycle.
fn hello(ctx: &Context) -> Result<()> {
    // Run through the engine lifecycle
    engine::execute_hello(ctx)
}

/// Doctor command - diagnose and repair repository issues.
///
/// Per ARCHITECTURE.md Section 8.3, doctor never applies fixes without
/// explicit confirmation:
/// - Interactive: user selects from presented options
/// - Non-interactive: user provides explicit `--fix` IDs
fn doctor(ctx: &Context, fix_ids: &[String], dry_run: bool, list: bool) -> Result<()> {
    // Initialize git interface
    let cwd = ctx
        .cwd
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd)?;

    // Scan the repository
    let snapshot = crate::engine::scan::scan(&git)?;

    // Create doctor and diagnose
    let doctor = Doctor::new().interactive(!ctx.quiet && fix_ids.is_empty());
    let diagnosis = doctor.diagnose(&snapshot);

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

    // Execute the plan
    // Note: For now, we just show the plan. Actual execution would use the executor.
    // This will be fully implemented when the executor is complete.
    if !ctx.quiet {
        println!(
            "Would apply {} fix(es). Execution not yet implemented.",
            parsed_fix_ids.len()
        );
    }

    Ok(())
}
