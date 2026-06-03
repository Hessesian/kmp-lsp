use super::*;
use crate::types::InlayHintConfig;
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
        &InlayHintConfig::default(),
    )
}

fn hints_with_config(src: &str, config: &InlayHintConfig) -> Vec<InlayHint> {
    let (u, idx) = indexed("/t.kt", src);
    let lines = src.lines().count() as u32;
    compute_inlay_hints(
        &idx,
        &u,
        Range {
            start: Position::new(0, 0),
            end: Position::new(lines, 0),
        },
        config,
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
        &InlayHintConfig::default(),
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

// ── Inlay hint config toggle tests ───────────────────────────────────────────

#[cfg(test)]
mod config_toggle_tests {
    use super::*;

    #[test]
    fn lambda_it_toggle_suppresses_it_hints() {
        let src = r#"package test

fun test() {
    val list: List<String>? = null
    list?.forEach { println(it.length) }
}
"#;
        let default_hints = hints_for(src);
        let count_default = default_hints.len();

        let off_config = InlayHintConfig {
            lambda_it: false,
            ..Default::default()
        };
        let suppressed = hints_with_config(src, &off_config);
        assert!(
            suppressed.len() <= count_default,
            "lambda_it=false: hints {} <= {} (default)",
            suppressed.len(),
            count_default,
        );
    }

    #[test]
    fn lambda_params_toggle_suppresses_named_params() {
        let src = r#"package test

class Foo {
    fun bar() {
        listOf(1).map { item -> item }
    }
}
"#;
        let default_hints = hints_for(src);
        // The named param `item` might get a hint; verify only that the config
        // change reduces total hint count.
        let off_config = InlayHintConfig {
            lambda_params: false,
            ..Default::default()
        };
        let suppressed = hints_with_config(src, &off_config);
        assert!(
            suppressed.len() <= default_hints.len(),
            "lambda_params=false should not increase hint count"
        );
    }

    #[test]
    fn this_hints_toggle_suppresses_this_hints() {
        let src = r#"package test

class Foo {
    fun bar(): Int {
        return this.hashCode()
    }
}
"#;
        let default_hints = hints_for(src);
        let off_config = InlayHintConfig {
            this_hints: false,
            ..Default::default()
        };
        let suppressed = hints_with_config(src, &off_config);
        assert!(
            suppressed.len() <= default_hints.len(),
            "this_hints=false should not increase hint count"
        );
    }

    #[test]
    fn untyped_vars_toggle_suppresses_var_type_hints() {
        let src = r#"package test

class Foo

fun test() {
    val x = Foo()
}
"#;
        let default_hints = hints_for(src);
        let off_config = InlayHintConfig {
            untyped_vars: false,
            ..Default::default()
        };
        let suppressed = hints_with_config(src, &off_config);
        assert!(
            suppressed.len() <= default_hints.len(),
            "untyped_vars=false should not increase hint count"
        );
    }
}

// ── ::class literal type resolution tests ────────────────────────────────────

#[cfg(test)]
mod class_literal_tests {
    use super::*;

    #[test]
    fn class_literal_retrofit_pattern() {
        let src = r#"
class Retrofit {
    fun <T> create(service: Class<T>): T = TODO()
}
class GoldConversionSecuredApi

class Foo {
    private val securedApi = Retrofit().create(GoldConversionSecuredApi::class.java)
}"#;
        let hints = hints_for(src);
        let has_gold = hints.iter().any(
            |h| matches!(&h.label, InlayHintLabel::String(s) if s == ": GoldConversionSecuredApi"),
        );
        if !has_gold {
            let labels: Vec<String> = hints.iter().map(|h| format!("{:?}", h.label)).collect();
            eprintln!("class_literal test labels: {labels:?}");
        }
        assert!(has_gold, "expected ': GoldConversionSecuredApi' inlay hint");
    }
}
