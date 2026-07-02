use super::{CompletionContext, LambdaScope, ScopeContext};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{Position, Url};

#[test]
fn scope_resolve_it_returns_innermost_it_type() {
    let scope = ScopeContext {
        enclosing_class: None,
        lambda_scopes: vec![
            LambdaScope {
                it_type: Some("OuterType".into()),
                named_params: vec![],
                label: Some("map".into()),
            },
            LambdaScope {
                it_type: Some("InnerType".into()),
                named_params: vec![],
                label: Some("forEach".into()),
            },
        ],
        bare_this_type: None,
    };

    assert_eq!(scope.resolve_receiver("it"), Some("InnerType"));
}

#[test]
fn scope_resolve_this_at_label() {
    let scope = ScopeContext {
        enclosing_class: Some("MyClass".into()),
        lambda_scopes: vec![LambdaScope {
            it_type: Some("Element".into()),
            named_params: vec![],
            label: Some("forEach".into()),
        }],
        bare_this_type: Some("MyClass".into()),
    };

    assert_eq!(scope.resolve_receiver("this@forEach"), Some("Element"));
    assert_eq!(scope.resolve_receiver("this@MyClass"), Some("MyClass"));
}

#[test]
fn scope_is_scope_receiver() {
    let scope = ScopeContext {
        enclosing_class: None,
        lambda_scopes: vec![],
        bare_this_type: None,
    };

    assert!(scope.is_scope_receiver("it"));
    assert!(scope.is_scope_receiver("this"));
    assert!(scope.is_scope_receiver("this@Foo"));
    assert!(!scope.is_scope_receiver("someVar"));
    assert!(!scope.is_scope_receiver("Companion"));
}

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn indexed_with_live(path: &str, src: &str) -> (Url, Indexer) {
    let uri = uri(path);
    let index = Indexer::new();
    index.index_content(&uri, src);
    index.store_live_tree(&uri, src);
    (uri, index)
}

fn call_paren_col(src: &str, line_no: usize, fn_name: &str) -> u32 {
    let line = src.lines().nth(line_no).expect("line out of range");
    let needle = format!("{fn_name}(");
    let pos = line
        .find(&needle)
        .unwrap_or_else(|| panic!("no `{needle}` on line"));
    (pos + needle.len()) as u32
}

#[test]
fn lambda_scope_found_beyond_backward_scan_window() {
    // The enclosing lambda opens more than 50 lines above the cursor: a
    // bounded backward text-scan never sees it, while the CST ancestor-walk
    // finds every enclosing lambda regardless of distance.
    let mut src = String::from(
        "package com.example\n\
         class Item { val price: Int = 0 }\n\
         fun main() {\n\
         \x20   val items: List<Item> = listOf()\n\
         \x20   items.forEach {\n",
    );
    for filler in 0..60 {
        src.push_str(&format!("        val filler{filler} = {filler}\n"));
    }
    src.push_str("        \n    }\n}\n");
    let (uri, index) = indexed_with_live("/FarLambda.kt", &src);
    let cursor_line = 65u32; // the blank body line, 61 lines below the `{`

    let scope = ScopeContext::build(Position::new(cursor_line, 8), &index, &uri);

    assert_eq!(scope.resolve_receiver("it"), Some("Item"));
    assert_eq!(scope.resolve_receiver("this@forEach"), Some("Item"));
}

#[test]
fn call_info_expected_name_at_first_arg() {
    let src =
        "package com.example\nfun greet(name: String, age: Int) {}\nfun main() {\n    greet()\n}\n";
    let (uri, index) = indexed_with_live("/CallInfo.kt", src);
    let position = Position::new(3, call_paren_col(src, 3, "greet"));
    let before_prefix = src.lines().nth(3).unwrap()[..position.character as usize].to_owned();

    let ctx = CompletionContext::analyse(&before_prefix, position, &index, &uri, false);

    let call_info = ctx.call_info.expect("call_info should be populated");
    assert_eq!(call_info.callee, "greet");
    assert_eq!(call_info.arg_index, 0);
    assert_eq!(call_info.expected_name.as_deref(), Some("name"));
    assert_eq!(call_info.expected_type.as_deref(), Some("String"));
}

#[test]
fn call_info_expected_name_none_when_not_in_call() {
    let src = "package com.example\nfun main() {\n    val value = 1\n    value\n}\n";
    let (uri, index) = indexed_with_live("/NoCallInfo.kt", src);
    let position = Position::new(3, 9);
    let before_prefix = src.lines().nth(3).unwrap()[..position.character as usize].to_owned();

    let ctx = CompletionContext::analyse(&before_prefix, position, &index, &uri, false);

    assert!(
        ctx.call_info.is_none(),
        "call_info should be None outside calls"
    );
}
