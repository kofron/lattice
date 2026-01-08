//! core::metadata
//!
//! Branch metadata schema and storage.
//!
//! # Modules
//!
//! - [`schema`] - Metadata schema types (v1)
//! - [`store`] - Metadata storage in refs
//!
//! # Architecture
//!
//! Metadata is stored as Git refs under `refs/branch-metadata/<branch>`.
//! Each ref points to a blob containing JSON.
//!
//! # Schema Design
//!
//! - Self-describing: includes `kind` and `schema_version`
//! - No boolean blindness: uses enums/structs instead of optionals
//! - Strict parsing: unknown fields are rejected
//!
//! # Example
//!
//! ```
//! use lattice::core::metadata::schema::{BranchMetadataV1, parse_metadata};
//! use lattice::core::types::{BranchName, Oid};
//!
//! let branch = BranchName::new("feature").unwrap();
//! let parent = BranchName::new("main").unwrap();
//! let base = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
//!
//! let meta = BranchMetadataV1::new(branch, parent, base);
//! let json = serde_json::to_string(&meta).unwrap();
//! let parsed = parse_metadata(&json).unwrap();
//! ```

pub mod schema;
pub mod store;

// Re-export commonly used types
pub use schema::{
    parse_metadata, BranchMetadataV1, FreezeScope, FreezeState, MetadataError, ParentInfo, PrState,
    StructuralMetadata, METADATA_KIND, SCHEMA_VERSION,
};
pub use store::{MetadataEntry, MetadataStore, StoreError, METADATA_REF_PREFIX};
