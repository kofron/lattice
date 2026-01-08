//! cli::commands::auth
//!
//! Authentication command for storing forge credentials.
//!
//! # Design
//!
//! Per SPEC.md Section 8A.1, the auth command:
//! - Stores tokens securely via SecretStore
//! - NEVER prints tokens to stdout/stderr
//! - Supports both interactive and non-interactive modes
//!
//! # Example
//!
//! ```bash
//! # Interactive (prompts for token)
//! lattice auth
//!
//! # Non-interactive
//! lattice auth --token ghp_xxxx
//!
//! # Check status
//! lattice auth --status
//!
//! # Remove stored token
//! lattice auth --logout
//! ```

use crate::engine::Context;
use crate::secrets::{self, SecretStore};
use anyhow::{bail, Context as _, Result};
use std::io::{self, Write};

/// Secret key for GitHub personal access token.
const GITHUB_TOKEN_KEY: &str = "github.pat";

/// Run the auth command.
///
/// # Arguments
///
/// * `ctx` - Engine context with interactive flag
/// * `token` - Optional token provided via --token flag
/// * `host` - Host to authenticate with (only "github" supported in v1)
/// * `status` - If true, show authentication status instead of storing
/// * `logout` - If true, remove stored authentication
///
/// # Security
///
/// This function NEVER prints the token value. It only confirms success/failure.
pub fn auth(
    ctx: &Context,
    token: Option<&str>,
    host: &str,
    status: bool,
    logout: bool,
) -> Result<()> {
    // Only GitHub supported in v1
    if host != "github" {
        bail!(
            "Unsupported host '{}'. Only 'github' is supported in v1.",
            host
        );
    }

    // Get secret store
    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .context("Failed to initialize secret store")?;

    // Handle --status
    if status {
        return show_status(store.as_ref(), host, ctx.quiet);
    }

    // Handle --logout
    if logout {
        return do_logout(store.as_ref(), host, ctx.quiet);
    }

    // Get token value
    let token_value = get_token(ctx, token)?;

    // Validate token format (basic checks)
    validate_token(&token_value)?;

    // Store the token
    store
        .set(GITHUB_TOKEN_KEY, &token_value)
        .context("Failed to store token")?;

    if !ctx.quiet {
        println!("Authentication configured for {}.", host);
    }

    Ok(())
}

/// Show authentication status.
fn show_status(store: &dyn SecretStore, host: &str, quiet: bool) -> Result<()> {
    let exists = store.exists(GITHUB_TOKEN_KEY)?;

    if quiet {
        // Machine-readable output
        if exists {
            println!("authenticated");
        } else {
            println!("not_authenticated");
        }
    } else if exists {
        println!("Authenticated with {}.", host);
        // Note: We intentionally do NOT print the token or any part of it
    } else {
        println!("Not authenticated with {}.", host);
        println!("Run 'lattice auth' to authenticate.");
    }

    Ok(())
}

/// Remove stored authentication.
fn do_logout(store: &dyn SecretStore, host: &str, quiet: bool) -> Result<()> {
    store
        .delete(GITHUB_TOKEN_KEY)
        .context("Failed to remove stored token")?;

    if !quiet {
        println!("Logged out from {}.", host);
    }

    Ok(())
}

/// Get token from argument or interactive prompt.
fn get_token(ctx: &Context, token_arg: Option<&str>) -> Result<String> {
    // If token provided via argument, use it
    if let Some(t) = token_arg {
        return Ok(t.to_string());
    }

    // If not interactive, we need the token argument
    if ctx.quiet || !ctx.interactive {
        bail!("Token required. Use --token <TOKEN> or run interactively.");
    }

    // Interactive prompt with masked input
    print!("GitHub Personal Access Token: ");
    io::stdout().flush()?;

    let token = rpassword::read_password().context("Failed to read token")?;

    if token.is_empty() {
        bail!("Token cannot be empty.");
    }

    Ok(token)
}

/// Validate token format (basic checks).
///
/// We don't validate the token against the API here - that would
/// require network access. We just do basic format validation.
fn validate_token(token: &str) -> Result<()> {
    if token.is_empty() {
        bail!("Token cannot be empty.");
    }

    if token.len() < 10 {
        bail!("Token appears to be too short.");
    }

    // Check for common mistakes
    if token.contains(' ') {
        bail!("Token should not contain spaces.");
    }

    if token.contains('\n') || token.contains('\r') {
        bail!("Token should not contain newlines.");
    }

    Ok(())
}

/// Get the stored GitHub token.
///
/// This is used by other commands that need authentication.
///
/// # Returns
///
/// The stored token if available, or an error if not authenticated.
pub fn get_github_token() -> Result<String> {
    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .context("Failed to initialize secret store")?;

    store
        .get(GITHUB_TOKEN_KEY)?
        .ok_or_else(|| anyhow::anyhow!("Not authenticated. Run 'lattice auth' first."))
}

/// Check if GitHub authentication is available.
///
/// Used by the scanner to set the AuthAvailable capability.
pub fn has_github_token() -> bool {
    let store = match secrets::create_store(secrets::DEFAULT_PROVIDER) {
        Ok(s) => s,
        Err(_) => return false,
    };

    store.exists(GITHUB_TOKEN_KEY).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_token_rejects_empty() {
        assert!(validate_token("").is_err());
    }

    #[test]
    fn validate_token_rejects_short() {
        assert!(validate_token("abc").is_err());
    }

    #[test]
    fn validate_token_rejects_spaces() {
        assert!(validate_token("token with spaces").is_err());
    }

    #[test]
    fn validate_token_rejects_newlines() {
        assert!(validate_token("token\nwith\nnewlines").is_err());
        assert!(validate_token("token\rwith\rcarriage").is_err());
    }

    #[test]
    fn validate_token_accepts_valid() {
        assert!(validate_token("ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx").is_ok());
        assert!(validate_token("github_pat_xxxxxxxxxxxxx").is_ok());
    }

    #[test]
    fn github_token_key_is_correct() {
        assert_eq!(GITHUB_TOKEN_KEY, "github.pat");
    }
}
