use tower_lsp::lsp_types::Url;

use crate::indexer::infer::{CstQuery, ResolveIo};
use crate::indexer::Indexer;
use crate::queries::KIND_FUN_BODY;

fn test_url(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).unwrap()
}

fn live_doc_for(src: &str) -> crate::indexer::live_tree::LiveDoc {
    crate::indexer::live_tree::parse_live(src, tree_sitter_kotlin::language())
        .expect("kotlin parse")
}

fn first_expr_in_fun(tree: &tree_sitter::Tree) -> Option<tree_sitter::Node<'_>> {
    let root = tree.root_node();
    let fun_decl = root.child(0)?;
    let body = (0..fun_decl.child_count())
        .map(|i| fun_decl.child(i).unwrap())
        .find(|n| n.kind() == KIND_FUN_BODY)?;
    body.child(1)
}

// ─── CstQuery tests ───────────────────────────────────────────────────────────

#[test]
fn cst_query_expr_type_resolves_int_literal() {
    let source = "fun f() = 1\n";
    let live_doc = live_doc_for(source);
    let int_literal_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/CstQuery.kt");
    indexer.index_content(&uri, source);

    let resolved = CstQuery::new(
        int_literal_node,
        &live_doc,
        &indexer,
        &uri,
        ResolveIo::IndexOnly,
    )
    .expr_type()
    .resolved();
    assert_eq!(
        resolved.map(|t| t.as_type_str().to_owned()).as_deref(),
        Some("Int")
    );
}

#[test]
fn cst_query_expr_type_unresolved_for_unknown_nav() {
    let source = "fun f() = list.size\n";
    let live_doc = live_doc_for(source);
    let nav_expr_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/B.kt");
    indexer.index_content(&uri, source);

    let resolved = CstQuery::new(
        nav_expr_node,
        &live_doc,
        &indexer,
        &uri,
        ResolveIo::IndexOnly,
    )
    .expr_type()
    .resolved();
    assert!(
        resolved.is_none(),
        "unresolvable nav expr should yield Unresolved"
    );
}

#[test]
fn resolved_type_nullable_flag() {
    let source = "fun f() = null\n";
    let live_doc = live_doc_for(source);
    let null_node = first_expr_in_fun(&live_doc.tree).expect("null expr node");

    let indexer = Indexer::new();
    let uri = test_url("/C.kt");
    indexer.index_content(&uri, source);

    let resolution =
        CstQuery::new(null_node, &live_doc, &indexer, &uri, ResolveIo::IndexOnly).expr_type();
    let resolved = resolution
        .resolved()
        .expect("null should resolve to Nothing?");
    assert_eq!(resolved.as_type_str(), "Nothing?");
    assert!(resolved.is_nullable(), "Nothing? should be nullable");
}

#[test]
fn resolved_type_non_nullable() {
    let source = "fun f() = 42\n";
    let live_doc = live_doc_for(source);
    let int_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/D.kt");
    indexer.index_content(&uri, source);

    let resolved = CstQuery::new(int_node, &live_doc, &indexer, &uri, ResolveIo::IndexOnly)
        .expr_type()
        .resolved()
        .expect("Int should resolve");
    assert!(!resolved.is_nullable(), "Int should not be nullable");
}
