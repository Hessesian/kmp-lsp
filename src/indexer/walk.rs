//! Iterative traversal of a CST.
//!
//! Descending a tree with Rust recursion spends one stack frame per level, so a
//! machine-generated file or an ERROR-recovered buffer can exhaust the stack and
//! abort the process. Keeping the position in a `TreeCursor` instead makes depth
//! free, so a walk built on this needs no depth bound and no cap to explain.
//!
//! It also lets a walk's context stay in local variables. A recursive walker has
//! to thread every accumulator and cache through every frame as a parameter,
//! which is what grew those signatures past what `clippy` accepts without an
//! `allow`.

use tree_sitter::Node;

/// Every node under `root`, including `root`, in pre-order — a parent before its
/// children, siblings left to right.
///
/// Pruning (skipping a subtree) and subtree-exit events are deliberately absent:
/// no caller needs them yet. Add them here when one does, rather than reaching
/// back for recursion.
pub(crate) fn descendants<'tree>(root: Node<'tree>) -> impl Iterator<Item = Node<'tree>> {
    let mut cursor = root.walk();
    let mut finished = false;
    // `TreeCursor` has no "visit next in pre-order" primitive, so the walk is
    // spelled out: descend if we can, else take the next sibling, else climb
    // until a sibling exists. Climbing back to `root` means the walk is over.
    std::iter::from_fn(move || {
        if finished {
            return None;
        }
        let node = cursor.node();
        if !cursor.goto_first_child() {
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() || cursor.node().id() == root.id() {
                    finished = true;
                    break;
                }
            }
        }
        Some(node)
    })
}

#[cfg(test)]
#[path = "walk_tests.rs"]
mod tests;
