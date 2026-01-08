//! git
//!
//! Single interface for all Git operations.
//!
//! # Architecture
//!
//! This module is the **ONLY doorway** to Git. All repository reads and writes
//! flow through this interface. Direct parsing of `.git` internal files
//! outside this module is prohibited. No other module should import `git2`.
//!
//! Per ROADMAP.md reconciliation 0.4, we use the `git2` crate exclusively
//! (no shelling out to the git CLI).
//!
//! # Responsibilities
//!
//! - Repository discovery and opening
//! - Ref operations (read, CAS update, delete)
//! - Object operations (read blob, write blob)
//! - Ancestry queries (merge-base, is-ancestor)
//! - Status and state detection
//! - Remote URL parsing
//!
//! # Invariants
//!
//! - All ref updates use CAS (compare-and-swap) semantics
//! - No other module calls git2 directly
//! - All operations return strong types (Oid, BranchName, RefName)
//!
//! # Example
//!
//! ```ignore
//! use lattice::git::Git;
//! use std::path::Path;
//!
//! let git = Git::open(Path::new("."))?;
//!
//! // Query operations
//! let oid = git.resolve_ref("refs/heads/main")?;
//! let branches = git.list_branches()?;
//!
//! // CAS update (fails if ref changed since read)
//! git.update_ref_cas(
//!     "refs/branch-metadata/feature",
//!     &new_oid,
//!     Some(&old_oid),
//!     "lattice: update metadata"
//! )?;
//! ```

mod interface;

pub use interface::{CommitInfo, Git, GitError, GitState, RefEntry, RepoInfo, WorktreeStatus};
