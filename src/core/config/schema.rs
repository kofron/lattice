//! core::config::schema
//!
//! Configuration schema types.
//!
//! # Global Config
//!
//! Located at (in order of precedence):
//! 1. `$LATTICE_CONFIG` if set
//! 2. `$XDG_CONFIG_HOME/lattice/config.toml`
//! 3. `~/.lattice/config.toml` (canonical write location)
//!
//! # Repo Config
//!
//! Located at `.git/lattice/config.toml` (canonical).
//!
//! # Validation
//!
//! Config values are validated after parsing to ensure they conform to
//! expected formats (e.g., trunk must be a valid branch name).

use serde::{Deserialize, Serialize};

use super::ConfigError;
use crate::core::types::BranchName;

/// Global configuration (user scope).
///
/// # Example
///
/// ```toml
/// default_forge = "github"
/// interactive = true
/// verify_hooks = true
///
/// [submit]
/// draft = false
/// restack = true
///
/// [secrets]
/// provider = "file"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalConfig {
    /// Default forge (e.g., "github")
    pub default_forge: Option<String>,

    /// Default interactive mode
    pub interactive: Option<bool>,

    /// Hook verification default
    pub verify_hooks: Option<bool>,

    /// Submit defaults
    pub submit: Option<SubmitDefaults>,

    /// Secret storage settings
    pub secrets: Option<SecretsConfig>,
}

impl GlobalConfig {
    /// Validate the configuration values.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::InvalidValue` if any value is invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate forge if specified
        if let Some(forge) = &self.default_forge {
            let valid_forges = crate::forge::valid_forge_names();
            if !valid_forges.contains(&forge.as_str()) {
                return Err(ConfigError::InvalidValue(format!(
                    "invalid forge '{}', must be one of: {}",
                    forge,
                    valid_forges.join(", ")
                )));
            }
        }

        // Validate secrets provider if specified
        if let Some(secrets) = &self.secrets {
            secrets.validate()?;
        }

        Ok(())
    }
}

/// Repository configuration.
///
/// # Example
///
/// ```toml
/// trunk = "main"
/// remote = "origin"
/// sync_metadata_refs = false
///
/// [forge_repo]
/// owner = "myorg"
/// repo = "myrepo"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct RepoConfig {
    /// Trunk branch name
    pub trunk: Option<String>,

    /// Remote name (default: "origin")
    pub remote: Option<String>,

    /// Whether to sync metadata refs
    pub sync_metadata_refs: Option<bool>,

    /// Forge-specific repository identification
    pub forge_repo: Option<ForgeRepoConfig>,
}

impl RepoConfig {
    /// Validate the configuration values.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::InvalidValue` if any value is invalid.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate trunk is a valid branch name if specified
        if let Some(trunk) = &self.trunk {
            BranchName::new(trunk).map_err(|e| {
                ConfigError::InvalidValue(format!("invalid trunk branch name: {}", e))
            })?;
        }

        // Validate remote is non-empty if specified
        if let Some(remote) = &self.remote {
            if remote.is_empty() {
                return Err(ConfigError::InvalidValue(
                    "remote cannot be empty".to_string(),
                ));
            }
        }

        Ok(())
    }
}

/// Submit command defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SubmitDefaults {
    /// Default to draft PRs
    pub draft: Option<bool>,

    /// Default to restack before submit
    pub restack: Option<bool>,

    /// Default reviewers
    pub reviewers: Option<Vec<String>>,
}

/// Secrets configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SecretsConfig {
    /// Provider to use ("file" or "keychain")
    pub provider: Option<String>,
}

impl SecretsConfig {
    /// Valid secret providers.
    pub const VALID_PROVIDERS: &'static [&'static str] = &["file", "keychain"];

    /// Validate the secrets configuration.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if let Some(provider) = &self.provider {
            if !Self::VALID_PROVIDERS.contains(&provider.as_str()) {
                return Err(ConfigError::InvalidValue(format!(
                    "invalid secrets provider '{}', must be one of: {}",
                    provider,
                    Self::VALID_PROVIDERS.join(", ")
                )));
            }
        }
        Ok(())
    }
}

/// Forge-specific repository configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ForgeRepoConfig {
    /// Override owner/org
    pub owner: Option<String>,

    /// Override repository name
    pub repo: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    mod global_config {
        use super::*;

        #[test]
        fn defaults() {
            let config = GlobalConfig::default();
            assert!(config.default_forge.is_none());
            assert!(config.interactive.is_none());
            assert!(config.verify_hooks.is_none());
        }

        #[test]
        fn valid_forge() {
            let config = GlobalConfig {
                default_forge: Some("github".to_string()),
                ..Default::default()
            };
            assert!(config.validate().is_ok());
        }

        #[test]
        fn valid_gitlab_forge() {
            // GitLab is now a valid forge (stub implementation)
            let config = GlobalConfig {
                default_forge: Some("gitlab".to_string()),
                ..Default::default()
            };
            assert!(config.validate().is_ok());
        }

        #[test]
        fn invalid_forge() {
            let config = GlobalConfig {
                default_forge: Some("bitbucket".to_string()), // Not supported
                ..Default::default()
            };
            assert!(config.validate().is_err());
        }

        #[test]
        fn roundtrip() {
            let config = GlobalConfig {
                default_forge: Some("github".to_string()),
                interactive: Some(true),
                verify_hooks: Some(false),
                submit: Some(SubmitDefaults {
                    draft: Some(true),
                    restack: Some(true),
                    reviewers: Some(vec!["alice".to_string()]),
                }),
                secrets: Some(SecretsConfig {
                    provider: Some("file".to_string()),
                }),
            };

            let toml = toml::to_string_pretty(&config).unwrap();
            let parsed: GlobalConfig = toml::from_str(&toml).unwrap();
            assert_eq!(config, parsed);
        }
    }

    mod repo_config {
        use super::*;

        #[test]
        fn defaults() {
            let config = RepoConfig::default();
            assert!(config.trunk.is_none());
            assert!(config.remote.is_none());
        }

        #[test]
        fn valid_trunk() {
            let config = RepoConfig {
                trunk: Some("main".to_string()),
                ..Default::default()
            };
            assert!(config.validate().is_ok());
        }

        #[test]
        fn invalid_trunk() {
            let config = RepoConfig {
                trunk: Some("invalid..name".to_string()),
                ..Default::default()
            };
            assert!(config.validate().is_err());
        }

        #[test]
        fn empty_remote_rejected() {
            let config = RepoConfig {
                remote: Some("".to_string()),
                ..Default::default()
            };
            assert!(config.validate().is_err());
        }

        #[test]
        fn roundtrip() {
            let config = RepoConfig {
                trunk: Some("main".to_string()),
                remote: Some("origin".to_string()),
                sync_metadata_refs: Some(false),
                forge_repo: Some(ForgeRepoConfig {
                    owner: Some("myorg".to_string()),
                    repo: Some("myrepo".to_string()),
                }),
            };

            let toml = toml::to_string_pretty(&config).unwrap();
            let parsed: RepoConfig = toml::from_str(&toml).unwrap();
            assert_eq!(config, parsed);
        }

        #[test]
        fn reject_unknown_fields() {
            let toml = r#"
                trunk = "main"
                unknown_field = true
            "#;

            let result: Result<RepoConfig, _> = toml::from_str(toml);
            assert!(result.is_err());
        }
    }

    mod secrets_config {
        use super::*;

        #[test]
        fn valid_file_provider() {
            let config = SecretsConfig {
                provider: Some("file".to_string()),
            };
            assert!(config.validate().is_ok());
        }

        #[test]
        fn valid_keychain_provider() {
            let config = SecretsConfig {
                provider: Some("keychain".to_string()),
            };
            assert!(config.validate().is_ok());
        }

        #[test]
        fn invalid_provider() {
            let config = SecretsConfig {
                provider: Some("invalid".to_string()),
            };
            assert!(config.validate().is_err());
        }
    }
}
