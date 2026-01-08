//! core::naming
//!
//! Branch naming rules and validation.
//!
//! # Features
//!
//! - Generate branch names from commit messages
//! - Validate branch name format
//! - Apply configured naming conventions

/// Generate a branch name slug from a commit message.
///
/// Converts the first line of a commit message into a valid branch name:
/// - Lowercase
/// - Spaces become hyphens
/// - Remove invalid characters
/// - Truncate to reasonable length
///
/// # Example
///
/// ```
/// use latticework::core::naming::slugify;
///
/// assert_eq!(slugify("Add user authentication"), "add-user-authentication");
/// assert_eq!(slugify("Fix bug #123"), "fix-bug-123");
/// ```
pub fn slugify(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or("");

    first_line
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == ' ' || c == '_' {
                '-'
            } else {
                // Skip invalid characters
                '\0'
            }
        })
        .filter(|&c| c != '\0')
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(50) // Reasonable max length
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("fix: something"), "fix-something");
        assert_eq!(slugify("Add feature"), "add-feature");
    }

    #[test]
    fn slugify_removes_invalid_chars() {
        assert_eq!(slugify("Fix bug [WIP]"), "fix-bug-wip");
        // Note: `/` is removed (not replaced) since it's not a valid branch name character
        assert_eq!(slugify("Test: foo/bar"), "test-foobar");
    }

    #[test]
    fn slugify_handles_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn slugify_uses_first_line() {
        assert_eq!(slugify("First line\nSecond line"), "first-line");
    }
}
