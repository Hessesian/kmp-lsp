use super::{derive_dot_receiver, CompletionContext, LambdaScope, ScopeContext};
use crate::indexer::Indexer;
use crate::resolver::complete::DotReceiver;
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
        lambda_this_type: None,
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
        lambda_this_type: None,
        bare_this_type: Some("MyClass".into()),
    };

    assert_eq!(scope.resolve_receiver("this@forEach"), Some("Element"));
    assert_eq!(scope.resolve_receiver("this@MyClass"), Some("MyClass"));
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

    let wants_receiver = before_prefix.trim_end().ends_with('.');
    let ctx = CompletionContext::analyse(position, &index, &uri, false, wants_receiver);

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

    let wants_receiver = before_prefix.trim_end().ends_with('.');
    let ctx = CompletionContext::analyse(position, &index, &uri, false, wants_receiver);

    assert!(
        ctx.call_info.is_none(),
        "call_info should be None outside calls"
    );
}

// ─── derive_dot_receiver (CST speculative parse) ─────────────────────────────

/// Fixture with a `|` caret marking the completion cursor.
fn derive_at_caret(path: &str, src_with_caret: &str) -> Option<DotReceiver> {
    let caret = src_with_caret.find('|').expect("caret");
    let src: String = src_with_caret.replace('|', "");
    let line = src_with_caret[..caret].matches('\n').count();
    let line_start = src_with_caret[..caret].rfind('\n').map_or(0, |p| p + 1);
    let col = src_with_caret[line_start..caret].encode_utf16().count();
    let (uri, index) = indexed_with_live(path, &src);
    derive_dot_receiver(&index, &uri, Position::new(line as u32, col as u32))
}

#[test]
fn derives_a_simple_identifier_receiver_with_no_early_resolution() {
    let recv = derive_at_caret(
        "/SimpleRecv.kt",
        "class User\nfun f() {\n    val user = User()\n    user.|\n}\n",
    )
    .unwrap();
    assert_eq!(
        recv,
        DotReceiver::Expr {
            text: "user".into(),
            is_call: false,
            resolved: None
        }
    );
}

#[test]
fn chain_receiver_resolves_at_analysis_time() {
    let recv = derive_at_caret(
        "/ChainRecv.kt",
        "package com.example\n\
         class Palette { fun swap() {} }\n\
         class Theme { val colors: Palette = Palette() }\n\
         fun f() {\n\
         \x20   val theme = Theme()\n\
         \x20   theme.colors.|\n\
         }\n",
    )
    .unwrap();
    match recv {
        DotReceiver::Expr {
            is_call: false,
            resolved: Some(resolved_type),
            ..
        } => assert_eq!(resolved_type, "Palette"),
        other => panic!("expected resolved chain receiver, got {other:?}"),
    }
}

#[test]
fn derives_a_call_receiver_with_callee_text() {
    let recv = derive_at_caret("/CallRecv.kt", "fun f() {\n    productFlow(x).|\n}\n").unwrap();
    match recv {
        DotReceiver::Expr {
            text,
            is_call: true,
            ..
        } => assert_eq!(text, "productFlow"),
        other => panic!("expected call receiver, got {other:?}"),
    }
}

#[test]
fn classifies_scope_receivers() {
    let it_recv = derive_at_caret("/It.kt", "fun f() { items.map { it.| } }\n").unwrap();
    assert_eq!(it_recv, DotReceiver::Scope("it".into()));

    let labeled = derive_at_caret(
        "/Labeled.kt",
        "fun f() { items.forEach { this@forEach.| } }\n",
    )
    .unwrap();
    assert_eq!(labeled, DotReceiver::Scope("this@forEach".into()));

    let bare_this = derive_at_caret("/This.kt", "class A { fun f() { this.| } }\n").unwrap();
    assert_eq!(bare_this, DotReceiver::Scope("this".into()));
}

#[test]
fn classifies_a_super_receiver() {
    let recv = derive_at_caret("/Super.kt", "class A { fun f() { super.| } }\n").unwrap();
    assert_eq!(recv, DotReceiver::Super);
}

#[test]
fn multiline_fluent_chain_derives_a_call_receiver() {
    let recv = derive_at_caret(
        "/Fluent.kt",
        "fun f() {\n\
         \x20   val m = Modifier\n\
         \x20       .fillMaxSize()\n\
         \x20       .|\n\
         }\n",
    )
    .unwrap();
    match recv {
        DotReceiver::Expr {
            text,
            is_call: true,
            ..
        } => {
            assert!(text.contains("Modifier"), "text: {text}");
            assert!(text.contains("fillMaxSize"), "text: {text}");
        }
        other => panic!("expected multiline chain receiver, got {other:?}"),
    }
}

#[test]
fn no_receiver_for_bare_word_or_string_interior() {
    assert!(derive_at_caret("/Bare.kt", "fun f() { Modif| }\n").is_none());
    assert!(derive_at_caret("/Str.kt", "fun f() { val s = \"foo.|\" }\n").is_none());
}
