use tower_lsp::lsp_types::Url;

use crate::indexer::Indexer;
use crate::resolver::infer::find_fun_return_type_by_name;

use super::resolve_chain_receiver;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

/// `foo.bar.` where `foo: Foo` and `Foo.bar: Flow<Cause>` (type-annotated member).
#[test]
fn chain_one_hop_annotated_member() {
    let host_uri = uri("/Host.kt");
    let foo_uri = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(
        &foo_uri,
        "package com.pkg\nclass Foo {\n    val bar: Flow<Cause> = TODO()\n}\n",
    );
    idx.index_content(
        &host_uri,
        "package com.pkg\nfun go(foo: Foo) { foo.bar. }\n",
    );

    let rt = resolve_chain_receiver(&idx, "foo.bar", &host_uri);
    assert!(rt.is_some(), "chain foo.bar should resolve; got None");
    let rt = rt.unwrap();
    assert_eq!(
        rt.outer, "Flow",
        "outer type should be 'Flow'; got '{}'",
        rt.outer
    );
}

/// `foo.bar.` where `bar` has no type annotation but is inferred via RHS.
///
/// `val bar = other.triggersFlow` — unannotated property; `infer_variable_type_raw`
/// must be used (not `infer_field_type_raw`) to follow the RHS chain.
#[test]
fn chain_one_hop_unannotated_member_via_rhs() {
    let host_uri = uri("/Host.kt");
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
    idx.index_content(
        &host_uri,
        "package com.pkg\nfun go(foo: Foo) { foo.bar. }\n",
    );

    let rt = resolve_chain_receiver(&idx, "foo.bar", &host_uri);
    assert!(
        rt.is_some(),
        "chain foo.bar should resolve via RHS; got None"
    );
    let rt = rt.unwrap();
    assert_eq!(
        rt.outer, "Flow",
        "outer type should be 'Flow'; got '{}'",
        rt.outer
    );
}

/// Contextual keywords (`this`, `super`, `it`) must not be chain-resolved —
/// they would attempt variable lookup under that literal name and silently fail
/// or give wrong results.
#[test]
fn chain_this_prefix_returns_none() {
    let host_uri = uri("/Host.kt");
    let idx = Indexer::new();
    idx.index_content(
        &host_uri,
        "package com.pkg\nclass Foo {\n    val bar: Int = 0\n}\n",
    );
    // "this.bar" should not go through chain resolver
    let rt = resolve_chain_receiver(&idx, "this.bar", &host_uri);
    assert!(
        rt.is_none(),
        "this.bar should NOT be resolved by chain resolver"
    );
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
        &super::ReceiverExpr {
            chain: "productFlow".to_string(),
            is_call: true,
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
