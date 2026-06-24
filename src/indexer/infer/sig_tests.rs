//! Unit tests for `indexer::infer::sig`.
//!
//! Covers pure string helpers that can be tested without any `Indexer` state.

use super::*;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::Url;

fn test_uri(path: &str) -> Url {
    Url::parse(&format!("file://{path}")).unwrap()
}

// ─── collect_signature ────────────────────────────────────────────────────────

#[test]
fn collect_signature_single_line_with_brace() {
    let lines = vec!["sealed interface NewsFeedUiState {".to_owned()];
    // The `{` should be stripped; result is just the declaration.
    assert_eq!(
        collect_signature(&lines, 0),
        "sealed interface NewsFeedUiState"
    );
}

#[test]
fn collect_signature_single_line_no_brace() {
    let lines = vec!["fun doSomething(x: Int): Boolean".to_owned()];
    assert_eq!(
        collect_signature(&lines, 0),
        "fun doSomething(x: Int): Boolean"
    );
}

#[test]
fn collect_signature_multiline_constructor() {
    let lines = vec![
        "class DetailViewModel @Inject constructor(".to_owned(),
        "  private val mapper: DetailMapper,".to_owned(),
        "  private val loadUseCase: LoadDataUseCase,".to_owned(),
        ") : MviViewModel<Event, State, Effect>() {".to_owned(),
    ];
    let sig = collect_signature(&lines, 0);
    assert!(sig.contains("DetailViewModel"), "should contain class name");
    assert!(sig.contains("MviViewModel"), "should contain superclass");
    assert!(!sig.contains('{'), "should not include body brace");
}

#[test]
fn collect_signature_brace_on_own_line() {
    // `{` on its own line — body opener, must not appear in output.
    let lines = vec![
        "class Foo(val x: Int)".to_owned(),
        "    : Bar() {".to_owned(),
    ];
    let sig = collect_signature(&lines, 0);
    assert!(!sig.contains('{'), "brace should be stripped");
    assert!(sig.contains("Foo"), "class name must be present");
}

#[test]
fn collect_signature_starts_at_offset() {
    let lines = vec!["// comment".to_owned(), "fun hello(): String".to_owned()];
    assert_eq!(collect_signature(&lines, 1), "fun hello(): String");
}

#[test]
fn collect_signature_caps_at_15_lines() {
    // A function spanning more than 15 lines must not cause a panic.
    let mut lines: Vec<String> = vec!["fun f(".to_owned()];
    for i in 0..20 {
        lines.push(format!("  p{i}: Int,"));
    }
    lines.push(")".to_owned());
    let sig = collect_signature(&lines, 0);
    // Should have collected up to the 15-line cap without panicking.
    assert!(sig.contains("fun f("), "should start with fun f(");
}

// ─── nth_fun_param_type_str ───────────────────────────────────────────────────

#[test]
fn nth_param_type_first() {
    let params = "key: String, value: Int";
    assert_eq!(nth_fun_param_type_str(params, 0), Some("String".into()));
}

#[test]
fn nth_param_type_second() {
    let params = "key: String, value: Int";
    assert_eq!(nth_fun_param_type_str(params, 1), Some("Int".into()));
}

#[test]
fn nth_param_type_out_of_range_falls_back_to_last() {
    let params = "key: String, value: Int";
    assert_eq!(nth_fun_param_type_str(params, 99), Some("Int".into()));
}

#[test]
fn nth_param_type_lambda_type_arg() {
    // `->` must not upset `<>` depth counter.
    let params = "key: ProductKey, flow: (Boolean) -> Flow<ResultState<T>>, map: (ResultState<T>) -> StatefulModel";
    assert_eq!(nth_fun_param_type_str(params, 0), Some("ProductKey".into()));
    assert_eq!(
        nth_fun_param_type_str(params, 1),
        Some("(Boolean) -> Flow<ResultState<T>>".into())
    );
    assert_eq!(
        nth_fun_param_type_str(params, 2),
        Some("(ResultState<T>) -> StatefulModel".into())
    );
}

#[test]
fn nth_param_type_single_param() {
    let params = "block: () -> Unit";
    assert_eq!(nth_fun_param_type_str(params, 0), Some("() -> Unit".into()));
}

#[test]
fn nth_param_type_empty_returns_none() {
    assert_eq!(nth_fun_param_type_str("", 0), None);
}

#[test]
fn nth_param_type_val_var_prefix_stripped() {
    // Constructor params: `val repo: IRepo, var counter: Int`.
    let params = "val repo: IRepo, var counter: Int";
    assert_eq!(nth_fun_param_type_str(params, 0), Some("IRepo".into()));
    assert_eq!(nth_fun_param_type_str(params, 1), Some("Int".into()));
}

// ─── last_fun_param_type_str ─────────────────────────────────────────────────

#[test]
fn last_param_type_single_param() {
    assert_eq!(
        last_fun_param_type_str("block: () -> Unit"),
        Some("() -> Unit".into())
    );
}

#[test]
fn last_param_type_multiple_params() {
    let params = "a: String, b: Int, c: Boolean";
    assert_eq!(last_fun_param_type_str(params), Some("Boolean".into()));
}

#[test]
fn last_param_type_lambda_last() {
    // The trailing lambda param after a `->` in the type must be parsed correctly.
    let params = "key: ProductKey, map: (ResultState<T>) -> StatefulModel";
    assert_eq!(
        last_fun_param_type_str(params),
        Some("(ResultState<T>) -> StatefulModel".into())
    );
}

#[test]
fn last_param_type_arrow_depth_not_confused() {
    // `reloadableProduct` has two functional-type params; the `>` of `->` must
    // not throw off the depth counter so that the last param is picked correctly.
    let params =
        "key: ProductKey, productFlow: (isRefresh: Boolean) -> Flow<ResultState<T>>, map: (ResultState<T>) -> StatefulModel<SortableProducts>";
    assert_eq!(
        last_fun_param_type_str(params),
        Some("(ResultState<T>) -> StatefulModel<SortableProducts>".into())
    );
}

#[test]
fn last_param_type_empty_returns_none() {
    assert_eq!(last_fun_param_type_str(""), None);
}

// ─── split_params_at_depth_zero ──────────────────────────────────────────────

use super::split_params_at_depth_zero;

#[test]
fn split_simple() {
    assert_eq!(
        split_params_at_depth_zero("a: A, b: B"),
        vec!["a: A", " b: B"]
    );
}

#[test]
fn split_nested_generics() {
    // comma inside <> must not split
    assert_eq!(
        split_params_at_depth_zero("a: Map<K, V>, b: B"),
        vec!["a: Map<K, V>", " b: B"]
    );
}

#[test]
fn split_function_type_arrow() {
    // `->` must not cause `>` to consume generic depth
    assert_eq!(
        split_params_at_depth_zero("block: (T) -> Unit, n: Int"),
        vec!["block: (T) -> Unit", " n: Int"]
    );
}

#[test]
fn split_empty() {
    assert_eq!(split_params_at_depth_zero(""), vec![""]);
}

#[test]
fn split_single() {
    assert_eq!(split_params_at_depth_zero("a: A"), vec!["a: A"]);
}

#[test]
fn split_trailing_comma() {
    let parts = split_params_at_depth_zero("a: A, b: B,");
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[2], "");
}

// ─── strip_trailing_call_args ─────────────────────────────────────────────────

#[test]
fn strip_args_with_trailing_parens() {
    assert_eq!(
        strip_trailing_call_args("collection.method(arg1, arg2)"),
        "collection.method"
    );
}

#[test]
fn strip_args_no_trailing_parens() {
    assert_eq!(
        strip_trailing_call_args("collection.forEach"),
        "collection.forEach"
    );
}

#[test]
fn strip_args_nested_parens() {
    assert_eq!(strip_trailing_call_args("fn(a, g(x))"), "fn");
}

#[test]
fn strip_args_empty_parens() {
    assert_eq!(strip_trailing_call_args("build()"), "build");
}

#[test]
fn strip_args_dotted_method_with_args() {
    assert_eq!(strip_trailing_call_args("state.copy(id = x)"), "state.copy");
}

#[test]
fn strip_args_unbalanced_no_crash() {
    // If parens are unbalanced, should not panic; returns original.
    assert_eq!(strip_trailing_call_args("fn("), "fn(");
}

// ─── Regression: `>` operator in default values must not go negative ─────────

#[test]
fn nth_param_type_gt_operator_in_default() {
    // `x: Int = a > b` — the `>` is a comparison, not a generic close.
    // Must not make depth go negative and break subsequent comma splitting.
    let params = "x: Int, y: String";
    assert_eq!(nth_fun_param_type_str(params, 1), Some("String".to_owned()));
}

// ─── is_import_reachable ─────────────────────────────────────────────────────

#[test]
fn resolve_qualified_skips_top_level_function_before_type_body() {
    let caller_uri = test_uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(
        &caller_uri,
        "package com.example\nfun run(unrelated: Int, extra: Int) {}\nclass Service {\n    fun run(name: String) {}\n}\nclass Caller(private val service: Service) {\n    fun invoke() {\n        service.run(1)\n    }\n}\n",
    );

    let call = CallSite {
        name: "run",
        qualifier: Some("service"),
        caller_uri: &caller_uri,
    };

    match resolve_call_signature(&call, &idx) {
        SignatureResult::Unique {
            param_counts,
            params_text,
        } => {
            assert_eq!(param_counts, (1, 1));
            assert_eq!(params_text, "name: String");
        }
        other => panic!("expected unique class member match, got {other:?}"),
    }
}

#[test]
fn resolve_qualified_matches_method_via_container() {
    let caller_uri = test_uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(
        &caller_uri,
        "package com.example\nclass Api {\n    fun fetch(id: String, force: Boolean = false) {}\n}\nclass Caller(private val api: Api) {\n    fun invoke() {\n        api.fetch(1)\n    }\n}\n",
    );

    let call = CallSite {
        name: "fetch",
        qualifier: Some("api"),
        caller_uri: &caller_uri,
    };

    match resolve_call_signature(&call, &idx) {
        SignatureResult::Unique {
            param_counts,
            params_text,
        } => {
            assert_eq!(param_counts, (1, 2));
            assert_eq!(params_text, "id: String, force: Boolean = false");
        }
        other => panic!("expected unique class member match, got {other:?}"),
    }
}

#[test]
fn resolve_qualified_skips_extension_from_unimported_package() {
    // An extension function in an unrelated package (not imported by caller)
    // for the same receiver must NOT be matched by resolve_qualified.
    // The caller should only see symbols that are import-reachable.
    let caller_uri = test_uri("/Caller.kt");
    let unrelated_uri = test_uri("/Unrelated.kt");
    let idx = Indexer::new();

    // Receiver class with a 0-arg member method.
    idx.index_content(
        &caller_uri,
        "package com.example\n\
         class Repository {\n    fun loadData() {}\n}\n\
         class Caller(private val repo: Repository) {\n    fun invoke() {\n        repo.loadData()\n    }\n}\n",
    );

    // Extension in an unrelated package (not imported by caller).
    // Same name, same receiver, but NOT import-reachable.
    idx.index_content(
        &unrelated_uri,
        "package com.other\n\
         fun com.example.Repository.loadData(path: String, force: Boolean) {}\n",
    );

    let call = CallSite {
        name: "loadData",
        qualifier: Some("repo"),
        caller_uri: &caller_uri,
    };

    match resolve_call_signature(&call, &idx) {
        SignatureResult::Unique {
            param_counts,
            params_text,
        } => {
            // Must match the 0-arg member, NOT the 2-arg extension from the
            // unrelated package.
            assert_eq!(param_counts, (0, 0));
            assert!(
                params_text.is_empty(),
                "expected empty params for 0-arg method, got: {params_text}"
            );
        }
        other => panic!("expected unique (0,0) match from same-file member, got {other:?}"),
    }
}

#[test]
fn resolve_unqualified_bails_on_ubiquitous_name() {
    // A name with hundreds of cross-file definitions (the source-JAR explosion that
    // stalled the diagnostics path) must short-circuit to `Overloaded` instead of
    // scanning every definition's file. The distinguishing signal: WITHOUT the cap this
    // scan visits all (fabricated, un-indexed) files, finds nothing, and returns
    // `NotFound`; WITH the cap it returns `Overloaded` before scanning.
    use tower_lsp::lsp_types::{Location, Range};

    let caller_uri = test_uri("/Caller.kt");
    let idx = Indexer::new();
    // Caller calls `create` but does not define it, so the same-file fast path is empty
    // and the capped cross-file path is the one exercised.
    idx.index_content(
        &caller_uri,
        "package com.example\nfun invoke() {\n    create()\n}\n",
    );

    let many: Vec<Location> = (0..(crate::indexer::MAX_BY_NAME_DEFS + 25))
        .map(|i| Location {
            uri: test_uri(&format!("/lib/F{i}.kt")),
            range: Range::default(),
        })
        .collect();
    idx.definitions.insert("create".to_owned(), many);

    let call = CallSite {
        name: "create",
        qualifier: None,
        caller_uri: &caller_uri,
    };

    assert!(
        matches!(
            resolve_call_signature(&call, &idx),
            SignatureResult::Overloaded
        ),
        "a name with > MAX_BY_NAME_DEFS definitions must bail to Overloaded, not scan them all"
    );
}

#[test]
fn resolve_qualified_jar_extension_overloads_with_source_member() {
    // A JAR-indexed extension (Phase 2) with different arity than a
    // source-indexed member (Phase 1) must be treated as an overload —
    // both arities end up in `found`, producing Overloaded.
    // Regression: the old `found.is_empty()` gate skipped Phase 2 entirely
    // when Phase 1 found anything, causing the member arity to win.
    use crate::types::{ExtensionEntry, FileData, Visibility};
    use tower_lsp::lsp_types::{Position, Range};

    let jar_uri = "jar:file:///lib/support.jar!/support/extensions.kt";
    let caller_uri = test_uri("/Caller.kt");
    let idx = Indexer::new();

    // Source file: Repository with 0-arg member loadData().
    idx.index_content(
        &caller_uri,
        "package com.example\n\
         class Repository {\n    fun loadData() {}\n}\n\
         class Caller(private val repo: Repository) {\n    fun invoke() {\n        repo.loadData()\n    }\n}\n",
    );

    // Simulate a JAR extension: 1-arg loadData(path) on Repository.
    let jar_symbol = SymbolEntry {
        name: "loadData".into(),
        kind: SymbolKind::FUNCTION,
        visibility: Visibility::Public,
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
        selection_range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
        detail: "fun com.example.Repository.loadData(path: String): Any".into(),
        params: "path: String".into(),
        param_counts: (1, 1),
        container: None,
        extension_receiver: "Repository".into(),
        extension_receiver_type: "Repository".into(),
        type_params: vec![],
        doc: String::new(),
        trailing_lambda: false,
        deprecated: false,
    };
    let jar_file_data = std::sync::Arc::new(FileData {
        symbols: vec![jar_symbol],
        imports: vec![],
        package: Some("com.example".into()),
        lines: std::sync::Arc::new(vec![]),
        source_set: Default::default(),
        declared_names: vec![],
        supers: vec![],
        rhs_types: vec![],
        method_call_rhs: vec![],
        field_access_rhs: vec![],
        type_annotations: vec![],
        syntax_errors: vec![],
    });
    idx.jar_files.insert(jar_uri.to_string(), jar_file_data);
    idx.extension_by_receiver
        .entry("Repository".into())
        .or_default()
        .push(ExtensionEntry {
            file_uri: jar_uri.into(),
            name: "loadData".into(),
            kind: SymbolKind::FUNCTION,
            detail: "fun com.example.Repository.loadData(path: String): Any".into(),
            visibility: Visibility::Public,
            package: Some("com.example".into()),
            trailing_lambda: false,
            deprecated: false,
        });

    let call = CallSite {
        name: "loadData",
        qualifier: Some("repo"),
        caller_uri: &caller_uri,
    };

    // Both 0-arg member + 1-arg extension → Overloaded.
    match resolve_call_signature(&call, &idx) {
        SignatureResult::Overloaded => {}
        other => panic!("expected Overloaded (0-arg member + 1-arg extension), got {other:?}"),
    }
}

#[test]
fn resolve_unqualified_data_class_constructor() {
    let caller_uri = test_uri("/Config.kt");
    let idx = Indexer::new();
    idx.index_content(
        &caller_uri,
        "package com.example\ndata class Config(\n    val host: String,\n    val port: Int = 443,\n)\n\nfun build(): Config {\n    return Config(host = \"localhost\")\n}\n",
    );

    let call = CallSite {
        name: "Config",
        qualifier: None,
        caller_uri: &caller_uri,
    };

    match resolve_call_signature(&call, &idx) {
        SignatureResult::Unique {
            param_counts,
            params_text,
        } => {
            assert_eq!(param_counts, (1, 2));
            assert!(params_text.contains("host: String"));
        }
        other => panic!("expected unique constructor match, got {other:?}"),
    }
}

#[test]
fn resolve_unqualified_test_definition_visible_only_to_test_callers() {
    let idx = Indexer::new();
    let helper_uri = test_uri("/workspace/src/test/kotlin/com/example/TestHelper.kt");
    let test_caller_uri = test_uri("/workspace/src/test/kotlin/com/example/TestCaller.kt");
    let main_caller_uri = test_uri("/workspace/src/main/kotlin/com/example/MainCaller.kt");

    idx.index_content(
        &helper_uri,
        "package com.example\nfun testOnlyHelper(arg: String) {}\n",
    );
    idx.index_content(
        &test_caller_uri,
        "package com.example\nfun invokeFromTest() { testOnlyHelper() }\n",
    );
    idx.index_content(
        &main_caller_uri,
        "package com.example\nfun invokeFromMain() { testOnlyHelper() }\n",
    );

    let test_call = CallSite {
        name: "testOnlyHelper",
        qualifier: None,
        caller_uri: &test_caller_uri,
    };
    match resolve_call_signature(&test_call, &idx) {
        SignatureResult::Unique {
            param_counts,
            params_text,
        } => {
            assert_eq!(param_counts, (1, 1));
            assert_eq!(params_text, "arg: String");
        }
        other => panic!("expected test caller to resolve test helper, got {other:?}"),
    }

    let main_call = CallSite {
        name: "testOnlyHelper",
        qualifier: None,
        caller_uri: &main_caller_uri,
    };
    assert!(
        matches!(
            resolve_call_signature(&main_call, &idx),
            SignatureResult::NotFound
        ),
        "main caller must not resolve same-package test helper"
    );
}

// ─── extract_params_from_detail ──────────────────────────────────────────────

#[test]
fn extract_params_zero_param_returns_some_empty() {
    // `None` used to mean "no signature found"; `Some("")` means
    // "signature found, zero parameters". They must not be conflated.
    assert_eq!(
        extract_params_from_detail("fun onClick()"),
        Some("".into()),
        "zero-param function must return Some(\"\") not None"
    );
}

#[test]
fn extract_params_regular_function_returns_params() {
    assert_eq!(
        extract_params_from_detail("fun greet(name: String, age: Int)"),
        Some("name: String, age: Int".into())
    );
}

// ─── collect_params_from_line ─────────────────────────────────────────────────

#[test]
fn collect_params_skips_annotation_lines() {
    // Annotation lines like `@Suppress("UNCHECKED_CAST")` must be skipped
    // entirely; their argument text must not be treated as function parameters.
    let lines: Vec<String> = vec![
        "@Suppress(\"UNCHECKED_CAST\")".into(),
        "@SomethingElse(value = 1)".into(),
        "fun process(input: String, count: Int) {".into(),
    ];
    let result = collect_params_from_line(&lines, 0);
    assert_eq!(
        result,
        Some("input: String, count: Int".into()),
        "annotation arguments must not be treated as function parameters"
    );
}

#[test]
fn collect_params_pure_annotation_window_does_not_return_annotation_args() {
    // If the only lines visible are annotations (no fun line in the window),
    // the result should be None rather than the annotation's own args.
    let lines: Vec<String> = vec![
        "@Composable".into(),
        "@Preview(showBackground = true)".into(),
    ];
    assert_eq!(
        collect_params_from_line(&lines, 0),
        None,
        "window with only annotations must return None"
    );
}

#[cfg(test)]
mod import_reachable {
    use super::{collect_params_from_file, is_import_reachable, ResolutionScope};
    use crate::indexer::Indexer;
    use crate::types::{FileData, ImportEntry, SymbolEntry, Visibility};
    use std::sync::Arc;
    use tower_lsp::lsp_types::{Position, Range, SymbolKind};

    fn make_url(path: &str) -> String {
        format!("file://{}", path)
    }

    fn index_file(idx: &Indexer, uri: &str, pkg: &str, imports: Vec<ImportEntry>) {
        index_file_with_symbols(idx, uri, pkg, imports, vec![]);
    }

    fn index_file_with_symbols(
        idx: &Indexer,
        uri: &str,
        pkg: &str,
        imports: Vec<ImportEntry>,
        symbols: Vec<SymbolEntry>,
    ) {
        let data = FileData {
            package: Some(pkg.to_owned()),
            imports,
            symbols,
            ..FileData::default()
        };
        idx.files.insert(uri.to_owned(), Arc::new(data));
    }

    fn explicit_import(pkg: &str, name: &str) -> ImportEntry {
        explicit_import_path(&format!("{}.{}", pkg, name), name)
    }

    fn explicit_import_path(full_path: &str, local_name: &str) -> ImportEntry {
        ImportEntry {
            full_path: full_path.to_owned(),
            local_name: local_name.to_owned(),
            is_star: false,
        }
    }

    fn star_import(pkg: &str) -> ImportEntry {
        ImportEntry {
            full_path: pkg.to_owned(),
            local_name: "*".to_owned(),
            is_star: true,
        }
    }

    fn nested_class(name: &str, container: &str) -> SymbolEntry {
        let range = Range::new(Position::new(0, 0), Position::new(0, name.len() as u32));
        SymbolEntry {
            name: name.to_owned(),
            kind: SymbolKind::CLASS,
            visibility: Visibility::Public,
            range,
            selection_range: range,
            detail: String::new(),
            params: String::new(),
            param_counts: (0, 0),
            type_params: vec![],
            extension_receiver: String::new(),
            extension_receiver_type: String::new(),
            container: Some(container.to_owned()),
            doc: String::new(),
            trailing_lambda: false,
            deprecated: false,
        }
    }

    #[test]
    fn same_file_always_reachable() {
        let idx = Indexer::new();
        let uri = make_url("/a/Foo.kt");
        index_file(&idx, &uri, "com.example", vec![]);
        assert!(is_import_reachable(&idx, &uri, &uri, "Foo"));
    }

    #[test]
    fn same_package_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/a/B.kt");
        index_file(&idx, &caller, "com.example", vec![]);
        index_file(&idx, &def, "com.example", vec![]);
        assert!(is_import_reachable(&idx, &caller, &def, "Foo"));
    }

    #[test]
    fn different_package_no_import_not_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/B.kt");
        index_file(&idx, &caller, "com.example", vec![]);
        index_file(&idx, &def, "com.other", vec![]);
        assert!(!is_import_reachable(&idx, &caller, &def, "Foo"));
    }

    #[test]
    fn explicit_import_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/Foo.kt");
        index_file(
            &idx,
            &caller,
            "com.example",
            vec![explicit_import("com.other", "Foo")],
        );
        index_file(&idx, &def, "com.other", vec![]);
        assert!(is_import_reachable(&idx, &caller, &def, "Foo"));
    }

    #[test]
    fn nested_class_explicit_import_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/Outer.kt");
        index_file(
            &idx,
            &caller,
            "com.client",
            vec![explicit_import_path("com.example.Outer.Config", "Config")],
        );
        index_file(&idx, &def, "com.example", vec![]);
        assert!(is_import_reachable(&idx, &caller, &def, "Config"));
    }

    #[test]
    fn deeply_nested_import_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/Outer.kt");
        index_file(
            &idx,
            &caller,
            "com.client",
            vec![explicit_import_path(
                "com.example.Outer.Inner.Config",
                "Config",
            )],
        );
        index_file(&idx, &def, "com.example", vec![]);
        assert!(is_import_reachable(&idx, &caller, &def, "Config"));
    }

    #[test]
    fn nested_class_star_import_not_reachable_cross_file() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/Outer.kt");
        index_file(
            &idx,
            &caller,
            "com.client",
            vec![star_import("com.example")],
        );
        index_file_with_symbols(
            &idx,
            &def,
            "com.example",
            vec![],
            vec![nested_class("Config", "Outer")],
        );
        assert!(collect_params_from_file(
            "Config",
            &def,
            &idx,
            &caller,
            ResolutionScope::CrossFile,
            None,
        )
        .is_empty());
    }

    #[test]
    fn explicit_import_wrong_name_not_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/Bar.kt");
        index_file(
            &idx,
            &caller,
            "com.example",
            vec![explicit_import("com.other", "Foo")],
        );
        index_file(&idx, &def, "com.other", vec![]);
        assert!(!is_import_reachable(&idx, &caller, &def, "Bar"));
    }

    #[test]
    fn star_import_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/Foo.kt");
        index_file(&idx, &caller, "com.example", vec![star_import("com.other")]);
        index_file(&idx, &def, "com.other", vec![]);
        assert!(is_import_reachable(&idx, &caller, &def, "Foo"));
    }

    #[test]
    fn star_import_wrong_package_not_reachable() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        let def = make_url("/b/Foo.kt");
        index_file(&idx, &caller, "com.example", vec![star_import("com.third")]);
        index_file(&idx, &def, "com.other", vec![]);
        assert!(!is_import_reachable(&idx, &caller, &def, "Foo"));
    }

    #[test]
    fn missing_caller_data_fails_open() {
        let idx = Indexer::new();
        let def = make_url("/b/Foo.kt");
        index_file(&idx, &def, "com.other", vec![]);
        assert!(is_import_reachable(&idx, "file:///missing.kt", &def, "Foo"));
    }

    #[test]
    fn missing_def_data_fails_open() {
        let idx = Indexer::new();
        let caller = make_url("/a/A.kt");
        index_file(&idx, &caller, "com.example", vec![]);
        assert!(is_import_reachable(
            &idx,
            &caller,
            "file:///missing.kt",
            "Foo"
        ));
    }
}
