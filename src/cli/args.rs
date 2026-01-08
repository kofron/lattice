//! cli::args
//!
//! Command-line argument definitions using clap derive.
//!
//! # Global Flags
//!
//! These flags are available on all commands:
//! - `--help` / `-h`: Show help
//! - `--version`: Show version
//! - `--cwd <path>`: Run as if in that directory
//! - `--debug`: Enable debug logging
//! - `--interactive` / `--no-interactive`: Control prompts
//! - `--quiet` / `-q`: Minimal output

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Lattice - A Rust-native CLI for stacked branches and PRs
#[derive(Parser, Debug)]
#[command(name = "lattice")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Run as if lattice was started in this directory
    #[arg(long, global = true)]
    pub cwd: Option<PathBuf>,

    /// Enable debug logging
    #[arg(long, global = true)]
    pub debug: bool,

    /// Minimal output; implies --no-interactive
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Enable interactive prompts
    #[arg(long, global = true, conflicts_with = "no_interactive")]
    pub interactive_flag: bool,

    /// Disable interactive prompts
    #[arg(long, global = true)]
    pub no_interactive: bool,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Parse command-line arguments.
    pub fn parse_args() -> Self {
        Parser::parse()
    }

    /// Determine if interactive mode is enabled.
    ///
    /// Returns true if:
    /// - `--interactive` was explicitly set, OR
    /// - Neither `--no-interactive` nor `--quiet` was set AND stdin is a TTY
    pub fn interactive(&self) -> bool {
        if self.interactive_flag {
            true
        } else if self.no_interactive || self.quiet {
            false
        } else {
            // Default: interactive if stdin is a TTY
            atty_check()
        }
    }
}

/// Check if stdin is a TTY.
///
/// This is a stub that always returns true for now.
/// Will be properly implemented when we add the `atty` crate.
fn atty_check() -> bool {
    // TODO: Use atty crate or std::io::IsTerminal when stabilized
    true
}

/// Available commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Bootstrap validation command (temporary)
    Hello,

    /// Diagnose and repair repository issues
    #[command(name = "doctor")]
    Doctor {
        /// Apply specific fix(es) by ID
        #[arg(long = "fix", value_name = "FIX_ID")]
        fix_ids: Vec<String>,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,

        /// List all issues and fixes in machine-readable format
        #[arg(long)]
        list: bool,
    },

    // ========== Phase A: Read-Only Commands ==========
    /// Display tracked branches in stack layout
    #[command(name = "log")]
    Log {
        /// Short format (branch names only)
        #[arg(short, long)]
        short: bool,

        /// Long format with full details
        #[arg(short, long)]
        long: bool,

        /// Filter to current branch's stack only
        #[arg(long)]
        stack: bool,

        /// Show all tracked branches
        #[arg(short, long)]
        all: bool,

        /// Reverse display order (oldest first)
        #[arg(short, long)]
        reverse: bool,
    },

    /// Show tracking status, parent, freeze state for a branch
    #[command(name = "info")]
    Info {
        /// Branch to show info for (defaults to current)
        branch: Option<String>,

        /// Show diff from base
        #[arg(long)]
        diff: bool,

        /// Show stat from base
        #[arg(long)]
        stat: bool,

        /// Show full patch from base
        #[arg(long)]
        patch: bool,
    },

    /// Print parent branch name
    #[command(name = "parent")]
    Parent,

    /// Print child branch names
    #[command(name = "children")]
    Children,

    /// Display or set the trunk branch
    #[command(name = "trunk")]
    Trunk {
        /// Set trunk to this branch
        #[arg(long)]
        set: Option<String>,
    },

    // ========== Phase B: Setup Commands ==========
    /// Authenticate with a remote forge (GitHub)
    #[command(name = "auth")]
    Auth {
        /// Provide token non-interactively
        #[arg(long)]
        token: Option<String>,

        /// Host to authenticate with
        #[arg(long, default_value = "github")]
        host: String,

        /// Show current authentication status
        #[arg(long)]
        status: bool,

        /// Remove stored authentication
        #[arg(long)]
        logout: bool,
    },

    /// Initialize Lattice in this repository
    #[command(name = "init")]
    Init {
        /// Set trunk branch
        #[arg(long)]
        trunk: Option<String>,

        /// Clear all metadata and reconfigure
        #[arg(long)]
        reset: bool,

        /// Skip confirmation prompts
        #[arg(long)]
        force: bool,
    },

    /// Get, set, or list configuration values
    #[command(name = "config")]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Generate shell completion scripts
    #[command(name = "completion")]
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Show version and changelog
    #[command(name = "changelog")]
    Changelog,

    // ========== Phase C: Tracking Commands ==========
    /// Start tracking a branch
    #[command(name = "track")]
    Track {
        /// Branch to track (defaults to current)
        branch: Option<String>,

        /// Set parent branch explicitly
        #[arg(long, short)]
        parent: Option<String>,

        /// Auto-select nearest tracked ancestor
        #[arg(long, short)]
        force: bool,

        /// Track as frozen
        #[arg(long)]
        as_frozen: bool,
    },

    /// Stop tracking a branch
    #[command(name = "untrack")]
    Untrack {
        /// Branch to untrack (defaults to current)
        branch: Option<String>,

        /// Also untrack all descendants without prompting
        #[arg(long, short)]
        force: bool,
    },

    /// Mark a branch as frozen (immutable to Lattice operations)
    #[command(name = "freeze")]
    Freeze {
        /// Branch to freeze (defaults to current)
        branch: Option<String>,

        /// Only freeze this branch, not downstack
        #[arg(long)]
        only: bool,
    },

    /// Unmark a branch as frozen
    #[command(name = "unfreeze")]
    Unfreeze {
        /// Branch to unfreeze (defaults to current)
        branch: Option<String>,

        /// Only unfreeze this branch, not downstack
        #[arg(long)]
        only: bool,
    },

    // ========== Phase D: Navigation Commands ==========
    /// Check out a branch
    #[command(name = "checkout", visible_alias = "co")]
    Checkout {
        /// Branch to check out
        branch: Option<String>,

        /// Check out trunk
        #[arg(long)]
        trunk: bool,

        /// Filter selector to current stack
        #[arg(long)]
        stack: bool,
    },

    /// Move up to a child branch
    #[command(name = "up")]
    Up {
        /// Number of steps to move (default 1)
        #[arg(default_value = "1")]
        steps: u32,
    },

    /// Move down to the parent branch
    #[command(name = "down")]
    Down {
        /// Number of steps to move (default 1)
        #[arg(default_value = "1")]
        steps: u32,
    },

    /// Move to the top of the current stack (leaf)
    #[command(name = "top")]
    Top,

    /// Move to the bottom of the current stack (trunk-child)
    #[command(name = "bottom")]
    Bottom,

    // ========== Phase E: Core Mutating Commands ==========
    /// Rebase tracked branches to align with parent tips
    #[command(name = "restack", visible_alias = "rs")]
    Restack {
        /// Specific branch to restack
        #[arg(long)]
        branch: Option<String>,

        /// Only restack this branch
        #[arg(long)]
        only: bool,

        /// Restack this branch and its ancestors
        #[arg(long)]
        downstack: bool,
    },

    /// Continue a paused operation after resolving conflicts
    #[command(name = "continue")]
    Continue {
        /// Stage all changes before continuing
        #[arg(long, short)]
        all: bool,
    },

    /// Abort a paused operation and restore pre-operation state
    #[command(name = "abort")]
    Abort,

    /// Undo the last completed operation
    #[command(name = "undo")]
    Undo,

    /// Create a new tracked branch
    #[command(name = "create", visible_alias = "c")]
    Create {
        /// Name for the new branch
        name: Option<String>,

        /// Commit message (creates a commit with staged changes)
        #[arg(short, long)]
        message: Option<String>,

        /// Stage all changes before committing
        #[arg(short, long)]
        all: bool,

        /// Stage modified tracked files before committing
        #[arg(short, long)]
        update: bool,

        /// Interactive patch staging
        #[arg(short, long)]
        patch: bool,

        /// Insert between current branch and its child
        #[arg(short, long)]
        insert: bool,
    },

    // ========== Phase 3: Advanced Rewriting Commands ==========
    /// Amend commits or create first commit, auto-restack descendants
    #[command(name = "modify")]
    Modify {
        /// Create new commit instead of amending
        #[arg(short, long)]
        create: bool,

        /// Stage all changes (git add -A)
        #[arg(short, long)]
        all: bool,

        /// Stage modified tracked files (git add -u)
        #[arg(short, long)]
        update: bool,

        /// Interactive patch staging (git add -p)
        #[arg(short, long)]
        patch: bool,

        /// Commit message
        #[arg(short, long)]
        message: Option<String>,

        /// Open editor for commit message
        #[arg(short, long)]
        edit: bool,
    },

    /// Reparent branch onto another branch
    #[command(name = "move")]
    Move {
        /// Target parent branch (required)
        #[arg(long)]
        onto: String,

        /// Branch to move (defaults to current)
        #[arg(long)]
        source: Option<String>,
    },

    /// Rename current branch
    #[command(name = "rename")]
    Rename {
        /// New name for the branch
        name: String,
    },

    /// Delete a branch
    #[command(name = "delete", visible_alias = "d")]
    Delete {
        /// Branch to delete (defaults to current)
        branch: Option<String>,

        /// Also delete all descendants
        #[arg(long)]
        upstack: bool,

        /// Also delete all ancestors (not trunk)
        #[arg(long)]
        downstack: bool,

        /// Skip confirmation prompts
        #[arg(long, short)]
        force: bool,
    },

    /// Squash all commits in current branch into one
    #[command(name = "squash")]
    Squash {
        /// Commit message for squashed commit
        #[arg(short, long)]
        message: Option<String>,

        /// Open editor for commit message
        #[arg(short, long)]
        edit: bool,
    },

    /// Fold current branch into parent
    #[command(name = "fold")]
    Fold {
        /// Keep the current branch name by renaming parent
        #[arg(long)]
        keep: bool,
    },

    /// Pop branch, keeping changes uncommitted
    #[command(name = "pop")]
    Pop,

    /// Reorder branches in current stack using editor
    #[command(name = "reorder")]
    Reorder,

    /// Split current branch
    #[command(name = "split")]
    Split {
        /// Split each commit into its own branch
        #[arg(long)]
        by_commit: bool,

        /// Extract changes to specified files into new branch
        #[arg(long, num_args = 1..)]
        by_file: Vec<String>,
    },

    /// Create a revert branch for a commit
    #[command(name = "revert")]
    Revert {
        /// Commit SHA to revert
        sha: String,
    },

    // ========== Phase F: GitHub Integration Commands ==========
    /// Submit branches as PRs to GitHub
    #[command(name = "submit", visible_alias = "s")]
    Submit {
        /// Submit entire stack (ancestors + descendants)
        #[arg(long)]
        stack: bool,

        /// Create PRs as drafts
        #[arg(long)]
        draft: bool,

        /// Publish draft PRs (make ready for review)
        #[arg(long)]
        publish: bool,

        /// Prompt for confirmation before each action
        #[arg(long)]
        confirm: bool,

        /// Show what would be done without making changes
        #[arg(long)]
        dry_run: bool,

        /// Force push even if remote has diverged
        #[arg(long, short)]
        force: bool,

        /// Push all branches regardless of changes
        #[arg(long)]
        always: bool,

        /// Only update existing PRs, don't create new ones
        #[arg(long)]
        update_only: bool,

        /// Request reviewers (comma-separated usernames)
        #[arg(long)]
        reviewers: Option<String>,

        /// Request team reviewers (comma-separated team slugs)
        #[arg(long)]
        team_reviewers: Option<String>,

        /// Skip restacking before submit
        #[arg(long)]
        no_restack: bool,

        /// Open PR URLs in browser after submit
        #[arg(long)]
        view: bool,
    },

    /// Sync with remote (fetch, update trunk, detect merged PRs)
    #[command(name = "sync")]
    Sync {
        /// Force reset trunk to remote even if diverged
        #[arg(long, short)]
        force: bool,

        /// Restack after syncing
        #[arg(long)]
        restack: bool,

        /// Skip restacking after sync
        #[arg(long)]
        no_restack: bool,
    },

    /// Fetch a branch or PR from remote
    #[command(name = "get")]
    Get {
        /// Branch name or PR number to fetch
        target: String,

        /// Only fetch this branch (not upstack)
        #[arg(long)]
        downstack: bool,

        /// Force overwrite local divergence
        #[arg(long, short)]
        force: bool,

        /// Restack after fetching
        #[arg(long)]
        restack: bool,

        /// Skip restacking after fetch
        #[arg(long)]
        no_restack: bool,

        /// Track as unfrozen (default is frozen)
        #[arg(long)]
        unfrozen: bool,
    },

    /// Merge PRs via GitHub API
    #[command(name = "merge")]
    Merge {
        /// Prompt for confirmation before merging
        #[arg(long)]
        confirm: bool,

        /// Show what would be merged without making changes
        #[arg(long)]
        dry_run: bool,

        /// Merge method (merge, squash, rebase)
        #[arg(long, value_enum)]
        method: Option<MergeMethodArg>,
    },

    /// Open PR URL in browser or print it
    #[command(name = "pr")]
    Pr {
        /// Branch or PR number (defaults to current)
        target: Option<String>,

        /// Show URLs for entire stack
        #[arg(long)]
        stack: bool,
    },

    /// Remove PR linkage from branch metadata
    #[command(name = "unlink")]
    Unlink {
        /// Branch to unlink (defaults to current)
        branch: Option<String>,
    },
}

/// Merge method for PRs
#[derive(clap::ValueEnum, Debug, Clone, Copy)]
pub enum MergeMethodArg {
    /// Create a merge commit
    Merge,
    /// Squash and merge
    Squash,
    /// Rebase and merge
    Rebase,
}

/// Config subcommands
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigAction {
    /// Get a configuration value
    Get {
        /// Configuration key
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        /// Value to set
        value: String,
    },
    /// List all configuration values
    List,
}

/// Supported shells for completion
#[derive(clap::ValueEnum, Debug, Clone, Copy)]
#[allow(clippy::enum_variant_names)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    PowerShell,
}
