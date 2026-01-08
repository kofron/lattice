//! ui
//!
//! User interaction utilities.
//!
//! # Modules
//!
//! - [`prompts`] - Interactive prompts and confirmations
//! - [`output`] - Output formatting and display
//! - [`stack_comment`] - Stack comment generation for PR descriptions
//!
//! # Design
//!
//! The UI module provides a consistent interface for user interaction.
//! All output and prompts go through this module to ensure consistent
//! formatting and proper handling of interactive vs non-interactive modes.

pub mod output;
pub mod prompts;
pub mod stack_comment;
