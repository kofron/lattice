//! cli
//!
//! Command-line interface layer for Lattice.
//!
//! # Responsibilities
//!
//! - Parse command-line arguments and global flags
//! - Delegate to command handlers
//! - Does NOT perform repository mutations directly
//!
//! # Architecture
//!
//! The CLI layer is thin. It parses arguments via clap and dispatches to the
//! [`crate::engine`] for execution. All repository state changes flow through
//! the engine's validated execution model.

pub mod args;
pub mod commands;

pub use args::{Cli, Shell};

use crate::engine;
use anyhow::Result;

/// Run the CLI application.
///
/// This is the main entry point called from `main.rs`.
pub fn run() -> Result<()> {
    let cli = Cli::parse_args();

    // Create context from CLI flags.
    // Note: verify defaults to true (hooks honored) per ARCHITECTURE.md §10.2.
    // Config-based defaults could be added later, but CLI flag always takes precedence.
    let ctx = engine::Context {
        cwd: cli.cwd.clone(),
        debug: cli.debug,
        quiet: cli.quiet,
        interactive: cli.interactive(),
        verify: cli.verify_flag().unwrap_or(true),
    };

    // Dispatch to command handler
    commands::dispatch(cli.command, &ctx)
}
