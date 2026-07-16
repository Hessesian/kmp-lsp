use super::super::infer_lines::{
    extract_property_type_from_detail, extract_return_type_from_detail, has_dot_after_first_call,
};

#[test]
fn return_type_simple() {
    assert_eq!(
        extract_return_type_from_detail("fun getDetail(req: Req): AccountDetail"),
        Some("AccountDetail".into()),
    );
}

#[test]
fn return_type_generic() {
    assert_eq!(
        extract_return_type_from_detail(
            "fun getAccountDetail(body: Body): Response<AccountDetail>"
        ),
        Some("Response<AccountDetail>".into()),
    );
}

#[test]
fn return_type_unit_returns_none() {
    assert_eq!(
        extract_return_type_from_detail("fun doSomething(x: Int)"),
        None
    );
}

#[test]
fn return_type_primitive_returns_none() {
    assert_eq!(extract_return_type_from_detail("fun count(): int"), None);
}

#[test]
fn return_type_nullable_stripped() {
    assert_eq!(
        extract_return_type_from_detail("fun find(): User?"),
        Some("User".into()),
    );
}

#[test]
fn has_dot_after_first_call_chained() {
    // paren_pos=7: "getList" is 7 chars, then "("
    assert!(has_dot_after_first_call("getList(isRefresh).joinAll()", 7));
}

#[test]
fn has_dot_after_first_call_standalone() {
    assert!(!has_dot_after_first_call(
        "getConnectedAccounts(isRefresh)",
        20
    ));
}

#[test]
fn has_dot_after_first_call_nested_parens() {
    // Nested parens inside arg list must not fool the scanner.
    assert!(has_dot_after_first_call("getList(foo(x)).map()", 7));
}

// ─── type_annotations (CST annotated property path) ──────────────────────────

#[test]
fn infer_annotated_property_from_cst() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::{infer_variable_type, infer_variable_type_raw};
    use tower_lsp::lsp_types::Url;

    fn uri(p: &str) -> Url {
        Url::parse(&format!("file://{p}")).unwrap()
    }

    let file_uri = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(
        &file_uri,
        "package com.example\nclass Foo {\n    val repo: UserRepository = inject()\n    val items: List<Product> = emptyList()\n    val state: StateFlow<UiState>? = null\n}",
    );

    // Non-raw: strips generics and nullability.
    assert_eq!(
        infer_variable_type(&idx, "repo", &file_uri),
        Some("UserRepository".into()),
        "simple annotated property"
    );
    assert_eq!(
        infer_variable_type(&idx, "items", &file_uri),
        Some("List".into()),
        "generic annotated property: non-raw strips generics"
    );
    assert_eq!(
        infer_variable_type(&idx, "state", &file_uri),
        Some("StateFlow".into()),
        "nullable annotated property: non-raw strips nullability"
    );

    // Raw: preserves generics and outer `?` (nullable flows through to ReceiverType).
    assert_eq!(
        infer_variable_type_raw(&idx, "items", &file_uri),
        Some("List<Product>".into()),
        "generic annotated property: raw preserves generics"
    );
    assert_eq!(
        infer_variable_type_raw(&idx, "state", &file_uri),
        Some("StateFlow<UiState>?".into()),
        "nullable annotated property: raw preserves ? (stripped in ReceiverType::from_raw)"
    );

    // Non-generic nullable: raw preserves ? too.
    let idx2 = Indexer::new();
    idx2.index_content(
        &file_uri,
        "package com.example\nclass Bar {\n    val user: User? = null\n}",
    );
    assert_eq!(
        infer_variable_type_raw(&idx2, "user", &file_uri),
        Some("User?".into()),
        "non-generic nullable: raw preserves ?"
    );
}

// ─── field_access_rhs: val x = recv.field preserves generics ─────────────────

#[test]
fn field_access_rhs_preserves_generics() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::{infer_variable_type, infer_variable_type_raw};
    use tower_lsp::lsp_types::Url;

    fn uri(p: &str) -> Url {
        Url::parse(&format!("file://{p}")).unwrap()
    }

    let helper_uri = uri("/DashboardTriggersHelper.kt");
    let interactor_uri = uri("/RefreshDashboardInteractor.kt");

    let idx = Indexer::new();

    // Index the helper class with a Flow<DashboardTrigger> field.
    idx.index_content(
        &helper_uri,
        "package com.example\nclass DashboardTriggersHelper {\n    val triggersFlow: Flow<DashboardTrigger> = MutableStateFlow(emptyList())\n}",
    );

    // Index the interactor: constructor param (no val) + unannotated val with field access RHS.
    idx.index_content(
        &interactor_uri,
        "package com.example\nclass RefreshDashboardInteractor(\n    dashboardTriggersHelper: DashboardTriggersHelper\n) {\n    val triggers = dashboardTriggersHelper.triggersFlow\n}",
    );

    // Raw path should preserve generics: Flow<DashboardTrigger>
    assert_eq!(
        infer_variable_type_raw(&idx, "triggers", &interactor_uri),
        Some("Flow<DashboardTrigger>".into()),
        "field_access_rhs raw should preserve generics"
    );

    // Non-raw path should also preserve generics for display (matching method_call_rhs behavior)
    assert_eq!(
        infer_variable_type(&idx, "triggers", &interactor_uri),
        Some("Flow<DashboardTrigger>".into()),
        "field_access_rhs non-raw should preserve generics (like method_call_rhs does)"
    );
}

/// Cross-file resolution: `find_field_type_in_class` should resolve unannotated
/// `val x = recv.field` properties by falling back to `infer_variable_type_raw`.
#[test]
fn find_field_type_in_class_resolves_unannotated_field_access() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::find_field_type_in_class;
    use tower_lsp::lsp_types::Url;

    fn uri(p: &str) -> Url {
        Url::parse(&format!("file://{p}")).unwrap()
    }

    let helper_uri = uri("/DashboardTriggersHelper.kt");
    let interactor_uri = uri("/RefreshDashboardInteractor.kt");

    let idx = Indexer::new();

    idx.index_content(
        &helper_uri,
        "package com.example\nclass DashboardTriggersHelper {\n    val triggersFlow: Flow<DashboardTrigger> = MutableStateFlow(emptyList())\n}",
    );
    idx.index_content(
        &interactor_uri,
        "package com.example\nclass RefreshDashboardInteractor(\n    dashboardTriggersHelper: DashboardTriggersHelper\n) {\n    val triggers = dashboardTriggersHelper.triggersFlow\n}",
    );

    // find_field_type_in_class should resolve through field_access_rhs fallback.
    assert_eq!(
        find_field_type_in_class(&idx, "RefreshDashboardInteractor", "triggers"),
        Some("Flow<DashboardTrigger>".into()),
        "find_field_type_in_class should resolve unannotated val with field_access_rhs"
    );
}

#[test]
fn supertype_subst_replaces_generic_params() {
    let raw = "Flow<ReducedResult<EffectType, StateType>>";
    let params = vec!["EventType".into(), "EffectType".into(), "StateType".into()];
    let args = vec![
        "BuildingSavingsInputEvent".into(),
        "BuildingSavingsEffect".into(),
        "Sheet".into(),
    ];
    assert_eq!(
        super::apply_supertype_subst(raw, &params, &args),
        "Flow<ReducedResult<BuildingSavingsEffect, Sheet>>"
    );
}

#[test]
fn supertype_subst_whole_word_only() {
    let raw = "EventTypeHandler<EventType>";
    let params = vec!["EventType".into()];
    let args = vec!["Click".into()];
    // "EventType" inside "EventTypeHandler" should NOT be replaced
    assert_eq!(
        super::apply_supertype_subst(raw, &params, &args),
        "EventTypeHandler<Click>"
    );
}

#[test]
fn property_type_extension_with_receiver() {
    assert_eq!(
        extract_property_type_from_detail(
            "val ViewModel.viewModelScope: CoroutineScope get() = TODO()"
        ),
        Some("CoroutineScope".into()),
    );
}

// Regression: library source detail strings include a visibility keyword before `val`/`var`.
// `extract_property_type_from_detail` must strip it, otherwise dot-completion on
// extension properties like `viewModelScope` returns nothing.
#[test]
fn property_type_with_public_visibility_prefix() {
    assert_eq!(
        extract_property_type_from_detail("public val ViewModel.viewModelScope: CoroutineScope"),
        Some("CoroutineScope".into()),
    );
}

#[test]
fn property_type_with_internal_visibility_prefix() {
    assert_eq!(
        extract_property_type_from_detail("internal var MyClass.count: Int"),
        Some("Int".into()),
    );
}

#[test]
fn property_type_with_protected_visibility_prefix() {
    assert_eq!(
        extract_property_type_from_detail("protected val Base.tag: String"),
        Some("String".into()),
    );
}

#[test]
fn property_type_simple() {
    assert_eq!(
        extract_property_type_from_detail("val items: List<Product>"),
        Some("List<Product>".into()),
    );
}

#[test]
fn property_type_no_keyword_returns_none() {
    assert_eq!(
        extract_property_type_from_detail("fun doSomething(): Int"),
        None,
    );
}

/// Completion/hover path: a `val` initialized by `remember { Constructor() }`
/// must resolve to the constructed type via the CST fallback (the line-based
/// heuristics can't see through the lambda), so `navigator.` offers the right
/// members.
#[test]
fn remember_initializer_resolves_via_cst_fallback() {
    use super::super::{infer_receiver_type, ReceiverKind};
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let uri = Url::parse("file:///app/Nav.kt").unwrap();
    let idx = Indexer::new();
    let src = "package app\nclass Navigator\nfun screen() {\n    val navigator = remember { Navigator() }\n}\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let rt = infer_receiver_type(&idx, ReceiverKind::Variable("navigator"), &uri);
    assert_eq!(
        rt.map(|r| r.raw),
        Some("Navigator".into()),
        "val from `remember {{ Navigator() }}` must infer Navigator"
    );
}

/// Import-aware return-type lookup must bind to the *imported* function, not an
/// arbitrary same-named overload. Mirrors nowinandroid `stringResource`: the
/// imported compose `stringResource: String` must win over a workspace test
/// extension `AndroidComposeTestRule.stringResource: ReadOnlyProperty<…>`.
#[test]
fn return_type_reachable_prefers_imported_symbol() {
    use super::find_fun_return_type_reachable;
    use crate::indexer::Indexer;
    use crate::sidecar::SidecarSymbol;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    // Compiled-jar compose `stringResource(): String`.
    crate::indexer::jar::populate_from_symbols(
        &idx,
        std::path::Path::new("/fake/compose-ui.jar"),
        &[SidecarSymbol {
            name: "stringResource".into(),
            kind: "fun".into(),
            container: "StringResourcesKt".into(),
            detail: "fun stringResource(id: Int): String".into(),
            doc: String::new(),
            type_params: vec![],
            extension_receiver_type: String::new(),
            trailing_lambda: false,
            deprecated: false,
            pkg: "androidx.compose.ui.res".into(),
            top_level: true,
            supers: vec![],
        }],
    );
    // Workspace decoy with the same name but a different return type.
    let decoy = Url::parse("file:///app/TestExt.kt").unwrap();
    idx.index_content(
        &decoy,
        "package app.test\nfun stringResource(id: Int): ReadOnlyProperty = TODO()\n",
    );

    // Caller imports the compose one.
    let caller = Url::parse("file:///app/Screen.kt").unwrap();
    idx.index_content(
        &caller,
        "package app\nimport androidx.compose.ui.res.stringResource\nfun s() { val x = stringResource(1) }\n",
    );

    assert_eq!(
        find_fun_return_type_reachable(&idx, "stringResource", &caller),
        Some("String".into()),
        "must bind to the imported compose stringResource (String), not the decoy"
    );
}

// ─── Inherited generic property inference ────────────────────────────────────

fn test_uri(path: &str) -> tower_lsp::lsp_types::Url {
    tower_lsp::lsp_types::Url::parse(&format!("file://{path}")).unwrap()
}

fn index_generic_binding_hierarchy(
    idx: &crate::indexer::Indexer,
    base_source: &str,
    child_source: &str,
) -> (tower_lsp::lsp_types::Url, tower_lsp::lsp_types::Url) {
    let base_uri = test_uri("/ViewBindingAdapter.kt");
    let child_uri = test_uri("/Foo.kt");
    idx.index_content(&base_uri, base_source);
    idx.index_content(&child_uri, child_source);
    (base_uri, child_uri)
}

#[test]
fn inherited_generic_property_resolves_concrete_binding_type() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::infer_variable_type_raw;

    let idx = Indexer::new();
    let base_source = "package com.example\nabstract class ViewBindingAdapter<T> {\n    val binding: T get() = error(\"not init\")\n}";
    let child_source = "package com.example\nclass Foo : ViewBindingAdapter<FooLayoutBinding>() {\n    fun bar() {\n        binding\n    }\n}";
    let (_, child_uri) = index_generic_binding_hierarchy(&idx, base_source, child_source);

    assert_eq!(
        infer_variable_type_raw(&idx, "binding", &child_uri),
        Some("FooLayoutBinding".into()),
        "inherited generic property should substitute T with concrete type arg"
    );
}

#[test]
fn inherited_generic_property_ignores_competing_class_in_other_file() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::infer_variable_type_raw;

    let idx = Indexer::new();
    let base_source = "package com.example\nabstract class ViewBindingAdapter<T> {\n    val binding: T get() = error(\"not init\")\n}";
    let child_source = "package com.example\nclass Foo : ViewBindingAdapter<FooLayoutBinding>() {\n    fun bar() { binding }\n}";
    let (_, child_uri) = index_generic_binding_hierarchy(&idx, base_source, child_source);

    let decoy_uri = test_uri("/Wrong.kt");
    idx.index_content(
        &decoy_uri,
        "package com.example.other\nclass WrongAdapter {\n    val binding: WrongBinding get() = error(\"wrong\")\n}",
    );

    assert_eq!(
        infer_variable_type_raw(&idx, "binding", &child_uri),
        Some("FooLayoutBinding".into()),
        "competing binding in another file must not override inherited resolution"
    );
}

#[test]
fn inherited_generic_property_multilevel_hierarchy() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::{find_field_type_in_class, infer_variable_type_raw};

    let idx = Indexer::new();
    idx.index_content(
        &test_uri("/Base0.kt"),
        "package com.example\nabstract class Base0<T> {\n    val binding: T get() = error(\"not init\")\n}",
    );
    idx.index_content(
        &test_uri("/Base1.kt"),
        "package com.example\nabstract class Base1<T> : Base0<T>()",
    );
    let child_uri = test_uri("/Foo.kt");
    idx.index_content(
        &child_uri,
        "package com.example\nclass Foo : Base1<FooLayoutBinding>() {\n    fun bar() { binding }\n}",
    );

    assert_eq!(
        infer_variable_type_raw(&idx, "binding", &child_uri),
        Some("FooLayoutBinding".into()),
        "multi-level inheritance should compose generic substitutions"
    );
    assert_eq!(
        find_field_type_in_class(&idx, "Foo", "binding"),
        Some("FooLayoutBinding".into()),
        "explicit receiver lookup should resolve inherited generic property"
    );
}

#[test]
fn inherited_generic_property_preserves_nullable_raw_type() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::infer_variable_type_raw;

    let idx = Indexer::new();
    let base_source = "package com.example\nabstract class ViewBindingAdapter<T> {\n    val binding: T? get() = null\n}";
    let child_source = "package com.example\nclass Foo : ViewBindingAdapter<FooLayoutBinding>() {\n    fun bar() { binding }\n}";
    let (_, child_uri) = index_generic_binding_hierarchy(&idx, base_source, child_source);

    assert_eq!(
        infer_variable_type_raw(&idx, "binding", &child_uri),
        Some("FooLayoutBinding?".into()),
        "nullable inherited generic property should preserve ? in raw inference"
    );
}

#[test]
fn infer_variable_type_view_binding_generic_delegate() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::infer_variable_type_raw;

    let idx = Indexer::new();
    let uri = test_uri("/MainFragment.kt");
    idx.index_content(
        &uri,
        "class MainFragment {\n    private val binding by viewBinding<FooBarBinding>()\n}",
    );
    assert_eq!(
        infer_variable_type_raw(&idx, "binding", &uri),
        Some("FooBarBinding".into())
    );
}

#[test]
fn infer_variable_type_view_binding_inflate_delegate() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::infer_variable_type_raw;

    let idx = Indexer::new();
    let uri = test_uri("/MainFragment.kt");
    idx.index_content(
        &uri,
        "class MainFragment {\n    private val binding by viewBinding(FooBarBinding::inflate)\n}",
    );
    assert_eq!(
        infer_variable_type_raw(&idx, "binding", &uri),
        Some("FooBarBinding".into())
    );
}

#[test]
fn infer_variable_type_non_binding_delegate_unaffected() {
    use crate::indexer::Indexer;
    use crate::resolver::infer::infer_variable_type_raw;

    let idx = Indexer::new();
    let uri = test_uri("/Main.kt");
    idx.index_content(
        &uri,
        "class Main {\n    private val repo by lazy { UserRepository() }\n}",
    );
    assert_eq!(
        infer_variable_type_raw(&idx, "repo", &uri),
        Some("UserRepository".into())
    );
}
