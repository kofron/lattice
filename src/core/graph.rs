//! core::graph
//!
//! Stack graph representation and operations.
//!
//! # Architecture
//!
//! The stack graph is a DAG where:
//! - Nodes are tracked branches
//! - Edges point from child to parent (stored as parent pointer in metadata)
//! - Root is trunk (configured)
//!
//! # Invariants
//!
//! - Graph must be acyclic
//! - All tracked branches must exist as local refs
//! - Exactly one configured trunk per stack root (v1: single trunk total)

use super::types::BranchName;
use std::collections::{HashMap, HashSet, VecDeque};

/// The stack graph derived from branch metadata.
///
/// This is an in-memory representation computed from metadata refs.
#[derive(Debug, Default)]
pub struct StackGraph {
    /// Parent pointer for each tracked branch
    parents: HashMap<BranchName, BranchName>,
    /// Cached children sets (derived from parents)
    children: HashMap<BranchName, HashSet<BranchName>>,
}

impl StackGraph {
    /// Create an empty stack graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a parent relationship.
    ///
    /// This also updates the children cache.
    pub fn add_edge(&mut self, child: BranchName, parent: BranchName) {
        self.children
            .entry(parent.clone())
            .or_default()
            .insert(child.clone());
        self.parents.insert(child, parent);
    }

    /// Get the parent of a branch.
    pub fn parent(&self, branch: &BranchName) -> Option<&BranchName> {
        self.parents.get(branch)
    }

    /// Get the children of a branch.
    pub fn children(&self, branch: &BranchName) -> Option<&HashSet<BranchName>> {
        self.children.get(branch)
    }

    /// Check if the graph contains cycles.
    ///
    /// Returns `Some(branch)` if a cycle is detected starting from that branch.
    pub fn find_cycle(&self) -> Option<BranchName> {
        let mut visited = HashSet::new();
        let mut path = HashSet::new();

        for branch in self.parents.keys() {
            if self.has_cycle_from(branch, &mut visited, &mut path) {
                return Some(branch.clone());
            }
        }
        None
    }

    fn has_cycle_from(
        &self,
        branch: &BranchName,
        visited: &mut HashSet<BranchName>,
        path: &mut HashSet<BranchName>,
    ) -> bool {
        if path.contains(branch) {
            return true;
        }
        if visited.contains(branch) {
            return false;
        }

        visited.insert(branch.clone());
        path.insert(branch.clone());

        if let Some(parent) = self.parents.get(branch) {
            if self.has_cycle_from(parent, visited, path) {
                return true;
            }
        }

        path.remove(branch);
        false
    }

    /// Get all branches in the graph.
    pub fn branches(&self) -> impl Iterator<Item = &BranchName> {
        self.parents.keys()
    }

    /// Get all descendants of a branch (children, grandchildren, etc.).
    ///
    /// Uses breadth-first traversal to find all branches reachable via
    /// the children relationship.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::core::graph::StackGraph;
    /// use latticework::core::types::BranchName;
    ///
    /// let mut graph = StackGraph::new();
    /// let main = BranchName::new("main").unwrap();
    /// let feature = BranchName::new("feature").unwrap();
    /// let child = BranchName::new("child").unwrap();
    ///
    /// graph.add_edge(feature.clone(), main.clone());
    /// graph.add_edge(child.clone(), feature.clone());
    ///
    /// let descendants = graph.descendants(&main);
    /// assert!(descendants.contains(&feature));
    /// assert!(descendants.contains(&child));
    /// ```
    pub fn descendants(&self, branch: &BranchName) -> HashSet<BranchName> {
        let mut result = HashSet::new();
        let mut queue = VecDeque::new();

        if let Some(children) = self.children(branch) {
            queue.extend(children.iter().cloned());
        }

        while let Some(current) = queue.pop_front() {
            if result.insert(current.clone()) {
                if let Some(children) = self.children(&current) {
                    queue.extend(children.iter().cloned());
                }
            }
        }

        result
    }

    /// Get all ancestors of a branch (parent, grandparent, etc.).
    ///
    /// Returns ancestors in order from immediate parent to root.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::core::graph::StackGraph;
    /// use latticework::core::types::BranchName;
    ///
    /// let mut graph = StackGraph::new();
    /// let main = BranchName::new("main").unwrap();
    /// let feature = BranchName::new("feature").unwrap();
    /// let child = BranchName::new("child").unwrap();
    ///
    /// graph.add_edge(feature.clone(), main.clone());
    /// graph.add_edge(child.clone(), feature.clone());
    ///
    /// let ancestors = graph.ancestors(&child);
    /// assert_eq!(ancestors, vec![feature, main]);
    /// ```
    pub fn ancestors(&self, branch: &BranchName) -> Vec<BranchName> {
        let mut result = Vec::new();
        let mut current = self.parent(branch);

        while let Some(parent) = current {
            result.push(parent.clone());
            current = self.parent(parent);
        }

        result
    }

    /// Compute topological ordering for restack traversal.
    ///
    /// Returns branches sorted by depth from trunk (closest to trunk first).
    /// This ordering ensures that when restacking, parents are processed
    /// before their children.
    ///
    /// # Example
    ///
    /// ```
    /// use latticework::core::graph::StackGraph;
    /// use latticework::core::types::BranchName;
    ///
    /// let mut graph = StackGraph::new();
    /// let main = BranchName::new("main").unwrap();
    /// let a = BranchName::new("a").unwrap();
    /// let b = BranchName::new("b").unwrap();
    ///
    /// graph.add_edge(a.clone(), main.clone());
    /// graph.add_edge(b.clone(), a.clone());
    ///
    /// let order = graph.topological_order();
    /// // a comes before b because a is closer to trunk
    /// let a_pos = order.iter().position(|x| x == &a);
    /// let b_pos = order.iter().position(|x| x == &b);
    /// assert!(a_pos < b_pos);
    /// ```
    pub fn topological_order(&self) -> Vec<BranchName> {
        // Compute depth for each branch and sort by depth
        let mut by_depth: Vec<(usize, BranchName)> = self
            .branches()
            .map(|b| (self.ancestors(b).len(), b.clone()))
            .collect();

        // Sort by depth (ascending), then by name for determinism
        by_depth.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));

        by_depth.into_iter().map(|(_, branch)| branch).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_graph_has_no_cycles() {
        let graph = StackGraph::new();
        assert!(graph.find_cycle().is_none());
    }

    #[test]
    fn linear_chain_has_no_cycles() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let feature_a = BranchName::new("feature-a").unwrap();
        let feature_b = BranchName::new("feature-b").unwrap();

        graph.add_edge(feature_a.clone(), main.clone());
        graph.add_edge(feature_b.clone(), feature_a.clone());

        assert!(graph.find_cycle().is_none());
        assert_eq!(graph.parent(&feature_b), Some(&feature_a));
    }

    #[test]
    fn descendants_empty_for_leaf() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let feature = BranchName::new("feature").unwrap();

        graph.add_edge(feature.clone(), main.clone());

        let descendants = graph.descendants(&feature);
        assert!(descendants.is_empty());
    }

    #[test]
    fn descendants_includes_all_children() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let a = BranchName::new("a").unwrap();
        let b = BranchName::new("b").unwrap();
        let c = BranchName::new("c").unwrap();

        // main -> a -> b -> c
        graph.add_edge(a.clone(), main.clone());
        graph.add_edge(b.clone(), a.clone());
        graph.add_edge(c.clone(), b.clone());

        let main_descendants = graph.descendants(&main);
        assert_eq!(main_descendants.len(), 3);
        assert!(main_descendants.contains(&a));
        assert!(main_descendants.contains(&b));
        assert!(main_descendants.contains(&c));

        let a_descendants = graph.descendants(&a);
        assert_eq!(a_descendants.len(), 2);
        assert!(a_descendants.contains(&b));
        assert!(a_descendants.contains(&c));
    }

    #[test]
    fn descendants_handles_wide_tree() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();

        // main with 5 direct children
        for i in 0..5 {
            let child = BranchName::new(format!("feature-{}", i)).unwrap();
            graph.add_edge(child, main.clone());
        }

        let descendants = graph.descendants(&main);
        assert_eq!(descendants.len(), 5);
    }

    #[test]
    fn ancestors_empty_for_root() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let feature = BranchName::new("feature").unwrap();

        graph.add_edge(feature.clone(), main.clone());

        // main has no parent in the graph, so no ancestors
        let ancestors = graph.ancestors(&main);
        assert!(ancestors.is_empty());
    }

    #[test]
    fn ancestors_returns_chain_in_order() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let a = BranchName::new("a").unwrap();
        let b = BranchName::new("b").unwrap();
        let c = BranchName::new("c").unwrap();

        // main -> a -> b -> c
        graph.add_edge(a.clone(), main.clone());
        graph.add_edge(b.clone(), a.clone());
        graph.add_edge(c.clone(), b.clone());

        let c_ancestors = graph.ancestors(&c);
        assert_eq!(c_ancestors, vec![b.clone(), a.clone(), main.clone()]);

        let b_ancestors = graph.ancestors(&b);
        assert_eq!(b_ancestors, vec![a.clone(), main.clone()]);
    }

    #[test]
    fn topological_order_empty_graph() {
        let graph = StackGraph::new();
        let order = graph.topological_order();
        assert!(order.is_empty());
    }

    #[test]
    fn topological_order_respects_parent_child() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let a = BranchName::new("a").unwrap();
        let b = BranchName::new("b").unwrap();
        let c = BranchName::new("c").unwrap();

        // main -> a -> b -> c
        graph.add_edge(a.clone(), main.clone());
        graph.add_edge(b.clone(), a.clone());
        graph.add_edge(c.clone(), b.clone());

        let order = graph.topological_order();

        // Each branch should come after its parent
        let a_pos = order.iter().position(|x| x == &a).unwrap();
        let b_pos = order.iter().position(|x| x == &b).unwrap();
        let c_pos = order.iter().position(|x| x == &c).unwrap();

        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn topological_order_is_deterministic() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();
        let a = BranchName::new("a").unwrap();
        let b = BranchName::new("b").unwrap();

        graph.add_edge(a.clone(), main.clone());
        graph.add_edge(b.clone(), main.clone());

        let order1 = graph.topological_order();
        let order2 = graph.topological_order();

        assert_eq!(order1, order2);
    }

    #[test]
    fn topological_order_wide_tree() {
        let mut graph = StackGraph::new();
        let main = BranchName::new("main").unwrap();

        // main with children: a, b, c (all at depth 1)
        let a = BranchName::new("a").unwrap();
        let b = BranchName::new("b").unwrap();
        let c = BranchName::new("c").unwrap();

        graph.add_edge(a.clone(), main.clone());
        graph.add_edge(b.clone(), main.clone());
        graph.add_edge(c.clone(), main.clone());

        // d is child of a (depth 2)
        let d = BranchName::new("d").unwrap();
        graph.add_edge(d.clone(), a.clone());

        let order = graph.topological_order();

        // All depth-1 branches come before depth-2
        let a_pos = order.iter().position(|x| x == &a).unwrap();
        let b_pos = order.iter().position(|x| x == &b).unwrap();
        let c_pos = order.iter().position(|x| x == &c).unwrap();
        let d_pos = order.iter().position(|x| x == &d).unwrap();

        assert!(a_pos < d_pos);
        assert!(b_pos < d_pos);
        assert!(c_pos < d_pos);
    }
}
