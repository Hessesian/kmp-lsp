use tower_lsp::lsp_types::{Position, Url};

use crate::features::completion_context::derive_dot_receiver;
use crate::indexer::Indexer;
use crate::resolver::infer::find_fun_return_type_by_name;

use super::{DotReceiver, ReceiverType};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

/// Derive the dot receiver at the `|` caret in `src` (live-tree backed) and
/// return the CST-resolved receiver type, if any.
fn derived_receiver_type(idx: &Indexer, path: &str, src_with_caret: &str) -> Option<ReceiverType> {
    let caret = src_with_caret.find('|').expect("caret");
    let src: String = src_with_caret.replace('|', "");
    let line = src_with_caret[..caret].matches('\n').count();
    let line_start = src_with_caret[..caret].rfind('\n').map_or(0, |p| p + 1);
    let col = src_with_caret[line_start..caret].encode_utf16().count();
    let host = uri(path);
    idx.index_content(&host, &src);
    idx.store_live_tree(&host, &src);
    match derive_dot_receiver(idx, &host, Position::new(line as u32, col as u32))? {
        DotReceiver::Expr {
            resolved: Some(raw),
            ..
        } => Some(ReceiverType::from_raw(raw)),
        _ => None,
    }
}

/// `foo.bar.` where `foo: Foo` and `Foo.bar: Flow<Cause>` (type-annotated member).
#[test]
fn chain_one_hop_annotated_member() {
    let foo_uri = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(
        &foo_uri,
        "package com.pkg\nclass Foo {\n    val bar: Flow<Cause> = TODO()\n}\n",
    );

    let rt = derived_receiver_type(
        &idx,
        "/Host.kt",
        "package com.pkg\nfun go(foo: Foo) { foo.bar.| }\n",
    );
    let rt = rt.expect("chain foo.bar should resolve; got None");
    assert_eq!(
        rt.outer, "Flow",
        "outer type should be 'Flow'; got '{}'",
        rt.outer
    );
}

/// `foo.bar.` where `bar` has no type annotation but is inferred via RHS.
///
/// `val bar = other.triggersFlow` — unannotated property; the CST chain walk
/// must follow the member's RHS to type it.
#[test]
fn chain_one_hop_unannotated_member_via_rhs() {
    let foo_uri = uri("/Foo.kt");
    let helper_uri = uri("/Helper.kt");
    let idx = Indexer::new();
    idx.index_content(
        &helper_uri,
        "package com.pkg\nclass Helper {\n    val triggersFlow: Flow<Event> = TODO()\n}\n",
    );
    idx.index_content(
        &foo_uri,
        "package com.pkg\nclass Foo(val helper: Helper) {\n    val bar = helper.triggersFlow\n}\n",
    );

    let rt = derived_receiver_type(
        &idx,
        "/Host.kt",
        "package com.pkg\nfun go(foo: Foo) { foo.bar.| }\n",
    );
    let rt = rt.expect("chain foo.bar should resolve via RHS; got None");
    assert_eq!(
        rt.outer, "Flow",
        "outer type should be 'Flow'; got '{}'",
        rt.outer
    );
}

/// A `resolved` type carried by the receiver is authoritative — the text
/// ladder must not run (pins the analysis-time CST fast path).
#[test]
fn cst_resolved_receiver_type_wins_over_the_text_ladder() {
    let idx = Indexer::new();
    let rt = super::resolve_dot_receiver_type(
        &idx,
        &DotReceiver::Expr {
            text: "theme.colors".to_string(),
            is_call: false,
            resolved: Some("Palette".to_string()),
        },
        &uri("/Empty.kt"),
        None,
    );
    assert_eq!(rt.map(|r| r.outer), Some("Palette".to_string()));
}

/// `productFlow(): Flow<Event>` — `find_fun_return_type_by_name` must find
/// the return type so that `productFlow().col` resolves correctly.
#[test]
fn fun_return_type_lookup_for_call_receiver() {
    let host_uri = uri("/Host.kt");
    let idx = Indexer::new();
    idx.index_content(
        &host_uri,
        "package com.pkg\nfun productFlow(): Flow<Event> { TODO() }\n",
    );

    let rt = find_fun_return_type_by_name(&idx, "productFlow");
    assert_eq!(
        rt.as_deref(),
        Some("Flow<Event>"),
        "return type must be 'Flow<Event>'"
    );
}

/// `productFlow: (isRefresh: Boolean) -> Flow<ResultState<T>>` passed as a lambda parameter.
/// Calling `productFlow(trigger.isRefresh()).` must resolve to `Flow<ResultState<T>>`.
#[test]
fn call_receiver_callable_parameter() {
    let host_uri = uri("/Host.kt");
    let idx = Indexer::new();
    idx.index_content(
        &host_uri,
        concat!(
            "package com.pkg\n",
            "fun <T : Any> reloadable(\n",
            "    key: String,\n",
            "    productFlow: (isRefresh: Boolean) -> Flow<ResultState<T>>,\n",
            ") {\n",
            "    productFlow(true)\n",
            "}\n"
        ),
    );

    // find_fun_return_type_by_name must NOT find "productFlow" (it's a param, not a def).
    use crate::resolver::infer::find_fun_return_type_by_name;
    assert!(
        find_fun_return_type_by_name(&idx, "productFlow").is_none(),
        "productFlow is a parameter, not a function definition"
    );

    // The line scanner must resolve the callable param return type directly.
    use crate::resolver::infer_lines::infer_callable_param_return_type;
    let file = idx
        .files
        .get(host_uri.as_str())
        .expect("file must be indexed");
    let ret = infer_callable_param_return_type(&file.lines, "productFlow");
    assert_eq!(
        ret.as_deref(),
        Some("Flow<ResultState<T>>"),
        "infer_callable_param_return_type must return the full return type for the lambda parameter"
    );

    // End-to-end: `resolve_dot_receiver_type` with the stripped name `"productFlow"`
    // (dot_receiver strips call args before passing to the resolver) must resolve to
    // the lambda's return type via the callable-param line-scan fallback.
    let rt = super::resolve_dot_receiver_type(
        &idx,
        &super::DotReceiver::Expr {
            text: "productFlow".to_string(),
            is_call: true,
            resolved: None,
        },
        &host_uri,
        None,
    );
    assert_eq!(
        rt.as_ref().map(|r| r.outer.as_str()),
        Some("Flow"),
        "resolve_dot_receiver_type('productFlow()') must resolve to 'Flow'"
    );
}
