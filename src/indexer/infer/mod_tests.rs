use tower_lsp::lsp_types::Url;

use crate::indexer::infer::CstQuery;
use crate::indexer::Indexer;
use crate::queries::KIND_FUN_BODY;

fn test_url(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).unwrap()
}

fn live_doc_for(src: &str) -> crate::indexer::live_tree::LiveDoc {
    crate::indexer::live_tree::parse_live(src, tree_sitter_kotlin::LANGUAGE.into())
        .expect("kotlin parse")
}

fn first_expr_in_fun(tree: &tree_sitter::Tree) -> Option<tree_sitter::Node<'_>> {
    let root = tree.root_node();
    let fun_decl = root.child(0)?;
    let body = (0..fun_decl.child_count() as u32)
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

    let resolved = CstQuery::new(int_literal_node, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved();
    assert_eq!(
        resolved.map(|t| t.as_type_str().to_owned()).as_deref(),
        Some("Int")
    );
}

#[test]
fn cst_query_expr_type_resolves_as_cast() {
    // Real Moneta gap: `val activity = LocalContext.current as Activity` --
    // no code path extracted the RHS type from an `as`/`as?` cast at all, so
    // `activity`'s type (and every member access on it, e.g.
    // `activity.finishAffinity()`) never resolved.
    let source = "fun f(x: Any) = x as Activity\n";
    let live_doc = live_doc_for(source);
    let as_expr_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/AsCast.kt");
    indexer.index_content(&uri, source);

    let resolved = CstQuery::new(as_expr_node, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved();
    assert_eq!(
        resolved.map(|t| t.as_type_str().to_owned()).as_deref(),
        Some("Activity")
    );
}

#[test]
fn cst_query_expr_type_resolves_safe_as_cast() {
    // `as?` (safe cast) uses the same `as_expression` grammar node as `as`,
    // but ALWAYS yields a nullable result -- `x as? Activity` is `Activity?`,
    // never bare `Activity`, regardless of whether the target type itself
    // carries a `?` in source (Copilot review finding on an earlier version
    // of this fix, which returned "Activity" here).
    let source = "fun f(x: Any) = x as? Activity\n";
    let live_doc = live_doc_for(source);
    let as_expr_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/AsSafeCast.kt");
    indexer.index_content(&uri, source);

    let resolution = CstQuery::new(as_expr_node, &live_doc, &indexer, &uri).expr_type();
    let resolved = resolution.resolved().expect("as? Activity should resolve");
    assert_eq!(resolved.as_type_str(), "Activity?");
    assert!(
        resolved.is_nullable(),
        "a safe cast's result must be nullable"
    );
}

#[test]
fn cst_query_expr_type_resolves_safe_as_cast_to_a_function_type() {
    // Copilot review finding: appending `?` directly to a function type's
    // raw text misrenders `x as? (String) -> Int` as `(String) -> Int?`
    // (nullable RETURN type) instead of `((String) -> Int)?` (nullable
    // function VALUE, the actually-correct meaning of the safe cast here).
    let source = "fun f(x: Any) = x as? (String) -> Int\n";
    let live_doc = live_doc_for(source);
    let as_expr_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/AsFnType.kt");
    indexer.index_content(&uri, source);

    let resolution = CstQuery::new(as_expr_node, &live_doc, &indexer, &uri).expr_type();
    let resolved = resolution
        .resolved()
        .expect("as? (String) -> Int should resolve");
    assert_eq!(resolved.as_type_str(), "((String) -> Int)?");
}

#[test]
fn cst_query_expr_type_unresolved_for_unknown_nav() {
    let source = "fun f() = list.size\n";
    let live_doc = live_doc_for(source);
    let nav_expr_node = first_expr_in_fun(&live_doc.tree).expect("expr node");

    let indexer = Indexer::new();
    let uri = test_url("/B.kt");
    indexer.index_content(&uri, source);

    let resolved = CstQuery::new(nav_expr_node, &live_doc, &indexer, &uri)
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

    let resolution = CstQuery::new(null_node, &live_doc, &indexer, &uri).expr_type();
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

    let resolved = CstQuery::new(int_node, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved()
        .expect("Int should resolve");
    assert!(!resolved.is_nullable(), "Int should not be nullable");
}

// ─── SuffixStrictness (chain forward walk) ───────────────────────────────────

fn find_first_node_of_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    for i in 0..node.child_count() as u32 {
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

/// `fun <T> T?.required(field: String): T` -- the extension's own type
/// param IS the whole receiver type, not nested inside a `<...>` type
/// argument. `StrategyOutcome::finalize`'s `ReceiverDerived` arm only tried
/// `build_type_arg_subst` (receiver's own class type params, extracted from
/// its `<...>`), which is empty for a plain `String` receiver, so `raw_return`
/// ("T") stayed unsubstituted. Real Moneta corpus case:
/// `textModel.required("textModel").title` needs `.required(...)` to resolve
/// to `String`, not the literal `"T"`.
#[test]
fn resolve_call_expr_type_substitutes_extension_receiver_as_the_whole_type_param() {
    use super::chain::resolve_call_expr_type;

    let uri = test_url("/Required.kt");
    let deps = super::deps::TestDeps::new()
        .with_var(uri.as_str(), "textModel", "String")
        .with_method_return_for_type("String", "required", "T")
        .with_callable_info("required", &["T"], "T?");
    let doc = live_doc_for("fun f() { textModel.required(\"textModel\") }\n");
    let call = find_first_node_of_kind(doc.tree.root_node(), crate::queries::KIND_CALL_EXPR)
        .expect("call expr node");

    let result = resolve_call_expr_type(call, &doc.bytes, &deps, &uri);
    assert_eq!(
        result.as_deref(),
        Some("String"),
        "required()'s return type param T must substitute to the receiver's \
         own type (String), not stay the literal \"T\" -- got {result:?}"
    );
}

/// The shape `resolve_call_expr_type_substitutes_extension_receiver_as_the_whole_type_param`
/// above misses: a real extension function is NOT a method of its receiver's
/// type, so `receiver_based_method` never matches and the return type comes
/// from the signature lookup (`SignatureDerived`), which used to skip
/// receiver substitution entirely -- `.required(...)` stayed the literal
/// `"T"` on a real index. Real Moneta case: `textModel.required("textModel").title`
/// with `textModel: IScenes.IPermanentAddress?`.
#[test]
fn resolve_call_expr_type_substitutes_extension_receiver_for_a_signature_derived_return() {
    use super::chain::resolve_call_expr_type;

    let uri = test_url("/Required.kt");
    let deps = super::deps::TestDeps::new()
        .with_var(uri.as_str(), "textModel", "IScenes.IPermanentAddress?")
        .with_return("required", "T")
        .with_callable_info("required", &["T"], "T?");
    let doc = live_doc_for("fun f() { textModel.required(\"textModel\") }\n");
    let call = find_first_node_of_kind(doc.tree.root_node(), crate::queries::KIND_CALL_EXPR)
        .expect("call expr node");

    let result = resolve_call_expr_type(call, &doc.bytes, &deps, &uri);
    assert_eq!(
        result.as_deref(),
        Some("IScenes.IPermanentAddress"),
        "required()'s T must bind to the receiver's non-null type even when the \
         return type came from the signature lookup -- got {result:?}"
    );
}

/// Real Moneta gap (`fragmentArguments`/`fragmentBundle`): a nested-class
/// constructor call like `CaliforniaActivity.Builder(...)` must report the
/// full qualified type name, not just the bare leaf "Builder" --
/// `constructor_fallback` used to report only `ctx.fn_name` (`call_fn_name`'s
/// bare leaf), discarding the "CaliforniaActivity." qualifier and colliding
/// with every other unrelated same-named nested class once that bare name
/// reached a global lookup downstream.
#[test]
fn resolve_call_expr_type_keeps_the_outer_qualifier_for_a_nested_class_constructor() {
    use super::chain::resolve_call_expr_type;

    let uri = test_url("/Ctor.kt");
    let deps = super::deps::TestDeps::new();
    let doc = live_doc_for("fun f() { CaliforniaActivity.Builder(x) }\n");
    let call = find_first_node_of_kind(doc.tree.root_node(), crate::queries::KIND_CALL_EXPR)
        .expect("call expr node");

    let result = resolve_call_expr_type(call, &doc.bytes, &deps, &uri);
    assert_eq!(
        result.as_deref(),
        Some("CaliforniaActivity.Builder"),
        "a nested-class constructor call must report the full qualified \
         type, not just the bare leaf name -- got {result:?}"
    );
}

/// Copilot review finding on the nested-class-constructor fix above: the
/// callee's raw span preserves source whitespace, so a legally-spaced
/// qualifier (`Outer . Builder(x)`) must not leak stray spaces into the
/// segments the dotted resolver splits on.
#[test]
fn resolve_call_expr_type_normalizes_a_spaced_qualifier_for_a_nested_class_constructor() {
    use super::chain::resolve_call_expr_type;

    let uri = test_url("/SpacedCtor.kt");
    let deps = super::deps::TestDeps::new();
    let doc = live_doc_for("fun f() { CaliforniaActivity . Builder(x) }\n");
    let call = find_first_node_of_kind(doc.tree.root_node(), crate::queries::KIND_CALL_EXPR)
        .expect("call expr node");

    let result = resolve_call_expr_type(call, &doc.bytes, &deps, &uri);
    assert_eq!(
        result.as_deref(),
        Some("CaliforniaActivity.Builder"),
        "a spaced qualifier must normalize to a clean dotted name, got {result:?}"
    );
}

/// Copilot review finding on the nested-class-constructor fix above: the old
/// version fired for EVERY `KIND_NAV_EXPR` callee, including a
/// value-qualified call whose root is a lowercase VARIABLE, not a type
/// (`factory.Builder(...)`) -- misclassifying a value path as a type path.
#[test]
fn resolve_call_expr_type_does_not_treat_a_value_qualified_call_as_a_type_path() {
    use super::chain::resolve_call_expr_type;

    let uri = test_url("/ValueQualifiedCall.kt");
    let deps = super::deps::TestDeps::new();
    let doc = live_doc_for("fun f() { factory.Builder(x) }\n");
    let call = find_first_node_of_kind(doc.tree.root_node(), crate::queries::KIND_CALL_EXPR)
        .expect("call expr node");

    let result = resolve_call_expr_type(call, &doc.bytes, &deps, &uri);
    assert_eq!(
        result.as_deref(),
        Some("Builder"),
        "a lowercase (value) root must not be treated as a type-qualified \
         constructor path -- got {result:?}"
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
    let resolved = CstQuery::new(call, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved();
    assert_eq!(
        resolved.map(|t| t.as_type_str().to_owned()).as_deref(),
        Some("List<Product>"),
        "scope-fn callee nav must resolve via the segment walk"
    );
}

// ─── identifier → the declaration that binds it at that node ─────────────────

/// `expr_type` of the `occurrence`-th (0-based) `name` identifier in `source`.
fn identifier_type(source: &str, name: &str, occurrence: usize) -> Option<String> {
    let live_doc = live_doc_for(source);
    let indexer = Indexer::new();
    let uri = test_url("/Binding.kt");
    indexer.index_content(&uri, source);
    let identifier = crate::indexer::walk::descendants(live_doc.tree.root_node())
        .filter(|node| node.kind() == crate::queries::KIND_SIMPLE_IDENT)
        .filter(|node| node.utf8_text(&live_doc.bytes) == Ok(name))
        .nth(occurrence)
        .expect("identifier occurrence");
    CstQuery::new(identifier, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved()
        .map(|resolved| resolved.as_type_str().to_owned())
}

/// A file-wide by-name lookup answers with the first `item` it finds — the
/// other function's parameter.
#[test]
fn expr_type_binds_a_parameter_to_its_own_function() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun first(item: Apple) = item\n\
                  fun second(item: Banana) = item\n";
    assert_eq!(
        identifier_type(source, "item", 3).as_deref(),
        Some("Banana")
    );
}

#[test]
fn expr_type_infers_an_unannotated_local_from_its_own_initializer() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun first(item: Apple) = item\n\
                  fun second() {\n\
                  \x20   val item = Banana()\n\
                  \x20   item\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "item", 3).as_deref(),
        Some("Banana")
    );
}

/// A local declared after the use site is not in scope there; the
/// constructor property is.
#[test]
fn expr_type_ignores_a_local_declared_after_the_use() {
    let source = "class Apple\n\
                  class Banana\n\
                  class Holder(val item: Apple) {\n\
                  \x20   fun use() {\n\
                  \x20       item\n\
                  \x20       val item: Banana = Banana()\n\
                  \x20   }\n\
                  }\n";
    assert_eq!(identifier_type(source, "item", 1).as_deref(), Some("Apple"));
}

/// A redeclaration's own initializer still sees the previous binding.
#[test]
fn expr_type_inside_a_redeclaration_initializer_sees_the_previous_binding() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun convert(apple: Apple): Banana = Banana()\n\
                  fun use(item: Apple) {\n\
                  \x20   val item: Banana = convert(item)\n\
                  \x20   item\n\
                  }\n";
    assert_eq!(identifier_type(source, "item", 2).as_deref(), Some("Apple"));
    assert_eq!(
        identifier_type(source, "item", 3).as_deref(),
        Some("Banana")
    );
}

#[test]
fn expr_type_prefers_a_nested_block_local_over_the_parameter() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun use(item: Apple, ripe: Boolean) {\n\
                  \x20   if (ripe) {\n\
                  \x20       val item: Banana = Banana()\n\
                  \x20       item\n\
                  \x20   }\n\
                  \x20   item\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "item", 2).as_deref(),
        Some("Banana")
    );
    assert_eq!(identifier_type(source, "item", 3).as_deref(), Some("Apple"));
}

/// A sibling `for` loop's variable is out of scope after the loop.
#[test]
fn expr_type_does_not_bind_to_a_finished_for_loop_variable() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun use(item: Banana, apples: List<Apple>) {\n\
                  \x20   for (item in apples) {}\n\
                  \x20   item\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "item", 2).as_deref(),
        Some("Banana")
    );
}

/// A lambda parameter the engine cannot type is still the binding in scope:
/// the by-name lookup must not answer with another function's `item: Apple`.
#[test]
fn expr_type_does_not_borrow_a_sibling_functions_parameter_for_a_lambda_parameter() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun first(item: Apple) = item\n\
                  fun second() { mystery { item -> item } }\n";
    assert_eq!(identifier_type(source, "item", 3), None);
}

/// Same for a `for` variable whose iterable the engine cannot type.
#[test]
fn expr_type_does_not_borrow_a_sibling_functions_parameter_for_a_for_variable() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun first(item: Apple) = item\n\
                  fun second() { for (item in mystery()) { item } }\n";
    assert_eq!(identifier_type(source, "item", 3), None);
}

/// The type [`crate::indexer::infer_local_type_at`] reports for local `name`
/// in scope at its `occurrence`-th (0-based) identifier.
fn local_type_at(source: &str, name: &str, occurrence: usize) -> Option<String> {
    let live_doc = live_doc_for(source);
    let indexer = Indexer::new();
    let uri = test_url("/Binding.kt");
    indexer.index_content(&uri, source);
    let identifier = crate::indexer::walk::descendants(live_doc.tree.root_node())
        .filter(|node| node.kind() == crate::queries::KIND_SIMPLE_IDENT)
        .filter(|node| node.utf8_text(&live_doc.bytes) == Ok(name))
        .nth(occurrence)
        .expect("identifier occurrence");
    match crate::indexer::infer_local_type_at(name, identifier, &live_doc.bytes, &indexer, &uri) {
        crate::indexer::BindingType::Typed { type_text, .. } => Some(type_text),
        crate::indexer::BindingType::LocalUnknown | crate::indexer::BindingType::NotLocal => None,
    }
}

#[test]
fn local_type_at_keeps_nullability_and_type_arguments() {
    let source = "class Apple\n\
                  fun use(item: Apple?, items: List<Apple>) { item; items }\n";
    assert_eq!(local_type_at(source, "item", 1).as_deref(), Some("Apple?"));
    assert_eq!(
        local_type_at(source, "items", 1).as_deref(),
        Some("List<Apple>")
    );
}

#[test]
fn local_type_at_keeps_type_arguments_through_an_identifier_initializer() {
    let source = "class Apple\n\
                  fun first(copy: Apple) {}\n\
                  fun use(items: List<Apple>?) { val copy = items; copy }\n";
    assert_eq!(
        local_type_at(source, "copy", 2).as_deref(),
        Some("List<Apple>?")
    );
}

#[test]
fn local_type_at_reads_an_annotated_type_past_its_annotation() {
    let source = "annotation class Ann\n\
                  class Apple\n\
                  fun use(item: @Ann Apple) { item }\n";
    assert_eq!(local_type_at(source, "item", 1).as_deref(), Some("Apple"));
}

// ─── elvis initializers ──────────────────────────────────────────────────────

/// `val item = pick() ?: <rhs>` in `second`, with a same-named `Banana`
/// parameter in `first`; the type `expr_type` gives the use of `item`.
fn elvis_local_type(right_side: &str) -> Option<String> {
    let source = format!(
        "class Apple\n\
         class Banana\n\
         fun pick(): Apple? = null\n\
         fun first(item: Banana) = item\n\
         fun second() {{\n\
         \x20   val item = pick() ?: {right_side}\n\
         \x20   item\n\
         }}\n"
    );
    identifier_type(&source, "item", 3)
}

#[test]
fn elvis_with_a_jump_takes_the_non_null_left_type() {
    assert_eq!(elvis_local_type("return").as_deref(), Some("Apple"));
    assert_eq!(
        elvis_local_type("throw IllegalStateException()").as_deref(),
        Some("Apple")
    );
}

#[test]
fn elvis_with_error_or_todo_takes_the_non_null_left_type() {
    assert_eq!(
        elvis_local_type("error(\"none\")").as_deref(),
        Some("Apple")
    );
    assert_eq!(elvis_local_type("TODO()").as_deref(), Some("Apple"));
}

#[test]
fn elvis_with_a_same_typed_default_takes_that_type() {
    assert_eq!(elvis_local_type("Apple()").as_deref(), Some("Apple"));
}

/// The result is the common supertype, which this engine does not compute.
#[test]
fn elvis_with_a_differently_typed_default_is_unknown() {
    assert_eq!(elvis_local_type("Banana()"), None);
}

/// Same bare name, different type arguments: still not the left side's type.
#[test]
fn elvis_with_a_default_of_other_type_arguments_is_unknown() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun <T> listOf(vararg elements: T): List<T> = TODO()\n\
                  fun pick(): List<Apple>? = null\n\
                  fun first(copy: Apple) {}\n\
                  fun use() { val copy = pick() ?: listOf(Banana()); copy }\n";
    assert_eq!(local_type_at(source, "copy", 2), None);
}

#[test]
fn elvis_initializer_keeps_type_arguments_and_drops_nullability() {
    let source = "class Apple\n\
                  fun first(copy: Apple) {}\n\
                  fun use(items: List<Apple>?) { val copy = items ?: return; copy }\n";
    assert_eq!(
        local_type_at(source, "copy", 2).as_deref(),
        Some("List<Apple>")
    );
}

// ─── delegated locals ────────────────────────────────────────────────────────

const STATE_DECLARATIONS: &str = "class Apple\n\
     class Banana\n\
     interface State<out T> { val value: T }\n\
     interface MutableState<T> : State<T>\n\
     operator fun <T> State<T>.getValue(thisObj: Any?, property: Any?): T = value\n\
     fun <T> mutableStateOf(value: T): MutableState<T> = TODO()\n\
     fun <T> remember(calculation: () -> T): T = calculation()\n\
     class StateFlow<T>\n\
     fun <T> StateFlow<T>.collectAsStateWithLifecycle(): State<T> = TODO()\n\
     fun appleState(): MutableState<Apple> = TODO()\n\
     class Model { val uiState: StateFlow<Apple> = TODO() }\n\
     fun first(item: Banana) = item\n";

#[test]
fn expr_type_unwraps_a_state_delegate() {
    let source = format!(
        "{STATE_DECLARATIONS}\
         fun second(model: Model) {{\n\
         \x20   val item by model.uiState.collectAsStateWithLifecycle()\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&source, "item", 3).as_deref(),
        Some("Apple")
    );
}

/// `MutableState<T>` has no `getValue` of its own; it inherits `State<T>`'s
/// through its supertype, which this lookup does not walk: unknown, never
/// `first`'s `Banana`.
#[test]
fn expr_type_leaves_a_mutable_state_value_unknown() {
    let source = format!(
        "{STATE_DECLARATIONS}\
         fun second() {{\n\
         \x20   var item by appleState()\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(identifier_type(&source, "item", 3), None);
}

/// A delegate whose `getValue` type this engine does not know stays unknown,
/// never borrowing `first`'s `item: Banana`.
#[test]
fn expr_type_leaves_an_unknown_delegate_untyped() {
    let source = format!(
        "{STATE_DECLARATIONS}\
         fun second() {{\n\
         \x20   val item by mystery()\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(identifier_type(&source, "item", 3), None);
}

/// `by lazy { .. }` reads as its lambda's result.
#[test]
fn expr_type_types_a_lazy_delegate_from_its_lambda() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun first(item: Banana) = item\n\
                  fun second() {\n\
                  \x20   val item by lazy { Apple() }\n\
                  \x20   item\n\
                  }\n";
    assert_eq!(identifier_type(source, "item", 3).as_deref(), Some("Apple"));
}

#[test]
fn expr_type_types_mutable_state_of_from_its_argument() {
    let remembered = format!(
        "{STATE_DECLARATIONS}\
         fun second() {{\n\
         \x20   val item by remember {{ mutableStateOf(Apple()) }}\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&remembered, "item", 3).as_deref(),
        Some("Apple")
    );
    let direct = format!(
        "{STATE_DECLARATIONS}\
         fun second() {{\n\
         \x20   var item by mutableStateOf(Apple())\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&direct, "item", 3).as_deref(),
        Some("Apple")
    );
}

/// The value type is what the delegate's own `getValue` returns, not its
/// type argument, even for a class that happens to be called `State`.
#[test]
fn expr_type_reads_a_delegate_through_its_own_get_value() {
    let source = "class Apple\n\
                  class Banana\n\
                  class State<T> { operator fun getValue(thisRef: Any?, property: Any?): Banana = TODO() }\n\
                  fun make(): State<Apple> = TODO()\n\
                  fun first(item: Apple) = item\n\
                  fun second() {\n\
                  \x20   val item by make()\n\
                  \x20   item\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "item", 3).as_deref(),
        Some("Banana")
    );
}

/// `expr_type` of the `occurrence`-th `name` in `caller_path`, with every file
/// in `files` indexed.
fn identifier_type_in_workspace(
    files: &[(&str, &str)],
    caller_path: &str,
    name: &str,
    occurrence: usize,
) -> Option<String> {
    let indexer = Indexer::new();
    for (path, source) in files {
        indexer.index_content(&test_url(path), source);
    }
    let caller_source = files
        .iter()
        .find(|(path, _)| *path == caller_path)
        .map(|(_, source)| *source)
        .expect("caller file");
    let uri = test_url(caller_path);
    let live_doc = live_doc_for(caller_source);
    let identifier = crate::indexer::walk::descendants(live_doc.tree.root_node())
        .filter(|node| node.kind() == crate::queries::KIND_SIMPLE_IDENT)
        .filter(|node| node.utf8_text(&live_doc.bytes) == Ok(name))
        .nth(occurrence)
        .expect("identifier occurrence");
    CstQuery::new(identifier, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved()
        .map(|resolved| resolved.as_type_str().to_owned())
}

/// `uiState: StateFlow<State>` declared in a generic base class: the delegate
/// reads as the subclass's type argument.
#[test]
fn expr_type_substitutes_a_delegate_through_the_view_model_supertype() {
    let files = [
        (
            "/compose/State.kt",
            "package androidx.compose.runtime\n\
             interface State<out T> { val value: T }\n\
             operator fun <T> State<T>.getValue(thisObj: Any?, property: Any?): T = value\n\
             class StateFlow<T>\n\
             fun <T> StateFlow<T>.collectAsStateWithLifecycle(): State<T> = TODO()\n",
        ),
        (
            "/mvi/MviViewModel.kt",
            "package app.mvi\n\
             import androidx.compose.runtime.StateFlow\n\
             interface UiState\n\
             abstract class MviViewModel<Event, State : UiState> {\n\
             \x20   val uiState: StateFlow<State> by lazy { TODO() }\n\
             }\n",
        ),
        (
            "/decoy/Decoy.kt",
            "package app.decoy\n\
             class State(val count: Int)\n\
             private fun State.content(): Int = count\n",
        ),
        (
            "/screen/Agreement.kt",
            "package app.screen\n\
             import androidx.compose.runtime.collectAsStateWithLifecycle\n\
             import androidx.compose.runtime.getValue\n\
             import app.mvi.MviViewModel\n\
             import app.mvi.UiState\n\
             class Apple : UiState\n\
             class AgreementViewModel : MviViewModel<Int, Apple>()\n\
             fun first(state: Int) {}\n\
             fun screen(viewModel: AgreementViewModel) {\n\
             \x20   val state by viewModel.uiState.collectAsStateWithLifecycle()\n\
             \x20   state\n\
             }\n",
        ),
    ];
    let state_type = identifier_type_in_workspace(&files, "/screen/Agreement.kt", "state", 2);
    assert_eq!(state_type.as_deref(), Some("Apple"));
}

/// The Moneta `FamilyCardDetailViewModel` shape: `event.event` inside
/// `is Event.Actions ->` is `Actions.event`, not the first `event` property
/// among `Event`'s subclasses.
#[test]
fn expr_type_types_a_chain_through_a_narrowed_subject() {
    let source = "class DetailEvent\n\
                  class ActionEvent\n\
                  sealed interface Event {\n\
                  \x20   data class Detail(val event: DetailEvent) : Event\n\
                  \x20   data class Actions(val event: ActionEvent) : Event\n\
                  }\n\
                  fun handle(event: Event) {\n\
                  \x20   when (event) {\n\
                  \x20       is Event.Actions -> when (val actionEvent = event.event) { else -> actionEvent }\n\
                  \x20       else -> {}\n\
                  \x20   }\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "actionEvent", 1).as_deref(),
        Some("ActionEvent")
    );
}

#[test]
fn expr_type_types_a_when_subject_binding_from_its_subject() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun pick(): Apple = Apple()\n\
                  fun first(item: Banana) = item\n\
                  fun second() = when (val item = pick()) { else -> item }\n";
    assert_eq!(identifier_type(source, "item", 3).as_deref(), Some("Apple"));
}
