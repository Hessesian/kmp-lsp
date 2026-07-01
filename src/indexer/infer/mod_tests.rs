use tower_lsp::lsp_types::Url;
use tree_sitter::Parser;

use crate::indexer::infer::{CstCtx, CstResolve, ResolveIo};
use crate::indexer::Indexer;
use crate::queries::KIND_FUN_BODY;

fn test_url(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).unwrap()
}

/// Parse `fun f() = <expr>` and return the expression node (and tree + bytes, to
/// keep the tree alive).
fn expr_node_for(src: &str) -> (tree_sitter::Tree, Vec<u8>) {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_kotlin::language())
        .unwrap();
    let bytes = src.as_bytes().to_vec();
    let tree = parser.parse(src, None).unwrap();
    (tree, bytes)
}

fn first_expr_in_fun(tree: &tree_sitter::Tree) -> Option<tree_sitter::Node<'_>> {
    let root = tree.root_node();
    let fun_decl = root.child(0)?;
    let body = (0..fun_decl.child_count())
        .map(|i| fun_decl.child(i).unwrap())
        .find(|n| n.kind() == KIND_FUN_BODY)?;
    body.child(1)
}

#[test]
fn cst_resolve_expr_type_resolves_int_literal() {
    let source = "fun f() = 1\n";
    let (tree, bytes) = expr_node_for(source);
    let int_literal_node = first_expr_in_fun(&tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/A.kt");
    indexer.index_content(&uri, source);

    let ctx = CstCtx {
        bytes: &bytes,
        uri: &uri,
        io: ResolveIo::IndexOnly,
    };
    let resolved = indexer.expr_type(int_literal_node, &ctx).resolved();
    assert_eq!(
        resolved.map(|t| t.as_type_str().to_owned()).as_deref(),
        Some("Int")
    );
}

#[test]
fn cst_resolve_expr_type_unresolved_for_unknown_nav() {
    let source = "fun f() = list.size\n";
    let (tree, bytes) = expr_node_for(source);
    let nav_expr_node = first_expr_in_fun(&tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/B.kt");
    indexer.index_content(&uri, source);

    let ctx = CstCtx {
        bytes: &bytes,
        uri: &uri,
        io: ResolveIo::IndexOnly,
    };
    let resolved = indexer.expr_type(nav_expr_node, &ctx).resolved();
    assert!(
        resolved.is_none(),
        "unresolvable nav expr should yield Unresolved"
    );
}

#[test]
fn resolved_type_nullable_flag() {
    let source = "fun f() = null\n";
    let (tree, bytes) = expr_node_for(source);
    let null_node = first_expr_in_fun(&tree).expect("null expr node");

    let indexer = Indexer::new();
    let uri = test_url("/C.kt");
    indexer.index_content(&uri, source);

    let ctx = CstCtx {
        bytes: &bytes,
        uri: &uri,
        io: ResolveIo::IndexOnly,
    };
    let resolution = indexer.expr_type(null_node, &ctx);
    let resolved = resolution
        .resolved()
        .expect("null should resolve to Nothing?");
    assert_eq!(resolved.as_type_str(), "Nothing?");
    assert!(resolved.is_nullable(), "Nothing? should be nullable");
}

#[test]
fn resolved_type_non_nullable() {
    let source = "fun f() = 42\n";
    let (tree, bytes) = expr_node_for(source);
    let int_node = first_expr_in_fun(&tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/D.kt");
    indexer.index_content(&uri, source);

    let ctx = CstCtx {
        bytes: &bytes,
        uri: &uri,
        io: ResolveIo::IndexOnly,
    };
    let resolved = indexer
        .expr_type(int_node, &ctx)
        .resolved()
        .expect("Int should resolve");
    assert!(!resolved.is_nullable(), "Int should not be nullable");
}
