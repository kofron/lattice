//! ui::output
//!
//! Output formatting and display.
//!
//! # Design
//!
//! Output is formatted consistently and respects the quiet flag.
//! When `--json` is enabled, output is machine-readable JSON.

use std::fmt::Display;

/// Output verbosity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// Quiet mode - minimal output
    Quiet,
    /// Normal mode - standard output
    Normal,
    /// Debug mode - verbose output
    Debug,
}

impl Verbosity {
    /// Create verbosity from flags.
    pub fn from_flags(quiet: bool, debug: bool) -> Self {
        if quiet {
            Verbosity::Quiet
        } else if debug {
            Verbosity::Debug
        } else {
            Verbosity::Normal
        }
    }
}

/// Print a message (respects quiet mode).
pub fn print(message: impl Display, verbosity: Verbosity) {
    if verbosity != Verbosity::Quiet {
        println!("{}", message);
    }
}

/// Print a debug message (only in debug mode).
pub fn debug(message: impl Display, verbosity: Verbosity) {
    if verbosity == Verbosity::Debug {
        eprintln!("[debug] {}", message);
    }
}

/// Print an error message (always shown).
pub fn error(message: impl Display) {
    eprintln!("error: {}", message);
}

/// Print a warning message (respects quiet mode).
pub fn warn(message: impl Display, verbosity: Verbosity) {
    if verbosity != Verbosity::Quiet {
        eprintln!("warning: {}", message);
    }
}

/// Print a success message (respects quiet mode).
pub fn success(message: impl Display, verbosity: Verbosity) {
    if verbosity != Verbosity::Quiet {
        println!("{}", message);
    }
}

/// Format a branch name for display.
pub fn format_branch(name: &str) -> String {
    name.to_string()
}

/// Format a list of items.
pub fn format_list<T: Display>(items: &[T], prefix: &str) -> String {
    items
        .iter()
        .map(|item| format!("{}{}", prefix, item))
        .collect::<Vec<_>>()
        .join("\n")
}
