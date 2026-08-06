//! CST-based folding ranges — walks the live tree-sitter tree to find foldable regions.
//!
//! Block nodes (class body, function body, lambdas, etc.) become `Region` folds.
//! Consecutive `line_comment` sibling runs become `Comment` folds.
//! `import_list` nodes become `Imports` folds.
//! `multiline_comment` nodes spanning multiple lines become `Comment` folds.

use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind, Url};

use crate::indexer::Indexer;
use crate::queries::{
    KIND_BLOCK, KIND_CATCH_BLOCK, KIND_CLASS_BODY, KIND_CONTROL_STRUCTURE_BODY,
    KIND_ENUM_CLASS_BODY, KIND_FUN_BODY, KIND_IMPORT_LIST, KIND_INTERFACE_BODY, KIND_LAMBDA_LIT,
    KIND_LINE_COMMENT, KIND_MULTILINE_COMMENT, KIND_OBJECT_BODY,
};

/// Block node kinds that should be folded as `Region` ranges.
const REGION_KINDS: &[&str] = &[
    KIND_CLASS_BODY,
    KIND_INTERFACE_BODY,
    KIND_ENUM_CLASS_BODY,
    KIND_OBJECT_BODY,
    KIND_FUN_BODY,
    KIND_BLOCK,
    KIND_CATCH_BLOCK,
    KIND_CONTROL_STRUCTURE_BODY,
    KIND_LAMBDA_LIT,
];

/// Compute folding ranges for `uri` using the live parse tree.
///
/// Returns `None` when no live tree exists for the file.
pub(crate) fn cst_folding_ranges(indexer: &Indexer, uri: &Url) -> Option<Vec<FoldingRange>> {
    let doc = indexer.live_doc(uri)?;
    let root = doc.tree.root_node();
    let mut ranges = Vec::new();

    collect_folds(root, &mut ranges);

    if ranges.is_empty() {
        None
    } else {
        Some(ranges)
    }
}

fn collect_folds(node: tree_sitter::Node, out: &mut Vec<FoldingRange>) {
    collect_folds_at(node, out, 0);
}

fn collect_folds_at(node: tree_sitter::Node, out: &mut Vec<FoldingRange>, depth: usize) {
    // See `crate::util::MAX_CST_DESCENT_DEPTH`: bail rather than overflow the
    // stack on a pathologically deep tree (huge chained expression, or
    // ERROR-recovery on a huge malformed file).
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        return;
    }

    let kind = node.kind();

    if REGION_KINDS.contains(&kind) {
        let start = node.start_position();
        let end = node.end_position();
        if end.row > start.row + 1 {
            out.push(FoldingRange {
                start_line: start.row as u32,
                end_line: end.row as u32,
                start_character: None,
                end_character: None,
                kind: Some(FoldingRangeKind::Region),
                collapsed_text: None,
            });
        }
    } else if kind == KIND_MULTILINE_COMMENT {
        let start = node.start_position();
        let end = node.end_position();
        if end.row > start.row {
            out.push(FoldingRange {
                start_line: start.row as u32,
                end_line: end.row as u32,
                start_character: None,
                end_character: None,
                kind: Some(FoldingRangeKind::Comment),
                collapsed_text: None,
            });
        }
    } else if kind == KIND_IMPORT_LIST {
        let start = node.start_position();
        let end = node.end_position();
        if end.row > start.row {
            out.push(FoldingRange {
                start_line: start.row as u32,
                end_line: end.row as u32,
                start_character: None,
                end_character: None,
                kind: Some(FoldingRangeKind::Imports),
                collapsed_text: None,
            });
        }
    }

    // Collect consecutive line_comment siblings as a single comment fold.
    let mut walker = node.walk();
    let children: Vec<_> = node.children(&mut walker).collect();
    let mut i = 0;
    while i < children.len() {
        let child = children[i];
        if child.kind() == KIND_LINE_COMMENT {
            let run_start = child.start_position().row;
            let mut run_end = run_start;
            let mut j = i + 1;
            while j < children.len() && children[j].kind() == KIND_LINE_COMMENT {
                run_end = children[j].start_position().row;
                j += 1;
            }
            if run_end > run_start {
                out.push(FoldingRange {
                    start_line: run_start as u32,
                    end_line: run_end as u32,
                    start_character: None,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Comment),
                    collapsed_text: None,
                });
            }
            i = j;
        } else {
            collect_folds_at(child, out, depth + 1);
            i += 1;
        }
    }
}

#[cfg(test)]
mod depth_guard_tests {
    //! Regression coverage for the `MAX_CST_DESCENT_DEPTH` guard added to
    //! `collect_folds`. Before the guard, a long chain of `+` (perfectly
    //! valid Kotlin — no malformed/ERROR-recovery input needed) parses as a
    //! deeply left-nested `binary_expression` tree, and `collect_folds`'s
    //! unconditional full-tree recursion overflowed an 8 MiB stack (matching
    //! Linux's real main-thread default) at ~10-20k chained terms in a debug
    //! build. This test proves the guard actually stops the descent — run on
    //! a thread with that same 8 MiB stack, well past the old crash
    //! threshold, and it must both survive and terminate promptly.
    use tree_sitter::Parser;

    #[test]
    fn collect_folds_does_not_overflow_on_a_pathologically_deep_chain() {
        let n = 200_000; // ~3x the old debug-build crash threshold
        let mut src = String::with_capacity(n * 2 + 32);
        src.push_str("fun f() { val x = 1");
        for _ in 0..n {
            src.push_str("+1");
        }
        src.push_str("\n}\n");

        let handle = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024) // match Linux's default main-thread stack
            .spawn(move || {
                let mut parser = Parser::new();
                parser
                    .set_language(&tree_sitter_kotlin::language())
                    .unwrap();
                let tree = parser.parse(&src, None).unwrap();
                let mut out = Vec::new();
                super::collect_folds(tree.root_node(), &mut out);
                out.len()
            })
            .unwrap();
        // If this doesn't overflow, `join` returns normally (a stack overflow
        // aborts the whole process rather than failing this join).
        handle.join().expect("must not overflow the stack");
    }
}
