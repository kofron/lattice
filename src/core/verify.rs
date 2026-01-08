//! core::verify
//!
//! Fast repository and metadata verification.
//!
//! # Modes
//!
//! - **Fast verify**: Default at start/end of mutating commands
//!   - Ensure parseability
//!   - Ensure acyclic graph
//!   - Ensure refs exist
//!   - Ensure base ancestry constraints
//!
//! - **Full verify**: Optional deep verification
//!   - Also validates children sets
//!   - Optionally validates PR linkage consistency
//!
//! # Invariants
//!
//! - Never mutates the repository
//! - Must be deterministic

use super::graph::StackGraph;
use thiserror::Error;

/// Errors from verification.
#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("cycle detected in stack graph at branch: {0}")]
    CycleDetected(String),

    #[error("tracked branch does not exist: {0}")]
    BranchMissing(String),

    #[error("base commit not found: {0}")]
    BaseMissing(String),

    #[error("base commit is not ancestor of branch tip")]
    BaseNotAncestor,

    #[error("metadata parse error: {0}")]
    MetadataParseError(String),
}

/// Result of fast verification.
#[derive(Debug)]
pub struct VerifyResult {
    /// Whether verification passed
    pub ok: bool,
    /// Errors found during verification
    pub errors: Vec<VerifyError>,
}

impl VerifyResult {
    /// Create a successful result.
    pub fn success() -> Self {
        Self {
            ok: true,
            errors: vec![],
        }
    }

    /// Create a failed result with errors.
    pub fn failure(errors: Vec<VerifyError>) -> Self {
        Self { ok: false, errors }
    }
}

/// Perform fast verification of the stack graph.
///
/// This is a stub implementation for Milestone 0.
pub fn fast_verify(graph: &StackGraph) -> VerifyResult {
    let mut errors = Vec::new();

    // Check for cycles
    if let Some(branch) = graph.find_cycle() {
        errors.push(VerifyError::CycleDetected(branch.to_string()));
    }

    // Future: check refs exist, base ancestry, etc.

    if errors.is_empty() {
        VerifyResult::success()
    } else {
        VerifyResult::failure(errors)
    }
}
