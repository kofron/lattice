//! core::types
//!
//! Strong types for core domain concepts.
//!
//! # Types
//!
//! - [`BranchName`] - Validated Git branch name
//! - [`Oid`] - Git object identifier (SHA)
//! - [`RefName`] - Validated Git reference name
//! - [`UtcTimestamp`] - RFC3339 timestamp
//! - [`Fingerprint`] - Repository state hash for divergence detection
//!
//! # Validation
//!
//! These types enforce validity at construction time. Invalid values
//! cannot be represented, preventing entire classes of bugs.
//!
//! # Examples
//!
//! ```
//! use lattice::core::types::{BranchName, Oid, RefName};
//!
//! // Valid constructions
//! let branch = BranchName::new("feature/my-branch").unwrap();
//! let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
//! let refname = RefName::for_branch(&branch);
//!
//! // Invalid constructions fail at creation time
//! assert!(BranchName::new("invalid..name").is_err());
//! assert!(Oid::new("not-a-sha").is_err());
//! ```

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Errors from type validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TypeError {
    #[error("invalid branch name: {0}")]
    InvalidBranchName(String),

    #[error("invalid object id: {0}")]
    InvalidOid(String),

    #[error("invalid ref name: {0}")]
    InvalidRefName(String),
}

/// A validated Git branch name.
///
/// Branch names must conform to Git's refname rules (see `git check-ref-format`):
/// - Cannot be empty
/// - Cannot start with `.` or `-`
/// - Cannot end with `.lock` or `/`
/// - Cannot contain `..`, `@{`, `//`, or ASCII control characters
/// - Cannot contain spaces, `~`, `^`, `:`, `\`, `?`, `*`, `[`
/// - Cannot be exactly `@`
///
/// # Example
///
/// ```
/// use lattice::core::types::BranchName;
///
/// // Valid branch names
/// let name = BranchName::new("feature/my-branch").unwrap();
/// assert_eq!(name.as_str(), "feature/my-branch");
///
/// let with_at = BranchName::new("user@feature").unwrap();
/// assert_eq!(with_at.as_str(), "user@feature");
///
/// // Invalid branch names
/// assert!(BranchName::new("").is_err());
/// assert!(BranchName::new(".hidden").is_err());
/// assert!(BranchName::new("branch.lock").is_err());
/// assert!(BranchName::new("has space").is_err());
/// assert!(BranchName::new("@").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct BranchName(String);

impl BranchName {
    /// Create a new validated branch name.
    ///
    /// # Errors
    ///
    /// Returns `TypeError::InvalidBranchName` if the name violates Git's refname rules.
    pub fn new(name: impl Into<String>) -> Result<Self, TypeError> {
        let name = name.into();
        Self::validate(&name)?;
        Ok(Self(name))
    }

    /// Validate a branch name against Git's refname rules.
    fn validate(name: &str) -> Result<(), TypeError> {
        // Cannot be empty
        if name.is_empty() {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot be empty".into(),
            ));
        }

        // Cannot be exactly "@" (reserved)
        if name == "@" {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot be '@' (reserved)".into(),
            ));
        }

        // Cannot start with '.' or '-'
        if name.starts_with('.') {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot start with '.'".into(),
            ));
        }
        if name.starts_with('-') {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot start with '-'".into(),
            ));
        }

        // Cannot end with ".lock" or "/"
        if name.ends_with(".lock") {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot end with '.lock'".into(),
            ));
        }
        if name.ends_with('/') {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot end with '/'".into(),
            ));
        }

        // Cannot contain "..", "@{", or "//"
        if name.contains("..") {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot contain '..'".into(),
            ));
        }
        if name.contains("@{") {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot contain '@{'".into(),
            ));
        }
        if name.contains("//") {
            return Err(TypeError::InvalidBranchName(
                "branch name cannot contain '//'".into(),
            ));
        }

        // Cannot contain certain special characters
        const INVALID_CHARS: [char; 8] = [' ', '~', '^', ':', '\\', '?', '*', '['];
        for c in INVALID_CHARS {
            if name.contains(c) {
                return Err(TypeError::InvalidBranchName(format!(
                    "branch name cannot contain '{c}'"
                )));
            }
        }

        // Cannot contain ASCII control characters (0x00-0x1F or 0x7F)
        for c in name.chars() {
            if c.is_ascii_control() {
                return Err(TypeError::InvalidBranchName(
                    "branch name cannot contain control characters".into(),
                ));
            }
        }

        // Check each component (split by /) for component-specific rules
        for component in name.split('/') {
            if component.is_empty() {
                // This would mean "//" which is already caught, or leading/trailing "/"
                continue;
            }
            if component.starts_with('.') {
                return Err(TypeError::InvalidBranchName(
                    "path component cannot start with '.'".into(),
                ));
            }
            if component.ends_with(".lock") {
                return Err(TypeError::InvalidBranchName(
                    "path component cannot end with '.lock'".into(),
                ));
            }
        }

        Ok(())
    }

    /// Get the branch name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BranchName {
    type Error = TypeError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<BranchName> for String {
    fn from(name: BranchName) -> Self {
        name.0
    }
}

impl AsRef<str> for BranchName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Git object identifier (SHA-1 or SHA-256).
///
/// OIDs are normalized to lowercase for consistency.
///
/// # Example
///
/// ```
/// use lattice::core::types::Oid;
///
/// // Create from hex string (normalized to lowercase)
/// let oid = Oid::new("ABC123DEF4567890ABC123DEF4567890ABC12345").unwrap();
/// assert_eq!(oid.as_str(), "abc123def4567890abc123def4567890abc12345");
///
/// // Get abbreviated form
/// assert_eq!(oid.short(7), "abc123d");
///
/// // Zero OID for null references
/// let zero = Oid::zero();
/// assert!(zero.is_zero());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Oid(String);

impl Oid {
    /// The zero OID (40 zeros for SHA-1).
    const ZERO_SHA1: &'static str = "0000000000000000000000000000000000000000";

    /// Create a new validated object id.
    ///
    /// The OID is normalized to lowercase.
    ///
    /// # Errors
    ///
    /// Returns `TypeError::InvalidOid` if the string is not a valid hex OID.
    pub fn new(oid: impl Into<String>) -> Result<Self, TypeError> {
        let oid = oid.into().to_ascii_lowercase();
        Self::validate(&oid)?;
        Ok(Self(oid))
    }

    /// Create the zero/null OID (40 zeros).
    ///
    /// This represents a null reference in Git.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::core::types::Oid;
    ///
    /// let zero = Oid::zero();
    /// assert!(zero.is_zero());
    /// assert_eq!(zero.as_str().len(), 40);
    /// ```
    pub fn zero() -> Self {
        Self(Self::ZERO_SHA1.to_string())
    }

    /// Check if this is the zero/null OID.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::core::types::Oid;
    ///
    /// let zero = Oid::zero();
    /// assert!(zero.is_zero());
    ///
    /// let non_zero = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
    /// assert!(!non_zero.is_zero());
    /// ```
    pub fn is_zero(&self) -> bool {
        self.0.chars().all(|c| c == '0')
    }

    /// Get an abbreviated form of the OID.
    ///
    /// Returns the first `len` characters. If `len` exceeds the OID length,
    /// returns the full OID.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::core::types::Oid;
    ///
    /// let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
    /// assert_eq!(oid.short(7), "abc123d");
    /// assert_eq!(oid.short(4), "abc1");
    /// ```
    pub fn short(&self, len: usize) -> &str {
        let end = len.min(self.0.len());
        &self.0[..end]
    }

    /// Validate an object id.
    fn validate(oid: &str) -> Result<(), TypeError> {
        // SHA-1 is 40 hex chars, SHA-256 is 64
        if oid.len() != 40 && oid.len() != 64 {
            return Err(TypeError::InvalidOid(format!(
                "expected 40 or 64 hex characters, got {}",
                oid.len()
            )));
        }
        if !oid.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(TypeError::InvalidOid(
                "object id must be hexadecimal".into(),
            ));
        }
        Ok(())
    }

    /// Get the object id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Oid {
    type Error = TypeError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<Oid> for String {
    fn from(oid: Oid) -> Self {
        oid.0
    }
}

impl AsRef<str> for Oid {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Oid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A validated Git reference name.
///
/// Reference names must conform to Git's refname rules (see `git check-ref-format`).
///
/// # Example
///
/// ```
/// use lattice::core::types::{BranchName, RefName};
///
/// // Create ref for a branch
/// let branch = BranchName::new("feature/foo").unwrap();
/// let refname = RefName::for_branch(&branch);
/// assert_eq!(refname.as_str(), "refs/heads/feature/foo");
///
/// // Create metadata ref
/// let meta_ref = RefName::for_metadata(&branch);
/// assert_eq!(meta_ref.as_str(), "refs/branch-metadata/feature/foo");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RefName(String);

impl RefName {
    /// Create a new validated ref name.
    ///
    /// # Errors
    ///
    /// Returns `TypeError::InvalidRefName` if the name violates Git's refname rules.
    pub fn new(name: impl Into<String>) -> Result<Self, TypeError> {
        let name = name.into();
        Self::validate(&name)?;
        Ok(Self(name))
    }

    /// Create a ref name for a branch (`refs/heads/<branch>`).
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::core::types::{BranchName, RefName};
    ///
    /// let branch = BranchName::new("main").unwrap();
    /// let refname = RefName::for_branch(&branch);
    /// assert_eq!(refname.as_str(), "refs/heads/main");
    /// ```
    pub fn for_branch(branch: &BranchName) -> Self {
        // Safe because branch names are validated and refs/heads/ prefix is valid
        Self(format!("refs/heads/{}", branch.as_str()))
    }

    /// Create a metadata ref name for a branch (`refs/branch-metadata/<branch>`).
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::core::types::{BranchName, RefName};
    ///
    /// let branch = BranchName::new("feature/foo").unwrap();
    /// let refname = RefName::for_metadata(&branch);
    /// assert_eq!(refname.as_str(), "refs/branch-metadata/feature/foo");
    /// ```
    pub fn for_metadata(branch: &BranchName) -> Self {
        // Safe because branch names are validated
        Self(format!("refs/branch-metadata/{}", branch.as_str()))
    }

    /// Strip a prefix from the ref name and return the remainder.
    ///
    /// Returns `None` if the ref doesn't start with the given prefix.
    ///
    /// # Example
    ///
    /// ```
    /// use lattice::core::types::RefName;
    ///
    /// let refname = RefName::new("refs/heads/feature/foo").unwrap();
    /// assert_eq!(refname.strip_prefix("refs/heads/"), Some("feature/foo"));
    /// assert_eq!(refname.strip_prefix("refs/tags/"), None);
    /// ```
    pub fn strip_prefix(&self, prefix: &str) -> Option<&str> {
        self.0.strip_prefix(prefix)
    }

    /// Check if this ref is under the branch metadata namespace.
    pub fn is_metadata_ref(&self) -> bool {
        self.0.starts_with("refs/branch-metadata/")
    }

    /// Check if this ref is a branch ref.
    pub fn is_branch_ref(&self) -> bool {
        self.0.starts_with("refs/heads/")
    }

    /// Validate a ref name against Git's refname rules.
    fn validate(name: &str) -> Result<(), TypeError> {
        // Cannot be empty
        if name.is_empty() {
            return Err(TypeError::InvalidRefName("ref name cannot be empty".into()));
        }

        // Cannot start with "/"
        if name.starts_with('/') {
            return Err(TypeError::InvalidRefName(
                "ref name cannot start with '/'".into(),
            ));
        }

        // Cannot end with "/" or ".lock"
        if name.ends_with('/') {
            return Err(TypeError::InvalidRefName(
                "ref name cannot end with '/'".into(),
            ));
        }
        if name.ends_with(".lock") {
            return Err(TypeError::InvalidRefName(
                "ref name cannot end with '.lock'".into(),
            ));
        }

        // Cannot contain "..", "@{", or "//"
        if name.contains("..") {
            return Err(TypeError::InvalidRefName(
                "ref name cannot contain '..'".into(),
            ));
        }
        if name.contains("@{") {
            return Err(TypeError::InvalidRefName(
                "ref name cannot contain '@{'".into(),
            ));
        }
        if name.contains("//") {
            return Err(TypeError::InvalidRefName(
                "ref name cannot contain '//'".into(),
            ));
        }

        // Cannot contain certain special characters
        const INVALID_CHARS: [char; 8] = [' ', '~', '^', ':', '\\', '?', '*', '['];
        for c in INVALID_CHARS {
            if name.contains(c) {
                return Err(TypeError::InvalidRefName(format!(
                    "ref name cannot contain '{c}'"
                )));
            }
        }

        // Cannot contain ASCII control characters
        for c in name.chars() {
            if c.is_ascii_control() {
                return Err(TypeError::InvalidRefName(
                    "ref name cannot contain control characters".into(),
                ));
            }
        }

        // Check each component
        for component in name.split('/') {
            if component.is_empty() {
                continue;
            }
            if component.starts_with('.') {
                return Err(TypeError::InvalidRefName(
                    "path component cannot start with '.'".into(),
                ));
            }
            if component.ends_with(".lock") {
                return Err(TypeError::InvalidRefName(
                    "path component cannot end with '.lock'".into(),
                ));
            }
        }

        Ok(())
    }

    /// Get the ref name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RefName {
    type Error = TypeError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<RefName> for String {
    fn from(name: RefName) -> Self {
        name.0
    }
}

impl AsRef<str> for RefName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RefName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A UTC timestamp in RFC3339 format.
///
/// # Example
///
/// ```
/// use lattice::core::types::UtcTimestamp;
///
/// let now = UtcTimestamp::now();
/// println!("Current time: {}", now);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtcTimestamp(chrono::DateTime<chrono::Utc>);

impl UtcTimestamp {
    /// Create a timestamp for the current moment.
    pub fn now() -> Self {
        Self(chrono::Utc::now())
    }

    /// Create a timestamp from a chrono DateTime.
    pub fn from_datetime(dt: chrono::DateTime<chrono::Utc>) -> Self {
        Self(dt)
    }

    /// Get the underlying datetime.
    pub fn as_datetime(&self) -> &chrono::DateTime<chrono::Utc> {
        &self.0
    }
}

impl std::fmt::Display for UtcTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.to_rfc3339())
    }
}

/// A stable hash over repository ref state for divergence detection.
///
/// Per ARCHITECTURE.md Section 7.1, the fingerprint is computed over a stable
/// set of ref values to detect out-of-band changes between Lattice operations.
///
/// # Example
///
/// ```
/// use lattice::core::types::{Fingerprint, RefName, Oid};
///
/// let refs = vec![
///     (RefName::new("refs/heads/main").unwrap(),
///      Oid::new("abc123def4567890abc123def4567890abc12345").unwrap()),
///     (RefName::new("refs/heads/feature").unwrap(),
///      Oid::new("def456abc7890123def456abc7890123def45678").unwrap()),
/// ];
///
/// let fp = Fingerprint::compute(&refs);
/// println!("Fingerprint: {}", fp);
///
/// // Same refs produce same fingerprint
/// let fp2 = Fingerprint::compute(&refs);
/// assert_eq!(fp, fp2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Compute a fingerprint from a set of (refname, oid) pairs.
    ///
    /// The refs are sorted by refname before hashing to ensure determinism
    /// regardless of input order.
    pub fn compute(refs: &[(RefName, Oid)]) -> Self {
        let mut sorted: Vec<_> = refs.iter().collect();
        sorted.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

        let mut hasher = Sha256::new();
        for (refname, oid) in sorted {
            hasher.update(refname.as_str().as_bytes());
            hasher.update(b"\0");
            hasher.update(oid.as_str().as_bytes());
            hasher.update(b"\n");
        }

        let result = hasher.finalize();
        Self(hex::encode(result))
    }

    /// Get the fingerprint as a hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod branch_name {
        use super::*;

        #[test]
        fn valid_branch_names() {
            assert!(BranchName::new("main").is_ok());
            assert!(BranchName::new("feature/foo").is_ok());
            assert!(BranchName::new("fix-123").is_ok());
            assert!(BranchName::new("user@feature").is_ok());
            assert!(BranchName::new("CamelCase").is_ok());
            assert!(BranchName::new("with.dot").is_ok());
            assert!(BranchName::new("a/b/c/d").is_ok());
        }

        #[test]
        fn empty_name_rejected() {
            assert!(BranchName::new("").is_err());
        }

        #[test]
        fn starts_with_dot_rejected() {
            assert!(BranchName::new(".hidden").is_err());
            assert!(BranchName::new("foo/.hidden").is_err());
        }

        #[test]
        fn starts_with_dash_rejected() {
            assert!(BranchName::new("-flag").is_err());
        }

        #[test]
        fn ends_with_lock_rejected() {
            assert!(BranchName::new("branch.lock").is_err());
            assert!(BranchName::new("foo/bar.lock").is_err());
        }

        #[test]
        fn ends_with_slash_rejected() {
            assert!(BranchName::new("branch/").is_err());
        }

        #[test]
        fn double_dot_rejected() {
            assert!(BranchName::new("bad..path").is_err());
        }

        #[test]
        fn at_brace_rejected() {
            assert!(BranchName::new("foo@{bar").is_err());
        }

        #[test]
        fn double_slash_rejected() {
            assert!(BranchName::new("foo//bar").is_err());
        }

        #[test]
        fn reserved_at_rejected() {
            assert!(BranchName::new("@").is_err());
        }

        #[test]
        fn special_chars_rejected() {
            assert!(BranchName::new("has space").is_err());
            assert!(BranchName::new("has~tilde").is_err());
            assert!(BranchName::new("has^caret").is_err());
            assert!(BranchName::new("has:colon").is_err());
            assert!(BranchName::new("has\\backslash").is_err());
            assert!(BranchName::new("has?question").is_err());
            assert!(BranchName::new("has*star").is_err());
            assert!(BranchName::new("has[bracket").is_err());
        }

        #[test]
        fn control_chars_rejected() {
            assert!(BranchName::new("has\ttab").is_err());
            assert!(BranchName::new("has\nnewline").is_err());
            assert!(BranchName::new("has\x7fDEL").is_err());
        }

        #[test]
        fn serde_roundtrip() {
            let name = BranchName::new("feature/test").unwrap();
            let json = serde_json::to_string(&name).unwrap();
            let parsed: BranchName = serde_json::from_str(&json).unwrap();
            assert_eq!(name, parsed);
        }
    }

    mod oid {
        use super::*;

        #[test]
        fn valid_sha1() {
            assert!(Oid::new("abc123def4567890abc123def4567890abc12345").is_ok());
        }

        #[test]
        fn valid_sha256() {
            // SHA-256 is exactly 64 hex characters
            let sha256 = "abc123def4567890abc123def4567890abc123def4567890abc123def456789a";
            assert_eq!(sha256.len(), 64);
            assert!(Oid::new(sha256).is_ok());
        }

        #[test]
        fn normalizes_to_lowercase() {
            let oid = Oid::new("ABC123DEF4567890ABC123DEF4567890ABC12345").unwrap();
            assert_eq!(oid.as_str(), "abc123def4567890abc123def4567890abc12345");
        }

        #[test]
        fn zero_oid() {
            let zero = Oid::zero();
            assert!(zero.is_zero());
            assert_eq!(zero.as_str().len(), 40);
            assert!(zero.as_str().chars().all(|c| c == '0'));
        }

        #[test]
        fn non_zero_is_not_zero() {
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            assert!(!oid.is_zero());
        }

        #[test]
        fn short_form() {
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            assert_eq!(oid.short(7), "abc123d");
            assert_eq!(oid.short(4), "abc1");
            assert_eq!(oid.short(100), oid.as_str()); // Exceeds length
        }

        #[test]
        fn invalid_length() {
            assert!(Oid::new("").is_err());
            assert!(Oid::new("tooshort").is_err());
            assert!(Oid::new("abc123").is_err());
        }

        #[test]
        fn non_hex_rejected() {
            // 'x', 'y', 'z' are not valid hex
            assert!(Oid::new("xyz123def4567890abc123def4567890abc12345").is_err());
        }

        #[test]
        fn serde_roundtrip() {
            let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();
            let json = serde_json::to_string(&oid).unwrap();
            let parsed: Oid = serde_json::from_str(&json).unwrap();
            assert_eq!(oid, parsed);
        }
    }

    mod ref_name {
        use super::*;

        #[test]
        fn valid_refs() {
            assert!(RefName::new("refs/heads/main").is_ok());
            assert!(RefName::new("refs/tags/v1.0").is_ok());
            assert!(RefName::new("refs/branch-metadata/feature").is_ok());
        }

        #[test]
        fn for_branch() {
            let branch = BranchName::new("feature/foo").unwrap();
            let refname = RefName::for_branch(&branch);
            assert_eq!(refname.as_str(), "refs/heads/feature/foo");
            assert!(refname.is_branch_ref());
            assert!(!refname.is_metadata_ref());
        }

        #[test]
        fn for_metadata() {
            let branch = BranchName::new("feature/foo").unwrap();
            let refname = RefName::for_metadata(&branch);
            assert_eq!(refname.as_str(), "refs/branch-metadata/feature/foo");
            assert!(refname.is_metadata_ref());
            assert!(!refname.is_branch_ref());
        }

        #[test]
        fn strip_prefix() {
            let refname = RefName::new("refs/heads/feature/foo").unwrap();
            assert_eq!(refname.strip_prefix("refs/heads/"), Some("feature/foo"));
            assert_eq!(refname.strip_prefix("refs/tags/"), None);
        }

        #[test]
        fn empty_rejected() {
            assert!(RefName::new("").is_err());
        }

        #[test]
        fn starts_with_slash_rejected() {
            assert!(RefName::new("/refs/heads/main").is_err());
        }

        #[test]
        fn ends_with_slash_rejected() {
            assert!(RefName::new("refs/heads/").is_err());
        }

        #[test]
        fn ends_with_lock_rejected() {
            assert!(RefName::new("refs/heads/main.lock").is_err());
        }

        #[test]
        fn double_dot_rejected() {
            assert!(RefName::new("refs/heads/bad..name").is_err());
        }

        #[test]
        fn double_slash_rejected() {
            assert!(RefName::new("refs//heads/main").is_err());
        }

        #[test]
        fn serde_roundtrip() {
            let refname = RefName::new("refs/heads/main").unwrap();
            let json = serde_json::to_string(&refname).unwrap();
            let parsed: RefName = serde_json::from_str(&json).unwrap();
            assert_eq!(refname, parsed);
        }
    }

    mod fingerprint {
        use super::*;

        #[test]
        fn deterministic() {
            let refs = vec![
                (
                    RefName::new("refs/heads/main").unwrap(),
                    Oid::new("abc123def4567890abc123def4567890abc12345").unwrap(),
                ),
                (
                    RefName::new("refs/heads/feature").unwrap(),
                    Oid::new("def456abc7890123def456abc7890123def45678").unwrap(),
                ),
            ];

            let fp1 = Fingerprint::compute(&refs);
            let fp2 = Fingerprint::compute(&refs);
            assert_eq!(fp1, fp2);
        }

        #[test]
        fn order_independent() {
            let refs1 = vec![
                (
                    RefName::new("refs/heads/main").unwrap(),
                    Oid::new("abc123def4567890abc123def4567890abc12345").unwrap(),
                ),
                (
                    RefName::new("refs/heads/feature").unwrap(),
                    Oid::new("def456abc7890123def456abc7890123def45678").unwrap(),
                ),
            ];

            let refs2 = vec![
                (
                    RefName::new("refs/heads/feature").unwrap(),
                    Oid::new("def456abc7890123def456abc7890123def45678").unwrap(),
                ),
                (
                    RefName::new("refs/heads/main").unwrap(),
                    Oid::new("abc123def4567890abc123def4567890abc12345").unwrap(),
                ),
            ];

            let fp1 = Fingerprint::compute(&refs1);
            let fp2 = Fingerprint::compute(&refs2);
            assert_eq!(fp1, fp2);
        }

        #[test]
        fn different_refs_different_fingerprint() {
            let refs1 = vec![(
                RefName::new("refs/heads/main").unwrap(),
                Oid::new("abc123def4567890abc123def4567890abc12345").unwrap(),
            )];

            let refs2 = vec![(
                RefName::new("refs/heads/main").unwrap(),
                Oid::new("def456abc7890123def456abc7890123def45678").unwrap(),
            )];

            let fp1 = Fingerprint::compute(&refs1);
            let fp2 = Fingerprint::compute(&refs2);
            assert_ne!(fp1, fp2);
        }

        #[test]
        fn empty_refs() {
            let refs: Vec<(RefName, Oid)> = vec![];
            let fp = Fingerprint::compute(&refs);
            assert!(!fp.as_str().is_empty());
        }
    }

    mod utc_timestamp {
        use super::*;

        #[test]
        fn now_works() {
            let ts = UtcTimestamp::now();
            assert!(ts.to_string().contains('T'));
        }

        #[test]
        fn serde_roundtrip() {
            let ts = UtcTimestamp::now();
            let json = serde_json::to_string(&ts).unwrap();
            let parsed: UtcTimestamp = serde_json::from_str(&json).unwrap();
            assert_eq!(ts, parsed);
        }
    }
}
