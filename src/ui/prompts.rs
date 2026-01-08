//! ui::prompts
//!
//! Interactive prompts and confirmations.
//!
//! # Design
//!
//! Prompts are only shown in interactive mode. In non-interactive mode,
//! operations requiring user input must either have defaults or fail
//! with a clear error message.

use thiserror::Error;

/// Errors from prompts.
#[derive(Debug, Error)]
pub enum PromptError {
    #[error("prompt cancelled by user")]
    Cancelled,

    #[error("not in interactive mode")]
    NotInteractive,

    #[error("IO error: {0}")]
    IoError(String),
}

/// Prompt for confirmation (yes/no).
///
/// Returns `Ok(true)` if the user confirms, `Ok(false)` if they decline.
/// Returns `Err(PromptError::NotInteractive)` if not in interactive mode.
///
/// This is a stub implementation for Milestone 0.
pub fn confirm(_message: &str, _default: bool, interactive: bool) -> Result<bool, PromptError> {
    if !interactive {
        return Err(PromptError::NotInteractive);
    }
    // Stub: always return true
    Ok(true)
}

/// Prompt for text input.
///
/// Returns the entered text, or `None` if cancelled.
///
/// This is a stub implementation for Milestone 0.
pub fn input(
    _message: &str,
    _default: Option<&str>,
    interactive: bool,
) -> Result<String, PromptError> {
    if !interactive {
        return Err(PromptError::NotInteractive);
    }
    // Stub: return empty string
    Ok(String::new())
}

/// Prompt to select from a list of options.
///
/// Returns the index of the selected option.
///
/// This is a stub implementation for Milestone 0.
pub fn select<T: AsRef<str>>(
    _message: &str,
    _options: &[T],
    _default: Option<usize>,
    interactive: bool,
) -> Result<usize, PromptError> {
    if !interactive {
        return Err(PromptError::NotInteractive);
    }
    // Stub: return first option
    Ok(0)
}

/// Prompt for masked input (e.g., passwords, tokens).
///
/// The input is not echoed to the terminal.
///
/// This is a stub implementation for Milestone 0.
pub fn password(_message: &str, interactive: bool) -> Result<String, PromptError> {
    if !interactive {
        return Err(PromptError::NotInteractive);
    }
    // Stub: return empty string
    Ok(String::new())
}
