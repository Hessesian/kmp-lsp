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

/// `substitute_direct_supertype_args` looks up the direct supertype's own
/// declared type params via `find_class_type_params`, which is keyed by the
/// bare class name (`FileData.symbols`' `name` field). `super_name` (as
/// stored in `FileData.supers`) can be a dotted qualified spelling (e.g.
/// `class Derived : app.Base<Int>()`), so it must be stripped to its last
/// segment before that lookup -- otherwise the lookup silently misses and
/// substitution is skipped entirely, leaving the raw literal type parameter
/// (`T`) instead of the concrete argument (`Int`).
#[test]
fn substitute_direct_supertype_args_handles_qualified_super_name() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Types.kt").unwrap();
    idx.index_content(
        &f,
        "package app\nopen class Base<T>\nclass Derived : app.Base<Int>()\n",
    );

    assert_eq!(
        super::substitute_direct_supertype_args(&idx, f.as_str(), "Derived", "app.Base", "T"),
        "Int",
        "a qualified supertype spelling (app.Base) must still resolve Base's \
         own type params for substitution"
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

/// Promotion test for the Task 8 wiring in `find_fun_return_type_reachable`.
/// This site runs with a ZERO sidecar-IPC budget (inference is called once
/// per name on latency-critical paths like inlay hints — unbudgeted blocking
/// IPC here was observed live as a 22s inlay stall), so only fresh-cache-
/// backed promotions happen: the fixture seeds a real on-disk jar-symbol
/// cache entry (isolated XDG) and asserts full materialization, mirroring
/// `extension_property_type_promotes_a_cache_backed_tier1_only_receiver`.
#[test]
fn return_type_reachable_promotes_a_cache_backed_tier1_only_symbol() {
    use super::find_fun_return_type_reachable;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let tmp = tempfile::tempdir().expect("tempdir");
    crate::indexer::test_helpers::with_xdg_cache(tmp.path(), || {
        let jar_path = tmp.path().join("helper-lib.jar");
        std::fs::write(&jar_path, b"fake jar bytes").expect("write fake jar");
        let jar_path_key = jar_path.to_string_lossy().to_string();

        let symbols = vec![crate::sidecar::SidecarSymbol {
            name: "remoteHelper".to_owned(),
            kind: "fun".to_owned(),
            container: String::new(),
            detail: "fun remoteHelper(): String".to_owned(),
            doc: String::new(),
            type_params: Vec::new(),
            extension_receiver_type: String::new(),
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
        idx.jar_bare_names
            .entry("remoteHelper".to_owned())
            .or_default()
            .push(jar_id);

        let caller = Url::parse("file:///app/Caller.kt").unwrap();
        let _ = find_fun_return_type_reachable(&idx, "remoteHelper", &caller);
        assert!(
            idx.materialized.contains(&jar_id),
            "find_fun_return_type_reachable must promote a fresh-cache-backed \
             Tier-1-only candidate (free, no sidecar IPC), not silently miss it"
        );
    });
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
fn extension_fn_return_type_scoped_promotes_a_cache_backed_tier1_only_symbol() {
    use super::find_extension_fn_return_type;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let tmp = tempfile::tempdir().expect("tempdir");
    crate::indexer::test_helpers::with_xdg_cache(tmp.path(), || {
        let jar_path = tmp.path().join("ext-lib.jar");
        std::fs::write(&jar_path, b"fake jar bytes").expect("write fake jar");
        let jar_path_key = jar_path.to_string_lossy().to_string();

        let symbols = vec![crate::sidecar::SidecarSymbol {
            name: "remoteExt".to_owned(),
            kind: "fun".to_owned(),
            container: String::new(),
            detail: "fun Foo.remoteExt(): Bar".to_owned(),
            doc: String::new(),
            type_params: Vec::new(),
            extension_receiver_type: "Foo".to_owned(),
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
        idx.jar_bare_names
            .entry("remoteExt".to_owned())
            .or_default()
            .push(jar_id);

        // Deliberately no `extension_by_receiver` entry — that map is only
        // ever populated by Tier-2 materialization, so a genuine Tier-1-only
        // symbol starts with it empty. The promotion (before the early
        // `extension_by_receiver.get(receiver_base)?` read) is what fills it.
        let caller = Url::parse("file:///app/Caller.kt").unwrap();
        idx.index_content(&caller, "package app\nimport lib.remoteExt\nfun m() {}\n");

        let inferred = find_extension_fn_return_type(&idx, "Foo", "remoteExt", Some(&caller));
        assert!(
            idx.materialized.contains(&jar_id),
            "find_extension_fn_return_type must promote a fresh-cache-backed \
             Tier-1-only candidate before the early extension_by_receiver \
             read, not bail out via `?` first"
        );
        assert_eq!(
            inferred.as_deref(),
            Some("Bar"),
            "after promotion the extension's return type must be inferable"
        );
    });
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

/// Regression for the "unscoped last-resort tail" gap in
/// `find_fun_return_type_by_name` itself (not the already-fixed Retrofit/
/// class-literal shape in `chain.rs`): a *bare* function call with nothing at
/// the call site to rescue it (no class-literal arg, no generic-factory name,
/// no receiver at all) whose name collides with an unrelated, unimported
/// symbol declared in a different package. Before the reachability
/// preference, `find_in_workspace_defs`/`workspace_def_candidates` returned
/// candidates in `definitions` insertion order and `find_fun_return_type_by_name`
/// took the first one with an extractable return type — here that is the
/// unrelated decoy (indexed first), not the reachable symbol, proving the
/// collision is real at this layer, independent of any receiver/CST fallback.
#[test]
fn find_fun_return_type_by_name_prefers_reachable_candidate_over_first_match() {
    use super::find_fun_return_type_by_name;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();

    // Indexed FIRST (so it would win under plain first-match-in-iteration-order):
    // an unrelated, unimported `helper()` in a different package.
    let decoy = Url::parse("file:///unrelated/Decoy.kt").unwrap();
    idx.index_content(
        &decoy,
        "package unrelated\nfun helper(): DecoyResult = TODO()\n",
    );

    // Indexed SECOND: the symbol actually reachable from the caller via
    // explicit import.
    let real = Url::parse("file:///lib/Real.kt").unwrap();
    idx.index_content(&real, "package lib\nfun helper(): RealResult = TODO()\n");

    let caller = Url::parse("file:///app/Caller.kt").unwrap();
    idx.index_content(
        &caller,
        "package app\nimport lib.helper\nfun m() { val x = helper() }\n",
    );

    assert_eq!(
        find_fun_return_type_by_name(&idx, "helper", &caller),
        Some("RealResult".to_string()),
        "the imported, reachable `helper` must win over an unrelated same-named \
         decoy that merely happens to be indexed first"
    );
}

/// Sibling case: same collision, but reachability comes from same-package
/// membership rather than an explicit import.
#[test]
fn find_fun_return_type_by_name_prefers_same_package_candidate_over_first_match() {
    use super::find_fun_return_type_by_name;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();

    let decoy = Url::parse("file:///unrelated/Decoy.kt").unwrap();
    idx.index_content(
        &decoy,
        "package unrelated\nfun helper(): DecoyResult = TODO()\n",
    );

    let real = Url::parse("file:///app/Real.kt").unwrap();
    idx.index_content(&real, "package app\nfun helper(): RealResult = TODO()\n");

    // Same package as `Real.kt`, no import needed.
    let caller = Url::parse("file:///app/Caller.kt").unwrap();
    idx.index_content(&caller, "package app\nfun m() { val x = helper() }\n");

    assert_eq!(
        find_fun_return_type_by_name(&idx, "helper", &caller),
        Some("RealResult".to_string()),
        "the same-package `helper` must win over an unrelated same-named decoy \
         from a different package"
    );
}

/// When NO candidate is reachable at all, the historical "grab the first with
/// an extractable return type" behavior is preserved deliberately (see the
/// doc comment on `find_fun_return_type_by_name`): this function only runs
/// after `find_fun_return_type_reachable` already failed, so returning
/// *something* is still judged more useful than nothing.
#[test]
fn find_fun_return_type_by_name_falls_back_to_first_match_when_nothing_reachable() {
    use super::find_fun_return_type_by_name;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();

    let first = Url::parse("file:///unrelated1/A.kt").unwrap();
    idx.index_content(
        &first,
        "package unrelated1\nfun helper(): FirstResult = TODO()\n",
    );
    let second = Url::parse("file:///unrelated2/B.kt").unwrap();
    idx.index_content(
        &second,
        "package unrelated2\nfun helper(): SecondResult = TODO()\n",
    );

    // Caller shares no package and imports neither candidate.
    let caller = Url::parse("file:///app/Caller.kt").unwrap();
    idx.index_content(&caller, "package app\nfun m() { val x = helper() }\n");

    assert_eq!(
        find_fun_return_type_by_name(&idx, "helper", &caller),
        Some("FirstResult".to_string()),
        "with no reachable candidate, the first indexed match is still returned \
         (unchanged historical behavior — the fix is a *preference*, not a filter)"
    );
}

/// `extension_is_in_scope`'s same-package check only fired when BOTH sides
/// carried a `Some` package -- so two files in Kotlin's default package (no
/// `package` header) were never considered same-package to each other, even
/// though `None == None` there means "both in the (same) default package,"
/// not "unknown, assume different." Caught by review after this function was
/// reused for `candidate_declaration_is_reachable`; a near-identical gap had
/// already been separately patched around once before, at a third call site
/// (`nullable_call_diagnostics.rs`'s `extension_in_scope_here`), instead of
/// being fixed here at the source.
///
/// Passes a real (known) `FileData` with `package: None` -- not a bare `None`
/// `caller_file_data` -- since those mean different things: an unloaded/
/// unknown caller file must NOT be treated as "confirmed default package"
/// (a second review round caught the first version of this test conflating
/// the two, which the fix itself had also conflated).
#[test]
fn extension_is_in_scope_treats_default_package_as_same_package() {
    use super::extension_is_in_scope;
    use crate::types::FileData;

    let caller_file_data = FileData {
        package: None,
        ..Default::default()
    };

    assert!(
        extension_is_in_scope(None, "helper", Some(&caller_file_data)),
        "two default-package files (no `package` header on either side) must \
         be considered same-package, not unreachable"
    );
}

/// The distinction the review round above caught: an *unknown* caller file
/// (`caller_file_data: None`, e.g. not yet indexed) must NOT be treated the
/// same as a *known* default-package caller -- unlike the previous test,
/// this must stay `false`.
#[test]
fn extension_is_in_scope_does_not_treat_unknown_caller_as_default_package() {
    use super::extension_is_in_scope;

    assert!(
        !extension_is_in_scope(None, "helper", None),
        "an unloaded/unknown caller file must not be guessed as \"confirmed \
         default package\" just because its package is also unrepresentable \
         as `Some`"
    );
}

/// End-to-end sibling of the `find_fun_return_type_by_name_prefers_*` tests
/// above, but for the default-package case specifically: a caller with no
/// `package` header must still prefer a same-(default-)package candidate
/// over an unrelated, differently-packaged decoy.
#[test]
fn find_fun_return_type_by_name_prefers_default_package_candidate_over_first_match() {
    use super::find_fun_return_type_by_name;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();

    let decoy = Url::parse("file:///unrelated/Decoy.kt").unwrap();
    idx.index_content(
        &decoy,
        "package unrelated\nfun helper(): DecoyResult = TODO()\n",
    );

    // No `package` header -- the default package.
    let real = Url::parse("file:///app/Real.kt").unwrap();
    idx.index_content(&real, "fun helper(): RealResult = TODO()\n");

    // Also no `package` header -- same default package as `Real.kt`.
    let caller = Url::parse("file:///app/Caller.kt").unwrap();
    idx.index_content(&caller, "fun m() { val x = helper() }\n");

    assert_eq!(
        find_fun_return_type_by_name(&idx, "helper", &caller),
        Some("RealResult".to_string()),
        "the default-package `helper` must win over an unrelated same-named \
         decoy from a different (real) package"
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

/// `find_method_return_type_via_supertypes` keys its workspace/JAR lookups by
/// the bare symbol name, so a *qualified* `class_name` (e.g.
/// `app.Derived`, as some callers pass) must still resolve -- the generics-
/// and-package stripping has to take the last dotted segment, not just cut
/// at `<`.
#[test]
fn catalog_method_return_type_folds_supertype_inheritance_for_qualified_class_name() {
    use crate::indexer::Indexer;
    use crate::resolver::Resolver;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Types.kt").unwrap();
    idx.index_content(
        &f,
        "package app\nopen class Base { fun who(): Identity = TODO() }\nclass Derived : Base()\n",
    );

    assert_eq!(
        idx.method_return_type("app.Derived", "who", None)
            .map(|r| r.into_inner()),
        Some("Identity".to_string()),
        "a qualified class_name must still resolve via the supertype walk, \
         not silently miss both the workspace and JAR lookups (which are \
         keyed by bare name)"
    );
}

/// Regression: `MutableSharedFlow.asSharedFlow()` (and `asStateFlow`, etc.)
/// erased their generic argument in hover/inlay because the supertype walk
/// only ever read *workspace*-indexed classes (`find_in_workspace_defs`).
/// `MutableSharedFlow` is JAR-only, and the extension `asSharedFlow` is
/// declared on its JAR supertype `SharedFlow` — so the walk silently no-oped
/// (never read `MutableSharedFlow`'s `supers`), and the caller fell through
/// to a receiver-agnostic by-name lookup that returns the extension's
/// literal, unsubstituted declaration text (`SharedFlow<T>`) instead of
/// resolving through the class hierarchy to the extension declared on the
/// JAR supertype.
#[test]
fn catalog_method_return_type_folds_jar_supertype_inheritance() {
    use crate::indexer::Indexer;
    use crate::resolver::Resolver;
    use crate::types::{ExtensionEntry, FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;
    use tower_lsp::lsp_types::{Location, Position, Range, SymbolKind, Url};

    let idx = Indexer::new();
    let jar_uri = Url::parse("jar:file:///lib/fake-flow.jar!/Flow.class").unwrap();

    let mk_range = |line: u32, len: u32| Range {
        start: Position { line, character: 0 },
        end: Position {
            line,
            character: len,
        },
    };
    let mk_class_symbol = |name: &str, line: u32| SymbolEntry {
        name: name.to_owned(),
        kind: SymbolKind::INTERFACE,
        visibility: Visibility::Public,
        range: mk_range(line, name.len() as u32),
        selection_range: mk_range(line, name.len() as u32),
        detail: format!("interface {name}<T>"),
        container: None,
        params: String::new(),
        param_counts: (0, 0),
        cold: crate::types::pack_cold_fields(
            vec!["T".to_owned()],
            String::new(),
            String::new(),
            String::new(),
        ),
        trailing_lambda: false,
        deprecated: false,
    };

    // `SharedFlow` (index 0) and `MutableSharedFlow` (index 1), where
    // `MutableSharedFlow`'s only super is `SharedFlow` (JAR-derived `supers`
    // never carry type args -- see `build_jar_file_data`).
    let shared_flow_symbol = mk_class_symbol("SharedFlow", 0);
    let mutable_shared_flow_symbol = mk_class_symbol("MutableSharedFlow", 1);

    // Both classes need a `jar_definitions` entry: `MutableSharedFlow` is the
    // walk's starting point, and `SharedFlow` must be independently
    // resolvable too -- `walk_hierarchy` resolves each ancestor by name via
    // `resolve_symbol_no_rg` (see `supertype_targets` in hierarchy.rs) rather
    // than reading `jar_files` directly.
    idx.jar_definitions
        .entry("SharedFlow".into())
        .or_default()
        .push(Location {
            uri: jar_uri.clone(),
            range: shared_flow_symbol.selection_range,
        });
    idx.jar_definitions
        .entry("MutableSharedFlow".into())
        .or_default()
        .push(Location {
            uri: jar_uri.clone(),
            range: mutable_shared_flow_symbol.selection_range,
        });
    idx.jar_files.insert(
        jar_uri.to_string(),
        Arc::new(FileData {
            symbols: vec![shared_flow_symbol, mutable_shared_flow_symbol],
            supers: vec![(1, "SharedFlow".to_owned(), Vec::new())],
            source_set: SourceSet::Library,
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );

    // `fun <T> SharedFlow<T>.asSharedFlow(): SharedFlow<T>` -- keyed by its
    // declared receiver `SharedFlow`, not `MutableSharedFlow`.
    idx.extension_by_receiver
        .entry("SharedFlow".to_owned())
        .or_default()
        .push(ExtensionEntry {
            file_uri: jar_uri.to_string(),
            name: "asSharedFlow".to_owned(),
            kind: SymbolKind::FUNCTION,
            detail: "fun <T> SharedFlow<T>.asSharedFlow(): SharedFlow<T>".to_owned(),
            visibility: Visibility::Public,
            package: Some("kotlinx.coroutines.flow".to_owned()),
            trailing_lambda: false,
            deprecated: false,
        });

    let caller = Url::parse("file:///app/Repo.kt").unwrap();
    idx.index_content(&caller, "package kotlinx.coroutines.flow\nclass Repo\n");

    assert_eq!(
        idx.method_return_type("MutableSharedFlow", "asSharedFlow", Some(&caller))
            .map(|r| r.into_inner()),
        Some("SharedFlow<T>".to_string()),
        "method_return_type must walk a JAR-only receiver's JAR-only \
         supertype to find an extension declared there, instead of silently \
         no-oping and letting the caller fall back to an unsubstituted, \
         receiver-agnostic by-name lookup"
    );
}

/// The supertype walk now goes through [`walk_hierarchy`] (multi-level,
/// cycle-safe) instead of only checking the *direct* supertype. This asserts
/// the case the old single-level implementation could never reach: a method
/// declared two levels up the chain.
#[test]
fn catalog_method_return_type_folds_multi_level_supertype_inheritance() {
    use crate::indexer::Indexer;
    use crate::resolver::Resolver;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Types.kt").unwrap();
    idx.index_content(
        &f,
        "package app\n\
         open class Grandparent { fun who(): Identity = TODO() }\n\
         open class Parent : Grandparent()\n\
         class Child : Parent()\n",
    );

    assert_eq!(
        idx.method_return_type("Child", "who", None)
            .map(|r| r.into_inner()),
        Some("Identity".to_string()),
        "method_return_type must find a method declared two levels up \
         (Grandparent), not just on the direct supertype (Parent)"
    );
}

/// End-to-end regression for the `.asSharedFlow()` generic-erasure bug,
/// reproduced with a purely workspace-declared analog (no JAR needed --
/// `kotlinx.coroutines.flow.SharedFlow`/`MutableSharedFlow`/`asSharedFlow`
/// have the exact same shape: a generic extension declared on a supertype of
/// the receiver's declared type). Covers the full chain: `infer_var_from_rhs_data`
/// falling back to the supertype walk (this module) AND substituting the
/// receiver's own concrete type argument into the raw, as-declared return
/// type (`crate::indexer::build_type_arg_subst`) -- both are required for
/// `flow`'s hover-augmented type to come out as `SharedFlow<Unit>` instead of
/// `SharedFlow<T>`.
#[test]
fn infer_variable_type_substitutes_supertype_extension_generic_arg() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Flow.kt").unwrap();
    idx.index_content(
        &f,
        "package app\n\
         interface SharedFlow<T>\n\
         interface MutableSharedFlow<T> : SharedFlow<T>\n\
         fun <T> SharedFlow<T>.asSharedFlow(): SharedFlow<T> = TODO()\n\
         class Repo {\n\
         \x20   private val _flow = MutableSharedFlow<Unit>()\n\
         \x20   val flow = _flow.asSharedFlow()\n\
         }\n",
    );

    assert_eq!(
        super::infer_variable_type_raw(&idx, "flow", &f),
        Some("SharedFlow<Unit>".to_string()),
        "the receiver's own concrete type argument (Unit) must be \
         substituted into the extension's declared return type, not left as \
         the literal type parameter (SharedFlow<T>)"
    );
}

/// Same generic-erasure bug as `infer_variable_type_substitutes_supertype_extension_generic_arg`,
/// but reproduced through `infer_method_return_type` — the cruder whole-line
/// regex scan `infer_var_from_rhs_data` falls back to when the calling file has
/// no indexed `FileData` at all (live-editor content only, e.g. a buffer whose
/// `didOpen`/index pass hasn't completed yet). This path never went through
/// `infer_var_from_rhs_data`'s `method_call_rhs` map (there is none — nothing
/// indexed this file), so the SharedFlow fix's substitution step, which only
/// touched `infer_var_from_rhs_data`, never reached it.
#[test]
fn infer_method_return_type_line_scan_substitutes_supertype_extension_generic_arg() {
    use crate::indexer::Indexer;
    use crate::types::FileData;
    use std::sync::Arc;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();

    // `SharedFlow`/`MutableSharedFlow`/`asSharedFlow` must be indexed somewhere
    // so the supertype walk and extension lookup can find them.
    let decls = Url::parse("file:///app/Flow.kt").unwrap();
    idx.index_content(
        &decls,
        "package app\n\
         interface SharedFlow<T>\n\
         interface MutableSharedFlow<T> : SharedFlow<T>\n\
         fun <T> SharedFlow<T>.asSharedFlow(): SharedFlow<T> = TODO()\n",
    );

    // The calling file is deliberately NOT indexed via `index_content` (which
    // would populate `method_call_rhs` and let `infer_var_from_rhs_data` handle
    // `flow` directly) — instead its `FileData` is hand-built with an empty
    // `method_call_rhs`, forcing `infer_var_from_rhs_data` to find no match and
    // `infer_variable_type_core` to fall through to the line-scan
    // `infer_method_return_type`. `package` is still set (as a normal indexed
    // file would have it) so the same-package extension-visibility check that
    // `find_extension_fn_return_type_scoped` performs still passes.
    let caller = Url::parse("file:///app/Repo.kt").unwrap();
    let src = "package app\n\
               class Repo {\n\
               \x20   val _flow: MutableSharedFlow<Unit> = MutableSharedFlow()\n\
               \x20   val flow = _flow.asSharedFlow()\n\
               }\n";
    idx.set_live_lines(&caller, src);
    idx.files.insert(
        caller.to_string(),
        Arc::new(FileData {
            package: Some("app".to_owned()),
            lines: Arc::new(src.lines().map(str::to_owned).collect()),
            ..Default::default()
        }),
    );

    assert_eq!(
        super::infer_variable_type_raw(&idx, "flow", &caller),
        Some("SharedFlow<Unit>".to_string()),
        "infer_method_return_type (the line-scan fallback) must substitute the \
         receiver's own concrete type argument (Unit) into the extension's \
         declared return type too, not just infer_var_from_rhs_data's \
         indexed-data path — otherwise the two STRING-side call sites disagree \
         with each other (and with the CST engine, which always substitutes)"
    );
}

/// Drift guard, not a bug reproduction: the STRING engine
/// (`infer_variable_type_raw`, what hover's `enrich_symbol` uses) and the CST
/// engine (`infer_variable_type_from_cst`, which resolves a `val`
/// initializer's type via `indexer/infer/chain.rs`'s `resolve_call_expr_type`
/// -- the same path inlay hints use) must agree on this "generic extension
/// declared on a supertype" shape. Both engines happen to reach the same
/// underlying composite today (`InferDeps::find_method_return_type_for_type`)
/// after the `resolve_method_return_type_substituted` refactor, so this is
/// green now -- its job is to catch a *future* change that lands on only one
/// side, which is exactly the class of bug that made hover show
/// `SharedFlow<T>` while inlay hints already showed `SharedFlow<Unit>`.
#[test]
fn string_and_cst_engines_agree_on_supertype_extension_generic_arg() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Flow.kt").unwrap();
    let src = "package app\n\
         interface SharedFlow<T>\n\
         interface MutableSharedFlow<T> : SharedFlow<T>\n\
         fun <T> SharedFlow<T>.asSharedFlow(): SharedFlow<T> = TODO()\n\
         class Repo {\n\
         \x20   private val _flow = MutableSharedFlow<Unit>()\n\
         \x20   val flow = _flow.asSharedFlow()\n\
         }\n";
    idx.index_content(&f, src);
    idx.store_live_tree(&f, src);

    let string_result = super::infer_variable_type_raw(&idx, "flow", &f);
    let cst_result = super::infer_variable_type_from_cst(&idx, "flow", &f);

    assert_eq!(
        string_result, cst_result,
        "hover's STRING-path inference and inlay-hints' CST-path inference \
         must agree on the same generic-return-type scenario -- a divergence \
         here means one engine's fallback/substitution policy changed \
         without the other"
    );
    assert_eq!(
        string_result,
        Some("SharedFlow<Unit>".to_string()),
        "both engines must substitute the receiver's concrete type argument, \
         not just agree with each other on the wrong answer"
    );
}

/// Regression: `retrofit.create(GoldConversionPublicApi::class.java)` --
/// `Retrofit`'s own `create` isn't indexed (not workspace/JAR-materialized in
/// this test, same as a real un-promoted Retrofit JAR), so
/// `resolve_call_expr_type` falls through to the receiver-agnostic bare-name
/// scan (`find_fun_return_type_reachable`/`find_fun_return_type`), which is a
/// pure name match with no regard for the receiver's actual type -- it
/// happily matches a COMPLETELY UNRELATED `create` declared on some other
/// class (here standing in for e.g. KSP's `SymbolProcessorProvider.create():
/// SymbolProcessor`, a real collision found live in a production project).
/// The `find_class_literal_arg_type` fallback that exists specifically to
/// handle "receiver not indexed" Retrofit-style calls never gets a chance to
/// run, because it's gated on `result.is_none()` and the bare-name scan
/// already produced a (wrong) `Some`.
#[test]
fn class_literal_arg_fallback_not_shadowed_by_unrelated_bare_name_match() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();

    // The decoy MUST be in a genuinely unrelated, unimported package (as the
    // real KSP collision was) -- a same-package decoy is legitimately
    // reachable via same-package visibility and doesn't reproduce the bug
    // (an earlier version of this test put the decoy in the caller's own
    // package by accident, which `find_fun_return_type_reachable`'s
    // same-package step correctly resolves to, and so stopped reproducing
    // the collision after the reachable-lookup-first reordering fix).
    let decoy = Url::parse("file:///ksp/Decoy.kt").unwrap();
    idx.index_content(
        &decoy,
        "package ksp\n\
         class SymbolProcessor\n\
         class SymbolProcessorProvider {\n\
         \x20   fun create(): SymbolProcessor = TODO()\n\
         }\n",
    );

    let f = Url::parse("file:///app/Repo.kt").unwrap();
    let src = "package app\n\
         class Retrofit\n\
         class GoldConversionPublicApi\n\
         class Repo(retrofit: Retrofit) {\n\
         \x20   val textApi = retrofit.create(GoldConversionPublicApi::class.java)\n\
         }\n";
    idx.index_content(&f, src);
    idx.store_live_tree(&f, src);

    assert_eq!(
        super::infer_variable_type_from_cst(&idx, "textApi", &f),
        Some("GoldConversionPublicApi".to_string()),
        "the class-literal argument names the answer unambiguously and must \
         win over an unrelated, unimported same-named `create` found by \
         bare-name scan"
    );
}

/// Sibling safety-net for the fix above: promoting the class-literal fallback
/// ahead of the bare-name scan must NOT become its own false-positive source.
/// `logEvent` isn't a `GENERIC_FACTORY_FNS` name, so a class-literal argument
/// passed to it for an unrelated reason (logging/reflection, not a
/// factory-returns-the-argument's-type pattern) must not hijack its real,
/// correctly-indexed return type.
#[test]
fn class_literal_arg_fallback_does_not_override_unrelated_indexed_function() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Repo.kt").unwrap();
    let src = "package app\n\
         class LogHandle\n\
         class SomeClass\n\
         class Logger {\n\
         \x20   fun logEvent(cls: Class<*>): LogHandle = TODO()\n\
         }\n\
         class Repo(logger: Logger) {\n\
         \x20   val handle = logger.logEvent(SomeClass::class.java)\n\
         }\n";
    idx.index_content(&f, src);
    idx.store_live_tree(&f, src);

    assert_eq!(
        super::infer_variable_type_from_cst(&idx, "handle", &f),
        Some("LogHandle".to_string()),
        "logEvent isn't a known factory-function name, so its real indexed \
         return type must win over guessing from the class-literal argument"
    );
}

/// Copilot review finding (round 5, on `chain.rs`'s reordered heuristics):
/// a genuinely receiver-less call to a real, indexed, differently-named
/// `toLong` must resolve to ITS declared return type, not the
/// `NUMERIC_CONVERSION_FNS` heuristic's hardcoded `"Long"`. Kotlin's actual
/// numeric-conversion `toLong()` is always a member call (`5.toLong()`); a
/// bare `toLong(x)` naming an unrelated top-level function is legal Kotlin
/// and must not be swallowed by the heuristic just because the name matches.
#[test]
fn numeric_conversion_heuristic_does_not_fire_on_receiver_less_call() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Repo.kt").unwrap();
    let src = "package app\n\
         fun toLong(x: Int): String = TODO()\n\
         class Repo {\n\
         \x20   val result = toLong(5)\n\
         }\n";
    idx.index_content(&f, src);
    idx.store_live_tree(&f, src);

    assert_eq!(
        super::infer_variable_type_from_cst(&idx, "result", &f),
        Some("String".to_string()),
        "a receiver-less toLong(x) naming a real, differently-typed function \
         must resolve to its own declared return type, not be guessed as \
         Long by the numeric-conversion heuristic"
    );
}

/// Copilot review finding: the DI-factory `<T>` heuristic must not override a
/// real, indexed, reachable `create`/`get`/etc. — this one also proves the
/// real path is *better*, not just different: `find_fun_return_type_reachable`
/// finds the true declared `Wrapper<T>` and the call-site substitution step
/// (`build_fn_subst`/`apply_simple_subst`) resolves it to `Wrapper<Foo>`,
/// which the heuristic (bare bare `Foo`, no knowledge of the `Wrapper<>`
/// wrapping) could never have produced correctly.
#[test]
fn di_factory_heuristic_does_not_override_real_indexed_generic_function() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Repo.kt").unwrap();
    let src = "package app\n\
         class Wrapper<T>\n\
         class Foo\n\
         fun <T> create(): Wrapper<T> = TODO()\n\
         class Repo {\n\
         \x20   val result = create<Foo>()\n\
         }\n";
    idx.index_content(&f, src);
    idx.store_live_tree(&f, src);

    assert_eq!(
        super::infer_variable_type_from_cst(&idx, "result", &f),
        Some("Wrapper<Foo>".to_string()),
        "a real, indexed generic `create<T>(): Wrapper<T>` must resolve (and \
         substitute) via the real declaration, not be guessed as the bare \
         type argument by the DI-factory heuristic"
    );
}

/// Copilot review finding: the Retrofit-style class-literal heuristic must
/// not override a real, indexed, reachable `create` either — a bare
/// (receiver-less) call to a real top-level `create` that happens to take a
/// class-literal argument for an unrelated reason must resolve to its own
/// declared return type.
#[test]
fn class_literal_heuristic_does_not_override_real_indexed_bare_function() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Repo.kt").unwrap();
    let src = "package app\n\
         class RealResult\n\
         class Foo\n\
         fun create(cls: Class<*>): RealResult = TODO()\n\
         class Repo {\n\
         \x20   val result = create(Foo::class.java)\n\
         }\n";
    idx.index_content(&f, src);
    idx.store_live_tree(&f, src);

    assert_eq!(
        super::infer_variable_type_from_cst(&idx, "result", &f),
        Some("RealResult".to_string()),
        "a real, indexed, reachable bare create(...) must resolve to its own \
         declared return type, not be guessed from the class-literal \
         argument"
    );
}

/// Real-world bug: `store.businessState.filterIsInstance<SnackBarState.Error>()`
/// inferred as `Flow<R>` instead of `Flow<SnackBarState.Error>`.
///
/// `filterIsInstance` resolves via the RECEIVER-based branch of
/// `resolve_call_expr_type` (an indexed extension on `Flow`), which only
/// ever substitutes the RECEIVER's own generic argument
/// (`build_type_arg_subst`) -- but `filterIsInstance`'s real signature is
/// `fun <R> Flow<*>.filterIsInstance(): Flow<R>`: `R` is the CALLED
/// FUNCTION's own type parameter, supplied only via the explicit `<T>` at
/// the call site, never derived from the receiver (which is star-projected,
/// `Flow<*>`). The call-site type-argument substitution
/// (`call_site_type_arg_strings` + `find_fun_callable_info` +
/// `build_fn_subst`) already existed for the *receiver-agnostic* branch
/// further down this function, but the receiver-based branch returned
/// early before ever reaching it.
#[test]
fn receiver_based_resolution_also_substitutes_call_site_type_argument() {
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    let idx = Indexer::new();
    let f = Url::parse("file:///app/Repo.kt").unwrap();
    let src = "package app\n\
         interface Flow<T>\n\
         open class SnackBarState {\n\
         \x20   object Empty : SnackBarState()\n\
         \x20   class Error : SnackBarState()\n\
         }\n\
         class Store<T>(initial: T) {\n\
         \x20   val businessState: Flow<T> = TODO()\n\
         }\n\
         fun <R> Flow<*>.filterIsInstance(): Flow<R> = TODO()\n\
         class Repo {\n\
         \x20   private val store: Store<SnackBarState> = Store(SnackBarState.Empty)\n\
         \x20   val result = store.businessState.filterIsInstance<SnackBarState.Error>()\n\
         }\n";
    idx.index_content(&f, src);
    idx.store_live_tree(&f, src);

    assert_eq!(
        super::infer_variable_type_from_cst(&idx, "result", &f),
        Some("Flow<SnackBarState.Error>".to_string()),
        "filterIsInstance's own type parameter R must be substituted from \
         the explicit call-site type argument, not left as the literal R \
         (the receiver's own generic argument is irrelevant here -- the \
         real receiver type is star-projected Flow<*>)"
    );
}
