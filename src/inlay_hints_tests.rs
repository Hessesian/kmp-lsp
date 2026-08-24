use super::*;
use std::sync::Arc;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn indexed(path: &str, src: &str) -> (Url, Arc<Indexer>) {
    let u = uri(path);
    let idx = Arc::new(Indexer::new());
    idx.index_content(&u, src);
    (u, idx)
}

fn hints_for(src: &str) -> Vec<InlayHint> {
    let (u, idx) = indexed("/t.kt", src);
    let lines = src.lines().count() as u32;
    compute_inlay_hints(
        &idx,
        &u,
        Range {
            start: Position::new(0, 0),
            end: Position::new(lines, 0),
        },
    )
}

/// Like `hints_for` but indexes `sig_src` into the global index and sets up a
/// live tree + live lines from `code_src`.  This mirrors the real editor path
/// where `textDocument/didOpen` has been processed and `live_doc` is available.
fn hints_for_with_live(sig_src: &str, code_src: &str) -> Vec<InlayHint> {
    let u = uri("/t.kt");
    let idx = Arc::new(Indexer::new());
    idx.index_content(&u, sig_src);
    idx.store_live_tree(&u, code_src);
    idx.set_live_lines(&u, code_src);
    let lines = code_src.lines().count() as u32;
    compute_inlay_hints(
        &idx,
        &u,
        Range {
            start: Position::new(0, 0),
            end: Position::new(lines, 0),
        },
    )
}

#[test]
fn it_type_hint() {
    let src = "val items: List<Product> = emptyList()\nitems.forEach { it.name }";
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": Product")),
        "expected ': Product' hint for it, got: {hints:?}",
    );
}

#[test]
fn named_param_type_hint() {
    let src = "val items: List<Order> = emptyList()\nitems.forEach { order ->\n    order.id\n}";
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": Order")),
        "expected ': Order' hint for named param, got: {hints:?}",
    );
}

#[test]
fn no_hint_for_typed_val() {
    let src = "val items: List<Product> = emptyList()";
    let hints = hints_for(src);
    assert!(
        !hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s.contains("items"))),
        "should not hint explicitly typed val",
    );
}

#[test]
fn hints_inject_constructor_lambdas() {
    let src = r#"package test

class ProductsUseCases
class MviViewModel

class DashboardProductsViewModel @javax.inject.Inject constructor(
  private val productsUseCases: ProductsUseCases,
) : MviViewModel() {

  private val items: List<String> = emptyList()

  fun loadData() {
    items.forEach { it.length }
    items.map { item ->
      item.uppercase()
    }
  }
}
"#;
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": String")),
        "expected ': String' hint for it/item in @Inject constructor class, got: {hints:?}",
    );
}

#[test]
fn hints_survive_syntax_error() {
    let src = "val items: List<Product> = emptyList()\nitems.forEach { it.name\n";
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": Product")),
        "hints should still work despite syntax error, got: {hints:?}",
    );
}

#[test]
fn hints_nested_named_arg_lambda() {
    let src = r#"package test

class SheetReloadActions(
    val buildingSavings: (String) -> Unit,
    val loan: (String, Boolean) -> Unit,
)

class Vm {
    private val reducer by lazy {
        SheetReloadActions(
            buildingSavings = { println(it) },
            loan = { loanId, isWustenrot -> println(loanId) },
        )
    }
}
"#;
    let hints = hints_for(src);
    let has_string = hints
        .iter()
        .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": String"));
    assert!(
        has_string,
        "expected ': String' hint for it/loanId in nested named-arg lambda, got: {hints:?}"
    );
}

#[test]
fn hints_nested_named_arg_cross_file() {
    let idx = Arc::new(Indexer::new());
    let u1 = uri("/DashboardProductsReducer.kt");
    idx.index_content(
        &u1,
        r#"package test

class DashboardProductsReducer {
    data class SheetReloadActions(
        val buildingSavings: (String) -> Unit,
        val cards: (CardProduct) -> Unit,
        val loan: (String, Boolean) -> Unit,
    )
}

class CardProduct
"#,
    );
    let u2 = uri("/Vm.kt");
    let vm_src = r#"package test

import test.DashboardProductsReducer

class Vm {
    private val reducer by lazy {
        DashboardProductsReducer.SheetReloadActions(
            buildingSavings = { println(it) },
            cards = { println(it) },
            loan = { loanId, isWustenrot -> println(loanId) },
        )
    }
}
"#;
    idx.index_content(&u2, vm_src);
    let lines = vm_src.lines().count() as u32;
    let hints = compute_inlay_hints(
        &idx,
        &u2,
        Range {
            start: Position::new(0, 0),
            end: Position::new(lines, 0),
        },
    );
    let has_string = hints
        .iter()
        .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": String"));
    let has_card = hints
        .iter()
        .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": CardProduct"));
    assert!(
        has_string,
        "expected ': String' hint for it in cross-file named-arg lambda, got: {hints:?}"
    );
    assert!(
        has_card,
        "expected ': CardProduct' hint for it in cards lambda, got: {hints:?}"
    );
}

#[test]
fn ts_byte_col_utf16_ascii() {
    // For ASCII content the UTF-16 column equals the byte column.
    let bytes = b"fun main() {}\n";
    let starts = line_starts(bytes);
    assert_eq!(ts_byte_col_to_utf16(bytes, &starts, 0, 4), 4); // "fun " = 4 bytes = 4 UTF-16 units
}

#[test]
fn ts_byte_col_utf16_multibyte() {
    // "café" — 'é' is U+00E9 (2 UTF-8 bytes, 1 UTF-16 unit).
    let line = "café foo";
    let bytes = line.as_bytes();
    let starts = line_starts(bytes);
    // byte offset 6 is after "café " (c=1,a=1,f=1,é=2,space=1 → 6 bytes)
    // char cols: c=0,a=1,f=1(wait: c-a-f-é = 4 chars, then space = 5 chars total for "café ")
    // UTF-16: same as char count for BMP chars = 5
    let byte_col = "café ".len(); // 6 bytes
    let utf16 = ts_byte_col_to_utf16(bytes, &starts, 0, byte_col);
    assert_eq!(utf16, 5, "expected 5 UTF-16 units for 'café '");
}

#[test]
fn untyped_val_constructor_call_gets_hint() {
    // `val user = User("alice")` — no explicit type annotation.
    // hint_property should emit `: User` from the CST initializer.
    let src = r#"package test
class User(val name: String)
fun make() {
    val user = User("alice")
}
"#;
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": User")),
        "expected ': User' hint for untyped val with constructor call, got: {hints:?}",
    );
}

/// CST-domain regression for `Resolver::field_type`'s supertype walk
/// (`src/resolver/infer.rs`): `uiState` is declared only on the generic
/// superclass `MviViewModel<S, E>`, not on `ContactAddressViewModel` itself.
/// This never touches `chain.rs`/`expr_type.rs` directly -- it proves the fix
/// landed once in `resolver/` reaches inlay hints automatically through the
/// `InferDeps` seam (`Indexer::find_field_type` -> `Resolver::field_type`),
/// exactly the "fixed once, propagates automatically" point of routing
/// through the trait instead of patching each CST consumer separately.
///
/// The base and derived classes live in separate files deliberately: in one
/// file, `find_field_type_in_class`'s own-body check would accidentally
/// "find" the base's `uiState` by whole-file line-proximity (see
/// `infer_field_type_raw`'s `near_line` doc comment) and return the raw,
/// unsubstituted `S` -- a separate, pre-existing quirk this test avoids
/// conflating with the supertype-walk substitution under test here.
#[test]
fn untyped_val_navigation_through_inherited_generic_field_gets_hint() {
    let base_uri = uri("/MviViewModel.kt");
    let idx = Arc::new(Indexer::new());
    idx.index_content(
        &base_uri,
        "package test\n\
         abstract class MviViewModel<S, E> {\n\
         \x20   val uiState: S = TODO()\n\
         \x20   val effect: E = TODO()\n\
         }\n",
    );

    let derived_uri = uri("/ContactAddressViewModel.kt");
    let derived_src = "package test\n\
         class ContactState\n\
         class ContactEffect\n\
         class ContactAddressViewModel : MviViewModel<ContactState, ContactEffect>()\n\
         fun make(viewModel: ContactAddressViewModel) {\n\
         \x20   val state = viewModel.uiState\n\
         }\n";
    idx.index_content(&derived_uri, derived_src);

    let lines = derived_src.lines().count() as u32;
    let hints = compute_inlay_hints(
        &idx,
        &derived_uri,
        Range {
            start: Position::new(0, 0),
            end: Position::new(lines, 0),
        },
    );
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": ContactState")),
        "expected ': ContactState' hint for a val initialized from a field \
         declared only on the generic superclass, got: {hints:?}",
    );
}

#[test]
fn untyped_val_unindexed_di_factory_call_gets_hint() {
    // `val repo = get<UserRepository>()` where `get` is a Koin-style DI factory
    // that is NOT declared anywhere in this file/index (unpromoted JAR / external
    // dependency in real usage). CST resolution alone (no STRING fallback) must
    // still produce the hint via the `GENERIC_FACTORY_FNS` call-site type-arg
    // recovery in `resolve_call_expr_type`.
    let src = r#"package test
class UserRepository
fun make() {
    val repo = get<UserRepository>()
}
"#;
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": UserRepository")),
        "expected ': UserRepository' hint for unindexed DI factory call, got: {hints:?}",
    );
}

#[test]
fn untyped_val_unindexed_retrofit_style_create_gets_hint() {
    // `val api = retrofit.create(DashboardApi::class.java)` where neither
    // `retrofit`'s type nor `create` is indexed (external Retrofit dependency
    // in real usage). CST resolution alone must still recover `DashboardApi`
    // from the call's own class-literal argument.
    let src = r#"package test
class DashboardApi
fun make() {
    val api = retrofit.create(DashboardApi::class.java)
}
"#;
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": DashboardApi")),
        "expected ': DashboardApi' hint for unindexed Retrofit-style create call, got: {hints:?}",
    );
}

#[test]
fn untyped_val_if_else_arithmetic_to_long_gets_hint() {
    // Verbatim (minus identifier renaming) from a real project file
    // (`FxMoneyVM.kt`), found via observational logging of `hint_property`'s
    // now-removed STRING fallback against production code: CST previously
    // returned `None` for the `if`/`else` because the `else` branch's
    // `(timeoutSeconds * 1000).toLong()` was unresolvable (arithmetic +
    // numeric-conversion-call gap, both now closed).
    let src = r#"package test
class Vm {
    private var mMillisUntilFinished: Long = 0
    private var timeoutSeconds: Int = 30
    val millisInFuture = if (mMillisUntilFinished > 0) mMillisUntilFinished else (timeoutSeconds * 1000).toLong()
}
"#;
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": Long")),
        "expected ': Long' hint for if/else-with-arithmetic property, got: {hints:?}",
    );
}

#[test]
fn untyped_val_bare_arithmetic_division_gets_hint() {
    // Verbatim shape: `private const val TIMER_TICK_MILLIS = 1000 / 2` had NO
    // inlay hint at all, live, in the real editor (neither CST nor the STRING
    // fallback handled bare arithmetic) — now closed on the CST side directly.
    let src = r#"package test
class Vm {
    companion object {
        private const val TIMER_TICK_MILLIS = 1000 / 2
    }
}
"#;
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": Int")),
        "expected ': Int' hint for bare arithmetic division property, got: {hints:?}",
    );
}

#[test]
fn it_inside_nested_lambda_not_suspend() {
    // Regression: `it` inside `setState { it }` where `setState` has a
    // `suspend` function type parameter was incorrectly showing `: suspend`.
    // `find_as_call_arg_type` must bail out when the backward scan crosses
    // an unmatched `{`, meaning `it` is inside a nested lambda body.
    let src = r#"package test

class State
class Effect

class Vm {
    private val items: List<State> = emptyList()

    fun load() {
        items.forEach { item ->
            setState { item }
        }
    }

    fun setState(reducer: suspend State.() -> State) {}
}
"#;
    let hints = hints_for(src);
    let bad = hints
        .iter()
        .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": suspend"));
    assert!(
        !bad,
        "must not emit ': suspend' hint for it inside nested lambda, got: {hints:?}"
    );
}

#[test]
fn fun_expr_body_comparison_hint() {
    let src = "fun check(n: Int) = n > 0";
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": Boolean")),
        "expected ': Boolean' hint for comparison expression body, got: {hints:?}",
    );
}

#[test]
fn fun_expr_body_prefix_not_hint() {
    let src = "fun neg(b: Boolean) = !b";
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": Boolean")),
        "expected ': Boolean' hint for !b expression body, got: {hints:?}",
    );
}

#[test]
fn fun_expr_body_range_hint() {
    let src = "fun r() = 1..10";
    let hints = hints_for(src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": IntRange")),
        "expected ': IntRange' hint for range expression body, got: {hints:?}",
    );
}

#[test]
fn it_hint_fastforeach_fun_param_chain_live_doc() {
    // Reproduces user-reported divergence: hover shows concrete type but inlay hint shows T.
    // Uses live tree (editor path) — the bug only manifests when live_doc is present.
    let sig_src = [
        "data class TableRowModel(val title: String)",
        "data class PortfolioProcessedItem(val tableRows: ImmutableList<TableRowModel>)",
        "fun <T> List<T>.fastForEach(action: (T) -> Unit) {}",
    ]
    .join("\n");
    let code_src = [
        "fun content(item: PortfolioProcessedItem) {",
        "  item.tableRows.fastForEach { it }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    assert!(
        hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": TableRowModel")),
        "expected ': TableRowModel' inlay hint for it in live-doc fastForEach chain, got: {hints:?}"
    );
    assert!(
        !hints
            .iter()
            .any(|h| matches!(&h.label, InlayHintLabel::String(s) if s == ": T")),
        "inlay hint must not show raw generic T, got: {hints:?}"
    );
}

#[test]
fn named_lambda_param_safe_call_let() {
    // Reproduces: `it.title?.let { sectionTitle -> }` where sectionTitle gets type "it"
    // instead of the actual type of `it.title` (String).
    let sig_src = [
        "data class Section(val title: String?, val subtitle: String?)",
        "fun <T> T.let(block: (T) -> Unit): Unit {}",
    ]
    .join("\n");
    let code_src = [
        "fun render(sections: List<Section>) {",
        "  sections.forEach {",
        "    it.title?.let { sectionTitle ->",
        "      println(sectionTitle)",
        "    }",
        "  }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    let labels: Vec<&str> = hints
        .iter()
        .filter_map(|h| match &h.label {
            InlayHintLabel::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("hints: {labels:?}");

    // sectionTitle should be `: String` (type of Section.title after safe-unwrap)
    // It must NOT be `: it` which is the bug
    assert!(
        !labels.iter().any(|l| l.contains("it")),
        "sectionTitle must not get type 'it', got: {labels:?}"
    );
}

#[test]
fn named_lambda_params_foreach_indexed() {
    // Reproduces: `items.forEachIndexed { index, item -> }` where BOTH params
    // get type "Item" instead of index=Int, item=Item.
    let sig_src = [
        "data class Item(val name: String)",
        "fun <T> List<T>.forEachIndexed(action: (Int, T) -> Unit) {}",
    ]
    .join("\n");
    let code_src = [
        "fun render(items: List<Item>) {",
        "  items.forEachIndexed { index, item ->",
        "    println(index)",
        "  }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    let labels: Vec<&str> = hints
        .iter()
        .filter_map(|h| match &h.label {
            InlayHintLabel::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("forEachIndexed hints: {labels:?}");

    // index should be Int, item should be Item
    assert!(
        labels.contains(&": Int"),
        "expected ': Int' for index param, got: {labels:?}"
    );
    assert!(
        labels.contains(&": Item"),
        "expected ': Item' for item param, got: {labels:?}"
    );
}

#[test]
fn named_lambda_params_foreach_indexed_immutable_list() {
    // Reproduces real bug: expanded.items.forEachIndexed { index, item -> }
    // where items: ImmutableList<Item> and forEachIndexed is on Iterable<T>
    let sig_src = [
        "data class Item(val name: String)",
        "data class Expanded(val items: ImmutableList<Item>)",
        "interface ImmutableList<out E> : List<E>",
        "fun <T> Iterable<T>.forEachIndexed(action: (index: Int, value: T) -> Unit) {}",
    ]
    .join("\n");
    let code_src = [
        "fun render(expanded: Expanded) {",
        "  expanded.items.forEachIndexed { index, item ->",
        "    println(index)",
        "  }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    let labels: Vec<&str> = hints
        .iter()
        .filter_map(|h| match &h.label {
            InlayHintLabel::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("forEachIndexed ImmutableList hints: {labels:?}");

    assert!(
        labels.contains(&": Int"),
        "expected ': Int' for index param, got: {labels:?}"
    );
    assert!(
        labels.contains(&": Item"),
        "expected ': Item' for item param, got: {labels:?}"
    );
}

#[test]
fn named_lambda_params_foreach_indexed_chain() {
    // Reproduces: `expanded.items.forEachIndexed { index, item -> }` with a chain receiver
    let sig_src = [
        "data class Item(val name: String)",
        "data class ExpandedState(val items: List<Item>)",
        "fun <T> List<T>.forEachIndexed(action: (Int, T) -> Unit) {}",
    ]
    .join("\n");
    let code_src = [
        "fun render(expanded: ExpandedState) {",
        "  expanded.items.forEachIndexed { index, item ->",
        "    println(index)",
        "  }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    let labels: Vec<&str> = hints
        .iter()
        .filter_map(|h| match &h.label {
            InlayHintLabel::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("forEachIndexed chain hints: {labels:?}");

    assert!(
        labels.contains(&": Int"),
        "expected ': Int' for index param, got: {labels:?}"
    );
    assert!(
        labels.contains(&": Item"),
        "expected ': Item' for item param, got: {labels:?}"
    );
}

#[test]
fn named_lambda_params_foreach_indexed_fun_param_receiver() {
    // Simulates: function parameter `expanded: Expanded` used as receiver
    // This tests the path where expanded is a function param (not a local val)
    let sig_src = [
        "data class Item(val preDivider: String?)",
        "data class Expanded(val items: ImmutableList<Item>)",
        "interface ImmutableList<out E> : List<E>",
        "fun <T> Iterable<T>.forEachIndexed(action: (index: Int, value: T) -> Unit) {}",
    ]
    .join("\n");
    let code_src = [
        "fun expanded(productIndex: Int, expanded: Expanded, keyPostfix: String) {",
        "  expanded.items.forEachIndexed { index, item ->",
        "    println(index)",
        "    println(item)",
        "  }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    let labels: Vec<&str> = hints
        .iter()
        .filter_map(|h| match &h.label {
            InlayHintLabel::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("forEachIndexed fun-param hints: {labels:?}");

    assert!(
        labels.contains(&": Int"),
        "expected ': Int' for index param, got: {labels:?}"
    );
    assert!(
        labels.contains(&": Item"),
        "expected ': Item' for item param, got: {labels:?}"
    );
}

#[test]
fn named_lambda_params_foreach_indexed_no_source_sig() {
    // Simulates real case: forEachIndexed is NOT in source index (only in JAR/stdlib)
    // Only the data classes are indexed.
    let sig_src = [
        "data class Item(val preDivider: String?)",
        "data class Expanded(val items: ImmutableList<Item>)",
        "interface ImmutableList<out E> : List<E>",
        // NOTE: forEachIndexed is NOT included — simulating JAR-only function
    ]
    .join("\n");
    let code_src = [
        "fun expanded(productIndex: Int, expanded: Expanded, keyPostfix: String) {",
        "  expanded.items.forEachIndexed { index, item ->",
        "    println(index)",
        "    println(item)",
        "  }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    let labels: Vec<&str> = hints
        .iter()
        .filter_map(|h| match &h.label {
            InlayHintLabel::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("forEachIndexed no-source hints: {labels:?}");

    // When forEachIndexed isn't indexed, we can't resolve the params.
    // But we must NOT show wrong types — either show nothing or correct types.
    // Definitely should NOT show both as "Item".
    let item_count = labels.iter().filter(|l| l.contains("Item")).count();
    assert!(
        item_count <= 1,
        "at most one param should be Item (not both), got: {labels:?}"
    );
}

#[test]
fn named_lambda_param_dotted_type_arg_preserved() {
    // Regression: `DashboardInvestedContract.Effect` was truncated to
    // `DashboardInvestedContract` because first_concrete_type_arg_str used
    // ident_prefix() (stops at '.') instead of dotted_ident_prefix().
    let sig_src = [
        "sealed class DashboardInvestedContract {",
        "  sealed class Effect",
        "}",
        "interface Flow<out T>",
        // collectAsEffect NOT in source — simulating extension fn from lib
    ]
    .join("\n");
    let code_src = [
        "fun collectEffects(effects: Flow<DashboardInvestedContract.Effect>) {",
        "  effects.collectAsEffect { effect ->",
        "    println(effect)",
        "  }",
        "}",
    ]
    .join("\n");
    let hints = hints_for_with_live(&sig_src, &code_src);
    let labels: Vec<&str> = hints
        .iter()
        .filter_map(|h| match &h.label {
            InlayHintLabel::String(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    eprintln!("collectAsEffect dotted hints: {labels:?}");

    // Must NOT truncate to bare "DashboardInvestedContract"
    assert!(
        !labels.contains(&": DashboardInvestedContract"),
        "hint must not drop .Effect suffix, got: {labels:?}"
    );
    // If a hint is shown it must be the full qualified name
    if let Some(hint) = labels.iter().find(|l| l.contains("Dashboard")) {
        assert!(
            hint.contains("DashboardInvestedContract.Effect"),
            "expected full qualified name, got: {hint}"
        );
    }
}
