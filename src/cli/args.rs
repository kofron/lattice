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
    /// Diagnose and repair repository issues
    #[command(
        name = "doctor",
        long_about = "Diagnose and repair repository issues.\n\n\
            The doctor command scans your repository for problems that could prevent \
            Lattice from working correctly. It detects issues like orphaned metadata, \
            broken parent references, merge conflicts, and configuration problems.\n\n\
            When issues are found, doctor suggests fixes. You must explicitly approve \
            fixes before they are applied - doctor never auto-repairs without consent.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Check repository health (good first step when something seems wrong)
    lt doctor

    # See all issues in machine-readable format for scripting
    lt doctor --list

    # Preview what a specific fix would do
    lt doctor --fix orphan-meta-1 --dry-run

    # Apply a specific fix
    lt doctor --fix orphan-meta-1

COMMON SCENARIOS:
    After a failed rebase or interrupted operation:
        lt doctor              # diagnose what went wrong
        lt abort               # if an operation is stuck
        lt doctor --fix ...    # apply recommended fixes"
    )]
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
    #[command(
        name = "log",
        long_about = "Display tracked branches in a visual stack layout.\n\n\
            Shows all branches that Lattice is tracking, organized by their parent-child \
            relationships. The current branch is marked with an asterisk (*). This is your \
            primary command for understanding the current state of your stack.",
        after_help = "\
WORKFLOW EXAMPLES:
    # See your current stack (most common usage)
    lt log

    # Quick overview - just branch names, no details
    lt log -s

    # Full details including parent, base commit, PR status
    lt log -l

    # See all tracked branches across all stacks
    lt log --all

READING THE OUTPUT:
    * feature-c       <- you are here
      feature-b       <- parent of feature-c
      feature-a       <- parent of feature-b
      main            <- trunk (root of the stack)"
    )]
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
    #[command(
        name = "info",
        long_about = "Show detailed information about a branch's tracking status.\n\n\
            Displays the parent branch, base commit, freeze state, and PR linkage for \
            a branch. Use --diff, --stat, or --patch to see the changes this branch \
            introduces relative to its parent.",
        after_help = "\
WORKFLOW EXAMPLES:
    # See info for current branch
    lt info

    # See info for a specific branch
    lt info feature-auth

    # Review changes before submitting
    lt info --stat           # summary of files changed
    lt info --diff           # full diff from parent

    # Detailed review workflow
    lt info --patch          # see full patch for code review"
    )]
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
    #[command(
        name = "parent",
        long_about = "Print the name of the current branch's parent.\n\n\
            Useful for scripting or quickly checking which branch the current \
            branch is stacked on top of.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Check what branch you're stacked on
    lt parent

    # Use in scripts
    git diff $(lt parent)..HEAD

    # Navigate to parent
    lt down   # equivalent to: lt checkout $(lt parent)"
    )]
    Parent,

    /// Print child branch names
    #[command(
        name = "children",
        long_about = "Print the names of branches stacked on top of the current branch.\n\n\
            A branch can have multiple children if you've created branches that \
            diverge from the same point.",
        after_help = "\
WORKFLOW EXAMPLES:
    # See what branches depend on this one
    lt children

    # Useful before modifying a branch - shows what will need restacking
    lt children            # see affected branches
    lt modify -a -m \"fix\" # make changes
    lt restack             # propagate to children"
    )]
    Children,

    /// Display or set the trunk branch
    #[command(
        name = "trunk",
        long_about = "Display or change the trunk (base) branch for your stacks.\n\n\
            The trunk is the branch that serves as the foundation for all your stacks \
            (typically 'main' or 'master'). All stacks ultimately trace their ancestry \
            back to trunk.",
        after_help = "\
WORKFLOW EXAMPLES:
    # See current trunk
    lt trunk

    # Change trunk (rare - usually set during init)
    lt trunk --set develop

    # After changing trunk, you may need to reparent branches
    lt move --onto develop   # move current branch onto new trunk"
    )]
    Trunk {
        /// Set trunk to this branch
        #[arg(long)]
        set: Option<String>,
    },

    // ========== Phase B: Setup Commands ==========
    /// Authenticate with GitHub using OAuth device flow
    #[command(
        name = "auth",
        long_about = "Authenticate with GitHub using OAuth device flow.\n\n\
            Lattice uses GitHub App OAuth to authenticate. This is more secure than \
            personal access tokens and supports automatic token refresh. Your browser \
            will open to authorize the Lattice GitHub App.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Authenticate (opens browser automatically)
    lt auth

    # Authenticate without opening browser
    lt auth --no-browser

    # Check if you're authenticated
    lt auth --status

    # Remove stored credentials
    lt auth --logout

HOW IT WORKS:
    1. Run 'lt auth' to start the device flow
    2. Your browser opens to GitHub's authorization page
    3. Enter the code displayed in your terminal
    4. Authorize the Lattice app
    5. You're ready to use 'lt submit' and 'lt merge'"
    )]
    Auth {
        /// Do not attempt to open browser automatically
        #[arg(long)]
        no_browser: bool,

        /// GitHub host to authenticate with
        #[arg(long, default_value = "github.com")]
        host: String,

        /// Show current authentication status
        #[arg(long)]
        status: bool,

        /// Remove stored authentication
        #[arg(long)]
        logout: bool,
    },

    /// Initialize Lattice in this repository
    #[command(
        name = "init",
        long_about = "Initialize Lattice tracking in a git repository.\n\n\
            This is the first command to run in a new repository. It configures the \
            trunk branch (usually 'main' or 'master') that serves as the base for all \
            your stacks. Run this once per repository.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Initialize with auto-detected trunk (usually 'main')
    lt init

    # Specify trunk explicitly
    lt init --trunk develop

    # Reset all Lattice configuration and start fresh
    lt init --reset

GETTING STARTED:
    1. cd into your git repository
    2. lt init
    3. lt auth                    # if you want to use GitHub features
    4. lt create my-first-branch  # start working!"
    )]
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
    #[command(
        name = "config",
        long_about = "View or modify Lattice configuration.\n\n\
            Configuration is stored per-repository in .git/lattice.toml. Use this \
            command to inspect or change settings like the default merge method.",
        after_help = "\
WORKFLOW EXAMPLES:
    # List all configuration values
    lt config list

    # Get a specific value
    lt config get merge.method

    # Set a value
    lt config set merge.method squash"
    )]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Generate shell completion scripts
    #[command(
        name = "completion",
        long_about = "Generate shell completion scripts for tab-completion.\n\n\
            Outputs a completion script for the specified shell. Add the output \
            to your shell's configuration to enable tab-completion for Lattice commands.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Bash (add to ~/.bashrc)
    lt completion bash >> ~/.bashrc

    # Zsh (add to ~/.zshrc)
    lt completion zsh >> ~/.zshrc

    # Fish
    lt completion fish > ~/.config/fish/completions/lt.fish

    # PowerShell
    lt completion powershell >> $PROFILE"
    )]
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Show version and changelog
    #[command(
        name = "changelog",
        long_about = "Display the current version and recent changes.\n\n\
            Shows what's new in this version of Lattice, including new features, \
            bug fixes, and breaking changes.",
        after_help = "\
WORKFLOW EXAMPLES:
    # See what's new
    lt changelog

    # Check version number
    lt --version"
    )]
    Changelog,

    // ========== Phase C: Tracking Commands ==========
    /// Start tracking a branch
    #[command(
        name = "track",
        long_about = "Start tracking an existing git branch with Lattice.\n\n\
            Tracking a branch tells Lattice to manage it as part of a stack. Lattice \
            will maintain its relationship to a parent branch and handle rebasing \
            when the parent changes. Use this for branches you created with plain git.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Track current branch with auto-detected parent
    lt track

    # Track with explicit parent
    lt track --parent feature-base

    # Track a branch created outside Lattice
    git checkout -b my-feature main
    # ... make commits ...
    lt track --parent main

    # Track a coworker's branch as frozen (won't rebase it)
    lt get 1234              # fetch PR #1234
    lt track --as-frozen     # track but don't modify

WHEN TO USE:
    - You created a branch with 'git checkout -b' instead of 'lt create'
    - You want to add an existing branch to your stack
    - You fetched a branch and want Lattice to manage it"
    )]
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
    #[command(
        name = "untrack",
        long_about = "Stop tracking a branch with Lattice.\n\n\
            The branch will still exist in git, but Lattice will no longer manage \
            its parent relationship or include it in restack operations. Child \
            branches will be reparented to this branch's parent.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Untrack current branch
    lt untrack

    # Untrack a specific branch
    lt untrack feature-old

    # Untrack branch and all its descendants
    lt untrack --force

WHEN TO USE:
    - You want to manage a branch manually with git
    - You're abandoning a branch but keeping it around
    - You fetched a branch temporarily and no longer need it tracked"
    )]
    Untrack {
        /// Branch to untrack (defaults to current)
        branch: Option<String>,

        /// Also untrack all descendants without prompting
        #[arg(long, short)]
        force: bool,
    },

    /// Mark a branch as frozen (immutable to Lattice operations)
    #[command(
        name = "freeze",
        long_about = "Mark a branch as frozen to prevent Lattice from modifying it.\n\n\
            Frozen branches are tracked but never rebased or modified by Lattice. \
            This is useful for branches you've fetched from others or branches that \
            have been merged and should remain stable.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Freeze current branch
    lt freeze

    # Freeze after fetching a coworker's branch
    lt get 1234        # fetch their PR
    lt freeze          # don't modify their work

    # Freeze just this branch, not downstack ancestors
    lt freeze --only

WHEN TO USE:
    - You fetched someone else's branch and don't want to rebase it
    - A branch has been merged and should stay as-is
    - You want to preserve exact commit history"
    )]
    Freeze {
        /// Branch to freeze (defaults to current)
        branch: Option<String>,

        /// Only freeze this branch, not downstack
        #[arg(long)]
        only: bool,
    },

    /// Unmark a branch as frozen
    #[command(
        name = "unfreeze",
        long_about = "Unmark a frozen branch so Lattice can modify it again.\n\n\
            After unfreezing, the branch will participate in restack operations \
            and can be modified by other Lattice commands.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Unfreeze current branch
    lt unfreeze

    # Unfreeze to allow restacking
    lt unfreeze
    lt restack         # now it will rebase onto parent

    # Unfreeze just this branch
    lt unfreeze --only"
    )]
    Unfreeze {
        /// Branch to unfreeze (defaults to current)
        branch: Option<String>,

        /// Only unfreeze this branch, not downstack
        #[arg(long)]
        only: bool,
    },

    // ========== Phase D: Navigation Commands ==========
    /// Check out a branch
    #[command(
        name = "checkout",
        visible_alias = "co",
        long_about = "Switch to a different branch in your stack.\n\n\
            Like git checkout, but with stack-aware features. Without arguments, \
            shows an interactive picker of tracked branches. Use --stack to limit \
            the picker to branches in the current stack.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Interactive branch picker
    lt checkout
    lt co                    # alias

    # Check out specific branch
    lt checkout feature-auth

    # Jump to trunk
    lt checkout --trunk

    # Pick from current stack only
    lt checkout --stack

NAVIGATING YOUR STACK:
    lt up      # go to child branch
    lt down    # go to parent branch
    lt top     # go to leaf (top of stack)
    lt bottom  # go to first branch above trunk"
    )]
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
    #[command(
        name = "up",
        long_about = "Navigate up the stack to a child branch.\n\n\
            Moves from the current branch to a branch that is stacked on top of it. \
            If there are multiple children, shows a picker in interactive mode.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Move to child branch
    lt up

    # Move up 2 levels
    lt up 2

    # Navigate through a stack
    lt bottom           # start at base
    lt up               # review first branch
    lt up               # review second branch
    # ... continue up the stack"
    )]
    Up {
        /// Number of steps to move (default 1)
        #[arg(default_value = "1")]
        steps: u32,
    },

    /// Move down to the parent branch
    #[command(
        name = "down",
        long_about = "Navigate down the stack to the parent branch.\n\n\
            Moves from the current branch to the branch it's stacked on top of. \
            Equivalent to 'lt checkout $(lt parent)'.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Move to parent branch
    lt down

    # Move down 2 levels
    lt down 2

    # Common workflow: fix something in parent, then come back
    lt down              # go to parent
    lt modify -a -m \"fix\"
    lt restack           # update children
    lt up                # back to where you were"
    )]
    Down {
        /// Number of steps to move (default 1)
        #[arg(default_value = "1")]
        steps: u32,
    },

    /// Move to the top of the current stack (leaf)
    #[command(
        name = "top",
        long_about = "Jump to the top (leaf) of the current stack.\n\n\
            Navigates to the branch at the end of the stack - the one with no \
            children. If there are multiple leaf branches, shows a picker.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Jump to top of stack
    lt top

    # Common workflow: start review from bottom, end at top
    lt bottom            # go to base of stack
    # ... review each branch ...
    lt top               # or jump straight to the end"
    )]
    Top,

    /// Move to the bottom of the current stack (trunk-child)
    #[command(
        name = "bottom",
        long_about = "Jump to the bottom of the current stack.\n\n\
            Navigates to the first branch above trunk - the base of your stack. \
            Useful for starting a review from the beginning.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Jump to bottom of stack
    lt bottom

    # Start reviewing a stack from the beginning
    lt bottom            # go to first branch
    lt info --diff       # review changes
    lt up                # move to next branch
    # ... continue review ..."
    )]
    Bottom,

    // ========== Phase E: Core Mutating Commands ==========
    /// Rebase tracked branches to align with parent tips
    #[command(
        name = "restack",
        visible_alias = "rs",
        long_about = "Rebase tracked branches to align with their parent branch tips.\n\n\
            When you modify a branch (amend, rebase, etc.), its children become \
            misaligned - their base commit no longer matches the parent's tip. \
            Restack detects this and rebases branches to restore the stack structure.\n\n\
            By default, restacks the current branch and all descendants (upstack). \
            Use --only for just one branch, or --downstack for ancestors.",
        after_help = "\
WORKFLOW EXAMPLES:
    # After amending a branch, propagate changes upstack
    lt modify -a -m \"address review feedback\"
    lt restack                   # rebase all children

    # After pulling upstream changes
    lt sync                      # update trunk
    lt restack                   # align stack with new trunk

    # Restack just one branch
    lt restack --only

    # Restack current branch and ancestors (rare)
    lt restack --downstack

HANDLING CONFLICTS:
    If a rebase conflicts, Lattice pauses:
    1. Resolve conflicts in your editor
    2. git add <resolved files>
    3. lt continue

    To give up: lt abort"
    )]
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
    #[command(
        name = "continue",
        long_about = "Continue a paused operation after resolving conflicts.\n\n\
            When a rebase or other operation encounters conflicts, Lattice pauses \
            and waits for you to resolve them. After fixing conflicts and staging \
            the resolved files, run this command to continue.",
        after_help = "\
WORKFLOW EXAMPLES:
    # After resolving conflicts
    git add <resolved-files>
    lt continue

    # Stage all changes and continue in one step
    lt continue --all

TYPICAL CONFLICT WORKFLOW:
    lt restack                   # conflicts!
    # ... edit files to resolve conflicts ...
    git add src/conflicted.rs
    lt continue                  # resume restack
    # ... may pause again if more conflicts ...
    lt continue                  # until done

IF YOU WANT TO GIVE UP:
    lt abort                     # restore pre-operation state"
    )]
    Continue {
        /// Stage all changes before continuing
        #[arg(long, short)]
        all: bool,
    },

    /// Abort a paused operation and restore pre-operation state
    #[command(
        name = "abort",
        long_about = "Abort a paused operation and restore the repository to its pre-operation state.\n\n\
            If you're in the middle of a restack or other operation that has paused \
            for conflict resolution, this command abandons the operation and restores \
            your repository to how it was before you started.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Abandon a conflicting restack
    lt restack                   # conflicts!
    # ... decide you don't want to deal with this now ...
    lt abort                     # back to before 'lt restack'

    # Check if an operation is in progress
    lt doctor                    # shows operation state"
    )]
    Abort,

    /// Undo the last completed operation
    #[command(
        name = "undo",
        long_about = "Undo the last completed Lattice operation.\n\n\
            Reverses the effects of the most recent operation (restack, create, \
            modify, etc.). This is a safety net for when an operation didn't \
            produce the results you expected.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Undo a restack that went wrong
    lt restack                   # oops, not what I wanted
    lt undo                      # back to before the restack

    # Undo branch creation
    lt create my-branch
    lt undo                      # branch is gone

NOTE:
    Only the most recent operation can be undone. Undo is not a full
    version control system - for that, use git reflog."
    )]
    Undo,

    /// Create a new tracked branch
    #[command(
        name = "create",
        visible_alias = "c",
        long_about = "Create a new branch stacked on top of the current branch.\n\n\
            This is the primary way to start new work in Lattice. The new branch \
            is automatically tracked with the current branch as its parent. \
            Optionally stage and commit changes in one step.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Create a new branch (prompts for name if interactive)
    lt create my-feature
    lt c my-feature              # alias

    # Create branch and commit staged changes
    git add src/
    lt create my-feature -m \"implement feature\"

    # Stage all changes and commit in one step
    lt create my-feature -a -m \"implement feature\"

    # Interactive staging (like git add -p)
    lt create my-feature -p -m \"partial changes\"

    # Insert a branch between current and its child
    lt create hotfix --insert   # becomes parent of current's child

BUILDING A STACK:
    lt create feature-part-1 -a -m \"first part\"
    lt create feature-part-2 -a -m \"second part\"
    lt create feature-part-3 -a -m \"third part\"
    lt log                       # see your stack"
    )]
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
    #[command(
        name = "modify",
        long_about = "Amend the current branch's commit and automatically restack descendants.\n\n\
            This is the safe way to amend commits when you have branches stacked on top. \
            After amending, Lattice automatically restacks all descendant branches to \
            incorporate your changes.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Amend with all current changes
    lt modify -a -m \"updated implementation\"

    # Amend with staged changes only
    git add specific-file.rs
    lt modify -m \"fix typo\"

    # Interactive staging before amend
    lt modify -p -m \"selected changes\"

    # Create new commit instead of amending
    lt modify --create -a -m \"additional changes\"

    # Edit commit message in editor
    lt modify --edit

RESPONDING TO CODE REVIEW:
    # Reviewer requested changes to an earlier branch
    lt checkout feature-auth     # go to that branch
    # ... make the requested changes ...
    lt modify -a -m \"address review feedback\"
    # descendants are automatically restacked
    lt submit --stack            # update all PRs"
    )]
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
    #[command(
        name = "move",
        long_about = "Move a branch to be stacked on a different parent.\n\n\
            Changes the parent of a branch without losing its commits. The branch \
            will be rebased onto the new parent. Children of the moved branch \
            come along with it.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Move current branch onto a different parent
    lt move --onto feature-base

    # Move a specific branch
    lt move --onto main --source feature-experiment

    # Restructure your stack
    # Before: main -> A -> B -> C
    # Move B onto main directly:
    lt checkout B
    lt move --onto main
    # After: main -> A -> C, main -> B

WHEN TO USE:
    - You started a branch from the wrong base
    - You want to restructure your stack
    - You're extracting part of a stack into a separate stack"
    )]
    Move {
        /// Target parent branch (required)
        #[arg(long)]
        onto: String,

        /// Branch to move (defaults to current)
        #[arg(long)]
        source: Option<String>,
    },

    /// Rename current branch
    #[command(
        name = "rename",
        long_about = "Rename the current branch.\n\n\
            Changes the branch name while preserving all tracking information, \
            commit history, and stack relationships.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Rename current branch
    lt rename better-name

    # Typical workflow: realize you want a better name
    lt create temp-name -a -m \"wip\"
    # ... work on it, realize what it should be called ...
    lt rename auth-token-refresh"
    )]
    Rename {
        /// New name for the branch
        name: String,
    },

    /// Delete a branch
    #[command(
        name = "delete",
        visible_alias = "d",
        long_about = "Delete a branch and optionally its descendants or ancestors.\n\n\
            Removes the branch from git and Lattice tracking. Children of the \
            deleted branch are reparented to the deleted branch's parent.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Delete current branch
    lt delete
    lt d                         # alias

    # Delete a specific branch
    lt delete old-feature

    # Delete branch and all descendants (whole upstack)
    lt delete --upstack

    # Delete without confirmation
    lt delete --force

AFTER MERGING:
    # Clean up merged branches
    lt sync                      # detects merged PRs
    lt delete merged-branch      # remove local branch"
    )]
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
    #[command(
        name = "squash",
        long_about = "Squash all commits in the current branch into a single commit.\n\n\
            Combines all commits that are unique to this branch (not in parent) \
            into one commit. Useful for cleaning up work-in-progress commits \
            before submitting for review.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Squash with new message
    lt squash -m \"feat: implement user authentication\"

    # Squash and edit message in editor
    lt squash --edit

BEFORE SUBMITTING:
    # Clean up messy commit history
    lt squash -m \"clean implementation\"
    lt submit                    # nice single commit in PR"
    )]
    Squash {
        /// Commit message for squashed commit
        #[arg(short, long)]
        message: Option<String>,

        /// Open editor for commit message
        #[arg(short, long)]
        edit: bool,
    },

    /// Fold current branch into parent
    #[command(
        name = "fold",
        long_about = "Fold the current branch into its parent branch.\n\n\
            Merges the current branch's commits into the parent branch, then \
            deletes the current branch. Children of the folded branch become \
            children of the parent.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Fold current branch into parent
    lt fold

    # Keep the current branch's name (rename parent)
    lt fold --keep

WHEN TO USE:
    - You split work too granularly and want to combine branches
    - A branch turned out to be too small for its own PR
    - You're simplifying your stack structure"
    )]
    Fold {
        /// Keep the current branch name by renaming parent
        #[arg(long)]
        keep: bool,
    },

    /// Pop branch, keeping changes uncommitted
    #[command(
        name = "pop",
        long_about = "Remove the current branch while keeping its changes as uncommitted modifications.\n\n\
            Like 'git reset HEAD~n' but stack-aware. The branch is deleted, and all \
            its changes become unstaged modifications in the working directory. \
            Useful for moving changes to a different branch.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Pop current branch, keeping changes
    lt pop
    # now changes are uncommitted in working directory

    # Move changes to a different branch
    lt pop                       # uncommit changes
    lt checkout other-branch
    lt modify -a -m \"moved changes here\"

WHEN TO USE:
    - You committed to the wrong branch
    - You want to reorganize which changes go where
    - You want to uncommit and re-stage differently"
    )]
    Pop,

    /// Reorder branches in current stack using editor
    #[command(
        name = "reorder",
        long_about = "Reorder branches in the current stack using an interactive editor.\n\n\
            Opens your editor with a list of branches in the stack. Reorder the \
            lines to change the stack structure. Branches will be rebased to match \
            your new ordering.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Open editor to reorder stack
    lt reorder

    # In the editor, you'll see something like:
    #   feature-c
    #   feature-b
    #   feature-a
    #
    # Reorder lines to change the stack:
    #   feature-a
    #   feature-c    # moved up
    #   feature-b

WHEN TO USE:
    - You want to change which branch depends on which
    - You realized branches should be in a different order
    - You're restructuring before submitting PRs"
    )]
    Reorder,

    /// Split current branch
    #[command(
        name = "split",
        long_about = "Split the current branch into multiple branches.\n\n\
            Use --by-commit to create a separate branch for each commit, or \
            --by-file to extract changes to specific files into a new branch.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Split each commit into its own branch
    lt split --by-commit

    # Extract changes to specific files into new branch
    lt split --by-file src/auth.rs src/auth_test.rs

WHEN TO USE:
    - A branch grew too large and should be multiple PRs
    - You want to submit part of your changes first
    - Reviewer asked you to split a large PR"
    )]
    Split {
        /// Split each commit into its own branch
        #[arg(long)]
        by_commit: bool,

        /// Extract changes to specified files into new branch
        #[arg(long, num_args = 1..)]
        by_file: Vec<String>,
    },

    /// Create a revert branch for a commit
    #[command(
        name = "revert",
        long_about = "Create a new branch that reverts a specific commit.\n\n\
            Creates a branch containing a commit that undoes the changes from \
            the specified commit. Useful for quickly backing out a change.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Revert a specific commit
    lt revert abc1234

    # Workflow: something broke, revert it
    git log                      # find the bad commit
    lt revert abc1234            # create revert branch
    lt submit                    # submit revert as PR"
    )]
    Revert {
        /// Commit SHA to revert
        sha: String,
    },

    // ========== Phase F: GitHub Integration Commands ==========
    /// Submit branches as PRs to GitHub
    #[command(
        name = "submit",
        visible_alias = "s",
        long_about = "Push branches and create or update pull requests on GitHub.\n\n\
            This is the main command for getting your work onto GitHub. It pushes \
            branches, creates PRs for branches that don't have them, and updates \
            existing PRs. PR descriptions automatically include a stack visualization \
            showing how PRs relate to each other.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Submit current branch and ancestors (most common)
    lt submit
    lt s                         # alias

    # Submit entire stack including descendants
    lt submit --stack

    # Create PRs as drafts (work in progress)
    lt submit --draft

    # Mark draft PRs as ready for review
    lt submit --publish

    # Preview without making changes
    lt submit --dry-run

    # Request reviewers
    lt submit --reviewers alice,bob
    lt submit --team-reviewers backend-team

TYPICAL WORKFLOW:
    # After finishing a feature
    lt submit                    # create/update PRs
    # ... address review feedback ...
    lt modify -a -m \"feedback\"
    lt submit                    # update PRs

    # Open PRs in browser after submit
    lt submit --view"
    )]
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
    #[command(
        name = "sync",
        long_about = "Synchronize with the remote repository.\n\n\
            Fetches from origin, updates your local trunk to match remote, and \
            detects which PRs have been merged. This is how you pull in changes \
            from teammates and keep your stack up to date.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Start of day: sync with remote
    lt sync
    lt restack                   # align your stack with updated trunk

    # Sync and restack in one step
    lt sync --restack

    # Force update trunk even if it diverged
    lt sync --force

TYPICAL DAILY WORKFLOW:
    lt sync                      # pull latest changes
    lt restack                   # update your stack
    lt log                       # see current state
    # ... continue working ..."
    )]
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
    #[command(
        name = "get",
        long_about = "Fetch a branch or pull request from the remote repository.\n\n\
            Downloads a branch or PR and sets it up for local tracking. By default, \
            fetched branches are frozen to prevent accidentally modifying someone \
            else's work. Use --unfrozen if you intend to modify it.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Fetch a PR by number
    lt get 1234

    # Fetch a branch by name
    lt get feature-branch

    # Fetch and allow modifications
    lt get 1234 --unfrozen

    # Fetch and restack onto it
    lt get 1234 --restack

REVIEWING A TEAMMATE'S PR:
    lt get 1234                  # fetch their PR
    lt log                       # see where it fits
    lt info --diff               # review the changes

BUILDING ON SOMEONE'S WORK:
    lt get 1234 --unfrozen       # fetch and allow changes
    lt create my-addition        # stack your work on top"
    )]
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

        /// Fetch and track without checkout (required for bare repos).
        /// Creates tracking metadata and computes base, but does not
        /// modify working directory. Prints worktree creation guidance.
        #[arg(long)]
        no_checkout: bool,
    },

    /// Merge PRs via GitHub API
    #[command(
        name = "merge",
        long_about = "Merge the current branch's PR via the GitHub API.\n\n\
            Triggers a merge on GitHub for the PR associated with the current \
            branch. After merging, run 'lt sync' to update your local state \
            and clean up merged branches.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Merge the current branch's PR
    lt merge

    # Choose merge method
    lt merge --method squash
    lt merge --method rebase

    # Preview what would be merged
    lt merge --dry-run

    # Merge with confirmation prompt
    lt merge --confirm

AFTER MERGING:
    lt sync                      # update local state
    lt delete merged-branch      # clean up (if not auto-deleted)"
    )]
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
    #[command(
        name = "pr",
        long_about = "Display or open the pull request URL for a branch.\n\n\
            Shows the GitHub PR URL for the current branch or a specified branch. \
            If your system supports it, can open the URL directly in your browser.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Show PR URL for current branch
    lt pr

    # Show URLs for entire stack
    lt pr --stack

    # Show PR URL for specific branch
    lt pr feature-auth

QUICK ACCESS:
    # After submitting, quickly open in browser
    lt submit
    lt pr                        # shows URL to copy/open"
    )]
    Pr {
        /// Branch or PR number (defaults to current)
        target: Option<String>,

        /// Show URLs for entire stack
        #[arg(long)]
        stack: bool,
    },

    /// Remove PR linkage from branch metadata
    #[command(
        name = "unlink",
        long_about = "Remove the PR association from a branch's metadata.\n\n\
            Disconnects a branch from its linked PR without closing the PR on \
            GitHub. Useful if you want to create a fresh PR or if the linkage \
            became stale.",
        after_help = "\
WORKFLOW EXAMPLES:
    # Unlink current branch from its PR
    lt unlink

    # Unlink a specific branch
    lt unlink old-feature

WHEN TO USE:
    - The PR was closed/recreated and you need fresh linkage
    - You want to submit this branch as a new PR
    - The PR metadata became corrupted"
    )]
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
