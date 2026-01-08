//! forge
//!
//! Abstraction for remote forges (GitHub, GitLab, etc.).
//!
//! # Architecture
//!
//! The `Forge` trait defines the interface for interacting with remote
//! hosting services. Commands use the [`create_forge`] factory function
//! rather than importing specific forge implementations directly.
//!
//! Per ARCHITECTURE.md Section 11:
//! - Forge operations are invoked only after local structural invariants are satisfied
//! - Forge failures do not compromise local correctness
//! - Forge results are written only to cached metadata fields
//!
//! # Modules
//!
//! - `traits`: Core `Forge` trait and request/response types
//! - [`github`]: GitHub implementation using REST and GraphQL APIs
//! - `gitlab`: GitLab stub (requires `gitlab` feature)
//! - [`mock`]: Mock implementation for deterministic testing
//! - `factory`: Forge selection and creation
//!
//! # Example
//!
//! ```ignore
//! use latticework::forge::{create_forge, Forge, CreatePrRequest};
//!
//! // Create a forge from remote URL (auto-detects provider)
//! let forge = create_forge(
//!     "git@github.com:owner/repo.git",
//!     token,
//!     None,  // No provider override
//! )?;
//!
//! // Create a PR
//! let pr = forge.create_pr(CreatePrRequest {
//!     head: "feature".to_string(),
//!     base: "main".to_string(),
//!     title: "Add feature".to_string(),
//!     body: None,
//!     draft: false,
//! }).await?;
//!
//! println!("Created PR #{}: {}", pr.number, pr.url);
//! ```

mod factory;
pub mod github;
#[cfg(feature = "gitlab")]
pub mod gitlab;
pub mod mock;
mod traits;

pub use factory::{create_forge, detect_provider, valid_forge_names, ForgeProvider};
pub use traits::*;
