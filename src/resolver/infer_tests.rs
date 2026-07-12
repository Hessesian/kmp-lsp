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

/// Decoy test for the Task 8 promotion wiring: `find_fun_return_type_reachable`
/// reads `jar_files` directly at its inner loop. No real sidecar is available
/// in a unit test, so this pins the CONTRACT that a Tier-1-only candidate
/// triggers a promotion *attempt* (observable via `materialization_failed`)
/// rather than the function silently reading `jar_files` and missing it.
#[test]
fn return_type_reachable_attempts_promotion_for_a_tier1_only_symbol() {
    use super::find_fun_return_type_reachable;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/nonexistent/fixture.jar");
    idx.jar_bare_names
        .entry("remoteHelper".to_owned())
        .or_default()
        .push(jar_id);

    let caller = Url::parse("file:///app/Caller.kt").unwrap();
    let _ = find_fun_return_type_reachable(&idx, "remoteHelper", &caller);
    assert!(
        idx.materialization_failed.contains(&jar_id),
        "find_fun_return_type_reachable must attempt promotion for a \
         Tier-1-only name, not silently miss it"
    );
}

/// Decoy test for the Task 8 promotion wiring: `find_extension_fn_return_type`
/// (the `_scoped` path) reads `jar_files` directly once `detail` fails to yield
/// a return type. No real sidecar is available in a unit test, so this pins the
/// CONTRACT that a Tier-1-only candidate triggers a promotion *attempt*
/// (observable via `materialization_failed`) before that fallback read, rather
/// than silently reading `jar_files` and missing it.
///
/// This seeds ONLY `jar_bare_names` (the real Tier-1 signal) and leaves
/// `extension_by_receiver` empty — the actual pre-materialization state.
/// `extension_by_receiver` is populated exclusively by Tier-2 materialization
/// (`build_jar_file_data`); a real Tier-1-only symbol can never have an
/// `extension_by_receiver` entry yet. (An earlier version of this test
/// manually injected an `extension_by_receiver` entry alongside the
/// `jar_bare_names` one — a combination that can never occur in production,
/// since both maps are populated together only by materialization. That
/// masked the fact that `find_extension_fn_return_type_scoped` read
/// `extension_by_receiver` and returned via `?` *before* any promotion
/// check ran, making the promotion attempt unreachable for a genuine
/// Tier-1-only symbol.)
#[test]
fn extension_fn_return_type_scoped_attempts_promotion_for_a_tier1_only_symbol() {
    use super::find_extension_fn_return_type;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/nonexistent/fixture.jar");
    idx.jar_bare_names
        .entry("remoteExt".to_owned())
        .or_default()
        .push(jar_id);

    // Deliberately no `extension_by_receiver` entry — that map is only ever
    // populated by Tier-2 materialization, so a genuine Tier-1-only symbol
    // starts with it empty.
    let caller = Url::parse("file:///app/Caller.kt").unwrap();
    idx.index_content(&caller, "package app\nfun m() {}\n");

    let _ = find_extension_fn_return_type(&idx, "Foo", "remoteExt", Some(&caller));
    assert!(
        idx.materialization_failed.contains(&jar_id),
        "find_extension_fn_return_type must attempt promotion for a Tier-1-only \
         name even when extension_by_receiver has no entry for it yet, not bail \
         out via the early `extension_by_receiver.get(receiver_base)?` read \
         before ever attempting promotion"
    );
}

/// `find_extension_property_type` walks the calling file's class hierarchy
/// and reads `extension_by_receiver` per ancestor to infer an extension
/// property's type (e.g. `viewModelScope`'s `CoroutineScope`, needed for
/// chained completion after the property). Like the two completion sites
/// fixed earlier, that read is Tier-2-only — without a receiver-keyed
/// promotion first, a Tier-1-only JAR's extension property is invisible to
/// type inference. Flagged as a known gap in the previous fix's review.
///
/// This site's promotion runs with a ZERO sidecar-IPC budget (it executes
/// inside completion requests, whose IPC cap belongs to the completion
/// sites), so only fresh-cache-backed candidates materialize — which is why
/// this test seeds a real on-disk jar-symbol cache entry (isolated XDG)
/// rather than the fake-path/`materialization_failed` pattern the budgeted
/// sites' tests use, and asserts full end-to-end materialization plus the
/// inferred type coming out the other side.
#[test]
fn extension_property_type_promotes_a_cache_backed_tier1_only_receiver() {
    use super::find_extension_property_type;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let tmp = tempfile::tempdir().expect("tempdir");
    crate::indexer::test_helpers::with_xdg_cache(tmp.path(), || {
        let jar_path = tmp.path().join("lifecycle-ktx.jar");
        std::fs::write(&jar_path, b"fake jar bytes").expect("write fake jar");
        let jar_path_key = jar_path.to_string_lossy().to_string();

        let symbols = vec![crate::sidecar::SidecarSymbol {
            name: "someScope".to_owned(),
            // The sidecar's kind string for a read-only property — maps to
            // SymbolKind::PROPERTY via `kind_str_to_lsp` ("property" would
            // map to NULL and fail the PROPERTY|VARIABLE filter).
            kind: "val".to_owned(),
            container: String::new(),
            detail: "val Holder.someScope: CoroutineScope".to_owned(),
            doc: String::new(),
            type_params: Vec::new(),
            extension_receiver_type: "Holder".to_owned(),
            trailing_lambda: false,
            deprecated: false,
            pkg: "lib".to_owned(),
            top_level: true,
            supers: vec![],
        }];
        let entry = crate::indexer::jar_cache::make_cache_entry(&jar_path, symbols)
            .expect("cache entry for existing file");
        let mut entries = std::collections::HashMap::new();
        entries.insert(jar_path_key.clone(), entry);
        crate::indexer::jar_cache::save_jar_cache(&entries);

        let idx = Indexer::new();
        let jar_id = idx.jar_table.intern(&jar_path_key);
        idx.jar_extension_receivers
            .entry("Holder".to_owned())
            .or_default()
            .push(jar_id);

        let caller = Url::parse("file:///app/Holder.kt").unwrap();
        idx.index_content(&caller, "package app\nclass Holder {\n    fun m() {}\n}\n");

        let inferred = find_extension_property_type(&idx, "someScope", &caller);
        assert!(
            idx.materialized.contains(&jar_id),
            "find_extension_property_type must promote a fresh-cache-backed \
             JAR that Tier 1 says declares an extension on an ancestor of \
             the calling file's classes, before reading extension_by_receiver"
        );
        assert_eq!(
            inferred.as_deref(),
            Some("CoroutineScope"),
            "after promotion the extension property's type must be inferable \
             from the freshly materialized extension_by_receiver entry"
        );
    });
}

/// `Resolver::function_return_type` is the import-aware catalog entry: it binds
/// through the scope chain first, then falls back to a workspace-wide by-name
/// lookup, returning a self-documenting [`ReturnType`]. This asserts the
/// fallback arm — a function in another package with no import is still found.
#[test]
fn catalog_function_return_type_falls_back_to_by_name() {
    use crate::indexer::Indexer;
    use crate::resolver::Resolver;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let def = Url::parse("file:///app/Repo.kt").unwrap();
    idx.index_content(&def, "package app\nfun buildRepo(): Repository = TODO()\n");

    // Caller in a different package with no import — reachable resolution fails,
    // so the by-name fallback must carry it.
    let caller = Url::parse("file:///other/Main.kt").unwrap();
    idx.index_content(&caller, "package other\nfun m() {}\n");

    assert_eq!(
        idx.function_return_type("buildRepo", &caller)
            .map(|r| r.into_inner()),
        Some("Repository".to_string()),
        "by-name fallback must find the workspace function"
    );
}

/// `Resolver::method_return_type` is the single composite for member resolution:
/// own/extension methods *and* inherited (supertype) methods resolve through one
/// call. This asserts the supertype arm — a method declared only on the base
/// class resolves when queried on the derived class.
#[test]
fn catalog_method_return_type_folds_supertype_inheritance() {
    use crate::indexer::Indexer;
    use crate::resolver::Resolver;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Types.kt").unwrap();
    idx.index_content(
        &f,
        "package app\nopen class Base { fun who(): Identity = TODO() }\nclass Derived : Base()\n",
    );

    // `who` is declared only on Base; querying it on Derived must resolve via the
    // supertype walk that `method_return_type` folds in.
    assert_eq!(
        idx.method_return_type("Derived", "who", None)
            .map(|r| r.into_inner()),
        Some("Identity".to_string()),
        "method_return_type must fold supertype inheritance into one call"
    );
}
