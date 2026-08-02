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

#[test]
fn cst_query_receiver_type_splits_qualified_generic_nullable() {
    let source = "fun f() = holder\n"; // `holder: Outer.Inner<Param>?`
    let live_doc = live_doc_for(source);
    let ident_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let uri = test_url("/Receiver.kt");
    // Contextual-type seam: the only `InferDeps` path that hands `expr_type()`
    // the raw type string untouched by `dotted_ident_prefix()` (the `find_var_type`
    // branch strips generics before `ResolvedType` ever sees them, which would
    // lose the nullable marker this test is pinning).
    let deps =
        super::deps::TestDeps::new().with_contextual(uri.as_str(), "holder", "Outer.Inner<Param>?");

    let receiver = CstQuery::new(ident_node, &live_doc, &deps, &uri, ResolveIo::IndexOnly)
        .receiver_type()
        .resolved()
        .expect("holder should resolve");
    assert_eq!(receiver.qualified, "Outer.Inner");
    assert_eq!(receiver.outer, "Outer");
    assert_eq!(receiver.leaf, "Inner");
    assert!(receiver.nullable);
}

// ─── SuffixStrictness (chain forward walk) ───────────────────────────────────

fn find_first_node_of_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    for i in 0..node.child_count() {
        if let Some(found) = find_first_node_of_kind(node.child(i)?, kind) {
            return Some(found);
        }
    }
    None
}

/// Strictness decoy: an unresolved FINAL member must fail the walk under
/// `Fail` (the old text walker's per-segment `?`), while `LeakReceiver`
/// keeps today's receiver-position best-effort.
#[test]
fn unresolved_final_suffix_fails_the_strict_walk() {
    use super::chain::{collect_nav_segments, resolve_segments_type, SuffixStrictness};

    let uri = test_url("/Strict.kt");
    let deps = super::deps::TestDeps::new().with_var(uri.as_str(), "wrapper", "Wrapper");
    // NOTE: no field `unknownField` on Wrapper anywhere.
    let doc = live_doc_for("fun f() { wrapper.unknownField }\n");
    let nav =
        find_first_node_of_kind(doc.tree.root_node(), "navigation_expression").expect("nav node");
    let segments = collect_nav_segments(nav, &doc.bytes);

    assert_eq!(
        resolve_segments_type(&segments, &doc.bytes, &deps, &uri, SuffixStrictness::Fail),
        None,
        "unknown member must not leak the receiver's type"
    );
    assert_eq!(
        resolve_segments_type(
            &segments,
            &doc.bytes,
            &deps,
            &uri,
            SuffixStrictness::LeakReceiver
        )
        .as_deref(),
        Some("Wrapper"),
        "receiver-position semantics unchanged"
    );
}

#[test]
fn resolve_callee_chain_reports_receiver_type_before_the_final_method_not_its_return_type() {
    use super::chain::resolve_callee_chain;

    // `container.items.map` -- a nav_expr callee for `container.items.map { ... }`.
    // resolve_callee_chain must report the type of `container.items` (Box<Thing>)
    // as the receiver, and "map" as the method name -- NOT "map"'s own (here,
    // deliberately wrong/unresolvable) return type folded into current_type.
    let uri = test_url("/Chain.kt");
    let deps = super::deps::TestDeps::new()
        .with_var(uri.as_str(), "container", "Container")
        .with_field("Container", "items", "Box<Thing>");
    let doc = live_doc_for("fun f() { container.items.map { it } }\n");
    let nav =
        find_first_node_of_kind(doc.tree.root_node(), "navigation_expression").expect("nav node");

    let result = resolve_callee_chain(nav, &doc.bytes, &deps, &uri);
    assert_eq!(
        result,
        Some(("Box<Thing>".to_owned(), "map".to_owned())),
        "receiver type must be Box<Thing> (the type before .map), with \"map\" \
         as the separately-reported method name -- got {result:?}"
    );
}

/// Supplementary pin for the same bug as the test above, using a receiver whose
/// final segment ("map") IS indexed as a real (generic, unresolvable) method --
/// this is what actually exercises `resolve_member_type_on`'s generic-param
/// fallback (`first_type_arg_raw`) and corrupts `current_type` pre-fix. The
/// verbatim brief test above happens to pass even before the fix, because
/// `TestDeps` there has no "map" method registered at all, so
/// `resolve_member_type_on` returns `None` cleanly and `LeakReceiver` lets the
/// type flow through unchanged by coincidence -- it does not exercise the
/// corruption path. This test does, matching the live-probed Flow<T>.map bug:
/// pre-fix this resolves to `Some(("Thing", "map"))` (the corrupted, unwrapped
/// element type); post-fix it must be `Some(("Box<Thing>", "map"))`.
#[test]
fn resolve_callee_chain_does_not_corrupt_receiver_when_final_method_is_indexed_and_generic() {
    use super::chain::resolve_callee_chain;

    let uri = test_url("/ChainIndexedMap.kt");
    let deps = super::deps::TestDeps::new()
        .with_var(uri.as_str(), "container", "Container")
        .with_field("Container", "items", "Box<Thing>")
        // "map" is indexed on Box, returning its own unresolved generic param "T"
        // (no `with_class_params` registered for "Box", so the type-arg subst is
        // empty and "T" stays a generic placeholder -- forcing the
        // `first_type_arg_raw` fallback in `resolve_member_type_on`).
        .with_method_return_for_type("Box", "map", "T");
    let doc = live_doc_for("fun f() { container.items.map { it } }\n");
    let nav =
        find_first_node_of_kind(doc.tree.root_node(), "navigation_expression").expect("nav node");

    let result = resolve_callee_chain(nav, &doc.bytes, &deps, &uri);
    assert_eq!(
        result,
        Some(("Box<Thing>".to_owned(), "map".to_owned())),
        "receiver type must stay Box<Thing> (the type before .map) even though \
         \"map\" is indexed with a generic return type -- got {result:?}"
    );
}

/// Unknown ROOT decoy: `resolve_root_node_type` falls back to `Some(name)`
/// for an unresolvable root ident; combined with a leaking walk this used to
/// be able to resolve a nav to the literal root string. The strict nav arm
/// must yield None instead.
#[test]
fn unknown_root_nav_expression_resolves_to_none() {
    use super::chain::resolve_root_node_type;

    let uri = test_url("/UnknownRoot.kt");
    let deps = super::deps::TestDeps::new(); // nothing indexed at all
    let doc = live_doc_for("fun f() { foo.bar }\n");
    let nav =
        find_first_node_of_kind(doc.tree.root_node(), "navigation_expression").expect("nav node");

    assert_eq!(resolve_root_node_type(nav, &doc.bytes, &deps, &uri), None);
}

/// Nav-arm behavioral decoy: a SCOPE-FUNCTION callee is a navigation node in
/// root position (`resolve_call_expr_type` → `resolve_root_node_type(nav)`).
/// The deleted text walker resolved it segment-by-segment as FIELDS only, so
/// the trailing `.let` failed the whole walk; the segment walk's scope-fn
/// flow-through resolves the receiver type.
#[test]
fn scope_fn_callee_nav_resolves_via_the_segment_walk() {
    let source = "class Product { val price: Int = 0 }\n\
                  class Wrapper { val items: List<Product> = listOf() }\n\
                  fun f(wrapper: Wrapper) { wrapper.items.let { it } }\n";
    let live_doc = live_doc_for(source);
    let indexer = Indexer::new();
    let uri = test_url("/ScopeFnNav.kt");
    indexer.index_content(&uri, source);

    let let_call_start = source.find("wrapper.items.let").expect("snippet");
    let call = live_doc
        .tree
        .root_node()
        .descendant_for_byte_range(
            let_call_start,
            let_call_start + "wrapper.items.let { it }".len(),
        )
        .expect("node covering the let call");
    assert_eq!(call.kind(), "call_expression", "got {}", call.kind());
    let resolved = CstQuery::new(call, &live_doc, &indexer, &uri, ResolveIo::NoRg)
        .expr_type()
        .resolved();
    assert_eq!(
        resolved.map(|t| t.as_type_str().to_owned()).as_deref(),
        Some("List<Product>"),
        "scope-fn callee nav must resolve via the segment walk"
    );
}
