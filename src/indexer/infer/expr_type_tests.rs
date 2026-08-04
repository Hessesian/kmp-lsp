use tree_sitter::Parser;

use super::infer_expr_type;
use crate::indexer::infer::deps::TestDeps;
use crate::queries::KIND_FUN_BODY;
use tower_lsp::lsp_types::Url;

fn test_url() -> Url {
    Url::parse("file:///tmp/test.kt").unwrap()
}

fn fun_body_expr_node(src: &str) -> (tree_sitter::Tree, Vec<u8>) {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_kotlin::language()).unwrap();
    let bytes = src.as_bytes().to_vec();
    let tree = p.parse(src, None).unwrap();
    (tree, bytes)
}

/// Parse `fun f() = <expr>` and run `infer_expr_type` on the expression.
fn infer(src: &str) -> Option<String> {
    let full = format!("fun f() = {src}");
    let (tree, bytes) = fun_body_expr_node(&full);
    let root = tree.root_node();
    let fun_decl = root.child(0)?;
    let body = (0..fun_decl.child_count())
        .map(|i| fun_decl.child(i).unwrap())
        .find(|n| n.kind() == KIND_FUN_BODY)?;
    let expr = body.child(1)?;
    infer_expr_type(expr, &bytes, &TestDeps::new(), &test_url())
}

/// Parse `fun f() = <expr>` and run `infer_expr_type` with explicit deps.
fn infer_with_deps(src: &str, deps: &TestDeps) -> Option<String> {
    let full = format!("fun f() = {src}");
    let (tree, bytes) = fun_body_expr_node(&full);
    let root = tree.root_node();
    let fun_decl = root.child(0)?;
    let body = (0..fun_decl.child_count())
        .map(|i| fun_decl.child(i).unwrap())
        .find(|n| n.kind() == KIND_FUN_BODY)?;
    let expr = body.child(1)?;
    infer_expr_type(expr, &bytes, deps, &test_url())
}

// ─── literals ─────────────────────────────────────────────────────────────────

#[test]
fn integer_literal() {
    assert_eq!(infer("42"), Some("Int".into()));
}

#[test]
fn long_literal() {
    assert_eq!(infer("42L"), Some("Long".into()));
}

#[test]
fn float_literal() {
    assert_eq!(infer("1.0f"), Some("Float".into()));
}

#[test]
fn double_literal() {
    assert_eq!(infer("3.14"), Some("Double".into()));
}

#[test]
fn string_literal() {
    assert_eq!(infer(r#""hello""#), Some("String".into()));
}

#[test]
fn boolean_true() {
    assert_eq!(infer("true"), Some("Boolean".into()));
}

#[test]
fn null_literal() {
    assert_eq!(infer("null"), Some("Nothing?".into()));
}

#[test]
fn char_literal() {
    assert_eq!(infer("'x'"), Some("Char".into()));
}

// ─── boolean-returning expressions ────────────────────────────────────────────

#[test]
fn check_expression() {
    assert_eq!(infer("a is String"), Some("Boolean".into()));
}

#[test]
fn check_not_expression() {
    assert_eq!(infer("a !is String"), Some("Boolean".into()));
}

#[test]
fn comparison_expression() {
    assert_eq!(infer("a > 0"), Some("Boolean".into()));
}

#[test]
fn disjunction_expression() {
    assert_eq!(infer("a || b"), Some("Boolean".into()));
}

#[test]
fn conjunction_expression() {
    assert_eq!(infer("a && b"), Some("Boolean".into()));
}

#[test]
fn prefix_not() {
    assert_eq!(infer("!flag"), Some("Boolean".into()));
}

#[test]
fn prefix_minus_no_hint() {
    assert_eq!(infer("-x"), None);
}

// ─── if expression ────────────────────────────────────────────────────────────

#[test]
fn if_else_literal() {
    assert_eq!(infer("if (ok) 1 else 2"), Some("Int".into()));
}

#[test]
fn if_else_string() {
    assert_eq!(infer(r#"if (ok) "yes" else "no""#), Some("String".into()));
}

#[test]
fn if_else_boolean_then() {
    // then-branch is a check expression → Boolean
    assert_eq!(
        infer("if (cond) a is String else false"),
        Some("Boolean".into())
    );
}

#[test]
fn if_without_else_no_hint() {
    // bare if is a statement, not an expression with a known type
    assert_eq!(infer("if (ok) 1"), None);
}

#[test]
fn if_else_unknown_call_no_hint() {
    // listOf is a stdlib function not in TestDeps → None
    assert_eq!(infer("if (ok) listOf(A()) else listOf()"), None);
}

#[test]
fn if_else_mismatched_types_no_hint() {
    assert_eq!(infer("if (ok) 1 else \"no\""), None);
}

// ─── range expression ─────────────────────────────────────────────────────────

#[test]
fn int_range() {
    assert_eq!(infer("1..10"), Some("IntRange".into()));
}

#[test]
fn long_range() {
    assert_eq!(infer("1L..10L"), Some("LongRange".into()));
}

#[test]
fn char_range() {
    assert_eq!(infer("'a'..'z'"), Some("CharRange".into()));
}

#[test]
fn mixed_range_no_hint() {
    // variable operands — can't infer without type-checking
    assert_eq!(infer("a..b"), None);
}

// ─── unresolvable forms (should remain None) ──────────────────────────────────

#[test]
fn navigation_expr_no_hint() {
    assert_eq!(infer("list.size"), None);
}

#[test]
fn additive_no_hint() {
    assert_eq!(infer("a + b"), None);
}

#[test]
fn elvis_no_hint() {
    assert_eq!(infer("a ?: 0"), None);
}

#[test]
fn when_expr_no_hint() {
    assert_eq!(infer(r#"when { x > 0 -> "pos"; else -> "neg" }"#), None);
}

// ─── arithmetic expressions ───────────────────────────────────────────────────
//
// Both shapes below are verbatim (minus identifier names) from a real project
// file (`FxMoneyVM.kt`), found by adding observational logging to
// `hint_property`'s STRING fallback and running against real production code:
// `infer_expr_type` returned `None` for both, live, in the editor.

#[test]
fn multiplicative_int_literals_infers_int() {
    // `private const val TIMER_TICK_MILLIS = 1000 / 2` — verbatim shape.
    assert_eq!(infer("1000 / 2"), Some("Int".into()));
}

#[test]
fn additive_int_literals_infers_int() {
    assert_eq!(infer("1000 + 2"), Some("Int".into()));
}

#[test]
fn multiplicative_unresolvable_operands_no_hint() {
    // Operand types unknown (no deps registered) — must not guess.
    assert_eq!(infer("a * b"), None);
}

#[test]
fn additive_identifier_operands_promote_to_long() {
    // `if (mMillisUntilFinished > 0) mMillisUntilFinished else (timeoutSeconds * 1000).toLong()`
    // reduced to its arithmetic core: `Int * Int` promoted through `.toLong()`
    // must agree with the `Long`-typed `mMillisUntilFinished` operand.
    let deps = TestDeps::new().with_var("file:///tmp/test.kt", "timeoutSeconds", "Int");
    assert_eq!(
        infer_with_deps("(timeoutSeconds * 1000).toLong()", &deps).as_deref(),
        Some("Long")
    );
}

#[test]
fn additive_mixed_int_long_promotes_to_long() {
    let deps = TestDeps::new()
        .with_var("file:///tmp/test.kt", "a", "Int")
        .with_var("file:///tmp/test.kt", "b", "Long");
    assert_eq!(infer_with_deps("a + b", &deps).as_deref(), Some("Long"));
}

#[test]
fn additive_mixed_int_double_promotes_to_double() {
    let deps = TestDeps::new()
        .with_var("file:///tmp/test.kt", "a", "Int")
        .with_var("file:///tmp/test.kt", "b", "Double");
    assert_eq!(infer_with_deps("a + b", &deps).as_deref(), Some("Double"));
}

#[test]
fn additive_byte_operands_promote_to_int() {
    // Kotlin has no `Byte.plus(Byte): Byte` overload -- only `Byte.plus(Byte): Int`
    // (same for `Short`), unlike `Int`/`Long`/`Float`/`Double` which each have a
    // same-type overload. Both operands ranking at `Byte`/`Short` must promote to
    // `Int`, not stay at the narrower operand type.
    let deps = TestDeps::new()
        .with_var("file:///tmp/test.kt", "a", "Byte")
        .with_var("file:///tmp/test.kt", "b", "Byte");
    assert_eq!(infer_with_deps("a + b", &deps).as_deref(), Some("Int"));
}

#[test]
fn additive_byte_and_short_operands_promote_to_int() {
    let deps = TestDeps::new()
        .with_var("file:///tmp/test.kt", "a", "Byte")
        .with_var("file:///tmp/test.kt", "b", "Short");
    assert_eq!(infer_with_deps("a + b", &deps).as_deref(), Some("Int"));
}

#[test]
fn additive_byte_and_int_still_promotes_to_int() {
    // Sanity check the existing "higher rank wins" path still holds once one
    // operand already outranks Byte/Short.
    let deps = TestDeps::new()
        .with_var("file:///tmp/test.kt", "a", "Byte")
        .with_var("file:///tmp/test.kt", "b", "Int");
    assert_eq!(infer_with_deps("a + b", &deps).as_deref(), Some("Int"));
}

#[test]
fn additive_string_concat_infers_string() {
    // `"Error: " + errorCode` — Kotlin's `String.plus(Any?): String`.
    assert_eq!(infer(r#""Error: " + 42"#), Some("String".into()));
}

#[test]
fn additive_non_numeric_operand_no_hint() {
    // `Foo() + 1` — `Foo` isn't a known numeric/String type, no guess.
    assert_eq!(infer("Foo() + 1"), None);
}

#[test]
fn parenthesized_expr_unwraps_to_inner_type() {
    assert_eq!(infer("(42)"), Some("Int".into()));
}

// ─── numeric/char conversion functions (`toLong()`, `toInt()`, …) ─────────────

#[test]
fn to_long_on_unresolvable_receiver_infers_long() {
    // `x.toLong()` where `x`'s type/`toLong` itself is not indexed anywhere —
    // the conversion function's name alone determines its return type.
    assert_eq!(
        infer_with_deps("x.toLong()", &TestDeps::new()).as_deref(),
        Some("Long")
    );
}

#[test]
fn to_int_on_unresolvable_receiver_infers_int() {
    assert_eq!(
        infer_with_deps("x.toInt()", &TestDeps::new()).as_deref(),
        Some("Int")
    );
}

// ─── constructor + lambda-result (remember) ───────────────────────────────────

#[test]
fn constructor_call_infers_type_name() {
    // `Foo(...)` with no resolvable function return type is a constructor → `Foo`.
    assert_eq!(infer("Foo(1, 2)"), Some("Foo".into()));
}

#[test]
fn lowercase_call_is_not_a_constructor() {
    // `foo()` (lowercase) is a function call, not a constructor — no bogus type.
    assert_eq!(infer("foo()"), None);
}

#[test]
fn remember_infers_lambda_constructor_result() {
    // Compose `remember { Foo() }` returns its lambda's value → `Foo`, instead of
    // resolving against an unrelated same-named overload.
    assert_eq!(infer("remember { Foo() }"), Some("Foo".into()));
}

#[test]
fn remember_saveable_infers_lambda_result() {
    assert_eq!(infer("rememberSaveable { Bar() }"), Some("Bar".into()));
}

#[test]
fn remember_empty_lambda_is_none() {
    assert_eq!(infer("remember { }"), None);
}

// ─── generic DI-factory calls (`get<T>()`, `inject<T>()`, …) ──────────────────
//
// See `resolver::infer_lines::infer_from_rhs_assignment`'s "Pattern 2": DI
// frameworks like Koin expose `inline fun <reified T> get(): T` as a top-level
// function, so when it isn't indexed (external/unpromoted JAR, or simply
// absent from a test fixture) `resolve_call_expr_type` has no return type to
// substitute into and no receiver to fall back on. TestDeps registers nothing
// for "get", reproducing that exact "unindexed DI call" shape — resolved via
// `GENERIC_FACTORY_FNS` reading the call's own `<T>` type argument directly.
#[test]
fn generic_factory_call_with_unindexed_fn_infers_type_arg() {
    assert_eq!(infer("get<Foo>()"), Some("Foo".into()));
}

#[test]
fn generic_factory_call_multi_type_arg_no_hint() {
    // Only single reified-type-arg factory calls are covered by the heuristic;
    // a multi-arg generic call isn't a DI-factory shape, so no guess is made.
    assert_eq!(infer("get<Foo, Bar>()"), None);
}

#[test]
fn generic_call_to_unknown_non_factory_fn_no_hint() {
    // `frobnicate<Foo>()` isn't in the known DI-factory name list — no guess.
    assert_eq!(infer("frobnicate<Foo>()"), None);
}

// ─── Retrofit-style class-literal argument (`recv.create(Foo::class.java)`) ──
//
// See `resolver::infer_lines::infer_from_rhs_assignment`'s "Pattern 3": when
// neither `retrofit`'s type nor `create` itself is indexed (external Retrofit
// dependency, unpromoted JAR, or absent from a test fixture), member/by-name
// return-type lookup finds nothing. The call's own argument list carries the
// answer directly: `Foo::class.java` names the exact type `create` returns.
#[test]
fn retrofit_style_create_with_class_literal_arg_infers_type() {
    assert_eq!(
        infer("retrofit.create(DashboardApi::class.java)"),
        Some("DashboardApi".into())
    );
}

#[test]
fn bare_class_literal_arg_without_java_suffix_infers_type() {
    // `::class` without a trailing `.java` — still a class-literal argument.
    // Uses `create` (a `GENERIC_FACTORY_FNS` name), not an arbitrary function
    // name: this fallback is deliberately gated to a curated list of known
    // factory-pattern names (see that constant's doc comment) precisely
    // because ungating it caused a real production bug -- a same-named,
    // completely unrelated `build`/`create`/etc. elsewhere in the workspace
    // could otherwise get bare-name-matched with higher priority, or (before
    // the ordering fix) this fallback could itself override a real,
    // correctly-indexed, differently-named function that merely happens to
    // take a class-literal argument for an unrelated reason.
    assert_eq!(
        infer("factory.create(Widget::class)"),
        Some("Widget".into())
    );
}

#[test]
fn qualified_class_literal_arg_infers_leaf_type() {
    // `com.example.Foo::class.java` -- the type_identifier's raw text
    // carries the full dotted path; the leaf (last segment) is the actual
    // class name, matching the STRING-side equivalent (`infer_lines`'s
    // "Pattern 3", which also extracts the last segment).
    assert_eq!(
        infer("retrofit.create(com.example.DashboardApi::class.java)"),
        Some("DashboardApi".into())
    );
}

#[test]
fn call_with_no_class_literal_arg_unaffected() {
    // Ordinary argument (not a class literal) — no bogus guess.
    assert_eq!(infer("retrofit.create(someService)"), None);
}

// ─── this_expression ──────────────────────────────────────────────────────────

#[test]
fn this_expr_empty_deps_returns_none() {
    // No contextual type registered → infer_this_expr_type returns None without panicking.
    assert_eq!(infer("this"), None);
}

#[test]
fn this_expr_resolves_to_contextual_receiver_type() {
    // `this` with a registered contextual type → resolves to the receiver class name.
    let deps = TestDeps::new().with_contextual("file:///tmp/test.kt", "this", "MyReceiver");
    assert_eq!(
        infer_with_deps("this", &deps).as_deref(),
        Some("MyReceiver")
    );
}

// ─── identifier / navigation / this kinds (new in Task 1) ─────────────────────

#[test]
fn infer_expr_type_resolves_simple_identifier() {
    // `value` where `value: MyType` → "MyType"
    let deps = TestDeps::new().with_var("file:///tmp/test.kt", "value", "MyType");
    assert_eq!(infer_with_deps("value", &deps).as_deref(), Some("MyType"));
}

// ─── has_type_definition branch (Step 0 of Task 3) ───────────────────────────

#[test]
fn bare_uppercase_ident_with_type_definition_resolves_to_name() {
    // `Foo` where `Foo` is a known type → "Foo" (companion / static access receiver)
    let deps = TestDeps::new().with_type("Foo");
    assert_eq!(infer_with_deps("Foo", &deps).as_deref(), Some("Foo"));
}

#[test]
fn bare_uppercase_ident_without_type_definition_returns_none() {
    // `Foo` where no type definition is registered → None (not a known type name)
    let deps = TestDeps::new();
    assert_eq!(infer_with_deps("Foo", &deps).as_deref(), None);
}

#[test]
fn lowercase_ident_not_affected_by_has_type_definition() {
    // `foo` is lowercase — the `has_type_definition` guard is never reached even if
    // a type named "foo" were registered.
    let deps = TestDeps::new().with_type("foo");
    assert_eq!(infer_with_deps("foo", &deps).as_deref(), None);
}

#[test]
fn var_type_takes_priority_over_type_definition() {
    // When `Foo` is declared as a local variable *and* is a known type, the
    // variable type wins (declared context is more specific).
    let deps = TestDeps::new()
        .with_var("file:///tmp/test.kt", "Foo", "Bar")
        .with_type("Foo");
    assert_eq!(infer_with_deps("Foo", &deps).as_deref(), Some("Bar"));
}

#[test]
fn infer_expr_type_resolves_navigation_chain_receiver() {
    // `data.field` where `data: Holder` and `Holder.field: Foo` → "Foo"
    let deps = TestDeps::new()
        .with_var("file:///tmp/test.kt", "data", "Holder")
        .with_field("Holder", "field", "Foo");
    assert_eq!(infer_with_deps("data.field", &deps).as_deref(), Some("Foo"));
}

#[test]
fn unknown_identifier_returns_none() {
    // An unregistered variable → no type known
    assert_eq!(
        infer_with_deps("unknown", &TestDeps::new()).as_deref(),
        None
    );
}
