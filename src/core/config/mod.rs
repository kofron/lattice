//! core::config
//!
//! Configuration schema and loading.
//!
//! # Overview
//!
//! Lattice has two configuration scopes:
//! - **Global**: User-level settings
//! - **Repo**: Repository-level overrides
//!
//! # Precedence
//!
//! Configuration values are resolved in this order (later overrides earlier):
//! 1. Default values
//! 2. Global config file
//! 3. Repo config file
//! 4. CLI flags (not handled here)
//!
//! # Global Config Locations
//!
//! Searched in order:
//! 1. `$LATTICE_CONFIG` if set
//! 2. `$XDG_CONFIG_HOME/lattice/config.toml`
//! 3. `~/.lattice/config.toml` (canonical write location)
//!
//! # Repo Config Locations
//!
//! Searched in order:
//! 1. `.git/lattice/config.toml` (canonical)
//! 2. `.git/lattice/repo.toml` (compatibility, warns)
//! 3. `.lattice/repo.toml` (compatibility, warns)
//!
//! # Example
//!
//! ```no_run
//! use latticework::core::config::Config;
//! use std::path::Path;
//!
//! // Load config for a repository
//! let result = Config::load(Some(Path::new("/path/to/repo"))).unwrap();
//! let config = result.config;
//!
//! // Access configuration values with precedence applied
//! if let Some(trunk) = config.trunk() {
//!     println!("Trunk branch: {}", trunk);
//! }
//! println!("Remote: {}", config.remote());
//! println!("Interactive: {}", config.interactive());
//! ```

pub mod schema;

pub use schema::{GlobalConfig, RepoConfig};

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors from configuration operations.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config file '{path}': {source}")]
    ReadError {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse config file '{path}': {message}")]
    ParseError { path: PathBuf, message: String },

    #[error("failed to write config file '{path}': {source}")]
    WriteError {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("invalid config value: {0}")]
    InvalidValue(String),

    #[error("home directory not found")]
    NoHomeDir,
}

/// Warnings generated during config loading.
#[derive(Debug, Clone)]
pub struct ConfigWarning {
    /// The warning message.
    pub message: String,
    /// The path that triggered the warning.
    pub path: PathBuf,
}

/// Result of loading configuration.
#[derive(Debug)]
pub struct ConfigLoadResult {
    /// The loaded configuration.
    pub config: Config,
    /// Any warnings generated during loading.
    pub warnings: Vec<ConfigWarning>,
}

/// Merged configuration from all sources.
///
/// This struct provides accessor methods that apply precedence rules
/// automatically. Repo config overrides global config.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Global configuration
    pub global: GlobalConfig,
    /// Repository configuration (if in a repo)
    pub repo: Option<RepoConfig>,
    /// Path to the global config file (if loaded)
    global_path: Option<PathBuf>,
    /// Path to the repo config file (if loaded)
    repo_path: Option<PathBuf>,
}

impl Config {
    /// Load configuration from default locations.
    ///
    /// If `repo_path` is provided, also loads repo-specific config.
    ///
    /// # Errors
    ///
    /// Returns an error if config files exist but cannot be parsed.
    /// Missing config files are not an error (defaults are used).
    pub fn load(repo_path: Option<&Path>) -> Result<ConfigLoadResult, ConfigError> {
        let mut warnings = Vec::new();

        // Load global config
        let (global, global_path) = Self::load_global()?;

        // Load repo config if path provided
        let (repo, repo_path_found) = if let Some(path) = repo_path {
            Self::load_repo(path, &mut warnings)?
        } else {
            (None, None)
        };

        // Validate loaded configs
        global.validate()?;
        if let Some(ref r) = repo {
            r.validate()?;
        }

        Ok(ConfigLoadResult {
            config: Config {
                global,
                repo,
                global_path,
                repo_path: repo_path_found,
            },
            warnings,
        })
    }

    /// Load global configuration from standard locations.
    fn load_global() -> Result<(GlobalConfig, Option<PathBuf>), ConfigError> {
        // 1. Check $LATTICE_CONFIG
        if let Ok(path) = std::env::var("LATTICE_CONFIG") {
            let path = PathBuf::from(path);
            if path.exists() {
                let config = Self::read_global_config(&path)?;
                return Ok((config, Some(path)));
            }
        }

        // 2. Check $XDG_CONFIG_HOME/lattice/config.toml
        if let Ok(xdg_home) = std::env::var("XDG_CONFIG_HOME") {
            let path = PathBuf::from(xdg_home).join("lattice/config.toml");
            if path.exists() {
                let config = Self::read_global_config(&path)?;
                return Ok((config, Some(path)));
            }
        }

        // 3. Check ~/.lattice/config.toml
        if let Some(home) = dirs::home_dir() {
            let path = home.join(".lattice/config.toml");
            if path.exists() {
                let config = Self::read_global_config(&path)?;
                return Ok((config, Some(path)));
            }
        }

        // No config found, use defaults
        Ok((GlobalConfig::default(), None))
    }

    /// Load repository configuration from standard locations.
    fn load_repo(
        repo_path: &Path,
        warnings: &mut Vec<ConfigWarning>,
    ) -> Result<(Option<RepoConfig>, Option<PathBuf>), ConfigError> {
        // Find the .git directory
        let git_dir = repo_path.join(".git");
        if !git_dir.exists() {
            return Ok((None, None));
        }

        // 1. Check .git/lattice/config.toml (canonical)
        let canonical = git_dir.join("lattice/config.toml");
        if canonical.exists() {
            let config = Self::read_repo_config(&canonical)?;
            return Ok((Some(config), Some(canonical)));
        }

        // 2. Check .git/lattice/repo.toml (compatibility)
        let compat_git = git_dir.join("lattice/repo.toml");
        if compat_git.exists() {
            warnings.push(ConfigWarning {
                message: format!(
                    "Using deprecated config location. Please rename to '{}'",
                    canonical.display()
                ),
                path: compat_git.clone(),
            });
            let config = Self::read_repo_config(&compat_git)?;
            return Ok((Some(config), Some(compat_git)));
        }

        // 3. Check .lattice/repo.toml (compatibility)
        let compat_root = repo_path.join(".lattice/repo.toml");
        if compat_root.exists() {
            warnings.push(ConfigWarning {
                message: format!(
                    "Using deprecated config location. Please move to '{}'",
                    canonical.display()
                ),
                path: compat_root.clone(),
            });
            let config = Self::read_repo_config(&compat_root)?;
            return Ok((Some(config), Some(compat_root)));
        }

        Ok((None, None))
    }

    /// Read and parse a global config file.
    fn read_global_config(path: &Path) -> Result<GlobalConfig, ConfigError> {
        let contents = fs::read_to_string(path).map_err(|e| ConfigError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;

        toml::from_str(&contents).map_err(|e| ConfigError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Read and parse a repo config file.
    fn read_repo_config(path: &Path) -> Result<RepoConfig, ConfigError> {
        let contents = fs::read_to_string(path).map_err(|e| ConfigError::ReadError {
            path: path.to_path_buf(),
            source: e,
        })?;

        toml::from_str(&contents).map_err(|e| ConfigError::ParseError {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    /// Get the canonical path for global config.
    ///
    /// Returns `~/.lattice/config.toml`.
    pub fn global_config_path() -> Result<PathBuf, ConfigError> {
        let home = dirs::home_dir().ok_or(ConfigError::NoHomeDir)?;
        Ok(home.join(".lattice/config.toml"))
    }

    /// Get the canonical path for repo config.
    ///
    /// Returns `.git/lattice/config.toml` relative to the given repo path.
    pub fn repo_config_path(repo_path: &Path) -> PathBuf {
        repo_path.join(".git/lattice/config.toml")
    }

    /// Write global config atomically.
    ///
    /// Creates parent directories if needed. Uses atomic write
    /// (write to temp file, then rename) to prevent corruption.
    pub fn write_global(config: &GlobalConfig) -> Result<PathBuf, ConfigError> {
        let path = Self::global_config_path()?;
        Self::write_config_atomic(&path, config)?;
        Ok(path)
    }

    /// Write repo config atomically.
    ///
    /// Creates parent directories if needed. Uses atomic write
    /// (write to temp file, then rename) to prevent corruption.
    pub fn write_repo(repo_path: &Path, config: &RepoConfig) -> Result<PathBuf, ConfigError> {
        let path = Self::repo_config_path(repo_path);
        Self::write_config_atomic(&path, config)?;
        Ok(path)
    }

    /// Write a config file atomically.
    fn write_config_atomic<T: serde::Serialize>(
        path: &Path,
        config: &T,
    ) -> Result<(), ConfigError> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ConfigError::WriteError {
                path: path.to_path_buf(),
                source: e,
            })?;
        }

        // Serialize to TOML
        let contents =
            toml::to_string_pretty(config).map_err(|e| ConfigError::InvalidValue(e.to_string()))?;

        // Write to temp file in same directory (for atomic rename)
        let temp_path = path.with_extension("toml.tmp");
        let mut file = fs::File::create(&temp_path).map_err(|e| ConfigError::WriteError {
            path: temp_path.clone(),
            source: e,
        })?;

        file.write_all(contents.as_bytes())
            .map_err(|e| ConfigError::WriteError {
                path: temp_path.clone(),
                source: e,
            })?;

        file.sync_all().map_err(|e| ConfigError::WriteError {
            path: temp_path.clone(),
            source: e,
        })?;

        // Atomic rename
        fs::rename(&temp_path, path).map_err(|e| ConfigError::WriteError {
            path: path.to_path_buf(),
            source: e,
        })?;

        Ok(())
    }

    // =========================================================================
    // Accessor methods with precedence
    // =========================================================================

    /// Get the trunk branch name.
    ///
    /// Returns `None` if not configured.
    pub fn trunk(&self) -> Option<&str> {
        self.repo.as_ref().and_then(|r| r.trunk.as_deref())
    }

    /// Get the remote name.
    ///
    /// Defaults to "origin" if not configured.
    pub fn remote(&self) -> &str {
        self.repo
            .as_ref()
            .and_then(|r| r.remote.as_deref())
            .unwrap_or("origin")
    }

    /// Check if interactive mode is enabled by default.
    ///
    /// Defaults to `true` if not configured.
    pub fn interactive(&self) -> bool {
        self.global.interactive.unwrap_or(true)
    }

    /// Check if hook verification is enabled by default.
    ///
    /// Defaults to `true` if not configured.
    pub fn verify_hooks(&self) -> bool {
        self.global.verify_hooks.unwrap_or(true)
    }

    /// Get the default forge.
    ///
    /// Defaults to "github" if not configured.
    pub fn default_forge(&self) -> &str {
        self.global.default_forge.as_deref().unwrap_or("github")
    }

    /// Get the secrets provider.
    ///
    /// Defaults to "file" if not configured.
    pub fn secrets_provider(&self) -> &str {
        self.global
            .secrets
            .as_ref()
            .and_then(|s| s.provider.as_deref())
            .unwrap_or("file")
    }

    /// Check if submit should default to draft.
    ///
    /// Defaults to `false` if not configured.
    pub fn submit_draft(&self) -> bool {
        self.global
            .submit
            .as_ref()
            .and_then(|s| s.draft)
            .unwrap_or(false)
    }

    /// Check if submit should restack by default.
    ///
    /// Defaults to `true` if not configured.
    pub fn submit_restack(&self) -> bool {
        self.global
            .submit
            .as_ref()
            .and_then(|s| s.restack)
            .unwrap_or(true)
    }

    /// Check if metadata refs should be synced.
    ///
    /// Defaults to `false` if not configured.
    pub fn sync_metadata_refs(&self) -> bool {
        self.repo
            .as_ref()
            .and_then(|r| r.sync_metadata_refs)
            .unwrap_or(false)
    }

    /// Get the path to the loaded global config file.
    pub fn global_config_loaded_from(&self) -> Option<&Path> {
        self.global_path.as_deref()
    }

    /// Get the path to the loaded repo config file.
    pub fn repo_config_loaded_from(&self) -> Option<&Path> {
        self.repo_path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_empty_defaults() {
        // Ensure no env vars interfere with this test
        std::env::remove_var("LATTICE_CONFIG");
        std::env::remove_var("XDG_CONFIG_HOME");

        // Load with no config files present
        let result = Config::load(None).unwrap();
        let config = result.config;

        assert!(config.trunk().is_none());
        assert_eq!(config.remote(), "origin");
        // Note: interactive defaults to true when not configured
        // but we can't assert this reliably if ~/.lattice/config.toml exists
        assert!(config.verify_hooks());
        assert_eq!(config.default_forge(), "github");
        assert_eq!(config.secrets_provider(), "file");
    }

    #[test]
    fn load_global_from_env() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.toml");

        fs::write(
            &config_path,
            r#"
            default_forge = "github"
            interactive = false
            "#,
        )
        .unwrap();

        // Set env var
        std::env::set_var("LATTICE_CONFIG", config_path.to_str().unwrap());

        let result = Config::load(None).unwrap();
        let config = result.config;

        assert!(!config.interactive());

        // Clean up
        std::env::remove_var("LATTICE_CONFIG");
    }

    #[test]
    fn load_repo_config() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git/lattice");
        fs::create_dir_all(&git_dir).unwrap();

        let config_path = git_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
            trunk = "main"
            remote = "upstream"
            "#,
        )
        .unwrap();

        let result = Config::load(Some(temp.path())).unwrap();
        let config = result.config;

        assert_eq!(config.trunk(), Some("main"));
        assert_eq!(config.remote(), "upstream");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn load_repo_compat_warns() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git/lattice");
        fs::create_dir_all(&git_dir).unwrap();

        // Use deprecated path
        let config_path = git_dir.join("repo.toml");
        fs::write(&config_path, "trunk = \"main\"").unwrap();

        let result = Config::load(Some(temp.path())).unwrap();

        assert_eq!(result.config.trunk(), Some("main"));
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].message.contains("deprecated"));
    }

    #[test]
    fn write_repo_config_atomic() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();

        let config = RepoConfig {
            trunk: Some("develop".to_string()),
            remote: Some("origin".to_string()),
            ..Default::default()
        };

        let path = Config::write_repo(temp.path(), &config).unwrap();

        assert!(path.exists());
        let loaded = Config::load(Some(temp.path())).unwrap();
        assert_eq!(loaded.config.trunk(), Some("develop"));
    }

    #[test]
    fn invalid_trunk_rejected() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git/lattice");
        fs::create_dir_all(&git_dir).unwrap();

        let config_path = git_dir.join("config.toml");
        fs::write(&config_path, "trunk = \"invalid..name\"").unwrap();

        let result = Config::load(Some(temp.path()));
        assert!(result.is_err());
    }

    #[test]
    fn unknown_fields_rejected() {
        let temp = TempDir::new().unwrap();
        let git_dir = temp.path().join(".git/lattice");
        fs::create_dir_all(&git_dir).unwrap();

        let config_path = git_dir.join("config.toml");
        fs::write(
            &config_path,
            r#"
            trunk = "main"
            unknown_field = true
            "#,
        )
        .unwrap();

        let result = Config::load(Some(temp.path()));
        assert!(result.is_err());
    }

    #[test]
    fn precedence_repo_overrides_global() {
        // This test verifies the concept - repo config overrides global.
        // In practice, most settings are scope-specific (trunk is repo-only,
        // interactive is global-only), but the pattern holds.

        let config = Config {
            global: GlobalConfig::default(),
            repo: Some(RepoConfig {
                remote: Some("upstream".to_string()),
                ..Default::default()
            }),
            global_path: None,
            repo_path: None,
        };

        // Repo remote overrides the default "origin"
        assert_eq!(config.remote(), "upstream");
    }
}
