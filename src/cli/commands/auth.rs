//! cli::commands::auth
//!
//! GitHub App OAuth authentication commands.
//!
//! # Design
//!
//! Per SPEC.md Section 8A.1, the auth command implements GitHub App device flow OAuth:
//! - `login`: Initiate device flow, display code, poll for authorization
//! - `status`: Show logged-in hosts and user info (never tokens)
//! - `logout`: Delete stored tokens
//!
//! # Security
//!
//! Per SPEC.md Section 4.4.4, tokens MUST never appear in:
//! - stdout/stderr
//! - log output
//! - error messages
//!
//! # Example
//!
//! ```bash
//! # Login with device flow
//! lattice auth login
//!
//! # Check status
//! lattice auth status
//!
//! # Remove stored authentication
//! lattice auth logout
//! ```

use crate::auth::{DeviceFlowClient, GitHubAuthManager, TokenBundle, TokenInfo, UserInfo};
use crate::engine::Context;
use crate::secrets;
use anyhow::{Context as _, Result};
use chrono::Utc;

/// Default GitHub host.
const DEFAULT_HOST: &str = "github.com";

/// Run the auth command.
///
/// # Arguments
///
/// * `ctx` - Engine context with interactive flag
/// * `host` - Host to authenticate with (default: github.com)
/// * `no_browser` - Do not attempt to open browser
/// * `status` - If true, show authentication status instead of login
/// * `logout` - If true, remove stored authentication
///
/// # Security
///
/// This function NEVER prints token values. It only confirms success/failure.
pub fn auth(ctx: &Context, host: &str, no_browser: bool, status: bool, logout: bool) -> Result<()> {
    // Normalize host
    let host = if host.is_empty() || host == "github" {
        DEFAULT_HOST
    } else {
        host
    };

    // Handle --status
    if status {
        return show_status(host, ctx.quiet);
    }

    // Handle --logout
    if logout {
        return do_logout(host, ctx.quiet);
    }

    // Default: login with device flow
    let rt = tokio::runtime::Runtime::new().context("Failed to create async runtime")?;
    rt.block_on(do_login(ctx, host, no_browser))
}

/// Perform device flow login.
async fn do_login(ctx: &Context, host: &str, no_browser: bool) -> Result<()> {
    let client = DeviceFlowClient::new(host);

    // Step 1: Request device code
    if !ctx.quiet {
        println!("Requesting device code from {}...", host);
    }

    let device_code = client
        .request_device_code()
        .await
        .context("Failed to request device code")?;

    // Step 2: Display instructions
    println!();
    println!("To authenticate, visit:");
    println!("  {}", device_code.verification_uri);
    println!();
    println!("And enter this code:");
    println!("  {}", device_code.user_code);
    println!();

    // Step 3: Optionally open browser
    if !no_browser {
        if let Err(e) = open::that(&device_code.verification_uri) {
            if !ctx.quiet {
                eprintln!("Could not open browser automatically: {}", e);
                eprintln!("Please open the URL manually.");
            }
        }
    }

    // Step 4: Poll for token
    if !ctx.quiet {
        println!("Waiting for authorization...");
    }

    let token_response = client
        .poll_for_token(&device_code)
        .await
        .context("Authorization failed")?;

    // Step 5: Fetch user info
    let user_info_response = client
        .fetch_user_info(&token_response.access_token)
        .await
        .context("Failed to fetch user info")?;

    let user = UserInfo {
        id: user_info_response.id,
        login: user_info_response.login,
    };

    // Step 6: Create and store token bundle
    let tokens = TokenInfo::new(
        token_response.access_token,
        token_response.expires_in,
        token_response.refresh_token,
        token_response.refresh_token_expires_in,
    );

    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .context("Failed to initialize secret store")?;

    let manager = GitHubAuthManager::new(host, store);
    manager
        .store_tokens(user.clone(), tokens)
        .context("Failed to store tokens")?;

    // Step 7: Success message
    println!();
    println!("Authenticated as {} for {}.", user.login, host);

    Ok(())
}

/// Show authentication status.
fn show_status(host: &str, quiet: bool) -> Result<()> {
    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .context("Failed to initialize secret store")?;

    let key = TokenBundle::secret_key(host);
    let bundle_json = store.get(&key).context("Failed to read secret store")?;

    match bundle_json {
        Some(json) => {
            let bundle = TokenBundle::parse(&json).context("Failed to parse token bundle")?;

            if quiet {
                println!("authenticated");
            } else {
                println!("Host: {}", bundle.host);
                println!("User: {} (id: {})", bundle.user.login, bundle.user.id);
                println!();

                // Show expiry info
                let now = Utc::now();
                let access_expires = bundle.tokens.access_token_expires_at;
                let refresh_expires = bundle.tokens.refresh_token_expires_at;

                if access_expires > now {
                    let remaining = access_expires - now;
                    println!(
                        "Access token expires: {} ({} remaining)",
                        access_expires.format("%Y-%m-%d %H:%M:%S UTC"),
                        format_duration(remaining)
                    );
                } else {
                    println!("Access token: expired (will refresh automatically)");
                }

                if refresh_expires > now {
                    let remaining = refresh_expires - now;
                    println!(
                        "Refresh token expires: {} ({} remaining)",
                        refresh_expires.format("%Y-%m-%d %H:%M:%S UTC"),
                        format_duration(remaining)
                    );
                } else {
                    println!("Refresh token: expired (re-authentication required)");
                }

                // Check overall validity
                if bundle.is_valid() {
                    println!();
                    println!("Status: authenticated");
                } else {
                    println!();
                    println!("Status: expired - run 'lattice auth login' to re-authenticate");
                }
            }
        }
        None => {
            if quiet {
                println!("not_authenticated");
            } else {
                println!("Not authenticated for {}.", host);
                println!("Run 'lattice auth login' to authenticate.");
            }
        }
    }

    Ok(())
}

/// Format a chrono::Duration for display.
fn format_duration(duration: chrono::Duration) -> String {
    let total_seconds = duration.num_seconds();
    if total_seconds < 0 {
        return "expired".to_string();
    }

    let days = total_seconds / 86400;
    let hours = (total_seconds % 86400) / 3600;
    let minutes = (total_seconds % 3600) / 60;

    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    }
}

/// Remove stored authentication.
fn do_logout(host: &str, quiet: bool) -> Result<()> {
    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .context("Failed to initialize secret store")?;

    let manager = GitHubAuthManager::new(host, store);
    manager.delete_tokens().context("Failed to remove tokens")?;

    if !quiet {
        println!("Logged out from {}.", host);
    }

    Ok(())
}

/// Check if GitHub App OAuth authentication is available.
///
/// Used by the scanner to set the AuthAvailable capability.
pub fn has_github_token() -> bool {
    crate::auth::has_github_auth(DEFAULT_HOST)
}

/// Get the stored GitHub token for API calls.
///
/// This function retrieves the access token from the stored token bundle.
/// If the token needs refresh, this will NOT refresh it - use `get_auth_manager()`
/// and call `bearer_token()` for automatic refresh.
///
/// # Returns
///
/// The stored access token if available.
///
/// # Errors
///
/// Returns an error if not authenticated or if the token cannot be loaded.
pub fn get_github_token() -> Result<String> {
    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .context("Failed to initialize secret store")?;

    let key = TokenBundle::secret_key(DEFAULT_HOST);
    let json = store
        .get(&key)
        .context("Failed to read secret store")?
        .ok_or_else(|| anyhow::anyhow!("Not authenticated. Run 'lattice auth login' first."))?;

    let bundle = TokenBundle::parse(&json).context("Failed to parse token bundle")?;

    if !bundle.is_valid() {
        anyhow::bail!("Authentication expired. Run 'lattice auth login' again.");
    }

    Ok(bundle.tokens.access_token)
}

/// Get an auth manager for the default host.
///
/// Used by commands that need to make authenticated API calls with
/// automatic token refresh via the `TokenProvider` trait.
#[allow(dead_code)] // Will be used when commands migrate to TokenProvider
pub fn get_auth_manager() -> Result<GitHubAuthManager> {
    let store = secrets::create_store(secrets::DEFAULT_PROVIDER)
        .context("Failed to initialize secret store")?;
    Ok(GitHubAuthManager::new(DEFAULT_HOST, store))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_host_is_github_com() {
        assert_eq!(DEFAULT_HOST, "github.com");
    }

    #[test]
    fn format_duration_days() {
        let duration = chrono::Duration::days(5) + chrono::Duration::hours(3);
        assert_eq!(format_duration(duration), "5d 3h");
    }

    #[test]
    fn format_duration_hours() {
        let duration = chrono::Duration::hours(2) + chrono::Duration::minutes(30);
        assert_eq!(format_duration(duration), "2h 30m");
    }

    #[test]
    fn format_duration_minutes() {
        let duration = chrono::Duration::minutes(45);
        assert_eq!(format_duration(duration), "45m");
    }

    #[test]
    fn format_duration_negative() {
        let duration = chrono::Duration::seconds(-100);
        assert_eq!(format_duration(duration), "expired");
    }

    #[test]
    fn has_github_token_function_exists() {
        // Just verify the function is callable
        let _ = has_github_token();
    }
}
