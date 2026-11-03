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

// ─── review follow-ups: elvis/delegate identity ──────────────────────────────

/// `pick(): Item?` (package `a`) `?: fallback(): Item` (package `b`): same
/// written name, two unrelated classes — their common supertype is not `a.Item`.
#[test]
fn elvis_with_a_same_named_default_from_another_package_is_unknown() {
    let files = [
        (
            "/a/Pick.kt",
            "package a\nclass Item\nfun pick(): Item? = null\n",
        ),
        (
            "/b/Fallback.kt",
            "package b\nclass Item\nfun fallback(): Item = TODO()\n",
        ),
        (
            "/c/Caller.kt",
            "package c\nimport a.pick\nimport b.fallback\nfun first(item: Int) = item\n\
             fun use() {\n    val item = pick() ?: fallback()\n    item\n}\n",
        ),
    ];
    assert_eq!(
        identifier_type_in_workspace(&files, "/c/Caller.kt", "item", 2),
        None
    );
}

/// The same class reached from two files of one package is one type.
#[test]
fn elvis_with_a_default_of_the_same_class_from_another_file_takes_that_type() {
    let files = [
        ("/a/Item.kt", "package a\nclass Item\n"),
        ("/a/Pick.kt", "package a\nfun pick(): Item? = null\n"),
        ("/a/Fallback.kt", "package a\nfun fallback(): Item = TODO()\n"),
        (
            "/c/Caller.kt",
            "package c\nimport a.Item\nimport a.pick\nimport a.fallback\nfun first(item: Int) = item\n\
             fun use() {\n    val item = pick() ?: fallback()\n    item\n}\n",
        ),
    ];
    assert_eq!(
        identifier_type_in_workspace(&files, "/c/Caller.kt", "item", 2).as_deref(),
        Some("Item")
    );
}

/// A reachable `error` that returns a value completes normally.
#[test]
fn elvis_with_a_user_error_function_returning_a_value_is_unknown() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun pickApple(): Apple? = null\n\
                  fun error(message: String): Banana = TODO()\n\
                  fun first(item: Int) = item\n\
                  fun use() { val item = pickApple() ?: error(\"missing\"); item }\n";
    assert_eq!(identifier_type(source, "item", 2), None);
}

/// `left ?: (right)` is still nullable when `right` is.
#[test]
fn elvis_with_a_parenthesized_nullable_default_is_unknown() {
    let source = "class Apple\n\
                  fun first(item: Int) = item\n\
                  fun use(left: Apple?, right: Apple?) { val item = left ?: (right); item }\n";
    assert_eq!(identifier_type(source, "item", 2), None);
}

#[test]
fn elvis_with_a_parenthesized_non_null_default_takes_the_type() {
    let source = "class Apple\n\
                  fun first(item: Int) = item\n\
                  fun use(left: Apple?, right: Apple) { val item = left ?: (right); item }\n";
    assert_eq!(identifier_type(source, "item", 2).as_deref(), Some("Apple"));
}

const POLICY_DECLARATIONS: &str = "class Apple\n\
     class Banana\n\
     interface SnapshotMutationPolicy<T>\n\
     interface State<out T> { val value: T }\n\
     interface MutableState<T> : State<T>\n\
     operator fun <T> State<T>.getValue(thisObj: Any?, property: Any?): T = value\n\
     fun <T> mutableStateOf(value: T, policy: SnapshotMutationPolicy<T>? = null): MutableState<T> = TODO()\n\
     fun <T> lazy(initializer: () -> T): Lazy<T> = TODO()\n\
     class Lazy<T>\n\
     fun first(item: Banana) = item\n";

/// `mutableStateOf(policy = .., value = Apple())`: the value is the named one.
#[test]
fn delegate_state_takes_the_named_value_argument() {
    let source = format!(
        "{POLICY_DECLARATIONS}\
         fun second(policy: SnapshotMutationPolicy<Apple>) {{\n\
         \x20   val item by mutableStateOf(policy = policy, value = Apple())\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&source, "item", 3).as_deref(),
        Some("Apple")
    );
}

/// An explicit type argument is the declared value type.
#[test]
fn delegate_state_and_lazy_honor_explicit_type_arguments() {
    let state = format!(
        "{POLICY_DECLARATIONS}\
         fun second() {{\n\
         \x20   val item by mutableStateOf<Any>(Apple())\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(identifier_type(&state, "item", 3).as_deref(), Some("Any"));
    let lazy = format!(
        "{POLICY_DECLARATIONS}\
         fun second() {{\n\
         \x20   val item by lazy<Any> {{ Apple() }}\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(identifier_type(&lazy, "item", 3).as_deref(), Some("Any"));
    let nullable = format!(
        "{POLICY_DECLARATIONS}\
         fun second() {{\n\
         \x20   val item by lazy<Apple?> {{ Apple() }}\n\
         \x20   item\n\
         }}\n"
    );
    assert_eq!(
        local_type_at(&nullable, "item", 3).as_deref(),
        Some("Apple?")
    );
}

/// A library delegate's `getValue(): Apple` is `lib.Apple`; the caller's own
/// `Apple` is a different class.
#[test]
fn delegate_get_value_keeps_the_library_class_not_the_callers_same_named_one() {
    let files = [
        (
            "/lib/Delegate.kt",
            "package lib\nclass Apple\nclass Delegate { operator fun getValue(thisRef: Any?, property: Any?): Apple = TODO() }\n\
             fun make(): Delegate = TODO()\n",
        ),
        (
            "/app/Caller.kt",
            "package app\nimport lib.make\nclass Apple\nfun first(item: Int) = item\n\
             fun use() {\n    val item by make()\n    item\n}\n",
        ),
    ];
    assert_eq!(
        identifier_type_in_workspace(&files, "/app/Caller.kt", "item", 2),
        None
    );
}

/// A non-generic extension `Delegate.getValue` types the local.
#[test]
fn delegate_get_value_extension_without_type_parameters_types_the_local() {
    let source = "class Apple\n\
                  class Delegate\n\
                  operator fun Delegate.getValue(thisRef: Any?, property: Any?): Apple = TODO()\n\
                  fun make(): Delegate = TODO()\n\
                  fun first(item: Int) = item\n\
                  fun use() { val item by make(); item }\n";
    assert_eq!(identifier_type(source, "item", 2).as_deref(), Some("Apple"));
}

/// `Other<T>.getValue(): Banana` precedes `Delegate<T>.getValue(): T`; the
/// receiver picks the second.
#[test]
fn delegate_get_value_extension_is_chosen_by_receiver_not_by_name() {
    let source = "class Apple\n\
                  class Banana\n\
                  class Other<T>\n\
                  class Delegate<T>\n\
                  operator fun <T> Other<T>.getValue(thisRef: Any?, property: Any?): Banana = TODO()\n\
                  operator fun <T> Delegate<T>.getValue(thisRef: Any?, property: Any?): T = TODO()\n\
                  fun make(): Delegate<Apple> = TODO()\n\
                  fun first(item: Int) = item\n\
                  fun use() { val item by make(); item }\n";
    assert_eq!(identifier_type(source, "item", 2).as_deref(), Some("Apple"));
}

// ─── review round 2: elvis identity and nullability ─────────────────────────

/// `a.List` and `b.List` both shadow Kotlin's `List`; neither is the
/// built-in, and they are not each other.
#[test]
fn shadowed_builtin_names_are_not_the_same_type_across_packages() {
    use crate::indexer::infer::deps::InferDeps as _;
    let indexer = Indexer::new();
    let first = test_url("/a/First.kt");
    let second = test_url("/b/Second.kt");
    let plain = test_url("/c/Plain.kt");
    indexer.index_content(&first, "package a\nclass List\n");
    indexer.index_content(&second, "package b\nclass List\n");
    indexer.index_content(&plain, "package c\nclass Other\n");
    assert!(!indexer.same_type_in_scopes("List", &first, &second));
    assert!(!indexer.same_type_in_scopes("List", &first, &plain));
    assert!(indexer.same_type_in_scopes("List", &plain, &plain));
    assert!(indexer.same_type_in_scopes("List", &plain, &test_url("/d/Other.kt")));
}

/// `pick(): List?` from `a`, `List()` constructed from `b`: the elvis has
/// no single type.
#[test]
fn elvis_of_same_named_shadowing_builtin_classes_is_unknown() {
    let files = [
        (
            "/a/Pick.kt",
            "package a\nclass List\nfun pick(): List? = null\n",
        ),
        ("/b/List.kt", "package b\nclass List\n"),
        (
            "/app/Use.kt",
            "package app\nimport a.pick\nimport b.List\n\
             fun use() { val item = pick() ?: List(); item }\n",
        ),
    ];
    assert_eq!(
        identifier_type_in_workspace(&files, "/app/Use.kt", "item", 1),
        None
    );
}

/// A nullable-returning call on the right can still be null, so the elvis
/// stays nullable-or-unknown, never `Apple`.
#[test]
fn elvis_with_a_nullable_returning_call_default_is_unknown() {
    let source = "class Apple\n\
                  fun pick(): Apple? = null\n\
                  fun fallback(): Apple? = null\n\
                  fun use() { val item = pick() ?: fallback(); item }\n";
    assert_eq!(local_type_at(source, "item", 1), None);
}

// ─── review round 2: narrowed and delegated locals inside typed lambdas ─────

#[test]
fn narrowed_when_subject_in_a_typed_lambda_takes_the_narrowed_type() {
    let source = "open class Event\n\
                  class Actions : Event()\n\
                  fun <T> List<T>.forEach(action: (T) -> Unit) {}\n\
                  fun use(events: List<Event>) {\n\
                  \x20   events.forEach { event ->\n\
                  \x20       when (event) { is Actions -> event else -> {} }\n\
                  \x20   }\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "event", 2).as_deref(),
        Some("Actions")
    );
}

#[test]
fn delegated_local_shadowing_a_lambda_parameter_takes_its_delegate_type() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun <T> List<T>.forEach(action: (T) -> Unit) {}\n\
                  fun use(items: List<Apple>) {\n\
                  \x20   items.forEach { item ->\n\
                  \x20       val item by lazy { Banana() }\n\
                  \x20       item\n\
                  \x20   }\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "item", 2).as_deref(),
        Some("Banana")
    );
}

// ─── review round 2: delegate shortcuts need the real callable ──────────────

const CUSTOM_DELEGATE: &str = "class Apple\n\
     class Banana\n\
     class CustomDelegate { operator fun getValue(thisRef: Any?, property: Any?): Banana = TODO() }\n";

#[test]
fn user_defined_mutable_state_of_reads_through_its_own_get_value() {
    let source = format!(
        "{CUSTOM_DELEGATE}\
         fun mutableStateOf(value: Apple): CustomDelegate = TODO()\n\
         fun second() {{ val item by mutableStateOf(Apple()); item }}\n"
    );
    assert_eq!(
        identifier_type(&source, "item", 1).as_deref(),
        Some("Banana")
    );
}

#[test]
fn user_defined_lazy_reads_through_its_own_get_value() {
    let source = format!(
        "{CUSTOM_DELEGATE}\
         fun lazy(initializer: () -> Apple): CustomDelegate = TODO()\n\
         fun second() {{ val item by lazy {{ Apple() }}; item }}\n"
    );
    assert_ne!(
        identifier_type(&source, "item", 1).as_deref(),
        Some("Apple")
    );
}

#[test]
fn user_defined_remember_does_not_unwrap_its_lambda() {
    let source = format!(
        "{CUSTOM_DELEGATE}\
         fun remember(calculation: () -> Apple): CustomDelegate = TODO()\n\
         fun second() {{ val item by remember {{ Apple() }}; item }}\n"
    );
    assert_ne!(
        identifier_type(&source, "item", 1).as_deref(),
        Some("Apple")
    );
}

#[test]
fn qualified_member_named_mutable_state_of_is_not_the_compose_function() {
    let source = format!(
        "{CUSTOM_DELEGATE}\
         class Factory {{ fun mutableStateOf(value: Apple): CustomDelegate = TODO() }}\n\
         fun second(factory: Factory) {{ val item by factory.mutableStateOf(Apple()); item }}\n"
    );
    assert_ne!(
        identifier_type(&source, "item", 1).as_deref(),
        Some("Apple")
    );
}

// ─── review round 2: getValue extension identity and nullability ────────────

/// A library extension returns `lib.Apple`; the caller has its own
/// unrelated `app.Apple`.
#[test]
fn get_value_extension_return_type_keeps_the_library_it_was_written_in() {
    let files = [
        (
            "/lib/Holder.kt",
            "package lib\nclass Apple\nclass Holder\n\
             operator fun Holder.getValue(thisRef: Any?, property: Any?): Apple = TODO()\n",
        ),
        (
            "/app/Use.kt",
            "package app\nimport lib.Holder\nimport lib.getValue\nclass Apple\n\
             fun make(): Holder = TODO()\n\
             fun use() { val item by make(); item }\n",
        ),
    ];
    assert_eq!(
        identifier_type_in_workspace(&files, "/app/Use.kt", "item", 1),
        None
    );
}

#[test]
fn state_of_a_nullable_type_reads_as_a_nullable_local() {
    let source = format!(
        "{STATE_DECLARATIONS}\
         fun nullableState(): State<Apple?> = TODO()\n\
         fun second() {{ val item by nullableState(); item }}\n"
    );
    assert_eq!(local_type_at(&source, "item", 3).as_deref(), Some("Apple?"));
}

// ─── review round 3: nullability through delegates, bindings, `error` ───────

/// `getValue(): Apple?` (member): the local reads `Apple?`, so it is no
/// non-null elvis default.
#[test]
fn member_get_value_returning_a_nullable_keeps_the_nullable_local() {
    let source = "class Apple\n\
                  class Delegate { operator fun getValue(thisRef: Any?, property: Any?): Apple? = null }\n\
                  fun make(): Delegate = TODO()\n\
                  fun first(item: Int) = item\n\
                  fun use(left: Apple?) { val item by make(); val result = left ?: item; result }\n";
    assert_eq!(local_type_at(source, "item", 3).as_deref(), Some("Apple?"));
    assert_eq!(identifier_type(source, "result", 1), None);
}

/// The extension path loses the `?` the same way.
#[test]
fn extension_get_value_returning_a_nullable_keeps_the_nullable_local() {
    let source = "class Apple\n\
                  class Delegate\n\
                  operator fun Delegate.getValue(thisRef: Any?, property: Any?): Apple? = null\n\
                  fun make(): Delegate = TODO()\n\
                  fun first(item: Int) = item\n\
                  fun use(left: Apple?) { val item by make(); val result = left ?: item; result }\n";
    assert_eq!(local_type_at(source, "item", 3).as_deref(), Some("Apple?"));
    assert_eq!(identifier_type(source, "result", 1), None);
}

/// `fallback`'s inferred type is a bare `Apple`, but `nullableCall()` can be
/// null: it is no non-null default.
#[test]
fn elvis_default_identifier_initialized_from_a_nullable_call_is_unknown() {
    let source = "class Apple\n\
                  fun nullableCall(): Apple? = null\n\
                  fun first(result: Int) = result\n\
                  fun use(left: Apple?) { val fallback = nullableCall(); val result = left ?: fallback; result }\n";
    assert_eq!(identifier_type(source, "result", 2), None);
}

#[test]
fn elvis_default_identifier_initialized_from_a_constructor_takes_the_type() {
    let source = "class Apple\n\
                  fun first(result: Int) = result\n\
                  fun use(left: Apple?) { val fallback = Apple(); val result = left ?: fallback; result }\n";
    assert_eq!(
        identifier_type(source, "result", 2).as_deref(),
        Some("Apple")
    );
}

/// A local callable named `error` that returns normally is not `Nothing`.
#[test]
fn elvis_with_a_local_lambda_named_error_is_unknown() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun first(result: Int) = result\n\
                  fun use(left: Apple?, error: (String) -> Banana) { val result = left ?: error(\"missing\"); result }\n";
    assert_eq!(identifier_type(source, "result", 2), None);
}

/// `Nothing?` can return `null`, so it does not end the elvis.
#[test]
fn elvis_with_an_error_function_returning_nullable_nothing_is_unknown() {
    let source = "class Apple\n\
                  fun error(message: String): Nothing? = null\n\
                  fun first(result: Int) = result\n\
                  fun use(left: Apple?) { val result = left ?: error(\"missing\"); result }\n";
    assert_eq!(identifier_type(source, "result", 2), None);
}

// ─── Moneta loss shapes (slice A round 4) ────────────────────────────────────

const COLLECTION_DECLARATIONS: &str = "class Apple(val amount: Int)\n\
     class Banana\n\
     fun <T> Iterable<T>.firstOrNull(predicate: (T) -> Boolean): T? = TODO()\n\
     fun <T> Iterable<T>.sumOf(selector: (T) -> Int): Int = TODO()\n";

/// `RetentionOfferTextsInteractor`: a named lambda parameter of a call on the
/// extension receiver `this`.
#[test]
fn lambda_parameter_on_an_extension_receiver_is_typed_from_its_element() {
    let source = format!(
        "{COLLECTION_DECLARATIONS}\
         fun sibling(offer: Banana) = offer\n\
         fun List<Apple>.findOffer() = this.firstOrNull {{ offer ->\n\
         \x20   offer\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&source, "offer", 3).as_deref(),
        Some("Apple")
    );
}

/// `ChartsOverviewMapper`: `it` of a call on a field read through the
/// extension receiver's implicit `this`.
#[test]
fn it_on_an_implicit_receiver_field_is_typed_from_its_element() {
    let source = format!(
        "{COLLECTION_DECLARATIONS}\
         class ItemsList(val items: List<Apple>)\n\
         fun sibling(it: Banana) = it\n\
         fun ItemsList.total() = items.sumOf {{ it.amount }}\n"
    );
    assert_eq!(identifier_type(&source, "it", 2).as_deref(), Some("Apple"));
}

/// Compose `CStateful(data: StatefulModel<T>, ..) { model: T -> }`: `T` is
/// bound by the `data` argument.
const STATEFUL_DECLARATIONS: &str = "class StatefulModel<T>(val data: T)\n\
     class Content(val title: String)\n\
     class Banana\n\
     class State(val content: StatefulModel<Content>)\n\
     fun <T> CStateful(data: StatefulModel<T>, loading: (T?) -> Unit = {}, content: (T) -> Unit) {}\n\
     fun sibling(model: Banana) = model\n";

#[test]
fn lambda_parameter_is_typed_from_a_named_argument_binding_its_type_parameter() {
    let source = format!(
        "{STATEFUL_DECLARATIONS}\
         fun screen(state: State) {{\n\
         \x20   CStateful(\n\
         \x20       data = state.content,\n\
         \x20   ) {{ model ->\n\
         \x20       model\n\
         \x20   }}\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&source, "model", 3).as_deref(),
        Some("Content")
    );
}

#[test]
fn lambda_parameter_is_typed_from_a_positional_argument_binding_its_type_parameter() {
    let source = format!(
        "{STATEFUL_DECLARATIONS}\
         fun screen(state: State) {{\n\
         \x20   CStateful(state.content) {{ model -> model }}\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&source, "model", 3).as_deref(),
        Some("Content")
    );
}

/// An argument whose type is unknown binds nothing: unknown, never `Banana`.
#[test]
fn lambda_parameter_stays_unknown_when_no_argument_binds_its_type_parameter() {
    let source = format!(
        "{STATEFUL_DECLARATIONS}\
         fun screen() {{\n\
         \x20   CStateful(mystery()) {{ model -> model }}\n\
         }}\n"
    );
    assert_eq!(identifier_type(&source, "model", 3), None);
}

/// `state.model` is a `lib.StatefulModel<lib.Content>`; the caller's own
/// `Content` is a different class that merely shares the name.
#[test]
fn lambda_parameter_bound_from_an_argument_keeps_the_arguments_own_class() {
    let files = [
        (
            "/lib/State.kt",
            "package lib\nclass Content\nclass StatefulModel<T>(val data: T)\n\
             class State(val model: StatefulModel<Content>)\n",
        ),
        (
            "/app/Screen.kt",
            "package app\nimport lib.State\nimport lib.StatefulModel\nclass Content\n\
             fun <T> CStateful(data: StatefulModel<T>, content: (T) -> Unit) {}\n\
             fun screen(state: State) { CStateful(state.model) { model -> model } }\n",
        ),
    ];
    assert_eq!(
        identifier_type_in_workspace(&files, "/app/Screen.kt", "model", 2),
        None
    );
}

/// `Reordered<X, Y> : Base<Y>`: the argument's first type argument is not
/// `Base`'s.
#[test]
fn lambda_parameter_bound_from_an_argument_of_a_different_type_constructor_is_not_zipped() {
    let source = "class Apple\n\
                  class Banana\n\
                  open class Base<T>\n\
                  class Reordered<X, Y> : Base<Y>()\n\
                  fun <T> consume(value: Base<T>, content: (T) -> Unit) {}\n\
                  fun sibling(model: Int) = model\n\
                  fun screen() { consume(Reordered<Apple, Banana>()) { model -> model } }\n";
    let model = identifier_type(source, "model", 3);
    assert!(
        matches!(model.as_deref(), None | Some("Banana")),
        "never Base's `T` read as Reordered's first argument: {model:?}"
    );
}

/// `consume(first: T, second: T)` with unrelated arguments needs a common
/// supertype, which this engine does not compute.
#[test]
fn lambda_parameter_bound_by_disagreeing_arguments_is_unknown() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun <T> consume(first: T, second: T, content: (T) -> Unit) {}\n\
                  fun sibling(model: Int) = model\n\
                  fun screen() { consume(Apple(), Banana()) { model -> model } }\n";
    assert_eq!(identifier_type(source, "model", 3), None);
}

#[test]
fn lambda_parameter_bound_by_agreeing_arguments_is_that_type() {
    let source = "class Apple\n\
                  fun <T> consume(first: T, second: T, content: (T) -> Unit) {}\n\
                  fun sibling(model: Int) = model\n\
                  fun screen() { consume(Apple(), Apple()) { model -> model } }\n";
    assert_eq!(
        identifier_type(source, "model", 3).as_deref(),
        Some("Apple")
    );
}

/// `ConfirmDataUseCase`: a property of a generic class read through a typed
/// field (`Store<State>.currentState`).
#[test]
fn local_initialized_from_a_generic_property_chain_is_typed() {
    let source = "class Personal(val street: String)\n\
                  class BusinessState(val personalData: Personal)\n\
                  class Store<T>(initial: T) { val currentState: T = initial }\n\
                  fun sibling(data: Int) = data\n\
                  class UseCase {\n\
                  \x20   private val store: Store<BusinessState> = TODO()\n\
                  \x20   fun read() {\n\
                  \x20       val data = store.currentState.personalData\n\
                  \x20       data\n\
                  \x20   }\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "data", 3).as_deref(),
        Some("Personal")
    );
}

/// `PersonalDetailMapper`: a safe call keeps the member's type, nullable.
#[test]
fn local_initialized_from_a_safe_call_is_typed_nullable() {
    let source = "class Address\n\
                  class Scenes(val addressSection: Address)\n\
                  fun sibling(addressTexts: Int) = addressTexts\n\
                  fun prepare(texts: Scenes?) {\n\
                  \x20   val addressTexts = texts?.addressSection\n\
                  \x20   addressTexts\n\
                  }\n";
    assert_eq!(
        local_type_at(source, "addressTexts", 3).as_deref(),
        Some("Address?")
    );
    assert_eq!(
        identifier_type(source, "addressTexts", 3).as_deref(),
        Some("Address")
    );
}

/// The element keeps the nullability it was written with.
#[test]
fn for_variable_over_a_list_of_nullable_elements_is_nullable() {
    let source = "class Apple\n\
                  fun walk(items: List<Apple?>) {\n\
                  \x20   for (item in items) { val copy = item; copy }\n\
                  }\n";
    assert_eq!(local_type_at(source, "item", 1).as_deref(), Some("Apple?"));
    assert_eq!(local_type_at(source, "copy", 1).as_deref(), Some("Apple?"));
}

/// `WidgetWorker`: a `for` variable over a typed list.
#[test]
fn for_variable_over_a_typed_list_is_typed_from_its_element() {
    let source = "class Widget(val error: Int)\n\
                  class Banana\n\
                  fun sibling(widget: Banana) = widget\n\
                  fun onSuccess(widgets: List<Widget>) {\n\
                  \x20   for (widget in widgets) { widget }\n\
                  }\n";
    assert_eq!(
        identifier_type(source, "widget", 3).as_deref(),
        Some("Widget")
    );
}

#[test]
fn for_variable_over_a_range_is_an_int() {
    let source = "class Banana\n\
                  fun sibling(index: Banana) = index\n\
                  fun walk() {\n\
                  \x20   for (index in 0..10) { index }\n\
                  }\n";
    assert_eq!(identifier_type(source, "index", 3).as_deref(), Some("Int"));
}

#[test]
fn for_variable_over_a_map_is_an_entry() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun sibling(entry: Banana) = entry\n\
                  fun walk(prices: Map<String, Apple>) {\n\
                  \x20   for (entry in prices) { entry }\n\
                  }\n";
    assert_eq!(
        local_type_at(source, "entry", 3).as_deref(),
        Some("Map.Entry<String, Apple>")
    );
}

/// An iterable of unknown type leaves the variable unknown, never `Banana`.
#[test]
fn for_variable_over_an_unknown_iterable_stays_unknown() {
    let source = "class Banana\n\
                  fun sibling(widget: Banana) = widget\n\
                  fun walk() {\n\
                  \x20   for (widget in mystery()) { widget }\n\
                  }\n";
    assert_eq!(identifier_type(source, "widget", 3), None);
}

// ─── destructuring ───────────────────────────────────────────────────────────

const DESTRUCTURING_DECLARATIONS: &str = "class Apple\n\
     class Banana\n\
     class Cherry\n\
     fun pair(): Pair<Apple, Banana> = TODO()\n\
     fun triple(): Triple<Apple, Banana, Cherry> = TODO()\n\
     fun sibling(first: Banana, second: Apple, third: Apple) = first\n";

#[test]
fn destructured_pair_components_are_its_type_arguments() {
    let source = format!(
        "{DESTRUCTURING_DECLARATIONS}\
         fun use() {{\n\
         \x20   val (first, second) = pair()\n\
         \x20   first; second\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&source, "first", 3).as_deref(),
        Some("Apple")
    );
    assert_eq!(
        identifier_type(&source, "second", 2).as_deref(),
        Some("Banana")
    );
}

#[test]
fn destructured_triple_components_are_its_type_arguments() {
    let source = format!(
        "{DESTRUCTURING_DECLARATIONS}\
         fun use() {{\n\
         \x20   val (first, _, third) = triple()\n\
         \x20   first; third\n\
         }}\n"
    );
    assert_eq!(
        identifier_type(&source, "first", 3).as_deref(),
        Some("Apple")
    );
    assert_eq!(
        identifier_type(&source, "third", 2).as_deref(),
        Some("Cherry")
    );
}

#[test]
fn destructured_map_entry_in_a_for_loop_is_key_and_value() {
    let source = "class Apple\n\
                  class Banana\n\
                  fun sibling(key: Banana, value: Banana) = key\n\
                  fun walk(prices: Map<String, Apple>) {\n\
                  \x20   for ((key, value) in prices) { key; value }\n\
                  }\n";
    assert_eq!(identifier_type(source, "key", 3).as_deref(), Some("String"));
    assert_eq!(
        identifier_type(source, "value", 2).as_deref(),
        Some("Apple")
    );
}

/// A source of unknown type, or one whose components this engine cannot
/// read (a data class), leaves the components unknown, never `Banana`.
#[test]
fn destructured_components_of_an_unknown_source_stay_unknown() {
    let source = format!(
        "{DESTRUCTURING_DECLARATIONS}\
         data class Basket(val first: Apple, val second: Banana)\n\
         fun basket(): Basket = TODO()\n\
         fun use() {{\n\
         \x20   val (first, second) = mystery()\n\
         \x20   first\n\
         }}\n\
         fun other() {{\n\
         \x20   val (first, second) = basket()\n\
         \x20   second\n\
         }}\n"
    );
    assert_eq!(identifier_type(&source, "first", 4), None);
    assert_eq!(identifier_type(&source, "second", 4), None);
}

// ─── unbound type parameters ─────────────────────────────────────────────────

/// `await`'s `A` and `B` are its own type parameters, never the unrelated
/// classes of the same name, and nothing binds them from the arguments.
#[test]
fn destructured_component_of_an_unbound_function_type_parameter_is_unknown() {
    let source = "class A { fun foo() {} }\n\
                  class B { fun foo() {} }\n\
                  class Deferred<T>\n\
                  fun <A, B> both(first: Deferred<A>, second: Deferred<B>): Pair<A, B> = TODO()\n\
                  fun use(left: Deferred<A>, right: Deferred<B>) {\n\
                  \x20   val (setup, texts) = both(left, right)\n\
                  \x20   setup; texts\n\
                  }\n";
    assert_eq!(local_type_at(source, "setup", 1), None);
    assert_eq!(local_type_at(source, "texts", 1), None);
}

/// An explicit call-site type argument binds the parameter.
#[test]
fn destructured_component_of_an_explicitly_bound_type_parameter_is_that_type() {
    let source = "class Apple\n\
                  class Banana\n\
                  class Deferred<T>\n\
                  fun <A, B> both(first: Deferred<A>, second: Deferred<B>): Pair<A, B> = TODO()\n\
                  fun use(left: Deferred<Apple>, right: Deferred<Banana>) {\n\
                  \x20   val (setup, texts) = both<Apple, Banana>(left, right)\n\
                  \x20   setup; texts\n\
                  }\n";
    assert_eq!(local_type_at(source, "setup", 1).as_deref(), Some("Apple"));
    assert_eq!(local_type_at(source, "texts", 1).as_deref(), Some("Banana"));
}

/// A class's own type parameter, read through a raw receiver, is unbound too.
#[test]
fn destructured_component_of_an_unbound_class_type_parameter_is_unknown() {
    let source = "class A { fun foo() {} }\n\
                  class Holder<A> { fun twins(): Pair<A, A> = TODO() }\n\
                  fun use(holder: Holder) {\n\
                  \x20   val (left, right) = holder.twins()\n\
                  \x20   left; right\n\
                  }\n";
    assert_eq!(local_type_at(source, "left", 1), None);
}

/// `items()` has nothing to infer `A` from: its elements are unknown, not the
/// unrelated class `A`.
#[test]
fn for_variable_over_an_unbound_type_parameter_collection_is_unknown() {
    let source = "class A { fun foo() {} }\n\
                  fun <A> items(): List<A> = TODO()\n\
                  fun walk() {\n\
                  \x20   for (element in items()) { element }\n\
                  }\n";
    assert_eq!(local_type_at(source, "element", 1), None);
}

/// A raw `Holder` reads its `value: A` as unknown, not as class `A`.
#[test]
fn member_of_a_raw_generic_receiver_is_unknown() {
    let source = "class A { fun foo() {} }\n\
                  class Holder<A>(val value: A, val all: List<A>)\n\
                  fun use(holder: Holder) {\n\
                  \x20   val one = holder.value\n\
                  \x20   val many = holder.all\n\
                  \x20   one; many\n\
                  }\n";
    assert_eq!(local_type_at(source, "one", 1), None);
    assert_eq!(local_type_at(source, "many", 1).as_deref(), Some("List<*>"));
}

/// The concrete `Factory.make(): List<Apple>` is the declaration the call
/// binds; an unrelated top-level `fun <Apple> make(..)` declares a type
/// parameter of that name, which must not star the member's `Apple`.
#[test]
fn member_return_type_is_not_starred_by_an_unrelated_same_named_generic_function() {
    let source = "class Apple\n\
                  class Factory { fun make(): List<Apple> = TODO() }\n\
                  fun <Apple> make(value: Apple): List<Apple> = TODO()\n\
                  fun use(factory: Factory) { val items = factory.make(); items }\n";
    assert_eq!(
        local_type_at(source, "items", 1).as_deref(),
        Some("List<Apple>")
    );
}

/// `model.A` is a class path, not a reference to the function's `A`.
#[test]
fn unbound_type_parameter_does_not_star_a_qualified_class_path() {
    let source = "fun <A> values(input: A): List<model.A> = TODO()\n\
                  fun use() { val items = values(1); items }\n";
    assert_eq!(
        local_type_at(source, "items", 1).as_deref(),
        Some("List<model.A>")
    );
}

fn this_type(source: &str) -> Option<String> {
    let live_doc = live_doc_for(source);
    let indexer = Indexer::new();
    let uri = test_url("/Binding.kt");
    indexer.index_content(&uri, source);
    let this_expression = crate::indexer::walk::descendants(live_doc.tree.root_node())
        .find(|node| node.kind() == crate::queries::KIND_THIS_EXPR)
        .expect("a this expression");
    CstQuery::new(this_expression, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved()
        .map(|resolved| resolved.as_type_str().to_owned())
}

/// `expr_type` of the `occurrence`-th (0-based) `this` expression in `source`.
fn nth_this_type(source: &str, occurrence: usize) -> Option<String> {
    let live_doc = live_doc_for(source);
    let indexer = Indexer::new();
    let uri = test_url("/Binding.kt");
    indexer.index_content(&uri, source);
    let this_expression = crate::indexer::walk::descendants(live_doc.tree.root_node())
        .filter(|node| node.kind() == crate::queries::KIND_THIS_EXPR)
        .nth(occurrence)
        .expect("a this expression");
    CstQuery::new(this_expression, &live_doc, &indexer, &uri)
        .expr_type()
        .resolved()
        .map(|resolved| resolved.as_type_str().to_owned())
}

/// `this@Outer` names the outer class, not the extension's innermost receiver.
#[test]
fn labelled_this_in_an_extension_is_not_the_extension_receiver() {
    let source = "class Inner\n\
                  class Outer { fun Inner.read() { this; this@Outer } }\n";
    assert_eq!(nth_this_type(source, 0).as_deref(), Some("Inner"));
    let outer = nth_this_type(source, 1);
    assert!(
        matches!(outer.as_deref(), None | Some("Outer")),
        "`this@Outer` is never the receiver `Inner`: {outer:?}"
    );
}

/// `this` in `fun actual.Widget.read()` is `actual.Widget`, not whichever
/// `Widget` the file imports.
#[test]
fn this_in_a_qualified_receiver_extension_keeps_the_package_path() {
    let source = "import other.Widget\n\
                  fun actual.Widget.read() { this }\n";
    assert_eq!(nth_this_type(source, 0).as_deref(), Some("actual.Widget"));
}

/// ...and past a local property: `val copy = this` is still the receiver.
#[test]
fn this_in_a_local_property_of_an_extension_is_its_receiver() {
    let source = "class Apple\nclass Host { fun Apple.read() { val copy = this } }\n";
    assert_eq!(nth_this_type(source, 0).as_deref(), Some("Apple"));
}

/// `this` in `fun <A> A.show()` is the extension's own type parameter `A`,
/// never the unrelated class of that name.
#[test]
fn this_in_an_extension_on_a_type_parameter_is_unknown() {
    let source = "class A { fun foo() {} }\n\
                  fun <A> A.show() { this }\n";
    assert_eq!(this_type(source), None);
}

/// A concrete extension receiver stays its written type.
#[test]
fn this_in_an_extension_on_a_class_is_that_class() {
    let source = "class A { fun foo() {} }\n\
                  fun A.show() { this }\n";
    assert_eq!(this_type(source).as_deref(), Some("A"));
}

// ─── type parameters bound through the receiver's declared type ─────────────

/// `store.currentState` is `State`, bound to `Content` by the receiver's type
/// argument through the `Store<State>` supertype.
#[test]
fn member_typed_by_a_supertype_type_parameter_is_bound_by_the_receiver() {
    let source = "class Content { val items: List<Item> = TODO() }\n\
                  class Item\n\
                  class Decoy { val items: Int = 0 }\n\
                  interface Store<State> { val currentState: State }\n\
                  interface MutableStore<State> : Store<State>\n\
                  class Owner(private val store: MutableStore<Content>) {\n\
                  \x20   fun use() { val current = store.currentState; current }\n\
                  \x20   fun chain() { val list = store.currentState.items; list }\n\
                  }\n";
    assert_eq!(
        local_type_at(source, "current", 1).as_deref(),
        Some("Content")
    );
    assert_eq!(
        local_type_at(source, "list", 1).as_deref(),
        Some("List<Item>")
    );
}

/// Same, but the supertype names its parameter differently.
#[test]
fn member_typed_by_a_renamed_supertype_type_parameter_is_bound_by_the_receiver() {
    let source = "class Content\n\
                  class State\n\
                  interface Store<S> { val currentState: S }\n\
                  interface MutableStore<State> : Store<State>\n\
                  class Owner(private val store: MutableStore<Content>) {\n\
                  \x20   fun use() { val current = store.currentState; current }\n\
                  }\n";
    assert_eq!(
        local_type_at(source, "current", 1).as_deref(),
        Some("Content")
    );
}

/// Concrete class with a bounded type parameter, property read.
#[test]
fn property_of_a_bounded_type_parameter_class_is_bound_by_the_receiver() {
    let source = "open class Base\n\
                  class Content : Base()\n\
                  class Store<State : Base>(initial: State) {\n\
                  \x20   val currentState: State get() = TODO()\n\
                  }\n\
                  class Owner(private val store: Store<Content>) {\n\
                  \x20   fun use() { val current = store.currentState; current }\n\
                  }\n";
    assert_eq!(
        local_type_at(source, "current", 1).as_deref(),
        Some("Content")
    );
}

// ─── explicit type arguments of a constructor call ──────────────────────────

/// `make<Content, Int>()` has no declared return type: it is the body's
/// `Mutable<State, Effect>(...)`, whose own type parameters the call site
/// binds. The `State` class beside it is not what `State` means there.
#[test]
fn expression_body_factory_keeps_the_constructor_call_type_arguments() {
    let source = "class Content\n\
                  class State\n\
                  interface Store<State> { val currentState: State }\n\
                  interface WithEffects<State, Effect> : Store<State>\n\
                  class Mutable<State, Effect> : WithEffects<State, Effect>\n\
                  fun <State, Effect> make() = Mutable<State, Effect>()\n\
                  fun <State, Effect> Mutable<State, Effect>.toWith(): WithEffects<State, Effect> = this\n\
                  class Owner {\n\
                  \x20   private val inner = make<Content, Int>()\n\
                  \x20   val store = inner.toWith()\n\
                  \x20   fun use() { val made = inner; val read = store.currentState; made; read }\n\
                  }\n";
    assert_eq!(
        local_type_at(source, "made", 1).as_deref(),
        Some("Mutable<Content, Int>")
    );
    assert_eq!(local_type_at(source, "read", 1).as_deref(), Some("Content"));
}

/// A constructor call that spells no type arguments stays the bare class.
#[test]
fn constructor_call_without_type_arguments_is_the_bare_class() {
    let source = "class Box<Item>\n\
                  fun use() { val made = Box(); made }\n";
    assert_eq!(local_type_at(source, "made", 1).as_deref(), Some("Box"));
}

/// A member's own type parameter, bound by nothing at the call, is unknown.
#[test]
fn member_function_unbound_type_parameter_is_unknown() {
    let source = "class R { fun foo() {} }\n\
                  class Foo { fun <R> twins(): Pair<R, R> = TODO() }\n\
                  fun use(foo: Foo) {\n\
                  \x20   val (left, right) = foo.twins()\n\
                  \x20   left; right\n\
                  }\n";
    assert_eq!(local_type_at(source, "left", 1), None);
}

// ─── review round 2: unbound type parameters belong to the selected member ──

/// `local_type_at` with every file in `files` indexed.
fn local_type_in_workspace(
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
    match crate::indexer::infer_local_type_at(name, identifier, &live_doc.bytes, &indexer, &uri) {
        crate::indexer::BindingType::Typed { type_text, .. } => Some(type_text),
        crate::indexer::BindingType::LocalUnknown | crate::indexer::BindingType::NotLocal => None,
    }
}

/// `Holder`'s parameter is *named* `Item`, but the extension's `List<Item>`
/// names the real class `Item`.
#[test]
fn extension_return_naming_a_class_is_not_starred_by_the_receivers_parameter() {
    let source = "class Item\n\
                  class Holder<Item>\n\
                  fun Holder<*>.items(): List<Item> = TODO()\n\
                  fun use(holder: Holder<*>) { val result = holder.items(); result }\n";
    assert_eq!(
        local_type_at(source, "result", 1).as_deref(),
        Some("List<Item>")
    );
}

/// Another package's generic `Factory<Apple>` must not decide what the
/// imported, non-generic `Factory.make()` returns.
#[test]
fn member_return_is_not_starred_by_a_same_named_class_in_another_package() {
    let files = [
        (
            "/lib/a/Factory.kt",
            "package lib.a\nclass Apple\nclass Factory { fun make(): List<Apple> = TODO() }\n",
        ),
        (
            "/lib/b/Factory.kt",
            "package lib.b\nclass Factory<Apple> { fun make(): List<Apple> = TODO() }\n",
        ),
        (
            "/app/Use.kt",
            "package app\nimport lib.a.Apple\nimport lib.a.Factory\n\
             fun use(factory: Factory) { val result = factory.make(); result }\n",
        ),
    ];
    assert_eq!(
        local_type_in_workspace(&files, "/app/Use.kt", "result", 1).as_deref(),
        Some("List<Apple>")
    );
}

/// `twin()` is inherited from `Base<R>` through the non-generic `Child`;
/// `R` must read as the supertype argument, never as the unrelated class `R`.
#[test]
fn inherited_member_return_does_not_name_its_declaring_class_parameter() {
    let source = "class R\n\
                  class Apple\n\
                  open class Base<R> { fun twin(): Pair<R, R> = TODO() }\n\
                  class Child : Base<Apple>()\n\
                  fun use(child: Child) { val result = child.twin(); result }\n";
    assert_eq!(
        local_type_at(source, "result", 1).as_deref(),
        Some("Pair<Apple, Apple>")
    );
}

// ─── review round 2: lambda parameters bound by several arguments ───────────

/// `Duo<Apple, Banana>` against `Duo<T, T>` binds `T` to both; with no
/// common supertype computed, the lambda parameter is unknown.
#[test]
fn lambda_parameter_bound_by_one_argument_naming_two_types_is_unknown() {
    let source = "class Apple\n\
                  class Banana\n\
                  class Duo<A, B>\n\
                  fun <T> consume(value: Duo<T, T>, content: (T) -> Unit) {}\n\
                  fun use(mixed: Duo<Apple, Banana>) { consume(mixed) { item -> item } }\n";
    assert_eq!(identifier_type(source, "item", 1), None);
}

/// Two arguments both spell `Box<Apple>`, but in different packages.
#[test]
fn lambda_parameter_bound_by_same_named_types_of_different_packages_is_unknown() {
    let files = [
        (
            "/lib/Box.kt",
            "package lib\nclass Box<E>\n\
                         fun <T> consume(first: Box<T>, second: Box<T>, content: (T) -> Unit) {}\n",
        ),
        (
            "/a/A.kt",
            "package a\nimport lib.Box\nclass Apple\nfun makeA(): Box<Apple> = TODO()\n",
        ),
        (
            "/b/B.kt",
            "package b\nimport lib.Box\nclass Apple\nfun makeB(): Box<Apple> = TODO()\n",
        ),
        (
            "/app/Use.kt",
            "package app\nimport a.Apple\nimport a.makeA\nimport b.makeB\nimport lib.consume\n\
             fun use() { consume(makeA(), makeB()) { item -> item } }\n",
        ),
    ];
    assert_eq!(
        identifier_type_in_workspace(&files, "/app/Use.kt", "item", 1),
        None
    );
}

// ─── review round 2: for-loop element types of written iterable types ──────

fn loop_variable_type(iterable_type: &str) -> Option<String> {
    let source = format!(
        "class Apple\n\
         fun use(items: {iterable_type}) {{ for (item in items) {{ item }} }}\n"
    );
    local_type_at(&source, "item", 1)
}

#[test]
fn loop_variable_of_a_fully_qualified_collection_is_its_element() {
    assert_eq!(
        loop_variable_type("kotlin.collections.List<Apple>").as_deref(),
        Some("Apple")
    );
}

#[test]
fn loop_variable_keeps_a_package_qualified_element() {
    assert_eq!(
        loop_variable_type("List<app.Apple>").as_deref(),
        Some("app.Apple")
    );
}

#[test]
fn loop_variable_of_a_projected_array_is_its_element() {
    assert_eq!(
        loop_variable_type("Array<out Apple>").as_deref(),
        Some("Apple")
    );
}

/// Not a collection: its first type argument is not an element.
#[test]
fn loop_variable_of_an_unrelated_generic_is_unknown() {
    assert_eq!(loop_variable_type("kotlin.Lazy<Apple>"), None);
    assert_eq!(loop_variable_type("Lazy<Apple>"), None);
}

// ─── review round 2: member types through safe calls and nullable arguments ─

/// The nullability of `holder?.value` is the function value's, not its
/// return type's.
#[test]
fn safe_call_on_a_function_valued_member_makes_the_function_nullable() {
    let source = "class Holder<T>(val value: T)\n\
                  fun use(holder: Holder<(Int) -> String>?) { val value = holder?.value; value }\n";
    assert_eq!(
        local_type_at(source, "value", 3).as_deref(),
        Some("((Int) -> String)?")
    );
}

#[test]
fn member_of_a_receiver_with_a_nullable_type_argument_is_nullable() {
    let source = "class Apple\n\
                  class Holder<T>(val value: T)\n\
                  fun use(holder: Holder<Apple?>) { val item = holder.value; item }\n";
    assert_eq!(local_type_at(source, "item", 1).as_deref(), Some("Apple?"));
}

/// A non-null argument stays non-null, and `Holder<Apple?>` does not make a
/// `T?` member doubly nullable.
#[test]
fn member_of_a_receiver_with_a_non_null_type_argument_is_not_nullable() {
    let source = "class Apple\n\
                  class Holder<T>(val value: T, val maybe: T?)\n\
                  fun use(holder: Holder<Apple>) { val item = holder.value; item }\n\
                  fun other(holder: Holder<Apple?>) { val item = holder.maybe; item }\n";
    assert_eq!(local_type_at(source, "item", 1).as_deref(), Some("Apple"));
    assert_eq!(local_type_at(source, "item", 3).as_deref(), Some("Apple?"));
}
