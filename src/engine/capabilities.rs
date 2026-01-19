//! engine::capabilities
//!
//! Capability system for command gating.
//!
//! # Architecture
//!
//! Capabilities are composable proofs about repository state. A capability
//! either exists or does not - there is no "partial" capability. Partial
//! states are represented as absence of a capability plus an issue in the
//! health report.
//!
//! Per ARCHITECTURE.md Section 5.2, the scanner produces capabilities that
//! describe what is known to be true. Commands declare their required
//! capabilities, and gating checks if those requirements are satisfied.
//!
//! # Example
//!
//! ```
//! use latticework::engine::capabilities::{Capability, CapabilitySet};
//!
//! let mut caps = CapabilitySet::new();
//! caps.insert(Capability::RepoOpen);
//! caps.insert(Capability::TrunkKnown);
//!
//! assert!(caps.has(&Capability::RepoOpen));
//! assert!(!caps.has(&Capability::AuthAvailable));
//!
//! let missing = caps.missing(&[
//!     Capability::RepoOpen,
//!     Capability::AuthAvailable,
//! ]);
//! assert_eq!(missing, vec![Capability::AuthAvailable]);
//! ```

use std::collections::HashSet;

/// A capability represents a proven fact about the repository state.
///
/// Capabilities are used for gating command execution. Each command
/// declares its required capabilities, and the engine only proceeds
/// when all requirements are satisfied.
///
/// # Invariants
///
/// - A capability is binary: present or absent
/// - Capabilities are established by the scanner
/// - Capabilities cannot be "partially" satisfied
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Repository can be opened and accessed.
    RepoOpen,

    /// Trunk branch is configured in repo config.
    TrunkKnown,

    /// No Lattice operation is in progress (no op-state marker).
    NoLatticeOpInProgress,

    /// No external Git operation in progress (rebase/merge/cherry-pick/etc.).
    NoExternalGitOpInProgress,

    /// All metadata refs are readable and parseable.
    MetadataReadable,

    /// Stack graph is valid (acyclic, all tracked branches exist).
    GraphValid,

    /// Working copy state is known (clean/dirty status available).
    WorkingCopyStateKnown,

    /// Authentication is available for remote operations.
    AuthAvailable,

    /// Remote is configured and resolvable.
    RemoteResolved,

    /// Repository authorization verified via GitHub App installation.
    ///
    /// Per SPEC.md Section 8E.0.1, this capability is established when the
    /// authenticated user has access to the target repository via an installed
    /// GitHub App. The check queries `/user/installations` and caches the
    /// result for 10 minutes.
    RepoAuthorized,

    /// Frozen policy is satisfied for target branches.
    ///
    /// This means no frozen branches will be modified by the operation.
    FrozenPolicySatisfied,

    /// Working directory is available.
    ///
    /// Per SPEC.md §4.6.6, bare repositories lack a working directory,
    /// which blocks commands that require checkout, staging, or working
    /// tree operations. Commands are categorized as:
    /// - Category A: Read-only (works everywhere)
    /// - Category B: Metadata-only mutations (works in bare)
    /// - Category C: Working-copy mutations (require this capability)
    /// - Category D: Remote/API-only (may work in bare with restrictions)
    WorkingDirectoryAvailable,
}

impl Capability {
    /// Get a human-readable description of the capability.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::capabilities::Capability;
    ///
    /// assert_eq!(Capability::RepoOpen.description(), "repository is accessible");
    /// assert_eq!(Capability::TrunkKnown.description(), "trunk branch is configured");
    /// ```
    pub fn description(&self) -> &'static str {
        match self {
            Capability::RepoOpen => "repository is accessible",
            Capability::TrunkKnown => "trunk branch is configured",
            Capability::NoLatticeOpInProgress => "no Lattice operation in progress",
            Capability::NoExternalGitOpInProgress => "no Git operation in progress",
            Capability::MetadataReadable => "all metadata is readable",
            Capability::GraphValid => "stack graph is valid",
            Capability::WorkingCopyStateKnown => "working copy state is known",
            Capability::AuthAvailable => "authentication is available",
            Capability::RemoteResolved => "remote is configured",
            Capability::RepoAuthorized => "repository authorization verified",
            Capability::FrozenPolicySatisfied => "frozen policy is satisfied",
            Capability::WorkingDirectoryAvailable => "working directory is available",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// A set of capabilities established by the scanner.
///
/// This is the primary interface for checking what is known about
/// the repository state.
///
/// # Example
///
/// ```
/// use latticework::engine::capabilities::{Capability, CapabilitySet};
///
/// let mut caps = CapabilitySet::new();
/// caps.insert(Capability::RepoOpen);
/// caps.insert(Capability::MetadataReadable);
///
/// assert!(caps.has(&Capability::RepoOpen));
/// assert!(caps.has_all(&[Capability::RepoOpen, Capability::MetadataReadable]));
/// assert!(!caps.has_all(&[Capability::RepoOpen, Capability::TrunkKnown]));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySet {
    capabilities: HashSet<Capability>,
}

impl CapabilitySet {
    /// Create an empty capability set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a capability set with the given capabilities.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::capabilities::{Capability, CapabilitySet};
    ///
    /// let caps = CapabilitySet::with([Capability::RepoOpen, Capability::TrunkKnown]);
    /// assert!(caps.has(&Capability::RepoOpen));
    /// assert!(caps.has(&Capability::TrunkKnown));
    /// ```
    pub fn with<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        Self {
            capabilities: iter.into_iter().collect(),
        }
    }

    /// Insert a capability into the set.
    pub fn insert(&mut self, cap: Capability) {
        self.capabilities.insert(cap);
    }

    /// Remove a capability from the set.
    pub fn remove(&mut self, cap: &Capability) -> bool {
        self.capabilities.remove(cap)
    }

    /// Check if a capability is present.
    pub fn has(&self, cap: &Capability) -> bool {
        self.capabilities.contains(cap)
    }

    /// Check if all given capabilities are present.
    ///
    /// Returns true if the slice is empty.
    pub fn has_all(&self, caps: &[Capability]) -> bool {
        caps.iter().all(|c| self.capabilities.contains(c))
    }

    /// Get the capabilities that are missing from the required set.
    ///
    /// Returns an empty vec if all required capabilities are present.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::engine::capabilities::{Capability, CapabilitySet};
    ///
    /// let caps = CapabilitySet::with([Capability::RepoOpen]);
    /// let missing = caps.missing(&[
    ///     Capability::RepoOpen,
    ///     Capability::TrunkKnown,
    ///     Capability::GraphValid,
    /// ]);
    /// assert_eq!(missing, vec![Capability::TrunkKnown, Capability::GraphValid]);
    /// ```
    pub fn missing(&self, required: &[Capability]) -> Vec<Capability> {
        required
            .iter()
            .filter(|c| !self.capabilities.contains(c))
            .copied()
            .collect()
    }

    /// Get the number of capabilities in the set.
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    /// Check if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// Iterate over all capabilities in the set.
    pub fn iter(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities.iter()
    }
}

impl FromIterator<Capability> for CapabilitySet {
    fn from_iter<T: IntoIterator<Item = Capability>>(iter: T) -> Self {
        Self {
            capabilities: iter.into_iter().collect(),
        }
    }
}

impl<'a> IntoIterator for &'a CapabilitySet {
    type Item = &'a Capability;
    type IntoIter = std::collections::hash_set::Iter<'a, Capability>;

    fn into_iter(self) -> Self::IntoIter {
        self.capabilities.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod capability {
        use super::*;

        #[test]
        fn all_variants_have_descriptions() {
            let caps = [
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::NoLatticeOpInProgress,
                Capability::NoExternalGitOpInProgress,
                Capability::MetadataReadable,
                Capability::GraphValid,
                Capability::WorkingCopyStateKnown,
                Capability::AuthAvailable,
                Capability::RemoteResolved,
                Capability::RepoAuthorized,
                Capability::FrozenPolicySatisfied,
                Capability::WorkingDirectoryAvailable,
            ];

            for cap in caps {
                assert!(!cap.description().is_empty());
            }
        }

        #[test]
        fn display_uses_description() {
            let cap = Capability::RepoOpen;
            assert_eq!(format!("{}", cap), cap.description());
        }

        #[test]
        fn capabilities_are_hashable() {
            let mut set = HashSet::new();
            set.insert(Capability::RepoOpen);
            set.insert(Capability::RepoOpen); // duplicate
            assert_eq!(set.len(), 1);
        }

        #[test]
        fn capabilities_are_copyable() {
            let cap = Capability::TrunkKnown;
            let cap2 = cap; // copy
            assert_eq!(cap, cap2);
        }
    }

    mod capability_set {
        use super::*;

        #[test]
        fn new_is_empty() {
            let caps = CapabilitySet::new();
            assert!(caps.is_empty());
            assert_eq!(caps.len(), 0);
        }

        #[test]
        fn with_creates_from_iter() {
            let caps = CapabilitySet::with([Capability::RepoOpen, Capability::TrunkKnown]);
            assert_eq!(caps.len(), 2);
            assert!(caps.has(&Capability::RepoOpen));
            assert!(caps.has(&Capability::TrunkKnown));
        }

        #[test]
        fn insert_adds_capability() {
            let mut caps = CapabilitySet::new();
            caps.insert(Capability::RepoOpen);
            assert!(caps.has(&Capability::RepoOpen));
            assert_eq!(caps.len(), 1);
        }

        #[test]
        fn insert_deduplicates() {
            let mut caps = CapabilitySet::new();
            caps.insert(Capability::RepoOpen);
            caps.insert(Capability::RepoOpen);
            assert_eq!(caps.len(), 1);
        }

        #[test]
        fn remove_returns_true_if_present() {
            let mut caps = CapabilitySet::with([Capability::RepoOpen]);
            assert!(caps.remove(&Capability::RepoOpen));
            assert!(!caps.has(&Capability::RepoOpen));
        }

        #[test]
        fn remove_returns_false_if_absent() {
            let mut caps = CapabilitySet::new();
            assert!(!caps.remove(&Capability::RepoOpen));
        }

        #[test]
        fn has_checks_presence() {
            let caps = CapabilitySet::with([Capability::RepoOpen]);
            assert!(caps.has(&Capability::RepoOpen));
            assert!(!caps.has(&Capability::TrunkKnown));
        }

        #[test]
        fn has_all_with_empty_required() {
            let caps = CapabilitySet::new();
            assert!(caps.has_all(&[]));
        }

        #[test]
        fn has_all_with_all_present() {
            let caps = CapabilitySet::with([
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::GraphValid,
            ]);
            assert!(caps.has_all(&[Capability::RepoOpen, Capability::TrunkKnown]));
        }

        #[test]
        fn has_all_with_some_missing() {
            let caps = CapabilitySet::with([Capability::RepoOpen]);
            assert!(!caps.has_all(&[Capability::RepoOpen, Capability::TrunkKnown]));
        }

        #[test]
        fn missing_returns_absent_capabilities() {
            let caps = CapabilitySet::with([Capability::RepoOpen]);
            let missing = caps.missing(&[
                Capability::RepoOpen,
                Capability::TrunkKnown,
                Capability::GraphValid,
            ]);
            assert_eq!(missing.len(), 2);
            assert!(missing.contains(&Capability::TrunkKnown));
            assert!(missing.contains(&Capability::GraphValid));
        }

        #[test]
        fn missing_preserves_order() {
            let caps = CapabilitySet::new();
            let missing = caps.missing(&[Capability::RepoOpen, Capability::TrunkKnown]);
            assert_eq!(missing, vec![Capability::RepoOpen, Capability::TrunkKnown]);
        }

        #[test]
        fn missing_returns_empty_when_all_present() {
            let caps = CapabilitySet::with([Capability::RepoOpen, Capability::TrunkKnown]);
            let missing = caps.missing(&[Capability::RepoOpen, Capability::TrunkKnown]);
            assert!(missing.is_empty());
        }

        #[test]
        fn from_iterator() {
            let caps: CapabilitySet = [Capability::RepoOpen, Capability::TrunkKnown]
                .into_iter()
                .collect();
            assert_eq!(caps.len(), 2);
        }

        #[test]
        fn iter_yields_all() {
            let caps = CapabilitySet::with([Capability::RepoOpen, Capability::TrunkKnown]);
            let collected: HashSet<_> = caps.iter().collect();
            assert_eq!(collected.len(), 2);
        }

        #[test]
        fn into_iter_ref() {
            let caps = CapabilitySet::with([Capability::RepoOpen]);
            let mut count = 0;
            for _ in &caps {
                count += 1;
            }
            assert_eq!(count, 1);
        }

        #[test]
        fn default_is_empty() {
            let caps = CapabilitySet::default();
            assert!(caps.is_empty());
        }

        #[test]
        fn equality() {
            let caps1 = CapabilitySet::with([Capability::RepoOpen, Capability::TrunkKnown]);
            let caps2 = CapabilitySet::with([Capability::TrunkKnown, Capability::RepoOpen]);
            assert_eq!(caps1, caps2);
        }
    }
}
