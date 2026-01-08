//! core::metadata::schema
//!
//! Branch metadata schema (v1).
//!
//! # Schema Design
//!
//! Per SPEC.md Appendix A, metadata is:
//! - Self-describing with `kind` and `schema_version`
//! - Structured with no boolean blindness
//! - Strictly parsed (unknown fields rejected)
//!
//! # Structural vs Cached
//!
//! Per ARCHITECTURE.md Section 3.2.1:
//! - **Structural**: parent, base, frozen (correctness-critical)
//! - **Cached**: PR linkage (may be stale, never justifies structural changes)
//!
//! # Example
//!
//! ```
//! use latticework::core::metadata::schema::{BranchMetadataV1, parse_metadata, METADATA_KIND};
//! use latticework::core::types::{BranchName, Oid};
//!
//! // Create metadata for a new branch
//! let branch = BranchName::new("feature-a").unwrap();
//! let parent = BranchName::new("main").unwrap();
//! let base = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
//!
//! let meta = BranchMetadataV1::new(branch, parent, base);
//! assert_eq!(meta.kind, METADATA_KIND);
//!
//! // Serialize and parse back
//! let json = serde_json::to_string(&meta).unwrap();
//! let parsed = parse_metadata(&json).unwrap();
//! assert_eq!(parsed.branch.name, "feature-a");
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::types::{BranchName, Oid, TypeError, UtcTimestamp};

/// The kind identifier for branch metadata.
pub const METADATA_KIND: &str = "lattice.branch-metadata";

/// Current schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// Errors from metadata operations.
#[derive(Debug, Error)]
pub enum MetadataError {
    #[error("failed to parse metadata: {0}")]
    ParseError(String),

    #[error("invalid kind '{found}', expected '{}'", METADATA_KIND)]
    InvalidKind { found: String },

    #[error("unsupported schema version {0}, supported: {SCHEMA_VERSION}")]
    UnsupportedVersion(u32),

    #[error("invalid metadata value: {0}")]
    InvalidValue(String),

    #[error("type validation failed: {0}")]
    TypeError(#[from] TypeError),
}

/// Envelope for version dispatch before full parsing.
///
/// This allows us to check the schema version before attempting
/// to parse the full metadata structure.
#[derive(Debug, Deserialize)]
struct MetadataEnvelope {
    kind: String,
    schema_version: u32,
}

/// Parse metadata JSON with version dispatch.
///
/// This function checks the schema version and dispatches to the
/// appropriate parser. Currently only v1 is supported.
///
/// # Errors
///
/// Returns an error if:
/// - The JSON is malformed
/// - The `kind` field doesn't match `METADATA_KIND`
/// - The `schema_version` is not supported
/// - Any field values are invalid
///
/// # Example
///
/// ```
/// use latticework::core::metadata::schema::parse_metadata;
///
/// let json = r#"{
///     "kind": "lattice.branch-metadata",
///     "schema_version": 1,
///     "branch": { "name": "feature" },
///     "parent": { "kind": "trunk", "name": "main" },
///     "base": { "oid": "abc123def4567890abc123def4567890abc12345" },
///     "freeze": { "state": "unfrozen" },
///     "pr": { "state": "none" },
///     "timestamps": {
///         "created_at": "2024-01-01T00:00:00Z",
///         "updated_at": "2024-01-01T00:00:00Z"
///     }
/// }"#;
///
/// let meta = parse_metadata(json).unwrap();
/// assert_eq!(meta.branch.name, "feature");
/// ```
pub fn parse_metadata(json: &str) -> Result<BranchMetadataV1, MetadataError> {
    // First, extract envelope to check version
    let envelope: MetadataEnvelope =
        serde_json::from_str(json).map_err(|e| MetadataError::ParseError(e.to_string()))?;

    // Validate kind
    if envelope.kind != METADATA_KIND {
        return Err(MetadataError::InvalidKind {
            found: envelope.kind,
        });
    }

    // Dispatch based on version
    match envelope.schema_version {
        1 => {
            let meta: BranchMetadataV1 =
                serde_json::from_str(json).map_err(|e| MetadataError::ParseError(e.to_string()))?;
            meta.validate()?;
            Ok(meta)
        }
        v => Err(MetadataError::UnsupportedVersion(v)),
    }
}

/// Branch metadata (v1).
///
/// This is the complete metadata stored for each tracked branch.
/// Use [`parse_metadata`] to parse from JSON with validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BranchMetadataV1 {
    /// Kind identifier (always "lattice.branch-metadata")
    pub kind: String,

    /// Schema version (always 1 for this struct)
    pub schema_version: u32,

    /// The branch this metadata describes
    pub branch: BranchInfo,

    /// Parent branch information
    pub parent: ParentInfo,

    /// Base commit information
    pub base: BaseInfo,

    /// Freeze state
    pub freeze: FreezeState,

    /// PR linkage state (cached, not structural)
    pub pr: PrState,

    /// Timestamps
    pub timestamps: Timestamps,
}

impl BranchMetadataV1 {
    /// Create new metadata for a branch.
    ///
    /// The parent is set as a regular branch. Use [`BranchMetadataBuilder`]
    /// for more control over the metadata structure.
    pub fn new(branch: BranchName, parent: BranchName, base_oid: Oid) -> Self {
        let now = UtcTimestamp::now();
        Self {
            kind: METADATA_KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            branch: BranchInfo {
                name: branch.to_string(),
            },
            parent: ParentInfo::Branch {
                name: parent.to_string(),
            },
            base: BaseInfo {
                oid: base_oid.to_string(),
            },
            freeze: FreezeState::Unfrozen,
            pr: PrState::None,
            timestamps: Timestamps {
                created_at: now.clone(),
                updated_at: now,
            },
        }
    }

    /// Create a builder for constructing metadata with more options.
    pub fn builder(branch: BranchName, parent: BranchName, base_oid: Oid) -> BranchMetadataBuilder {
        BranchMetadataBuilder::new(branch, parent, base_oid)
    }

    /// Validate the metadata structure.
    ///
    /// This checks that:
    /// - `kind` matches `METADATA_KIND`
    /// - `schema_version` equals `SCHEMA_VERSION`
    /// - Branch name is valid
    /// - Parent name is valid
    /// - Base OID is valid
    pub fn validate(&self) -> Result<(), MetadataError> {
        // Validate kind
        if self.kind != METADATA_KIND {
            return Err(MetadataError::InvalidKind {
                found: self.kind.clone(),
            });
        }

        // Validate version
        if self.schema_version != SCHEMA_VERSION {
            return Err(MetadataError::UnsupportedVersion(self.schema_version));
        }

        // Validate branch name
        BranchName::new(&self.branch.name)?;

        // Validate parent name
        BranchName::new(self.parent.name())?;

        // Validate base OID
        Oid::new(&self.base.oid)?;

        Ok(())
    }

    /// Extract structural metadata with validation.
    ///
    /// This returns only the correctness-critical fields (parent, base, frozen)
    /// with validated types. Per ARCHITECTURE.md, cached fields (like PR linkage)
    /// must not be used to justify structural changes.
    ///
    /// # Errors
    ///
    /// Returns an error if any structural field contains invalid data.
    pub fn into_structural(self) -> Result<StructuralMetadata, MetadataError> {
        let parent = BranchName::new(self.parent.name())?;
        let base = Oid::new(&self.base.oid)?;
        let frozen = self.freeze.is_frozen();

        Ok(StructuralMetadata {
            parent,
            base,
            frozen,
        })
    }

    /// Get a reference view of structural metadata.
    ///
    /// Unlike [`into_structural`](Self::into_structural), this doesn't validate
    /// the types. Use this for quick access when you know the metadata is valid.
    pub fn structural_view(&self) -> StructuralView<'_> {
        StructuralView {
            parent: &self.parent,
            base: &self.base,
            freeze: &self.freeze,
        }
    }

    /// Update the `updated_at` timestamp to now.
    pub fn touch(&mut self) {
        self.timestamps.updated_at = UtcTimestamp::now();
    }

    /// Serialize to canonical JSON (compact, deterministic).
    ///
    /// This produces a stable JSON string suitable for fingerprinting.
    pub fn to_canonical_json(&self) -> Result<String, MetadataError> {
        serde_json::to_string(self).map_err(|e| MetadataError::ParseError(e.to_string()))
    }
}

/// Builder for constructing branch metadata with more options.
pub struct BranchMetadataBuilder {
    branch: BranchName,
    parent: BranchName,
    base_oid: Oid,
    parent_is_trunk: bool,
    freeze_state: FreezeState,
    pr_state: PrState,
}

impl BranchMetadataBuilder {
    /// Create a new builder with required fields.
    pub fn new(branch: BranchName, parent: BranchName, base_oid: Oid) -> Self {
        Self {
            branch,
            parent,
            base_oid,
            parent_is_trunk: false,
            freeze_state: FreezeState::Unfrozen,
            pr_state: PrState::None,
        }
    }

    /// Mark the parent as trunk.
    pub fn parent_is_trunk(mut self) -> Self {
        self.parent_is_trunk = true;
        self
    }

    /// Set the freeze state.
    pub fn freeze_state(mut self, state: FreezeState) -> Self {
        self.freeze_state = state;
        self
    }

    /// Set the PR state.
    pub fn pr_state(mut self, state: PrState) -> Self {
        self.pr_state = state;
        self
    }

    /// Build the metadata.
    pub fn build(self) -> BranchMetadataV1 {
        let now = UtcTimestamp::now();

        let parent = if self.parent_is_trunk {
            ParentInfo::Trunk {
                name: self.parent.to_string(),
            }
        } else {
            ParentInfo::Branch {
                name: self.parent.to_string(),
            }
        };

        BranchMetadataV1 {
            kind: METADATA_KIND.to_string(),
            schema_version: SCHEMA_VERSION,
            branch: BranchInfo {
                name: self.branch.to_string(),
            },
            parent,
            base: BaseInfo {
                oid: self.base_oid.to_string(),
            },
            freeze: self.freeze_state,
            pr: self.pr_state,
            timestamps: Timestamps {
                created_at: now.clone(),
                updated_at: now,
            },
        }
    }
}

/// Structural metadata (correctness-critical fields only).
///
/// Per ARCHITECTURE.md Section 3.2.1, structural fields define the stack graph.
/// Cached fields must not be used to justify structural changes.
///
/// This struct contains validated types, unlike [`StructuralView`] which
/// holds references to the raw data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuralMetadata {
    /// Parent branch name (validated)
    pub parent: BranchName,
    /// Base commit OID (validated)
    pub base: Oid,
    /// Whether the branch is frozen
    pub frozen: bool,
}

/// Reference view of structural metadata.
///
/// This provides quick access to structural fields without validation
/// or cloning. Use [`BranchMetadataV1::into_structural`] when you need
/// validated types.
#[derive(Debug)]
pub struct StructuralView<'a> {
    /// Parent information
    pub parent: &'a ParentInfo,
    /// Base commit
    pub base: &'a BaseInfo,
    /// Freeze state
    pub freeze: &'a FreezeState,
}

/// Branch identification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BranchInfo {
    /// Branch name
    pub name: String,
}

/// Parent branch information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParentInfo {
    /// Parent is another branch
    Branch { name: String },
    /// Parent is trunk
    Trunk { name: String },
}

impl ParentInfo {
    /// Get the parent branch name.
    pub fn name(&self) -> &str {
        match self {
            ParentInfo::Branch { name } | ParentInfo::Trunk { name } => name,
        }
    }

    /// Check if the parent is trunk.
    pub fn is_trunk(&self) -> bool {
        matches!(self, ParentInfo::Trunk { .. })
    }
}

/// Base commit information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BaseInfo {
    /// Object id of the base commit
    pub oid: String,
}

/// Freeze state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FreezeState {
    /// Branch is not frozen
    Unfrozen,
    /// Branch is frozen
    Frozen {
        /// Scope of the freeze
        scope: FreezeScope,
        /// Reason for freezing
        reason: Option<String>,
        /// When the branch was frozen
        frozen_at: UtcTimestamp,
    },
}

impl FreezeState {
    /// Check if the branch is frozen.
    pub fn is_frozen(&self) -> bool {
        matches!(self, FreezeState::Frozen { .. })
    }

    /// Create a frozen state with the given scope.
    pub fn frozen(scope: FreezeScope, reason: Option<String>) -> Self {
        FreezeState::Frozen {
            scope,
            reason,
            frozen_at: UtcTimestamp::now(),
        }
    }
}

/// Scope of a freeze.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FreezeScope {
    /// Only this branch is frozen
    Single,
    /// This branch and all downstack ancestors are frozen
    DownstackInclusive,
}

/// PR linkage state (cached, not structural).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrState {
    /// No PR linked
    None,
    /// PR is linked
    Linked {
        /// Forge name (e.g., "github")
        forge: String,
        /// PR number
        number: u64,
        /// PR URL
        url: String,
        /// Last known PR status (cached)
        last_known: Option<PrStatusCache>,
    },
}

impl PrState {
    /// Create a linked PR state.
    pub fn linked(forge: &str, number: u64, url: &str) -> Self {
        PrState::Linked {
            forge: forge.to_string(),
            number,
            url: url.to_string(),
            last_known: None,
        }
    }

    /// Check if a PR is linked.
    pub fn is_linked(&self) -> bool {
        matches!(self, PrState::Linked { .. })
    }

    /// Get the PR number if linked.
    pub fn number(&self) -> Option<u64> {
        match self {
            PrState::Linked { number, .. } => Some(*number),
            PrState::None => None,
        }
    }
}

/// Cached PR status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrStatusCache {
    /// PR state (open, closed, merged)
    pub state: String,
    /// Whether the PR is a draft
    pub is_draft: bool,
}

/// Timestamps.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Timestamps {
    /// When the metadata was created
    pub created_at: UtcTimestamp,
    /// When the metadata was last updated
    pub updated_at: UtcTimestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_oid() -> Oid {
        Oid::new("abc123def4567890abc123def4567890abc12345").unwrap()
    }

    mod parse_metadata_fn {
        use super::*;

        #[test]
        fn valid_metadata() {
            let json = r#"{
                "kind": "lattice.branch-metadata",
                "schema_version": 1,
                "branch": { "name": "feature" },
                "parent": { "kind": "trunk", "name": "main" },
                "base": { "oid": "abc123def4567890abc123def4567890abc12345" },
                "freeze": { "state": "unfrozen" },
                "pr": { "state": "none" },
                "timestamps": {
                    "created_at": "2024-01-01T00:00:00Z",
                    "updated_at": "2024-01-01T00:00:00Z"
                }
            }"#;

            let meta = parse_metadata(json).unwrap();
            assert_eq!(meta.branch.name, "feature");
            assert!(meta.parent.is_trunk());
        }

        #[test]
        fn invalid_kind() {
            let json = r#"{
                "kind": "wrong-kind",
                "schema_version": 1
            }"#;

            let result = parse_metadata(json);
            assert!(matches!(result, Err(MetadataError::InvalidKind { .. })));
        }

        #[test]
        fn unsupported_version() {
            let json = r#"{
                "kind": "lattice.branch-metadata",
                "schema_version": 99
            }"#;

            let result = parse_metadata(json);
            assert!(matches!(result, Err(MetadataError::UnsupportedVersion(99))));
        }

        #[test]
        fn invalid_branch_name() {
            let json = r#"{
                "kind": "lattice.branch-metadata",
                "schema_version": 1,
                "branch": { "name": "invalid..name" },
                "parent": { "kind": "trunk", "name": "main" },
                "base": { "oid": "abc123def4567890abc123def4567890abc12345" },
                "freeze": { "state": "unfrozen" },
                "pr": { "state": "none" },
                "timestamps": {
                    "created_at": "2024-01-01T00:00:00Z",
                    "updated_at": "2024-01-01T00:00:00Z"
                }
            }"#;

            let result = parse_metadata(json);
            assert!(matches!(result, Err(MetadataError::TypeError(_))));
        }

        #[test]
        fn invalid_oid() {
            let json = r#"{
                "kind": "lattice.branch-metadata",
                "schema_version": 1,
                "branch": { "name": "feature" },
                "parent": { "kind": "trunk", "name": "main" },
                "base": { "oid": "not-a-valid-oid" },
                "freeze": { "state": "unfrozen" },
                "pr": { "state": "none" },
                "timestamps": {
                    "created_at": "2024-01-01T00:00:00Z",
                    "updated_at": "2024-01-01T00:00:00Z"
                }
            }"#;

            let result = parse_metadata(json);
            assert!(matches!(result, Err(MetadataError::TypeError(_))));
        }

        #[test]
        fn unknown_fields_rejected() {
            let json = r#"{
                "kind": "lattice.branch-metadata",
                "schema_version": 1,
                "branch": { "name": "feature" },
                "parent": { "kind": "trunk", "name": "main" },
                "base": { "oid": "abc123def4567890abc123def4567890abc12345" },
                "freeze": { "state": "unfrozen" },
                "pr": { "state": "none" },
                "timestamps": {
                    "created_at": "2024-01-01T00:00:00Z",
                    "updated_at": "2024-01-01T00:00:00Z"
                },
                "unknown_field": true
            }"#;

            let result = parse_metadata(json);
            assert!(matches!(result, Err(MetadataError::ParseError(_))));
        }
    }

    mod branch_metadata_v1 {
        use super::*;

        #[test]
        fn new_creates_valid_metadata() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::new(branch, parent, sample_oid());

            assert_eq!(meta.kind, METADATA_KIND);
            assert_eq!(meta.schema_version, SCHEMA_VERSION);
            assert_eq!(meta.branch.name, "feature");
            assert!(!meta.freeze.is_frozen());
            assert!(!meta.pr.is_linked());
        }

        #[test]
        fn roundtrip() {
            let branch = BranchName::new("feature-a").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::new(branch, parent, sample_oid());

            let json = serde_json::to_string_pretty(&meta).unwrap();
            let parsed: BranchMetadataV1 = serde_json::from_str(&json).unwrap();

            assert_eq!(meta, parsed);
        }

        #[test]
        fn validate_catches_bad_kind() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let mut meta = BranchMetadataV1::new(branch, parent, sample_oid());
            meta.kind = "wrong".to_string();

            assert!(meta.validate().is_err());
        }

        #[test]
        fn into_structural() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();
            let oid = sample_oid();

            let meta = BranchMetadataV1::new(branch, parent.clone(), oid.clone());
            let structural = meta.into_structural().unwrap();

            assert_eq!(structural.parent, parent);
            assert_eq!(structural.base, oid);
            assert!(!structural.frozen);
        }

        #[test]
        fn canonical_json_is_deterministic() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta1 = BranchMetadataV1::new(branch.clone(), parent.clone(), sample_oid());

            // Create another with same data but different timestamp
            // Since we can't control the timestamp, we'll just verify the format is stable
            let json1 = meta1.to_canonical_json().unwrap();
            let json2 = meta1.to_canonical_json().unwrap();

            assert_eq!(json1, json2);
        }
    }

    mod builder {
        use super::*;

        #[test]
        fn basic_build() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::builder(branch, parent, sample_oid()).build();

            assert_eq!(meta.branch.name, "feature");
            assert!(!meta.parent.is_trunk());
        }

        #[test]
        fn with_trunk_parent() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::builder(branch, parent, sample_oid())
                .parent_is_trunk()
                .build();

            assert!(meta.parent.is_trunk());
        }

        #[test]
        fn with_frozen_state() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::builder(branch, parent, sample_oid())
                .freeze_state(FreezeState::frozen(
                    FreezeScope::Single,
                    Some("testing".to_string()),
                ))
                .build();

            assert!(meta.freeze.is_frozen());
        }

        #[test]
        fn with_pr_state() {
            let branch = BranchName::new("feature").unwrap();
            let parent = BranchName::new("main").unwrap();

            let meta = BranchMetadataV1::builder(branch, parent, sample_oid())
                .pr_state(PrState::linked(
                    "github",
                    42,
                    "https://github.com/org/repo/pull/42",
                ))
                .build();

            assert!(meta.pr.is_linked());
            assert_eq!(meta.pr.number(), Some(42));
        }
    }

    mod freeze_state {
        use super::*;

        #[test]
        fn unfrozen() {
            let state = FreezeState::Unfrozen;
            assert!(!state.is_frozen());
        }

        #[test]
        fn frozen() {
            let state = FreezeState::frozen(FreezeScope::Single, None);
            assert!(state.is_frozen());
        }

        #[test]
        fn frozen_with_reason() {
            let state = FreezeState::frozen(FreezeScope::DownstackInclusive, Some("test".into()));
            assert!(state.is_frozen());

            if let FreezeState::Frozen { reason, scope, .. } = state {
                assert_eq!(reason, Some("test".to_string()));
                assert_eq!(scope, FreezeScope::DownstackInclusive);
            } else {
                panic!("Expected frozen state");
            }
        }
    }

    mod pr_state {
        use super::*;

        #[test]
        fn none() {
            let state = PrState::None;
            assert!(!state.is_linked());
            assert_eq!(state.number(), None);
        }

        #[test]
        fn linked() {
            let state = PrState::linked("github", 123, "https://example.com");
            assert!(state.is_linked());
            assert_eq!(state.number(), Some(123));
        }
    }

    mod parent_info {
        use super::*;

        #[test]
        fn branch_parent() {
            let parent = ParentInfo::Branch {
                name: "feature-a".to_string(),
            };
            assert_eq!(parent.name(), "feature-a");
            assert!(!parent.is_trunk());
        }

        #[test]
        fn trunk_parent() {
            let parent = ParentInfo::Trunk {
                name: "main".to_string(),
            };
            assert_eq!(parent.name(), "main");
            assert!(parent.is_trunk());
        }
    }
}
