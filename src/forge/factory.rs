//! forge::factory
//!
//! Forge selection and creation.
//!
//! # Design
//!
//! This module provides a central location for forge selection logic.
//! Commands use `create_forge()` instead of directly importing specific
//! forge implementations, ensuring the architecture boundary is maintained.
//!
//! Per ARCHITECTURE.md Section 11:
//! > "The adapter boundary ensures core logic remains independent of
//! > specific forge implementations."
//!
//! # Provider Detection
//!
//! The factory can detect the appropriate forge from a remote URL:
//! - GitHub URLs (`github.com`) → `GitHubForge`
//! - GitLab URLs (`gitlab.com`) → `GitLabForge` (when feature enabled)
//!
//! # Example
//!
//! ```ignore
//! use latticework::forge::{create_forge, ForgeProvider};
//!
//! // Auto-detect from URL
//! let forge = create_forge(
//!     "git@github.com:owner/repo.git",
//!     "ghp_token",
//!     None,  // No override
//! )?;
//!
//! // Or explicitly specify provider
//! let forge = create_forge(
//!     "git@github.com:owner/repo.git",
//!     "ghp_token",
//!     Some("github"),
//! )?;
//! ```

use super::github::{parse_github_url, GitHubForge};
use super::traits::{Forge, ForgeError};

#[cfg(feature = "gitlab")]
use super::gitlab::{parse_gitlab_url, GitLabForge};

/// Supported forge providers.
///
/// This enum represents the available forge backends. Use `ForgeProvider::all()`
/// to get a list of available providers for the current build configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeProvider {
    /// GitHub (always available)
    GitHub,
    /// GitLab (requires `gitlab` feature)
    #[cfg(feature = "gitlab")]
    GitLab,
}

impl ForgeProvider {
    /// Get all available providers.
    ///
    /// Returns providers enabled in the current build configuration.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::ForgeProvider;
    ///
    /// let providers = ForgeProvider::all();
    /// assert!(providers.contains(&ForgeProvider::GitHub));
    /// ```
    pub fn all() -> &'static [ForgeProvider] {
        &[
            ForgeProvider::GitHub,
            #[cfg(feature = "gitlab")]
            ForgeProvider::GitLab,
        ]
    }

    /// Get the provider name as a string.
    ///
    /// This matches the name used in configuration files.
    pub fn name(&self) -> &'static str {
        match self {
            ForgeProvider::GitHub => "github",
            #[cfg(feature = "gitlab")]
            ForgeProvider::GitLab => "gitlab",
        }
    }

    /// Parse a provider from a string.
    ///
    /// # Returns
    ///
    /// `Some(ForgeProvider)` if the string matches a known provider,
    /// `None` otherwise.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::forge::ForgeProvider;
    ///
    /// assert_eq!(ForgeProvider::parse("github"), Some(ForgeProvider::GitHub));
    /// assert_eq!(ForgeProvider::parse("unknown"), None);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "github" => Some(ForgeProvider::GitHub),
            #[cfg(feature = "gitlab")]
            "gitlab" => Some(ForgeProvider::GitLab),
            _ => None,
        }
    }
}

impl std::fmt::Display for ForgeProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Detect the forge provider from a remote URL.
///
/// Examines the URL to determine which forge it belongs to.
///
/// # Arguments
///
/// * `remote_url` - Git remote URL (SSH or HTTPS format)
///
/// # Returns
///
/// `Some(ForgeProvider)` if the URL matches a known forge, `None` otherwise.
///
/// # Example
///
/// ```
/// use latticework::forge::{detect_provider, ForgeProvider};
///
/// assert_eq!(
///     detect_provider("git@github.com:owner/repo.git"),
///     Some(ForgeProvider::GitHub)
/// );
/// ```
pub fn detect_provider(remote_url: &str) -> Option<ForgeProvider> {
    // Try GitHub first (most common)
    if parse_github_url(remote_url).is_some() {
        return Some(ForgeProvider::GitHub);
    }

    // Try GitLab if feature enabled
    #[cfg(feature = "gitlab")]
    if parse_gitlab_url(remote_url).is_some() {
        return Some(ForgeProvider::GitLab);
    }

    None
}

/// Create a forge from a remote URL and token.
///
/// This is the primary entry point for creating forge instances in commands.
/// It handles provider detection and selection, ensuring commands don't need
/// to import specific forge implementations.
///
/// # Arguments
///
/// * `remote_url` - Git remote URL (SSH or HTTPS format)
/// * `token` - Authentication token for the forge
/// * `provider_override` - Optional provider name to use instead of auto-detection
///
/// # Returns
///
/// A boxed `Forge` trait object on success.
///
/// # Errors
///
/// - `ForgeError::NotImplemented` if the provider is not supported or not enabled
/// - `ForgeError::NotFound` if the URL cannot be parsed for the provider
///
/// # Example
///
/// ```ignore
/// use latticework::forge::create_forge;
///
/// // Auto-detect from URL
/// let forge = create_forge(
///     "git@github.com:owner/repo.git",
///     "ghp_token",
///     None,
/// )?;
///
/// // Use forge...
/// let pr = forge.create_pr(request).await?;
/// ```
pub fn create_forge(
    remote_url: &str,
    token: &str,
    provider_override: Option<&str>,
) -> Result<Box<dyn Forge>, ForgeError> {
    // Determine provider: override or auto-detect
    let provider = if let Some(name) = provider_override {
        resolve_provider_override(name)?
    } else {
        detect_provider(remote_url).ok_or_else(|| {
            ForgeError::NotFound(format!(
                "Could not detect forge provider from remote URL: {}. \
                 Supported forges: {}",
                remote_url,
                available_providers_string()
            ))
        })?
    };

    // Create the appropriate forge
    create_forge_for_provider(provider, remote_url, token)
}

/// Resolve a provider override string to a ForgeProvider.
fn resolve_provider_override(name: &str) -> Result<ForgeProvider, ForgeError> {
    // Check if it's a known provider name
    if let Some(provider) = ForgeProvider::parse(name) {
        return Ok(provider);
    }

    // Check if it's a provider that exists but isn't enabled
    if is_known_but_disabled(name) {
        return Err(ForgeError::NotImplemented(format!(
            "Forge '{}' is not enabled in this build. \
             Rebuild with `--features {}` to enable it.",
            name, name
        )));
    }

    // Unknown provider
    Err(ForgeError::NotFound(format!(
        "Unknown forge provider '{}'. Available providers: {}",
        name,
        available_providers_string()
    )))
}

/// Check if a provider name is known but disabled.
fn is_known_but_disabled(name: &str) -> bool {
    match name.to_lowercase().as_str() {
        #[cfg(not(feature = "gitlab"))]
        "gitlab" => true,
        _ => false,
    }
}

/// Create a forge for a specific provider.
fn create_forge_for_provider(
    provider: ForgeProvider,
    remote_url: &str,
    token: &str,
) -> Result<Box<dyn Forge>, ForgeError> {
    match provider {
        ForgeProvider::GitHub => {
            let forge = GitHubForge::from_remote_url(remote_url, token).ok_or_else(|| {
                ForgeError::NotFound(format!(
                    "Could not parse '{}' as a GitHub URL. \
                     Expected format: git@github.com:owner/repo.git or https://github.com/owner/repo.git",
                    remote_url
                ))
            })?;
            Ok(Box::new(forge))
        }
        #[cfg(feature = "gitlab")]
        ForgeProvider::GitLab => {
            let forge = GitLabForge::from_remote_url(remote_url, token).ok_or_else(|| {
                ForgeError::NotFound(format!(
                    "Could not parse '{}' as a GitLab URL. \
                     Expected format: git@gitlab.com:owner/project.git or https://gitlab.com/owner/project.git",
                    remote_url
                ))
            })?;
            Ok(Box::new(forge))
        }
    }
}

/// Get a comma-separated string of available providers.
fn available_providers_string() -> String {
    ForgeProvider::all()
        .iter()
        .map(|p| p.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Get list of valid forge names for configuration validation.
///
/// This is used by the config schema to validate forge settings.
/// It includes all known forges, not just enabled ones, so that
/// configuration can be validated before features are enabled.
pub fn valid_forge_names() -> &'static [&'static str] {
    // Include all known forges for config validation
    // This allows users to configure gitlab even before enabling the feature
    &["github", "gitlab"]
}

#[cfg(test)]
mod tests {
    use super::*;

    mod forge_provider {
        use super::*;

        #[test]
        fn all_includes_github() {
            let all = ForgeProvider::all();
            assert!(all.contains(&ForgeProvider::GitHub));
        }

        #[test]
        fn name_returns_lowercase() {
            assert_eq!(ForgeProvider::GitHub.name(), "github");
        }

        #[test]
        fn parse_github() {
            assert_eq!(ForgeProvider::parse("github"), Some(ForgeProvider::GitHub));
            assert_eq!(ForgeProvider::parse("GitHub"), Some(ForgeProvider::GitHub));
            assert_eq!(ForgeProvider::parse("GITHUB"), Some(ForgeProvider::GitHub));
        }

        #[test]
        fn parse_unknown() {
            assert_eq!(ForgeProvider::parse("unknown"), None);
            assert_eq!(ForgeProvider::parse(""), None);
        }

        #[test]
        fn display() {
            assert_eq!(format!("{}", ForgeProvider::GitHub), "github");
        }

        #[cfg(feature = "gitlab")]
        #[test]
        fn all_includes_gitlab() {
            let all = ForgeProvider::all();
            assert!(all.contains(&ForgeProvider::GitLab));
        }

        #[cfg(feature = "gitlab")]
        #[test]
        fn parse_gitlab() {
            assert_eq!(ForgeProvider::parse("gitlab"), Some(ForgeProvider::GitLab));
        }
    }

    mod detect_provider {
        use super::*;

        #[test]
        fn github_ssh() {
            assert_eq!(
                detect_provider("git@github.com:owner/repo.git"),
                Some(ForgeProvider::GitHub)
            );
        }

        #[test]
        fn github_https() {
            assert_eq!(
                detect_provider("https://github.com/owner/repo.git"),
                Some(ForgeProvider::GitHub)
            );
        }

        #[test]
        fn unknown_url() {
            assert_eq!(detect_provider("git@unknown.com:owner/repo.git"), None);
        }

        #[cfg(feature = "gitlab")]
        #[test]
        fn gitlab_ssh() {
            assert_eq!(
                detect_provider("git@gitlab.com:owner/project.git"),
                Some(ForgeProvider::GitLab)
            );
        }

        #[cfg(feature = "gitlab")]
        #[test]
        fn gitlab_https() {
            assert_eq!(
                detect_provider("https://gitlab.com/owner/project.git"),
                Some(ForgeProvider::GitLab)
            );
        }
    }

    mod create_forge {
        use super::*;

        #[test]
        fn github_url_auto_detect() {
            let result = create_forge("git@github.com:owner/repo.git", "token", None);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().name(), "github");
        }

        #[test]
        fn github_url_explicit_override() {
            let result = create_forge("git@github.com:owner/repo.git", "token", Some("github"));
            assert!(result.is_ok());
            assert_eq!(result.unwrap().name(), "github");
        }

        #[test]
        fn unknown_url_returns_error() {
            let result = create_forge("git@unknown.com:owner/repo.git", "token", None);
            assert!(matches!(result, Err(ForgeError::NotFound(_))));
        }

        #[test]
        fn unknown_provider_override_returns_error() {
            let result = create_forge(
                "git@github.com:owner/repo.git",
                "token",
                Some("unknown_forge"),
            );
            assert!(matches!(result, Err(ForgeError::NotFound(_))));
        }

        #[cfg(not(feature = "gitlab"))]
        #[test]
        fn gitlab_override_without_feature_returns_not_implemented() {
            let result = create_forge("git@github.com:owner/repo.git", "token", Some("gitlab"));
            assert!(matches!(result, Err(ForgeError::NotImplemented(_))));

            // Error message should mention the feature flag
            if let Err(ForgeError::NotImplemented(msg)) = result {
                assert!(msg.contains("--features gitlab"));
            }
        }

        #[cfg(feature = "gitlab")]
        #[test]
        fn gitlab_url_auto_detect() {
            let result = create_forge("git@gitlab.com:owner/project.git", "token", None);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().name(), "gitlab");
        }

        #[cfg(feature = "gitlab")]
        #[test]
        fn gitlab_url_explicit_override() {
            let result = create_forge("git@gitlab.com:owner/project.git", "token", Some("gitlab"));
            assert!(result.is_ok());
            assert_eq!(result.unwrap().name(), "gitlab");
        }
    }

    mod valid_forge_names {
        use super::*;

        #[test]
        fn includes_github() {
            assert!(valid_forge_names().contains(&"github"));
        }

        #[test]
        fn includes_gitlab() {
            // GitLab should be in valid names even without feature
            // This allows config validation before enabling feature
            assert!(valid_forge_names().contains(&"gitlab"));
        }
    }
}
