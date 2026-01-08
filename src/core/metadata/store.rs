//! core::metadata::store
//!
//! Metadata storage in Git refs.
//!
//! # Architecture
//!
//! Metadata is stored as refs under `refs/branch-metadata/<branch>`.
//! Each ref points to a blob containing JSON. This module provides
//! the `MetadataStore` which uses the Git interface to manage these refs.
//!
//! # CAS Semantics
//!
//! All write and delete operations use compare-and-swap (CAS) semantics
//! to prevent race conditions and ensure correctness when the repository
//! is modified by external processes.
//!
//! # Example
//!
//! ```ignore
//! use latticework::core::metadata::store::MetadataStore;
//! use latticework::core::metadata::schema::BranchMetadataV1;
//! use latticework::core::types::{BranchName, Oid};
//! use latticework::git::Git;
//!
//! let git = Git::open(Path::new("."))?;
//! let store = MetadataStore::new(&git);
//!
//! // Read metadata
//! if let Some(entry) = store.read(&branch)? {
//!     println!("Parent: {}", entry.metadata.parent.name());
//!
//!     // Update with CAS
//!     let mut meta = entry.metadata;
//!     meta.touch();
//!     store.write_cas(&branch, Some(&entry.ref_oid), &meta)?;
//! }
//! ```

use thiserror::Error;

use super::schema::{parse_metadata, BranchMetadataV1, MetadataError};
use crate::core::types::{BranchName, Oid, RefName};
use crate::git::{Git, GitError};

/// Prefix for metadata refs.
pub const METADATA_REF_PREFIX: &str = "refs/branch-metadata/";

/// Errors from metadata storage operations.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Metadata not found for the specified branch.
    #[error("metadata not found for branch: {0}")]
    NotFound(String),

    /// CAS precondition failed - the ref changed since we read it.
    #[error("CAS precondition failed: expected {expected}, found {actual}")]
    CasFailed {
        /// The expected ref OID
        expected: String,
        /// The actual ref OID found
        actual: String,
    },

    /// Failed to parse metadata JSON.
    #[error("failed to parse metadata: {0}")]
    ParseError(String),

    /// Failed to serialize metadata to JSON.
    #[error("failed to serialize metadata: {0}")]
    SerializeError(String),

    /// Invalid branch name.
    #[error("invalid branch name: {0}")]
    InvalidBranchName(String),

    /// Git operation failed.
    #[error("git error: {0}")]
    GitError(#[from] GitError),

    /// Metadata validation failed.
    #[error("metadata error: {0}")]
    MetadataError(#[from] MetadataError),
}

/// Result of reading metadata.
///
/// Contains both the metadata and the ref OID, which is needed for
/// CAS updates.
#[derive(Debug, Clone)]
pub struct MetadataEntry {
    /// The ref's current OID (blob pointer).
    ///
    /// This is the OID of the blob object containing the JSON, not the
    /// branch tip. Pass this to `write_cas` as `expected_old` when updating.
    pub ref_oid: Oid,

    /// The parsed and validated metadata.
    pub metadata: BranchMetadataV1,
}

/// Metadata store backed by Git refs.
///
/// This store manages branch metadata stored as JSON blobs pointed to
/// by refs under `refs/branch-metadata/<branch>`. All operations use
/// compare-and-swap (CAS) semantics to prevent race conditions.
///
/// # Architecture
///
/// The store does not directly use `git2`. Instead, it uses the `Git`
/// interface which provides typed operations with proper error handling.
/// This maintains the "single doorway" architecture for Git operations.
///
/// # Example
///
/// ```ignore
/// let git = Git::open(Path::new("."))?;
/// let store = MetadataStore::new(&git);
///
/// // List all tracked branches
/// for branch in store.list()? {
///     println!("Tracked: {}", branch);
/// }
/// ```
pub struct MetadataStore<'a> {
    git: &'a Git,
}

impl<'a> MetadataStore<'a> {
    /// Create a new metadata store using the given Git interface.
    pub fn new(git: &'a Git) -> Self {
        Self { git }
    }

    /// Get the ref name for a branch's metadata.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::core::metadata::store::MetadataStore;
    /// use latticework::core::types::BranchName;
    ///
    /// let branch = BranchName::new("feature-a").unwrap();
    /// let refname = MetadataStore::ref_name(&branch);
    /// assert_eq!(refname.as_str(), "refs/branch-metadata/feature-a");
    /// ```
    pub fn ref_name(branch: &BranchName) -> RefName {
        RefName::for_metadata(branch)
    }

    /// Read metadata for a branch.
    ///
    /// Returns `Ok(None)` if the branch has no metadata (not tracked).
    /// Returns `Ok(Some(entry))` with the metadata and ref OID for CAS.
    ///
    /// # Errors
    ///
    /// - [`StoreError::ParseError`] if the metadata JSON is malformed
    /// - [`StoreError::MetadataError`] if the metadata fails validation
    /// - [`StoreError::GitError`] for Git operation failures
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(entry) = store.read(&branch)? {
    ///     println!("Parent: {}", entry.metadata.parent.name());
    ///     println!("Frozen: {}", entry.metadata.freeze.is_frozen());
    /// } else {
    ///     println!("Branch is not tracked");
    /// }
    /// ```
    pub fn read(&self, branch: &BranchName) -> Result<Option<MetadataEntry>, StoreError> {
        let refname = Self::ref_name(branch);

        // Try to resolve the metadata ref to its blob OID
        // Note: We use try_resolve_ref_to_object instead of try_resolve_ref
        // because metadata refs point to blobs, not commits
        let ref_oid = match self.git.try_resolve_ref_to_object(refname.as_str())? {
            Some(oid) => oid,
            None => return Ok(None),
        };

        // Read the blob content as UTF-8 string
        let json = self.git.read_blob_as_string(&ref_oid)?;

        // Parse with strict validation
        let metadata = parse_metadata(&json)?;

        Ok(Some(MetadataEntry { ref_oid, metadata }))
    }

    /// Write metadata for a branch with CAS semantics.
    ///
    /// The update only succeeds if the ref's current value matches `expected_old`.
    /// Pass `None` for `expected_old` when creating new metadata (ref must not exist).
    ///
    /// # Arguments
    ///
    /// * `branch` - The branch to write metadata for
    /// * `expected_old` - Expected current ref OID, or `None` if creating
    /// * `metadata` - The metadata to write
    ///
    /// # Returns
    ///
    /// The new ref OID (blob OID) on success.
    ///
    /// # Errors
    ///
    /// - [`StoreError::CasFailed`] if the current ref doesn't match expected_old
    /// - [`StoreError::SerializeError`] if the metadata can't be serialized
    /// - [`StoreError::GitError`] for Git operation failures
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Create new metadata (must not exist)
    /// let new_oid = store.write_cas(&branch, None, &metadata)?;
    ///
    /// // Update existing metadata (must match expected)
    /// let entry = store.read(&branch)?.unwrap();
    /// let mut updated = entry.metadata;
    /// updated.touch();
    /// store.write_cas(&branch, Some(&entry.ref_oid), &updated)?;
    /// ```
    pub fn write_cas(
        &self,
        branch: &BranchName,
        expected_old: Option<&Oid>,
        metadata: &BranchMetadataV1,
    ) -> Result<Oid, StoreError> {
        let refname = Self::ref_name(branch);

        // Serialize to canonical JSON
        let json = metadata
            .to_canonical_json()
            .map_err(|e| StoreError::SerializeError(e.to_string()))?;

        // Write blob to repository
        let blob_oid = self.git.write_blob(json.as_bytes())?;

        // Update ref with CAS semantics
        self.git
            .update_ref_cas(
                refname.as_str(),
                &blob_oid,
                expected_old,
                &format!("lattice: update metadata for {}", branch),
            )
            .map_err(|e| match e {
                GitError::CasFailed {
                    expected, actual, ..
                } => StoreError::CasFailed { expected, actual },
                other => StoreError::GitError(other),
            })?;

        Ok(blob_oid)
    }

    /// Delete metadata for a branch with CAS semantics.
    ///
    /// The delete only succeeds if the ref's current value matches `expected_old`.
    ///
    /// # Arguments
    ///
    /// * `branch` - The branch to delete metadata for
    /// * `expected_old` - Expected current ref OID
    ///
    /// # Errors
    ///
    /// - [`StoreError::CasFailed`] if the current ref doesn't match expected_old
    /// - [`StoreError::NotFound`] if the metadata ref doesn't exist
    /// - [`StoreError::GitError`] for Git operation failures
    ///
    /// # Example
    ///
    /// ```ignore
    /// let entry = store.read(&branch)?.unwrap();
    /// store.delete_cas(&branch, &entry.ref_oid)?;
    /// ```
    pub fn delete_cas(&self, branch: &BranchName, expected_old: &Oid) -> Result<(), StoreError> {
        let refname = Self::ref_name(branch);

        self.git
            .delete_ref_cas(refname.as_str(), expected_old)
            .map_err(|e| match e {
                GitError::CasFailed {
                    expected, actual, ..
                } => StoreError::CasFailed { expected, actual },
                GitError::RefNotFound { refname } => StoreError::NotFound(refname),
                other => StoreError::GitError(other),
            })?;

        Ok(())
    }

    /// List all branches with metadata.
    ///
    /// Returns a list of branch names that have metadata refs.
    ///
    /// # Example
    ///
    /// ```ignore
    /// for branch in store.list()? {
    ///     println!("Tracked branch: {}", branch);
    /// }
    /// ```
    pub fn list(&self) -> Result<Vec<BranchName>, StoreError> {
        let refs = self.git.list_metadata_refs()?;
        Ok(refs.into_iter().map(|(name, _)| name).collect())
    }

    /// List all metadata entries with their ref OIDs.
    ///
    /// This returns the branch name and ref OID for each tracked branch,
    /// without loading the full metadata. Useful for bulk operations.
    ///
    /// # Example
    ///
    /// ```ignore
    /// for (branch, oid) in store.list_with_oids()? {
    ///     println!("{}: {}", branch, oid.short(7));
    /// }
    /// ```
    pub fn list_with_oids(&self) -> Result<Vec<(BranchName, Oid)>, StoreError> {
        Ok(self.git.list_metadata_refs()?)
    }

    /// Check if a branch has metadata (is tracked).
    ///
    /// This is more efficient than `read()` when you only need to know
    /// if the branch is tracked, not the actual metadata.
    ///
    /// # Example
    ///
    /// ```ignore
    /// if store.exists(&branch)? {
    ///     println!("Branch is tracked by Lattice");
    /// }
    /// ```
    pub fn exists(&self, branch: &BranchName) -> Result<bool, StoreError> {
        let refname = Self::ref_name(branch);
        Ok(self.git.ref_exists(refname.as_str()))
    }

    /// Read multiple branch metadata entries at once.
    ///
    /// Returns a vector of `(branch, Option<MetadataEntry>)` pairs.
    /// Branches without metadata will have `None`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let branches = vec![branch_a, branch_b, branch_c];
    /// for (branch, entry) in store.read_many(&branches)? {
    ///     match entry {
    ///         Some(e) => println!("{}: tracked", branch),
    ///         None => println!("{}: not tracked", branch),
    ///     }
    /// }
    /// ```
    pub fn read_many(
        &self,
        branches: &[BranchName],
    ) -> Result<Vec<(BranchName, Option<MetadataEntry>)>, StoreError> {
        let mut results = Vec::with_capacity(branches.len());
        for branch in branches {
            let entry = self.read(branch)?;
            results.push((branch.clone(), entry));
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::metadata::schema::{FreezeScope, FreezeState, PrState};

    // Note: Full integration tests are in tests/persistence_integration.rs
    // These unit tests cover error handling and type conversions

    #[test]
    fn ref_name_construction() {
        let branch = BranchName::new("feature-a").unwrap();
        let refname = MetadataStore::ref_name(&branch);
        assert_eq!(refname.as_str(), "refs/branch-metadata/feature-a");
    }

    #[test]
    fn ref_name_with_slashes() {
        let branch = BranchName::new("user/feature-a").unwrap();
        let refname = MetadataStore::ref_name(&branch);
        assert_eq!(refname.as_str(), "refs/branch-metadata/user/feature-a");
    }

    #[test]
    fn store_error_display() {
        let err = StoreError::NotFound("feature".into());
        assert!(err.to_string().contains("feature"));

        let err = StoreError::CasFailed {
            expected: "abc".into(),
            actual: "def".into(),
        };
        assert!(err.to_string().contains("CAS"));
        assert!(err.to_string().contains("abc"));
        assert!(err.to_string().contains("def"));

        let err = StoreError::ParseError("invalid json".into());
        assert!(err.to_string().contains("parse"));

        let err = StoreError::SerializeError("cannot serialize".into());
        assert!(err.to_string().contains("serialize"));
    }

    #[test]
    fn metadata_entry_debug() {
        let branch = BranchName::new("test").unwrap();
        let parent = BranchName::new("main").unwrap();
        let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();

        let entry = MetadataEntry {
            ref_oid: oid.clone(),
            metadata: BranchMetadataV1::new(branch, parent, oid),
        };

        let debug_str = format!("{:?}", entry);
        assert!(debug_str.contains("MetadataEntry"));
        assert!(debug_str.contains("ref_oid"));
        assert!(debug_str.contains("metadata"));
    }

    mod metadata_serialization {
        use super::*;

        fn sample_oid() -> Oid {
            Oid::new("abc123def4567890abc123def4567890abc12345").unwrap()
        }

        #[test]
        fn basic_metadata_roundtrip() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::new(branch, parent, sample_oid());

            let json = meta.to_canonical_json().unwrap();
            let parsed = parse_metadata(&json).unwrap();

            assert_eq!(meta.branch.name, parsed.branch.name);
            assert_eq!(meta.base.oid, parsed.base.oid);
        }

        #[test]
        fn metadata_with_freeze_state() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::builder(branch, parent, sample_oid())
                .freeze_state(FreezeState::frozen(
                    FreezeScope::DownstackInclusive,
                    Some("teammate branch".into()),
                ))
                .build();

            let json = meta.to_canonical_json().unwrap();
            let parsed = parse_metadata(&json).unwrap();

            assert!(parsed.freeze.is_frozen());
        }

        #[test]
        fn metadata_with_pr_state() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::builder(branch, parent, sample_oid())
                .pr_state(PrState::linked(
                    "github",
                    42,
                    "https://github.com/org/repo/pull/42",
                ))
                .build();

            let json = meta.to_canonical_json().unwrap();
            let parsed = parse_metadata(&json).unwrap();

            assert!(parsed.pr.is_linked());
            assert_eq!(parsed.pr.number(), Some(42));
        }
    }
}
