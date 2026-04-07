//! LaCAM3 LNode — low-level constraint propagation node.
//!
//! REFERENCE: docs/papers_codes/lacam3/lacam3/src/lnode.cpp (15 lines)
//!            docs/papers_codes/lacam3/lacam3/include/lnode.hpp
//!
//! An LNode encodes a partial assignment of agents to cells: `who[d] = i`
//! means agent `i` was constrained to cell `where[d]` at depth `d` of the
//! low-level search. When `depth == N`, the constraint is fully specified
//! and can be passed to the configuration generator (PIBT) which fills in
//! the remaining agents.
//!
//! ## Adaptations
//!
//! lacam3 stores `who: vector<int>` and `where: vector<Vertex*>` as parallel
//! vectors. We use the same parallel-vector layout, with `where` storing flat
//! cell ids (`u32`) instead of pointers.
//!
//! lacam3 maintains a static `LNode::COUNT` counter for instrumentation. We
//! drop it (logging only).

/// Low-level search node: a partial agent → cell constraint chain.
///
/// REFERENCE: lacam3 lnode.hpp lines 9-19.
#[derive(Debug, Clone)]
pub struct LNode {
    /// Agent ids that have been constrained, in chain order.
    pub who: Vec<u32>,
    /// Flat cell ids those agents are constrained to.
    pub where_: Vec<u32>,
    /// Chain depth = `who.len()`.
    /// REFERENCE: lacam3 lnode.hpp line 14 `const int depth`.
    pub depth: usize,
}

impl LNode {
    /// Empty root constraint (no agents fixed yet).
    /// REFERENCE: lacam3 lnode.cpp line 5 `LNode() : who(), where(), depth(0)`.
    pub fn new_root() -> Self {
        Self { who: Vec::new(), where_: Vec::new(), depth: 0 }
    }

    /// Extend a parent constraint by fixing agent `i` to cell `v`.
    /// REFERENCE: lacam3 lnode.cpp lines 7-13 `LNode(parent, i, v)`.
    pub fn extend(parent: &LNode, i: u32, v: u32) -> Self {
        let mut who = parent.who.clone();
        let mut where_ = parent.where_.clone();
        who.push(i);
        where_.push(v);
        Self { who, where_, depth: parent.depth + 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lnode_root_is_empty() {
        let root = LNode::new_root();
        assert_eq!(root.depth, 0);
        assert!(root.who.is_empty());
        assert!(root.where_.is_empty());
    }

    #[test]
    fn lnode_extend_propagates_chain() {
        let root = LNode::new_root();
        let l1 = LNode::extend(&root, 0, 5);
        assert_eq!(l1.depth, 1);
        assert_eq!(l1.who, vec![0]);
        assert_eq!(l1.where_, vec![5]);

        let l2 = LNode::extend(&l1, 1, 10);
        assert_eq!(l2.depth, 2);
        assert_eq!(l2.who, vec![0, 1]);
        assert_eq!(l2.where_, vec![5, 10]);
    }
}
