//! Property-based tests for core domain types.
//!
//! These tests use proptest to verify invariants hold across
//! randomly generated inputs.

use std::collections::HashMap;

use proptest::prelude::*;

use latticework::core::graph::StackGraph;
use latticework::core::metadata::schema::{parse_metadata, BranchMetadataV1};
use latticework::core::types::{BranchName, Fingerprint, Oid, RefName};

/// Strategy for generating valid branch name characters.
fn branch_name_char() -> impl Strategy<Value = char> {
    prop_oneof![
        // Alphanumeric - use prop::char::range for char ranges
        prop::char::range('a', 'z'),
        prop::char::range('A', 'Z'),
        prop::char::range('0', '9'),
        // Allowed special chars
        Just('-'),
        Just('_'),
        Just('.'),
        Just('/'),
    ]
}

/// Strategy for generating valid branch names.
fn valid_branch_name() -> impl Strategy<Value = String> {
    prop::collection::vec(branch_name_char(), 1..50).prop_filter_map(
        "must be valid branch name",
        |chars| {
            let name: String = chars.into_iter().collect();
            // Filter out names that would fail validation
            if name.is_empty()
                || name.starts_with('.')
                || name.starts_with('-')
                || name.ends_with('/')
                || name.ends_with(".lock")
                || name.contains("..")
                || name.contains("//")
                || name.contains("@{")
                || name == "@"
            {
                None
            } else {
                // Also check that no component starts with '.'
                if name
                    .split('/')
                    .any(|c| c.starts_with('.') || c.ends_with(".lock"))
                {
                    None
                } else {
                    Some(name)
                }
            }
        },
    )
}

/// Strategy for generating valid hex OIDs.
fn valid_oid_string() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
        ]),
        40,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    /// Any valid branch name round-trips through serde.
    #[test]
    fn branch_name_serde_roundtrip(name in valid_branch_name()) {
        let branch = BranchName::new(&name).unwrap();
        let json = serde_json::to_string(&branch).unwrap();
        let parsed: BranchName = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(branch, parsed);
    }

    /// Any valid OID round-trips through serde.
    #[test]
    fn oid_serde_roundtrip(oid_str in valid_oid_string()) {
        let oid = Oid::new(&oid_str).unwrap();
        let json = serde_json::to_string(&oid).unwrap();
        let parsed: Oid = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(oid, parsed);
    }

    /// OIDs are normalized to lowercase.
    #[test]
    fn oid_normalized_to_lowercase(oid_str in valid_oid_string()) {
        let upper = oid_str.to_uppercase();
        let oid = Oid::new(&upper).unwrap();
        prop_assert_eq!(oid.as_str(), oid_str.to_lowercase());
    }

    /// Fingerprint is deterministic for same input.
    #[test]
    fn fingerprint_deterministic(
        ref1 in "[a-z]{1,20}",
        oid1 in valid_oid_string(),
        ref2 in "[a-z]{1,20}",
        oid2 in valid_oid_string(),
    ) {
        // Skip if refs are invalid
        let r1 = match RefName::new(format!("refs/heads/{}", ref1)) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let r2 = match RefName::new(format!("refs/heads/{}", ref2)) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let o1 = Oid::new(&oid1).unwrap();
        let o2 = Oid::new(&oid2).unwrap();

        let refs = vec![(r1.clone(), o1.clone()), (r2.clone(), o2.clone())];

        let fp1 = Fingerprint::compute(&refs);
        let fp2 = Fingerprint::compute(&refs);

        prop_assert_eq!(fp1, fp2);
    }

    /// Fingerprint is order-independent.
    #[test]
    fn fingerprint_order_independent(
        ref1 in "[a-z]{1,20}",
        oid1 in valid_oid_string(),
        ref2 in "[a-z]{1,20}",
        oid2 in valid_oid_string(),
    ) {
        // Skip if refs are the same (would be duplicate)
        if ref1 == ref2 {
            return Ok(());
        }

        let r1 = match RefName::new(format!("refs/heads/{}", ref1)) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let r2 = match RefName::new(format!("refs/heads/{}", ref2)) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let o1 = Oid::new(&oid1).unwrap();
        let o2 = Oid::new(&oid2).unwrap();

        let refs_order1 = vec![(r1.clone(), o1.clone()), (r2.clone(), o2.clone())];
        let refs_order2 = vec![(r2, o2), (r1, o1)];

        let fp1 = Fingerprint::compute(&refs_order1);
        let fp2 = Fingerprint::compute(&refs_order2);

        prop_assert_eq!(fp1, fp2);
    }

    /// Valid branch names can be used to create RefNames.
    #[test]
    fn branch_name_to_refname(name in valid_branch_name()) {
        let branch = BranchName::new(&name).unwrap();
        let refname = RefName::for_branch(&branch);

        prop_assert!(refname.as_str().starts_with("refs/heads/"));
        prop_assert!(refname.is_branch_ref());

        let meta_ref = RefName::for_metadata(&branch);
        prop_assert!(meta_ref.as_str().starts_with("refs/branch-metadata/"));
        prop_assert!(meta_ref.is_metadata_ref());
    }

    /// Metadata round-trips through JSON.
    #[test]
    fn metadata_serde_roundtrip(
        branch_name in valid_branch_name(),
        parent_name in valid_branch_name(),
        oid_str in valid_oid_string(),
    ) {
        let branch = BranchName::new(&branch_name).unwrap();
        let parent = BranchName::new(&parent_name).unwrap();
        let oid = Oid::new(&oid_str).unwrap();

        let meta = BranchMetadataV1::new(branch, parent, oid);
        let json = serde_json::to_string(&meta).unwrap();
        let parsed = parse_metadata(&json).unwrap();

        prop_assert_eq!(meta.branch.name, parsed.branch.name);
        prop_assert_eq!(meta.parent.name(), parsed.parent.name());
        prop_assert_eq!(meta.base.oid, parsed.base.oid);
    }

    /// Oid::short returns correct prefix.
    #[test]
    fn oid_short_is_prefix(oid_str in valid_oid_string(), len in 1usize..40) {
        let oid = Oid::new(&oid_str).unwrap();
        let short = oid.short(len);

        prop_assert_eq!(short.len(), len);
        prop_assert!(oid.as_str().starts_with(short));
    }

    /// Zero OID is recognized correctly.
    #[test]
    fn zero_oid_detection(oid_str in valid_oid_string()) {
        let oid = Oid::new(&oid_str).unwrap();
        let is_all_zeros = oid_str.chars().all(|c| c == '0');

        prop_assert_eq!(oid.is_zero(), is_all_zeros);
    }
}

#[cfg(test)]
mod determinism_tests {
    use super::*;

    /// Test that metadata serialization is deterministic.
    #[test]
    fn metadata_serialization_deterministic() {
        let branch = BranchName::new("feature").unwrap();
        let parent = BranchName::new("main").unwrap();
        let oid = Oid::new("abc123def4567890abc123def4567890abc12345").unwrap();

        let meta = BranchMetadataV1::new(branch, parent, oid);

        // Serialize multiple times
        let json1 = meta.to_canonical_json().unwrap();
        let json2 = meta.to_canonical_json().unwrap();
        let json3 = serde_json::to_string(&meta).unwrap();

        assert_eq!(json1, json2);
        assert_eq!(json2, json3);
    }

    /// Test that branch name validation is consistent.
    #[test]
    fn branch_name_validation_consistent() {
        let test_cases = vec![
            ("main", true),
            ("feature/foo", true),
            ("", false),
            (".hidden", false),
            ("-flag", false),
            ("bad..path", false),
            ("branch.lock", false),
            ("branch/", false),
            ("@", false),
            ("user@work", true),
        ];

        for (name, expected_valid) in test_cases {
            let result = BranchName::new(name);
            assert_eq!(
                result.is_ok(),
                expected_valid,
                "Branch name '{}' validation mismatch",
                name
            );
        }
    }

    /// Test that OID validation is consistent.
    #[test]
    fn oid_validation_consistent() {
        // Valid SHA-1
        assert!(Oid::new("abc123def4567890abc123def4567890abc12345").is_ok());

        // Valid SHA-256
        assert!(
            Oid::new("abc123def4567890abc123def4567890abc123def4567890abc123def456789a").is_ok()
        );

        // Too short
        assert!(Oid::new("abc123").is_err());

        // Non-hex
        assert!(Oid::new("xyz123def4567890abc123def4567890abc12345").is_err());

        // Wrong length
        assert!(Oid::new("abc123def4567890abc123def4567890abc1234").is_err());
    }
}

// =============================================================================
// Graph Property Tests (Milestone 7)
// =============================================================================

/// Strategy for generating valid DAG edges.
///
/// Generates a list of (child, parent) edges where each child picks a parent
/// from branches created earlier, ensuring no cycles by construction.
fn dag_edges_strategy() -> impl Strategy<Value = Vec<(String, String)>> {
    // Generate 2-15 branches
    (2usize..15).prop_flat_map(|num_branches| {
        let branch_names: Vec<String> =
            (0..num_branches).map(|i| format!("branch-{}", i)).collect();

        // For each branch (except the first, which acts like trunk),
        // pick a parent from earlier branches
        let edge_strategies: Vec<BoxedStrategy<(String, String)>> = branch_names
            .iter()
            .enumerate()
            .skip(1) // First branch has no parent
            .map(|(i, name)| {
                let parents: Vec<String> = branch_names[..i].to_vec();
                let child_name = name.clone();

                proptest::sample::select(parents)
                    .prop_map(move |parent| (child_name.clone(), parent))
                    .boxed()
            })
            .collect();

        edge_strategies
    })
}

/// Build a StackGraph from a list of edges.
fn build_graph_from_edges(edges: &[(String, String)]) -> StackGraph {
    let mut graph = StackGraph::new();

    for (child, parent) in edges {
        let child_branch = BranchName::new(child).unwrap();
        let parent_branch = BranchName::new(parent).unwrap();
        graph.add_edge(child_branch, parent_branch);
    }

    graph
}

proptest! {
    /// Generated DAGs never have cycles (validates our strategy is correct).
    #[test]
    fn generated_dag_has_no_cycles(edges in dag_edges_strategy()) {
        let graph = build_graph_from_edges(&edges);
        prop_assert!(
            graph.find_cycle().is_none(),
            "Generated DAG should never have cycles"
        );
    }

    /// Descendants of a branch have that branch as an ancestor.
    #[test]
    fn descendants_are_reachable(edges in dag_edges_strategy()) {
        let graph = build_graph_from_edges(&edges);

        for branch in graph.branches() {
            let descendants = graph.descendants(branch);

            // Every descendant should have this branch as an ancestor
            for desc in &descendants {
                let ancestors = graph.ancestors(desc);
                prop_assert!(
                    ancestors.contains(branch),
                    "Descendant {:?} doesn't have {:?} as ancestor",
                    desc, branch
                );
            }
        }
    }

    /// Ancestor chain follows parent pointers correctly.
    #[test]
    fn ancestors_follow_parent_chain(edges in dag_edges_strategy()) {
        let graph = build_graph_from_edges(&edges);

        for branch in graph.branches() {
            let ancestors = graph.ancestors(branch);

            // Verify each step follows the parent pointer
            if !ancestors.is_empty() {
                // First ancestor should be the direct parent
                prop_assert_eq!(
                    graph.parent(branch),
                    Some(&ancestors[0]),
                    "First ancestor should be direct parent"
                );
            }

            // Each ancestor (except the last) should be the parent of the next
            for window in ancestors.windows(2) {
                prop_assert_eq!(
                    graph.parent(&window[0]),
                    Some(&window[1]),
                    "Ancestor chain broken between {:?} and {:?}",
                    window[0], window[1]
                );
            }
        }
    }

    /// Topological order ensures parents come before children.
    #[test]
    fn topological_order_respects_parents(edges in dag_edges_strategy()) {
        let graph = build_graph_from_edges(&edges);
        let order = graph.topological_order();

        // Build position map
        let positions: HashMap<_, _> = order.iter()
            .enumerate()
            .map(|(i, b)| (b.clone(), i))
            .collect();

        // Every child must come after its parent in the ordering
        for branch in graph.branches() {
            if let Some(parent) = graph.parent(branch) {
                let child_pos = positions.get(branch);
                let parent_pos = positions.get(parent);

                if let (Some(&c), Some(&p)) = (child_pos, parent_pos) {
                    prop_assert!(
                        c > p,
                        "Child {:?} at {} comes before parent {:?} at {}",
                        branch, c, parent, p
                    );
                }
            }
        }
    }

    /// Introducing a cycle is detected by find_cycle.
    #[test]
    fn cycle_detection_finds_introduced_cycles(edges in dag_edges_strategy()) {
        prop_assume!(!edges.is_empty());

        let mut graph = build_graph_from_edges(&edges);

        // Find a leaf (no children) and a root (no parent)
        let branches: Vec<_> = graph.branches().cloned().collect();
        prop_assume!(branches.len() >= 2);

        let leaf = branches.iter()
            .find(|b| graph.children(b).map(|c| c.is_empty()).unwrap_or(true))
            .cloned();

        let root = branches.iter()
            .find(|b| graph.parent(b).is_none())
            .cloned();

        if let (Some(leaf), Some(root)) = (leaf, root) {
            if leaf != root {
                // Introduce a cycle: make root a child of leaf
                graph.add_edge(root.clone(), leaf.clone());

                // Now there should be a cycle
                prop_assert!(
                    graph.find_cycle().is_some(),
                    "Cycle not detected after adding edge from root {:?} to leaf {:?}",
                    root, leaf
                );
            }
        }
    }

    /// No branch is its own ancestor (self-loop detection).
    #[test]
    fn no_self_ancestry(edges in dag_edges_strategy()) {
        let graph = build_graph_from_edges(&edges);

        for branch in graph.branches() {
            let ancestors = graph.ancestors(branch);
            prop_assert!(
                !ancestors.contains(branch),
                "Branch {:?} is its own ancestor",
                branch
            );
        }
    }

    /// Parent-children relationship is bidirectionally consistent.
    #[test]
    fn parent_children_consistency(edges in dag_edges_strategy()) {
        let graph = build_graph_from_edges(&edges);

        for branch in graph.branches() {
            // If B's parent is A, then A's children should include B
            if let Some(parent) = graph.parent(branch) {
                let children = graph.children(parent);
                prop_assert!(
                    children.map(|c| c.contains(branch)).unwrap_or(false),
                    "{:?} has parent {:?} but isn't in parent's children",
                    branch, parent
                );
            }

            // If B is in A's children, then B's parent should be A
            if let Some(children) = graph.children(branch) {
                for child in children {
                    prop_assert_eq!(
                        graph.parent(child),
                        Some(branch),
                        "{:?} is in {:?}'s children but has different parent",
                        child, branch
                    );
                }
            }
        }
    }
}

// =============================================================================
// Deterministic Graph Edge Case Tests
// =============================================================================

#[cfg(test)]
mod graph_edge_case_tests {
    use super::*;

    #[test]
    fn empty_graph_topological_order() {
        let graph = StackGraph::new();
        let order = graph.topological_order();
        assert!(order.is_empty());
    }

    #[test]
    fn single_branch_graph() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let feature = BranchName::new("feature").unwrap();

        graph.add_edge(feature.clone(), main.clone());

        assert_eq!(graph.ancestors(&feature), vec![main.clone()]);
        assert!(graph.descendants(&main).contains(&feature));
        assert!(graph.descendants(&feature).is_empty());
    }

    #[test]
    fn deep_chain_graph() {
        let mut graph = StackGraph::new();
        let names: Vec<_> = (0..10)
            .map(|i| BranchName::new(format!("b{}", i)).unwrap())
            .collect();

        for i in 1..names.len() {
            graph.add_edge(names[i].clone(), names[i - 1].clone());
        }

        // Last branch should have all others as ancestors
        let ancestors = graph.ancestors(&names[9]);
        assert_eq!(ancestors.len(), 9);

        // First branch should have all others as descendants
        let descendants = graph.descendants(&names[0]);
        assert_eq!(descendants.len(), 9);

        // Topological order should be 0, 1, 2, ..., 9
        let order = graph.topological_order();
        for (i, branch) in order.iter().enumerate() {
            assert_eq!(branch, &names[i + 1]); // +1 because b0 has no parent in graph
        }
    }

    #[test]
    fn diamond_graph() {
        //     main
        //    /    \
        //   a      b
        //    \    /
        //      c (picks a as parent)
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let a = BranchName::new("a").unwrap();
        let b = BranchName::new("b").unwrap();
        let c = BranchName::new("c").unwrap();

        graph.add_edge(a.clone(), main.clone());
        graph.add_edge(b.clone(), main.clone());
        // c has parent a (in our model, each branch has exactly one parent)
        graph.add_edge(c.clone(), a.clone());

        // main's descendants include a, b, c
        let main_desc = graph.descendants(&main);
        assert!(main_desc.contains(&a));
        assert!(main_desc.contains(&b));
        assert!(main_desc.contains(&c));

        // c's ancestors are a and main
        let c_ancestors = graph.ancestors(&c);
        assert_eq!(c_ancestors.len(), 2);
        assert_eq!(c_ancestors[0], a);
        assert_eq!(c_ancestors[1], main);
    }

    #[test]
    fn wide_tree_graph() {
        // main with 10 direct children
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();

        for i in 0..10 {
            let child = BranchName::new(format!("feature-{}", i)).unwrap();
            graph.add_edge(child, main.clone());
        }

        let descendants = graph.descendants(&main);
        assert_eq!(descendants.len(), 10);

        // Topological order should have all 10 branches (all at depth 1)
        let order = graph.topological_order();
        assert_eq!(order.len(), 10);

        // All are at same depth, so order is alphabetical
        for (i, branch) in order.iter().enumerate() {
            assert_eq!(branch.as_str(), &format!("feature-{}", i));
        }
    }
}
