//! engine::modes
//!
//! Mode types for commands with flag-dependent requirements.
//!
//! # Architecture
//!
//! Per ROADMAP.md, several commands have different requirement sets depending
//! on flags (--no-restack, --no-checkout) and repository context (bare repo).
//!
//! Mode types model these variations so each mode has a static requirement set.
//! This avoids dynamic branching in gating and ensures compile-time safety.
//!
//! # Bare Repository Policy
//!
//! Per SPEC.md §4.6.7, bare repositories cannot perform rebases, checkouts,
//! or any operation that uses index/worktree state. Commands that normally
//! require these operations MUST refuse in bare repos unless the user
//! explicitly opts into a restricted mode via flags.
//!
//! **Key principle:** No silent downgrades. If a user runs `lattice submit`
//! in a bare repo, it MUST fail with guidance, not silently skip restack.
//!
//! # Example
//!
//! ```ignore
//! use latticework::engine::modes::{SubmitMode, ModeError};
//!
//! // Resolve mode from flags and repo context
//! let mode = SubmitMode::resolve(args.no_restack, is_bare_repo)?;
//!
//! // Each mode has its own static requirements
//! let requirements = mode.requirements();
//! ```

use super::gate::requirements;
use super::gate::RequirementSet;
use thiserror::Error;

/// Errors from mode resolution.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ModeError {
    /// Bare repository requires an explicit flag.
    #[error("{command} in bare repository requires {required_flag}")]
    BareRepoRequiresFlag {
        /// The command being run
        command: &'static str,
        /// The flag that must be provided
        required_flag: &'static str,
    },
}

/// Submit mode determines gating requirements.
///
/// Per SPEC.md §4.6.7:
/// - Default: restack before submit (requires working directory)
/// - `--no-restack`: skip restack (bare-repo compatible, but requires alignment)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitMode {
    /// Default: restack before submit.
    ///
    /// Requires working directory. Uses `requirements::REMOTE`.
    WithRestack,

    /// Skip restack (`--no-restack`).
    ///
    /// Bare-repo compatible. Uses `requirements::REMOTE_BARE_ALLOWED`.
    /// Note: Still requires ancestry alignment check at plan time.
    NoRestack,
}

impl SubmitMode {
    /// Get the requirement set for this mode.
    pub fn requirements(&self) -> &'static RequirementSet {
        match self {
            Self::WithRestack => &requirements::REMOTE,
            Self::NoRestack => &requirements::REMOTE_BARE_ALLOWED,
        }
    }

    /// Resolve mode from flags and repo context.
    ///
    /// # Arguments
    ///
    /// * `no_restack` - Whether `--no-restack` flag was provided
    /// * `is_bare` - Whether the repository is bare
    ///
    /// # Returns
    ///
    /// The resolved mode, or an error if bare repo without required flag.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::modes::SubmitMode;
    ///
    /// // Normal repo, default mode
    /// let mode = SubmitMode::resolve(false, false).unwrap();
    /// assert_eq!(mode, SubmitMode::WithRestack);
    ///
    /// // Bare repo with --no-restack
    /// let mode = SubmitMode::resolve(true, true).unwrap();
    /// assert_eq!(mode, SubmitMode::NoRestack);
    ///
    /// // Bare repo without flag - error
    /// let err = SubmitMode::resolve(false, true).unwrap_err();
    /// assert!(err.to_string().contains("--no-restack"));
    /// ```
    pub fn resolve(no_restack: bool, is_bare: bool) -> Result<Self, ModeError> {
        match (no_restack, is_bare) {
            (true, _) => Ok(Self::NoRestack),
            (false, false) => Ok(Self::WithRestack),
            (false, true) => Err(ModeError::BareRepoRequiresFlag {
                command: "submit",
                required_flag: "--no-restack",
            }),
        }
    }
}

/// Sync mode determines gating requirements.
///
/// Per SPEC.md §4.6.7:
/// - Default: may restack after sync (requires working directory)
/// - `--no-restack`: skip restack (bare-repo compatible)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Default: may restack after sync.
    ///
    /// Requires working directory. Uses `requirements::REMOTE`.
    WithRestack,

    /// Skip restack (`--no-restack`).
    ///
    /// Bare-repo compatible. Uses `requirements::REMOTE_BARE_ALLOWED`.
    NoRestack,
}

impl SyncMode {
    /// Get the requirement set for this mode.
    pub fn requirements(&self) -> &'static RequirementSet {
        match self {
            Self::WithRestack => &requirements::REMOTE,
            Self::NoRestack => &requirements::REMOTE_BARE_ALLOWED,
        }
    }

    /// Resolve mode from flags and repo context.
    ///
    /// # Arguments
    ///
    /// * `no_restack` - Whether `--no-restack` flag was provided
    /// * `is_bare` - Whether the repository is bare
    ///
    /// # Returns
    ///
    /// The resolved mode, or an error if bare repo without required flag.
    pub fn resolve(no_restack: bool, is_bare: bool) -> Result<Self, ModeError> {
        match (no_restack, is_bare) {
            (true, _) => Ok(Self::NoRestack),
            (false, false) => Ok(Self::WithRestack),
            (false, true) => Err(ModeError::BareRepoRequiresFlag {
                command: "sync",
                required_flag: "--no-restack",
            }),
        }
    }
}

/// Get mode determines gating requirements.
///
/// Per SPEC.md §4.6.7:
/// - Default: checkout after get (requires working directory)
/// - `--no-checkout`: skip checkout (bare-repo compatible)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetMode {
    /// Default: checkout after get.
    ///
    /// Requires working directory. Uses `requirements::REMOTE`.
    WithCheckout,

    /// Skip checkout (`--no-checkout`).
    ///
    /// Bare-repo compatible. Uses `requirements::REMOTE_BARE_ALLOWED`.
    NoCheckout,
}

impl GetMode {
    /// Get the requirement set for this mode.
    pub fn requirements(&self) -> &'static RequirementSet {
        match self {
            Self::WithCheckout => &requirements::REMOTE,
            Self::NoCheckout => &requirements::REMOTE_BARE_ALLOWED,
        }
    }

    /// Resolve mode from flags and repo context.
    ///
    /// # Arguments
    ///
    /// * `no_checkout` - Whether `--no-checkout` flag was provided
    /// * `is_bare` - Whether the repository is bare
    ///
    /// # Returns
    ///
    /// The resolved mode, or an error if bare repo without required flag.
    pub fn resolve(no_checkout: bool, is_bare: bool) -> Result<Self, ModeError> {
        match (no_checkout, is_bare) {
            (true, _) => Ok(Self::NoCheckout),
            (false, false) => Ok(Self::WithCheckout),
            (false, true) => Err(ModeError::BareRepoRequiresFlag {
                command: "get",
                required_flag: "--no-checkout",
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod mode_error {
        use super::*;

        #[test]
        fn display_formatting() {
            let err = ModeError::BareRepoRequiresFlag {
                command: "submit",
                required_flag: "--no-restack",
            };
            let msg = err.to_string();
            assert!(msg.contains("submit"));
            assert!(msg.contains("bare repository"));
            assert!(msg.contains("--no-restack"));
        }
    }

    mod submit_mode {
        use super::*;

        #[test]
        fn resolve_with_flag() {
            // --no-restack always selects NoRestack mode
            assert_eq!(
                SubmitMode::resolve(true, false).unwrap(),
                SubmitMode::NoRestack
            );
            assert_eq!(
                SubmitMode::resolve(true, true).unwrap(),
                SubmitMode::NoRestack
            );
        }

        #[test]
        fn resolve_normal_repo_default() {
            // Normal repo without flag selects WithRestack
            assert_eq!(
                SubmitMode::resolve(false, false).unwrap(),
                SubmitMode::WithRestack
            );
        }

        #[test]
        fn resolve_bare_repo_without_flag_errors() {
            // Bare repo without flag is an error
            let err = SubmitMode::resolve(false, true).unwrap_err();
            assert_eq!(
                err,
                ModeError::BareRepoRequiresFlag {
                    command: "submit",
                    required_flag: "--no-restack",
                }
            );
        }

        #[test]
        fn requirements_match_mode() {
            assert_eq!(
                SubmitMode::WithRestack.requirements().name,
                requirements::REMOTE.name
            );
            assert_eq!(
                SubmitMode::NoRestack.requirements().name,
                requirements::REMOTE_BARE_ALLOWED.name
            );
        }
    }

    mod sync_mode {
        use super::*;

        #[test]
        fn resolve_with_flag() {
            assert_eq!(SyncMode::resolve(true, false).unwrap(), SyncMode::NoRestack);
            assert_eq!(SyncMode::resolve(true, true).unwrap(), SyncMode::NoRestack);
        }

        #[test]
        fn resolve_normal_repo_default() {
            assert_eq!(
                SyncMode::resolve(false, false).unwrap(),
                SyncMode::WithRestack
            );
        }

        #[test]
        fn resolve_bare_repo_without_flag_errors() {
            let err = SyncMode::resolve(false, true).unwrap_err();
            assert_eq!(
                err,
                ModeError::BareRepoRequiresFlag {
                    command: "sync",
                    required_flag: "--no-restack",
                }
            );
        }

        #[test]
        fn requirements_match_mode() {
            assert_eq!(
                SyncMode::WithRestack.requirements().name,
                requirements::REMOTE.name
            );
            assert_eq!(
                SyncMode::NoRestack.requirements().name,
                requirements::REMOTE_BARE_ALLOWED.name
            );
        }
    }

    mod get_mode {
        use super::*;

        #[test]
        fn resolve_with_flag() {
            assert_eq!(GetMode::resolve(true, false).unwrap(), GetMode::NoCheckout);
            assert_eq!(GetMode::resolve(true, true).unwrap(), GetMode::NoCheckout);
        }

        #[test]
        fn resolve_normal_repo_default() {
            assert_eq!(
                GetMode::resolve(false, false).unwrap(),
                GetMode::WithCheckout
            );
        }

        #[test]
        fn resolve_bare_repo_without_flag_errors() {
            let err = GetMode::resolve(false, true).unwrap_err();
            assert_eq!(
                err,
                ModeError::BareRepoRequiresFlag {
                    command: "get",
                    required_flag: "--no-checkout",
                }
            );
        }

        #[test]
        fn requirements_match_mode() {
            assert_eq!(
                GetMode::WithCheckout.requirements().name,
                requirements::REMOTE.name
            );
            assert_eq!(
                GetMode::NoCheckout.requirements().name,
                requirements::REMOTE_BARE_ALLOWED.name
            );
        }
    }
}
