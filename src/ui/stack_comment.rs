//! ui::stack_comment
//!
//! Pure functions for generating stack comment tables for PR descriptions.
//!
//! # Design
//!
//! Per CLAUDE.md principles, this module contains only pure functions that:
//! - Take immutable inputs (stack info, current branch, PR linkage)
//! - Return formatted strings
//! - Have no side effects
//!
//! The stack comment is appended to PR descriptions with HTML comment markers
//! that allow it to be regenerated on subsequent submits while preserving
//! any user-provided description above the markers.
//!
//! # Example Output
//!
//! ```markdown
//! <!-- lattice:stack:start -->
//!
//! ### Stack
//!
//! | | Branch | PR |
//! |---|--------|-----|
//! | ⬆️ | `feature-a` | [#10](https://github.com/org/repo/pull/10) |
//! | 👉 | `feature-b` | [#11](https://github.com/org/repo/pull/11) |
//! | ⬇️ | `feature-c` | ❓ |
//!
//! <!-- lattice:stack:end -->
//! ```

/// Marker indicating the start of the stack comment section.
pub const STACK_MARKER_START: &str = "<!-- lattice:stack:start -->";

/// Marker indicating the end of the stack comment section.
pub const STACK_MARKER_END: &str = "<!-- lattice:stack:end -->";

/// Position of a branch relative to the current PR's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackPosition {
    /// Branch is an ancestor (closer to trunk, shown above in stack)
    Ancestor,
    /// This is the current branch
    Current,
    /// Branch is a descendant (stacked on current, shown below)
    Descendant,
}

impl StackPosition {
    /// Get the emoji indicator for this position.
    ///
    /// - Ancestor: ⬆️ (up arrow)
    /// - Current: 👉 (pointing hand)
    /// - Descendant: ⬇️ (down arrow)
    pub fn indicator(&self) -> &'static str {
        match self {
            StackPosition::Ancestor => "⬆️",
            StackPosition::Current => "👉",
            StackPosition::Descendant => "⬇️",
        }
    }
}

/// Information about a single branch in the stack.
#[derive(Debug, Clone)]
pub struct StackBranchInfo {
    /// Branch name
    pub name: String,
    /// PR number if linked, None if not submitted
    pub pr_number: Option<u64>,
    /// PR URL if linked
    pub pr_url: Option<String>,
    /// Position relative to current branch
    pub position: StackPosition,
}

/// Input for generating a stack comment.
#[derive(Debug)]
pub struct StackCommentInput {
    /// All branches in the stack, ordered from top (closest to trunk) to bottom
    pub branches: Vec<StackBranchInfo>,
}

/// Generate the stack comment table.
///
/// This is a pure function that produces markdown content including
/// the start and end markers.
///
/// # Arguments
///
/// * `input` - Stack information with branch details
///
/// # Returns
///
/// A string containing the full stack comment section including markers.
///
/// # Example
///
/// ```
/// use lattice::ui::stack_comment::{generate_stack_comment, StackCommentInput, StackBranchInfo, StackPosition};
///
/// let input = StackCommentInput {
///     branches: vec![
///         StackBranchInfo {
///             name: "feature-a".to_string(),
///             pr_number: Some(10),
///             pr_url: Some("https://github.com/org/repo/pull/10".to_string()),
///             position: StackPosition::Ancestor,
///         },
///         StackBranchInfo {
///             name: "feature-b".to_string(),
///             pr_number: Some(11),
///             pr_url: Some("https://github.com/org/repo/pull/11".to_string()),
///             position: StackPosition::Current,
///         },
///     ],
/// };
///
/// let comment = generate_stack_comment(&input);
/// assert!(comment.contains("feature-a"));
/// assert!(comment.contains("[#10]"));
/// ```
pub fn generate_stack_comment(input: &StackCommentInput) -> String {
    let mut lines = vec![
        STACK_MARKER_START.to_string(),
        String::new(),
        "### Stack".to_string(),
        String::new(),
        "| | Branch | PR |".to_string(),
        "|---|--------|-----|".to_string(),
    ];

    for branch in &input.branches {
        let indicator = branch.position.indicator();
        let pr_cell = match (&branch.pr_number, &branch.pr_url) {
            (Some(num), Some(url)) => format!("[#{}]({})", num, url),
            (Some(num), None) => format!("#{}", num),
            (None, _) => "❓".to_string(),
        };

        lines.push(format!(
            "| {} | `{}` | {} |",
            indicator, branch.name, pr_cell
        ));
    }

    lines.push(String::new());
    lines.push(STACK_MARKER_END.to_string());

    lines.join("\n")
}

/// Merge a stack comment into an existing PR body.
///
/// If the body already contains a stack comment section (between markers),
/// it will be replaced. Otherwise, the stack comment is appended.
///
/// This function preserves any user-provided content:
/// - Content before the start marker is kept
/// - Content after the end marker is kept
/// - Only the section between markers is replaced
///
/// # Arguments
///
/// * `existing_body` - The current PR body (may be None or empty)
/// * `stack_comment` - The new stack comment to insert
///
/// # Returns
///
/// The merged body string.
///
/// # Example
///
/// ```
/// use lattice::ui::stack_comment::merge_stack_comment;
///
/// // Append to existing description
/// let result = merge_stack_comment(
///     Some("This PR adds a new feature."),
///     "<!-- lattice:stack:start -->\nstack\n<!-- lattice:stack:end -->"
/// );
/// assert!(result.starts_with("This PR adds a new feature."));
/// assert!(result.contains("stack"));
/// ```
pub fn merge_stack_comment(existing_body: Option<&str>, stack_comment: &str) -> String {
    let body = existing_body.unwrap_or("");

    // Handle empty/whitespace-only body
    if body.trim().is_empty() {
        return stack_comment.to_string();
    }

    // Check if markers exist (not inside code blocks)
    if let Some((before, after)) = find_marker_bounds(body) {
        // Replace existing section
        let before = before.trim_end();
        let after = after.trim_start();

        if before.is_empty() && after.is_empty() {
            stack_comment.to_string()
        } else if before.is_empty() {
            format!("{}\n\n{}", stack_comment, after)
        } else if after.is_empty() {
            format!("{}\n\n{}", before, stack_comment)
        } else {
            format!("{}\n\n{}\n\n{}", before, stack_comment, after)
        }
    } else {
        // Append to existing body
        format!("{}\n\n{}", body.trim_end(), stack_comment)
    }
}

/// Find the bounds of the stack comment section, excluding markers inside code blocks.
///
/// Returns `Some((before, after))` where `before` is content before the start marker
/// and `after` is content after the end marker. Returns `None` if valid markers not found.
fn find_marker_bounds(body: &str) -> Option<(&str, &str)> {
    // Find start marker, ensuring it's not inside a code block
    let start_idx = find_marker_outside_code_blocks(body, STACK_MARKER_START)?;

    // Find end marker after start, ensuring it's not inside a code block
    let search_start = start_idx + STACK_MARKER_START.len();
    let end_idx_relative =
        find_marker_outside_code_blocks(&body[search_start..], STACK_MARKER_END)?;
    let end_idx = search_start + end_idx_relative;

    // Ensure start comes before end
    if start_idx >= end_idx {
        return None;
    }

    let before = &body[..start_idx];
    let after = &body[end_idx + STACK_MARKER_END.len()..];

    Some((before, after))
}

/// Find a marker in text, but only if it's not inside a code block.
fn find_marker_outside_code_blocks(text: &str, marker: &str) -> Option<usize> {
    let mut in_code_block = false;
    let mut search_start = 0;

    for line in text.lines() {
        let line_start = text[search_start..]
            .find(line)
            .map(|i| search_start + i)
            .unwrap_or(search_start);

        // Check for code block fence
        if line.trim().starts_with("```") {
            in_code_block = !in_code_block;
        }

        // If not in code block, check for marker in this line
        if !in_code_block {
            if let Some(marker_offset) = line.find(marker) {
                return Some(line_start + marker_offset);
            }
        }

        search_start = line_start + line.len();
    }

    // Also check the last bit if there's trailing content
    if !in_code_block {
        if let Some(idx) = text.find(marker) {
            // Verify it's not in a code block by checking
            let text_before = &text[..idx];
            let fence_count = text_before.matches("```").count();
            if fence_count.is_multiple_of(2) {
                return Some(idx);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // =============================================================
    // Position indicator tests
    // =============================================================

    #[test]
    fn ancestor_indicator_is_up_arrow() {
        assert_eq!(StackPosition::Ancestor.indicator(), "⬆️");
    }

    #[test]
    fn current_indicator_is_pointing_hand() {
        assert_eq!(StackPosition::Current.indicator(), "👉");
    }

    #[test]
    fn descendant_indicator_is_down_arrow() {
        assert_eq!(StackPosition::Descendant.indicator(), "⬇️");
    }

    // =============================================================
    // Stack comment generation tests
    // =============================================================

    #[test]
    fn generate_single_branch_stack() {
        let input = StackCommentInput {
            branches: vec![StackBranchInfo {
                name: "feature".to_string(),
                pr_number: Some(1),
                pr_url: Some("https://github.com/org/repo/pull/1".to_string()),
                position: StackPosition::Current,
            }],
        };

        let result = generate_stack_comment(&input);
        assert!(result.contains(STACK_MARKER_START));
        assert!(result.contains(STACK_MARKER_END));
        assert!(result.contains("### Stack"));
        assert!(result.contains("`feature`"));
        assert!(result.contains("[#1]"));
        assert!(result.contains("👉"));
    }

    #[test]
    fn generate_linear_stack() {
        let input = StackCommentInput {
            branches: vec![
                StackBranchInfo {
                    name: "ancestor".to_string(),
                    pr_number: Some(1),
                    pr_url: Some("url1".to_string()),
                    position: StackPosition::Ancestor,
                },
                StackBranchInfo {
                    name: "current".to_string(),
                    pr_number: Some(2),
                    pr_url: Some("url2".to_string()),
                    position: StackPosition::Current,
                },
                StackBranchInfo {
                    name: "descendant".to_string(),
                    pr_number: Some(3),
                    pr_url: Some("url3".to_string()),
                    position: StackPosition::Descendant,
                },
            ],
        };

        let result = generate_stack_comment(&input);
        assert!(result.contains("⬆️"));
        assert!(result.contains("👉"));
        assert!(result.contains("⬇️"));

        // Check order: ancestor should come before current, current before descendant
        let ancestor_pos = result.find("ancestor").unwrap();
        let current_pos = result.find("current").unwrap();
        let descendant_pos = result.find("descendant").unwrap();
        assert!(ancestor_pos < current_pos);
        assert!(current_pos < descendant_pos);
    }

    #[test]
    fn generate_multi_ancestor_stack() {
        let input = StackCommentInput {
            branches: vec![
                StackBranchInfo {
                    name: "grandparent".to_string(),
                    pr_number: Some(1),
                    pr_url: Some("url1".to_string()),
                    position: StackPosition::Ancestor,
                },
                StackBranchInfo {
                    name: "parent".to_string(),
                    pr_number: Some(2),
                    pr_url: Some("url2".to_string()),
                    position: StackPosition::Ancestor,
                },
                StackBranchInfo {
                    name: "current".to_string(),
                    pr_number: Some(3),
                    pr_url: Some("url3".to_string()),
                    position: StackPosition::Current,
                },
            ],
        };

        let result = generate_stack_comment(&input);
        // Both ancestors should have up arrows
        assert_eq!(result.matches("⬆️").count(), 2);
        assert_eq!(result.matches("👉").count(), 1);
    }

    #[test]
    fn generate_multi_descendant_stack() {
        let input = StackCommentInput {
            branches: vec![
                StackBranchInfo {
                    name: "current".to_string(),
                    pr_number: Some(1),
                    pr_url: Some("url1".to_string()),
                    position: StackPosition::Current,
                },
                StackBranchInfo {
                    name: "child1".to_string(),
                    pr_number: Some(2),
                    pr_url: Some("url2".to_string()),
                    position: StackPosition::Descendant,
                },
                StackBranchInfo {
                    name: "child2".to_string(),
                    pr_number: Some(3),
                    pr_url: Some("url3".to_string()),
                    position: StackPosition::Descendant,
                },
            ],
        };

        let result = generate_stack_comment(&input);
        assert_eq!(result.matches("⬇️").count(), 2);
        assert_eq!(result.matches("👉").count(), 1);
    }

    #[test]
    fn generate_mixed_pr_states() {
        let input = StackCommentInput {
            branches: vec![
                StackBranchInfo {
                    name: "with-pr".to_string(),
                    pr_number: Some(10),
                    pr_url: Some("https://example.com/10".to_string()),
                    position: StackPosition::Ancestor,
                },
                StackBranchInfo {
                    name: "without-pr".to_string(),
                    pr_number: None,
                    pr_url: None,
                    position: StackPosition::Current,
                },
            ],
        };

        let result = generate_stack_comment(&input);
        assert!(result.contains("[#10](https://example.com/10)"));
        assert!(result.contains("❓"));
    }

    #[test]
    fn unsubmitted_branch_shows_question_mark() {
        let input = StackCommentInput {
            branches: vec![StackBranchInfo {
                name: "unsubmitted".to_string(),
                pr_number: None,
                pr_url: None,
                position: StackPosition::Current,
            }],
        };

        let result = generate_stack_comment(&input);
        assert!(result.contains("❓"));
        assert!(!result.contains("[#"));
    }

    #[test]
    fn pr_link_format_correct() {
        let input = StackCommentInput {
            branches: vec![StackBranchInfo {
                name: "feature".to_string(),
                pr_number: Some(123),
                pr_url: Some("https://github.com/org/repo/pull/123".to_string()),
                position: StackPosition::Current,
            }],
        };

        let result = generate_stack_comment(&input);
        assert!(result.contains("[#123](https://github.com/org/repo/pull/123)"));
    }

    // =============================================================
    // Body merging - empty/new cases
    // =============================================================

    #[test]
    fn merge_into_none_body() {
        let comment = "<!-- lattice:stack:start -->\ntest\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(None, comment);
        assert_eq!(result, comment);
    }

    #[test]
    fn merge_into_empty_string() {
        let comment = "<!-- lattice:stack:start -->\ntest\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(""), comment);
        assert_eq!(result, comment);
    }

    #[test]
    fn merge_into_whitespace_only() {
        let comment = "<!-- lattice:stack:start -->\ntest\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some("  \n  "), comment);
        assert_eq!(result, comment);
    }

    // =============================================================
    // Body merging - append cases
    // =============================================================

    #[test]
    fn merge_appends_to_user_description() {
        let existing = "This is my PR description.";
        let comment = "<!-- lattice:stack:start -->\ntest\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        assert!(result.starts_with("This is my PR description."));
        assert!(result.contains(comment));
    }

    #[test]
    fn merge_appends_with_proper_spacing() {
        let existing = "Description";
        let comment = "<!-- lattice:stack:start -->\ntest\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        assert!(result.contains("Description\n\n<!-- lattice:stack:start -->"));
    }

    #[test]
    fn merge_preserves_user_markdown() {
        let existing = "# Header\n\n- List item\n- Another item\n\n```rust\ncode\n```";
        let comment = "<!-- lattice:stack:start -->\ntest\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        assert!(result.contains("# Header"));
        assert!(result.contains("- List item"));
        assert!(result.contains("```rust\ncode\n```"));
    }

    // =============================================================
    // Body merging - replace cases
    // =============================================================

    #[test]
    fn merge_replaces_existing_stack_section() {
        let existing =
            "Description\n\n<!-- lattice:stack:start -->\nold content\n<!-- lattice:stack:end -->";
        let new_comment = "<!-- lattice:stack:start -->\nnew content\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), new_comment);
        assert!(result.contains("new content"));
        assert!(!result.contains("old content"));
    }

    #[test]
    fn merge_preserves_content_before_markers() {
        let existing = "My custom description\n\n<!-- lattice:stack:start -->\nold\n<!-- lattice:stack:end -->";
        let new_comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), new_comment);
        assert!(result.contains("My custom description"));
        assert!(result.contains("new"));
    }

    #[test]
    fn merge_preserves_content_after_markers() {
        let existing =
            "<!-- lattice:stack:start -->\nold\n<!-- lattice:stack:end -->\n\nFooter content";
        let new_comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), new_comment);
        assert!(result.contains("Footer content"));
        assert!(result.contains("new"));
    }

    #[test]
    fn merge_handles_content_both_sides() {
        let existing =
            "Header\n\n<!-- lattice:stack:start -->\nold\n<!-- lattice:stack:end -->\n\nFooter";
        let new_comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), new_comment);
        assert!(result.contains("Header"));
        assert!(result.contains("Footer"));
        assert!(result.contains("new"));
        assert!(!result.contains("old"));
    }

    // =============================================================
    // Body merging - edge cases
    // =============================================================

    #[test]
    fn merge_handles_only_start_marker() {
        let existing = "Content\n<!-- lattice:stack:start -->\norphaned";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        // Should append since markers are incomplete
        assert!(result.contains("Content"));
        assert!(result.ends_with(comment));
    }

    #[test]
    fn merge_handles_only_end_marker() {
        let existing = "Content\n<!-- lattice:stack:end -->\norphaned";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        // Should append since markers are incomplete
        assert!(result.contains("Content"));
    }

    #[test]
    fn merge_handles_reversed_markers() {
        let existing = "<!-- lattice:stack:end -->\ncontent\n<!-- lattice:stack:start -->";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        // Should append since markers are in wrong order
        assert!(result.ends_with(comment));
    }

    #[test]
    fn merge_handles_nested_markers() {
        let existing = "Before\n<!-- lattice:stack:start -->\nfirst\n<!-- lattice:stack:start -->\nnested\n<!-- lattice:stack:end -->\ninner\n<!-- lattice:stack:end -->\nAfter";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        // Should replace from first start to first end after it
        assert!(result.contains("Before"));
        assert!(result.contains("new"));
    }

    #[test]
    fn merge_handles_markers_in_code_blocks() {
        let existing = "Description\n\n```markdown\n<!-- lattice:stack:start -->\nexample\n<!-- lattice:stack:end -->\n```";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        // Should append since markers are inside code block
        assert!(result.contains("```markdown"));
        assert!(result.contains("example"));
        assert!(result.ends_with(comment));
    }

    // =============================================================
    // User modification scenarios
    // =============================================================

    #[test]
    fn merge_preserves_user_edits_above_markers() {
        let existing = "User added this later\n\n<!-- lattice:stack:start -->\nold\n<!-- lattice:stack:end -->";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        assert!(result.contains("User added this later"));
        assert!(result.contains("new"));
    }

    #[test]
    fn merge_preserves_user_edits_below_markers() {
        let existing =
            "<!-- lattice:stack:start -->\nold\n<!-- lattice:stack:end -->\n\nUser added footer";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        assert!(result.contains("User added footer"));
        assert!(result.contains("new"));
    }

    #[test]
    fn merge_preserves_user_edits_both_sides() {
        let existing = "User header\n\n<!-- lattice:stack:start -->\nold\n<!-- lattice:stack:end -->\n\nUser footer";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        assert!(result.contains("User header"));
        assert!(result.contains("User footer"));
        assert!(result.contains("new"));
        assert!(!result.contains("old"));
    }

    #[test]
    fn merge_handles_user_deleted_markers() {
        let existing = "User deleted the stack section and just has their description";
        let comment = "<!-- lattice:stack:start -->\nnew\n<!-- lattice:stack:end -->";
        let result = merge_stack_comment(Some(existing), comment);
        // Should append fresh stack section
        assert!(result.contains("User deleted the stack section"));
        assert!(result.contains("new"));
    }
}
