use tower_lsp::lsp_types::Url;

use crate::indexer::live_tree::parse_live;
use crate::indexer::Indexer;

use super::call_arg_diagnostics;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn setup(sources: &[(&str, &str)]) -> (Url, Indexer, String) {
    let idx = Indexer::new();
    let mut last_uri = uri("/test.kt");
    let mut last_src = String::new();
    for (path, src) in sources {
        let u = uri(path);
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        last_uri = u;
        last_src = (*src).to_string();
    }
    (last_uri, idx, last_src)
}

/// Run diagnostics using a locally-parsed tree (mirrors production flow).
fn run_diagnostics(
    idx: &Indexer,
    uri: &Url,
    source: &str,
) -> Vec<tower_lsp::lsp_types::Diagnostic> {
    let doc = parse_live(source, tree_sitter_kotlin::language()).unwrap();
    call_arg_diagnostics(idx, uri, &doc)
}

#[test]
fn no_diagnostic_when_args_match() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String, age: Int) {}\n",
            "fun main() {\n",
            "    greet(\"Alice\", 30)\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "expected no diagnostics: {diags:?}");
}

#[test]
fn too_few_args_warns() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String, age: Int) {}\n",
            "fun main() {\n",
            "    greet(\"Alice\")\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected 1 diagnostic: {diags:?}");
    assert!(
        diags[0].message.contains("expected 2"),
        "msg: {}",
        diags[0].message
    );
    assert!(
        diags[0].message.contains("found 1"),
        "msg: {}",
        diags[0].message
    );
}

#[test]
fn too_many_args_warns() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String) {}\n",
            "fun main() {\n",
            "    greet(\"Alice\", 30, true)\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected 1 diagnostic: {diags:?}");
    assert!(
        diags[0].message.contains("at most 1"),
        "msg: {}",
        diags[0].message
    );
    assert!(
        diags[0].message.contains("found 3"),
        "msg: {}",
        diags[0].message
    );
}

#[test]
fn default_params_not_required() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String, greeting: String = \"Hello\") {}\n",
            "fun main() {\n",
            "    greet(\"Alice\")\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "default param should not be required: {diags:?}"
    );
}

#[test]
fn default_params_still_cap_max() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String, greeting: String = \"Hello\") {}\n",
            "fun main() {\n",
            "    greet(\"Alice\", \"Hi\", \"extra\")\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "too many args: {diags:?}");
    assert!(
        diags[0].message.contains("at most 2"),
        "msg: {}",
        diags[0].message
    );
}

#[test]
fn extension_fn_default_param_not_required() {
    // `cancel()` should be valid — `cause` has a default value.
    // This tests that extract_detail preserves the `= null` part even
    // when the signature is multiline.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun CoroutineContext.cancel(cause: CancellationException? = null) {\n",
            "}\n",
            "fun test(ctx: CoroutineContext) {\n",
            "    ctx.cancel()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "cancel() with default param should not error: {diags:?}"
    );
}

#[test]
fn named_arg_missing_required_param_is_flagged() {
    // greet(name = "Alice") provides only 1 of 2 required args → diagnostic
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String, age: Int) {}\n",
            "fun main() {\n",
            "    greet(name = \"Alice\")\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        !diags.is_empty(),
        "missing required named arg should be flagged"
    );
    assert!(
        diags[0].message.contains("expected 2"),
        "{:?}",
        diags[0].message
    );
}

#[test]
fn named_args_all_provided_ok() {
    // All params supplied by name → no diagnostic
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String, age: Int) {}\n",
            "fun main() {\n",
            "    greet(name = \"Alice\", age = 1)\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "all named args provided: {diags:?}");
}

#[test]
fn trailing_lambda_skipped() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun run(action: () -> Unit) {}\n",
            "fun main() {\n",
            "    run { println(\"hi\") }\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "trailing lambda should be skipped: {diags:?}"
    );
}

#[test]
fn vararg_skipped() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun log(vararg messages: String) {}\n",
            "fun main() {\n",
            "    log(\"a\", \"b\", \"c\", \"d\")\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "vararg should be skipped: {diags:?}");
}

#[test]
fn cross_file_resolution() {
    let (uri, idx, src) = setup(&[
        ("/lib.kt", "fun helper(x: Int, y: Int, z: Int) {}\n"),
        (
            "/main.kt",
            concat!("fun main() {\n", "    helper(1)\n", "}\n",),
        ),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "cross-file: {diags:?}");
    assert!(
        diags[0].message.contains("expected 3"),
        "msg: {}",
        diags[0].message
    );
}

#[test]
fn zero_args_when_params_required() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun process(data: String) {}\n",
            "fun main() {\n",
            "    process()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "zero args: {diags:?}");
    assert!(
        diags[0].message.contains("found 0"),
        "msg: {}",
        diags[0].message
    );
}

#[test]
fn no_params_no_args_ok() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!("fun noop() {}\n", "fun main() {\n", "    noop()\n", "}\n",),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "no params, no args: {diags:?}");
}

#[test]
fn complex_default_value_detected() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun config(timeout: Int = 30, retries: Int = 3, label: String) {}\n",
            "fun main() {\n",
            "    config(label = \"x\")\n",
            "}\n",
        ),
    )]);
    // Named arg → skipped
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "named arg with defaults: {diags:?}");
}

#[test]
fn function_type_default_not_confused() {
    // `=` inside a function type like `(Int) -> String` should not be treated as default
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun transform(mapper: (Int) -> String, fallback: String) {}\n",
            "fun main() {\n",
            "    transform({ it.toString() }, \"none\")\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "function type param not confused: {diags:?}"
    );
}

#[test]
fn diagnostic_on_correct_call_not_next_line() {
    let src = concat!(
        "class FamilyAccount(val members: List<String>)\n",
        "fun loadData(account: FamilyAccount, refresh: Boolean) {}\n",
        "suspend fun test() {\n",
        "    loadData(FamilyAccount(listOf()))\n",
        "    return withContext(ioDispatcher) {\n",
        "    }\n",
        "}\n",
    );
    let (uri, idx, _) = setup(&[("/a.kt", src)]);
    let diags = run_diagnostics(&idx, &uri, src);
    // loadData gets 1 arg, expects 2 → diagnostic
    // withContext has trailing lambda → skipped
    assert_eq!(
        diags.len(),
        1,
        "should be exactly one diagnostic: {diags:?}"
    );
    assert!(
        diags[0].message.contains("expected 2"),
        "should expect 2 args: {}",
        diags[0].message
    );
    // Diagnostic must be on line 3 (loadData), not line 4 (withContext)
    assert_eq!(
        diags[0].range.start.line, 3,
        "diagnostic should be on loadData line, got line {}",
        diags[0].range.start.line
    );
}

#[test]
fn test_file_functions_excluded_from_resolution() {
    let idx = Indexer::new();

    let test_uri = uri("/src/test/kotlin/MyTest.kt");
    idx.index_content(&test_uri, "fun loadData() { /* test helper */ }\n");

    let main_uri = uri("/src/main/kotlin/Main.kt");
    let main_src = concat!(
        "fun loadData(account: String, refresh: Boolean) {}\n",
        "fun caller() {\n",
        "    loadData()\n",
        "}\n",
    );
    idx.index_content(&main_uri, main_src);

    let diags = run_diagnostics(&idx, &main_uri, main_src);
    assert_eq!(
        diags.len(),
        1,
        "test file overload should be excluded: {diags:?}"
    );
    assert!(
        diags[0].message.contains("expected 2"),
        "should see production signature: {}",
        diags[0].message
    );
}

#[test]
fn no_stale_diagnostic_after_deleting_bad_call() {
    let idx = Indexer::new();

    let lib_uri = uri("/lib.kt");
    let lib_src = "fun loadData(account: String, refresh: Boolean) {}\n";
    idx.index_content(&lib_uri, lib_src);

    let main_uri = uri("/main.kt");

    // Step 1: file has the bad call
    let src_before = concat!(
        "suspend fun test() {\n",
        "    loadData()\n",
        "    withContext(ioDispatcher) {\n",
        "        doWork()\n",
        "    }\n",
        "}\n",
    );
    idx.index_content(&main_uri, src_before);
    let diags = run_diagnostics(&idx, &main_uri, src_before);
    assert_eq!(diags.len(), 1, "before deletion: {diags:?}");
    assert!(
        diags[0].message.contains("expected 2"),
        "before: {}",
        diags[0].message
    );

    // Step 2: user deletes loadData() line
    let src_after = concat!(
        "suspend fun test() {\n",
        "    withContext(ioDispatcher) {\n",
        "        doWork()\n",
        "    }\n",
        "}\n",
    );
    idx.index_content(&main_uri, src_after);
    let diags = run_diagnostics(&idx, &main_uri, src_after);
    assert!(
        diags.is_empty(),
        "after deletion, no diagnostic should remain: {diags:?}"
    );
}

#[test]
fn no_false_diagnostic_on_incomplete_trailing_lambda() {
    let idx = Indexer::new();

    let lib_uri = uri("/lib.kt");
    idx.index_content(
        &lib_uri,
        "suspend fun <T> withContext(context: CoroutineContext, block: suspend CoroutineScope.() -> T): T {}\n",
    );

    let main_uri = uri("/a.kt");
    let src = concat!(
        "override suspend fun loadData(args: FamilyAccount): TipsResult {\n",
        "    loadData()\n",
        "    return withContext(ioDispatcher) {\n",
    );
    idx.index_content(&main_uri, src);

    let diags = run_diagnostics(&idx, &main_uri, src);
    for d in &diags {
        eprintln!(
            "  diag line={} col={}: {}",
            d.range.start.line, d.range.start.character, d.message
        );
    }
    // withContext should NOT be flagged (trailing lambda, even if unclosed)
    let flagged_lines: Vec<_> = diags.iter().map(|d| d.range.start.line).collect();
    assert!(
        !flagged_lines.contains(&2),
        "withContext on line 2 should not be flagged: {diags:?}"
    );
}

#[test]
fn no_diagnostic_on_withcontext_after_deletion() {
    // After user deletes a bad call, withContext(x) { ... } must not be flagged.
    let idx = Indexer::new();

    let lib_uri = uri("/lib.kt");
    idx.index_content(
        &lib_uri,
        "suspend fun <T> withContext(context: CoroutineContext, block: suspend CoroutineScope.() -> T): T {}\n",
    );

    let def_uri = uri("/def.kt");
    idx.index_content(
        &def_uri,
        "fun loadData(args: String, refresh: Boolean) {}\n",
    );

    let main_uri = uri("/main.kt");

    // Step 1: verify the "after deletion" state has no diagnostics
    let src_after = concat!(
        "override suspend fun doWork(): String {\n",
        "    return withContext(ioDispatcher) {\n",
        "        \"result\"\n",
        "    }\n",
        "}\n",
    );
    idx.index_content(&main_uri, src_after);
    let diags = run_diagnostics(&idx, &main_uri, src_after);
    for d in &diags {
        eprintln!(
            "  UNEXPECTED diag line={} col={}: {}",
            d.range.start.line, d.range.start.character, d.message
        );
    }
    assert!(
        diags.is_empty(),
        "withContext with trailing lambda should not be flagged: {diags:?}"
    );
}

#[test]
fn no_false_diagnostic_on_let_lambda_chain() {
    let src = concat!(
        "fun toMillis(days: Int): Long = 0L\n",
        "class Foo {\n",
        "  var familyCreationDate: Long? = null\n",
        "  fun test() {\n",
        "    val result = familyCreationDate\n",
        "      ?.let {\n",
        "        if (it == 0L) System.currentTimeMillis().also {\n",
        "          familyCreationDate = it\n",
        "        } else it\n",
        "      }\n",
        "      ?.let { System.currentTimeMillis() - it }\n",
        "      ?.let { it > toMillis(2) } ?: false\n",
        "  }\n",
        "}\n",
    );
    let (uri, idx, _) = setup(&[("/chain.kt", src)]);
    let diags = run_diagnostics(&idx, &uri, src);
    for d in &diags {
        eprintln!(
            "  UNEXPECTED diag line={} col={}: {}",
            d.range.start.line, d.range.start.character, d.message
        );
    }
    assert!(
        diags.is_empty(),
        "let/also lambda chains should not produce diagnostics: {diags:?}"
    );
}

#[test]
fn trailing_lambda_with_args_skipped() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun <T> withContext(context: Any, block: suspend () -> T): T = TODO()\n",
            "fun launch(context: Any, block: suspend () -> Unit): Unit = TODO()\n",
            "fun observe(owner: Any, observer: (String) -> Unit) {}\n",
            "class Vm {\n",
            "    fun load() {\n",
            "        withContext(dispatcher) {\n",
            "            doSomething()\n",
            "        }\n",
            "        launch(dispatcher) {\n",
            "            doSomething()\n",
            "        }\n",
            "        observe(this) { value ->\n",
            "            doSomething()\n",
            "        }\n",
            "    }\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "trailing lambda with preceding args should be skipped: {diags:?}"
    );
}

#[test]
fn trailing_lambda_same_line_three_args_skipped() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun <T> loadProduct(key: String, data: T, mapper: (T) -> Any) {}\n",
            "fun getData(): String = \"\"\n",
            "fun main() {\n",
            "    loadProduct(\"A\", getData()) { it.toString() }\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "trailing lambda same line should be skipped: {diags:?}"
    );
}

#[test]
fn same_file_diagnostic_retype_cycle() {
    // Regression: type loadData() → delete → retype should still emit diagnostic.
    // Tests that index_content + call_arg_diagnostics work correctly on re-index
    // when content returns to a previously-seen state.
    let idx = Indexer::new();
    let u = uri("/a.kt");

    let with_call = concat!(
        "fun loadData(arg: String) {}\n",
        "fun main() {\n",
        "    loadData()\n",
        "}\n",
    );
    let without_call = concat!("fun loadData(arg: String) {}\n", "fun main() {\n", "}\n",);

    // Step 1: type the call → should get diagnostic
    idx.index_content(&u, with_call);
    let diags1 = run_diagnostics(&idx, &u, with_call);
    assert!(!diags1.is_empty(), "step1: expected diagnostic, got none");

    // Step 2: delete the call → should be clear
    idx.index_content(&u, without_call);
    let diags2 = run_diagnostics(&idx, &u, without_call);
    assert!(
        diags2.is_empty(),
        "step2: expected no diagnostic after delete"
    );

    // Step 3: retype the call → should get diagnostic again
    idx.index_content(&u, with_call);
    let diags3 = run_diagnostics(&idx, &u, with_call);
    assert!(
        !diags3.is_empty(),
        "step3: expected diagnostic after retype, got none"
    );
}

#[test]
fn same_file_diagnostic_with_live_lines_cycle() {
    // Mirrors production: set_live_lines called before index_content.
    // Tests that even when index_content returns None (hash-cache hit),
    // call_arg_diagnostics still fires using diag_indexer.files.
    let idx = Indexer::new();
    let u = uri("/a.kt");

    let with_call = concat!(
        "fun loadData(arg: String) {}\n",
        "fun main() {\n",
        "    loadData()\n",
        "}\n",
    );
    let without_call = concat!("fun loadData(arg: String) {}\n", "fun main() {\n", "}\n",);

    // Step 1: type the call (production: set_live_lines THEN index_content)
    idx.set_live_lines(&u, with_call);
    idx.index_content(&u, with_call);
    let diags1 = run_diagnostics(&idx, &u, with_call);
    assert!(!diags1.is_empty(), "step1: expected diagnostic");

    // Step 2: delete call
    idx.set_live_lines(&u, without_call);
    idx.index_content(&u, without_call);
    let diags2 = run_diagnostics(&idx, &u, without_call);
    assert!(diags2.is_empty(), "step2: expected no diagnostic");

    // Step 3: simulate save from disk (re-indexes H1) — mimics handle_file_saved
    // racing with or preceding the debounce task
    idx.index_content(&u, with_call); // ← now content_hash = H1 again

    // Step 4: retype — index_content called with H1, but content_hash already H1
    // (from step 3), so it returns None. Diagnostics must still work via files.
    idx.set_live_lines(&u, with_call);
    let none_result = idx.index_content(&u, with_call); // should be None = hash-cache hit
                                                        // We still need diagnostics to fire using stale diag_indexer.files (set in step 3)
    let diags3 = if none_result.is_none() {
        // Production code: use diag_indexer.files when index_content returned None
        let doc = crate::indexer::live_tree::parse_live(with_call, tree_sitter_kotlin::language())
            .unwrap();
        call_arg_diagnostics(&idx, &u, &doc)
    } else {
        run_diagnostics(&idx, &u, with_call)
    };
    assert!(
        !diags3.is_empty(),
        "step4: expected diagnostic after retype (hash-cache hit path)"
    );
}

/// Regression: method call with wrong args inside a coroutine lambda (withContext)
/// should still produce a diagnostic. The function is defined in the same class.
#[test]
fn method_call_wrong_args_inside_coroutine_lambda() {
    let (uri, idx, src) = setup(&[(
        "/Interactor.kt",
        concat!(
            "class Interactor {\n",
            "  suspend fun loadData(args: String): String {\n",
            "    return withContext(Dispatchers.IO) {\n",
            "      loadData()\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        !diags.is_empty(),
        "expected diagnostic for loadData() inside withContext lambda: {diags:?}"
    );
}

/// Regression: override with base class also indexed — both define loadData with 1 arg.
/// The override in the derived class should still produce a diagnostic for loadData().
#[test]
fn method_call_wrong_args_with_base_class_indexed() {
    let idx = Indexer::new();
    let base_uri = uri("/LoadingInteractor.kt");
    let base_src = concat!(
        "abstract class LoadingInteractor<Args : Any, Result : Any> {\n",
        "  abstract suspend fun loadData(args: Args): Result\n",
        "}\n",
    );
    idx.index_content(&base_uri, base_src);
    idx.store_live_tree(&base_uri, base_src);

    let impl_src = concat!(
        "class ShowChildNewTipsInteractor : LoadingInteractor<String, String>() {\n",
        "  override suspend fun loadData(args: String): String {\n",
        "    return withContext(ioDispatcher) {\n",
        "      loadData()\n", // ← 0 args — should fire diagnostic
        "      \"\"\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let impl_uri = uri("/ShowChildNewTipsInteractor.kt");
    idx.index_content(&impl_uri, impl_src);
    idx.store_live_tree(&impl_uri, impl_src);

    let doc = parse_live(impl_src, tree_sitter_kotlin::language()).unwrap();
    let diags = call_arg_diagnostics(&idx, &impl_uri, &doc);
    assert!(
        !diags.is_empty(),
        "expected diagnostic for loadData() with base class indexed: {diags:?}"
    );
}

/// Regression: workspace has many same-name functions with different arities.
/// Unqualified call should be resolved against the CURRENT FILE only,
/// not skipped because of 945+ same-name functions in the workspace.
#[test]
fn method_call_same_file_wins_over_workspace_overloads() {
    let idx = Indexer::new();

    // Simulate 3 other files with different arities for "loadData"
    for i in 0..3 {
        let other_uri = uri(&format!("/Other{i}.kt"));
        // Each has loadData with a different number of args
        let other_src = format!(
            "class Other{i} {{\n  fun loadData({}) {{}}\n}}\n",
            std::iter::repeat_n("x: Int", i + 2)
                .collect::<Vec<_>>()
                .join(", ")
        );
        idx.index_content(&other_uri, &other_src);
    }

    // Current file has loadData with 1 required arg — call with 0 should diagnose
    let src = concat!(
        "class MyClass {\n",
        "  fun loadData(arg: String) {}\n",
        "  fun test() {\n",
        "    loadData()\n",
        "  }\n",
        "}\n",
    );
    let u = uri("/MyClass.kt");
    idx.index_content(&u, src);
    let doc = parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let diags = call_arg_diagnostics(&idx, &u, &doc);
    assert!(
        !diags.is_empty(),
        "expected diagnostic even with many workspace overloads: {diags:?}"
    );
}

/// Regression: same as above but with multiple chained lambdas inside withContext,
/// mirroring the actual production file shape that was showing 0 diagnostics.
#[test]
fn method_call_wrong_args_inside_complex_coroutine_lambda() {
    let (uri, idx, src) = setup(&[(
        "/Interactor.kt",
        concat!(
            "class ShowChildNewTipsInteractor {\n",
            "  sealed interface TipsResult {\n",
            "    data object No : TipsResult\n",
            "  }\n",
            "  override suspend fun loadData(args: String): TipsResult {\n",
            "    return withContext(ioDispatcher) {\n",
            "      loadData()\n", // ← the call under test
            "      val x = settings?.let {\n",
            "        if (it == 0L) System.currentTimeMillis().also {\n",
            "          settings = it\n",
            "        } else it\n",
            "      }?.let { it > 0L } ?: false\n",
            "      TipsResult.No\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        !diags.is_empty(),
        "expected diagnostic for loadData() in complex coroutine context: {diags:?}"
    );
}

#[test]
fn extension_fn_resolved_not_member() {
    // Extension function IMockProvider.loadJSONFromAssets(path: String): T
    // should be resolved correctly for qualified calls, not confused with
    // a member function of a different type (e.g. an unrelated class with
    // the same-named member).
    let src = [
        "class IMockProvider",
        "inline fun <reified T> IMockProvider.loadJSONFromAssets(path: String): T = TODO()",
        // Competitor: same-named member on an unrelated type — must NOT be selected.
        "class Service { fun loadJSONFromAssets(): String = TODO() }",
        "class Foo(private val context: IMockProvider) {",
        "  val result = context.loadJSONFromAssets(\"test\")",
        "}",
    ]
    .join("\n");
    let (uri, idx, src) = setup(&[("/test.kt", &src)]);
    let doc = parse_live(
        &src,
        crate::indexer::live_tree::lang_for_path(uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &uri, &doc);
    assert!(
        diagnostics.is_empty(),
        "extension fn: no diagnostic expected for correct arg count, got: {diagnostics:?}"
    );
}

#[test]
fn extension_fn_wrong_arg_count_detected() {
    // Extension function with 1 param, but call provides 0 args.
    // Also includes a competing 0-arg member on an unrelated type —
    // the resolver must NOT match that member and silently succeed.
    let src = [
        "class IMockProvider",
        "fun IMockProvider.loadJSONFromAssets(path: String): Any = TODO()",
        // Competitor: 0-arg member on an unrelated type — must NOT shadow the extension.
        "class Service { fun loadJSONFromAssets(): String = TODO() }",
        "class Foo(private val context: IMockProvider) {",
        "  val result = context.loadJSONFromAssets()",
        "}",
    ]
    .join("\n");
    let (uri, idx, src) = setup(&[("/test.kt", &src)]);
    let doc = parse_live(
        &src,
        crate::indexer::live_tree::lang_for_path(uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &uri, &doc);
    assert!(
        !diagnostics.is_empty(),
        "expected diagnostic for wrong arg count on extension fn, got: {diagnostics:?}"
    );
    assert!(
        diagnostics[0].message.contains("loadJSONFromAssets"),
        "diagnostic should mention the function name, got: {}",
        diagnostics[0].message
    );
}

#[test]
fn extension_fn_cross_file_resolved() {
    // Extension function defined in a DIFFERENT file than the type.
    // This is the common case (e.g., Retrofit extensions in a JAR).
    //
    // A competing 0-arg member `loadJSONFromAssets()` on the receiver class
    // guards against silently passing when resolution is broken: if the
    // extension is not found (e.g., import filtering bug), only the 0-arg
    // member matches, producing a diagnostic for the 1-arg call.
    let (uri, idx, src) = setup(&[
        (
            "/provider.kt",
            "class IMockProvider {\n    fun loadJSONFromAssets() { /* competing 0-arg member */ }\n}",
        ),
        (
            "/extensions.kt",
            "inline fun <reified T> IMockProvider.loadJSONFromAssets(path: String): T = TODO()",
        ),
        (
            "/usage.kt",
            "class Foo(private val context: IMockProvider) {\n  val result = context.loadJSONFromAssets(\"test\")\n}",
        ),
    ]);
    // Use the usage file for diagnostics
    let usage_uri = uri;
    let doc = parse_live(
        &src,
        crate::indexer::live_tree::lang_for_path(usage_uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &usage_uri, &doc);
    // With both member (0-arg) and extension (1-arg) reachable,
    // resolution returns Overloaded → no diagnostic.
    assert!(
        diagnostics.is_empty(),
        "cross-file extension fn: no diagnostic expected with competing member; got: {diagnostics:?}"
    );
}

#[test]
fn extension_fn_cross_file_overloaded_no_false_positive() {
    // When multiple competing definitions (member + extension) with different
    // arities are both import-reachable, resolution returns Overloaded and no
    // diagnostic fires — even when the call arg count matches neither.
    //
    // The 2-arg member on the receiver class acts as a guard: if the extension
    // is not found (broken resolution), the 2-arg member would produce a
    // diagnostic "expected 2, found 0" — causing this test to fail.
    let (uri, idx, src) = setup(&[
        (
            "/provider.kt",
            "class IMockProvider {\n    fun loadJSONFromAssets(path: String, force: Boolean) { /* competing 2-arg member */ }\n}",
        ),
        (
            "/extensions.kt",
            "fun IMockProvider.loadJSONFromAssets(path: String): Any = TODO()",
        ),
        (
            "/usage.kt",
            "class Foo(private val context: IMockProvider) {\n  val result = context.loadJSONFromAssets()\n}",
        ),
    ]);
    let usage_uri = uri;
    let doc = parse_live(
        &src,
        crate::indexer::live_tree::lang_for_path(usage_uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &usage_uri, &doc);
    // Overloaded resolution suppresses diagnostics; no false positive.
    assert!(
        diagnostics.is_empty(),
        "cross-file extension fn: overloaded resolution should suppress diagnostics; got: {diagnostics:?}"
    );
}

#[test]
fn data_class_copy_no_false_diagnostic() {
    // data class copy() has all params optional — should not produce a diagnostic
    // when called with any number of args (including zero).
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "data class Foo(val x: Int, val y: String)\n",
            "fun test() {\n",
            "    val foo = Foo(1, \"a\")\n",
            "    foo.copy()\n",
            "    foo.copy(x = 2)\n",
            "    foo.copy(x = 2, y = \"b\")\n",
            "}\n",
        ),
    )]);
    let doc = parse_live(
        &src,
        crate::indexer::live_tree::lang_for_path(uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &uri, &doc);
    assert!(
        diagnostics.is_empty(),
        "data class copy() should not produce diagnostics; got: {diagnostics:?}"
    );
}

#[test]
fn data_class_copy_too_many_args_diagnostic() {
    // data class copy() should flag too many arguments
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "data class Foo(val x: Int, val y: String)\n",
            "fun test() {\n",
            "    val foo = Foo(1, \"a\")\n",
            "    foo.copy(x = 2, y = \"b\", z = 3)\n",
            "}\n",
        ),
    )]);
    let doc = parse_live(
        &src,
        crate::indexer::live_tree::lang_for_path(uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &uri, &doc);
    assert!(
        !diagnostics.is_empty(),
        "data class copy() with too many args should produce a diagnostic"
    );
}

#[test]
fn data_class_copy_not_confused_by_jar_copy() {
    // When a JAR has a copy() function with different params, the data class
    // copy() should still be resolved correctly via receiver type matching.
    let idx = Indexer::new();
    // Index a data class file
    let data_class_src = concat!(
        "data class Foo(val x: Int, val y: String)\n",
        "fun test() {\n",
        "    val foo = Foo(1, \"a\")\n",
        "    foo.copy(x = 2)\n",
        "}\n",
    );
    let data_class_uri = Url::parse("file:///test/data_class.kt").unwrap();
    idx.index_content(&data_class_uri, data_class_src);
    idx.store_live_tree(&data_class_uri, data_class_src);

    // Simulate a JAR copy() function being indexed
    let jar_uri = Url::parse("jar:file:///some/jar.jar!/AbstractList.kt").unwrap();
    let jar_symbols = vec![crate::types::SymbolEntry {
        name: "copy".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::FUNCTION,
        visibility: crate::types::Visibility::Public,
        range: Default::default(),
        selection_range: Default::default(),
        detail: "fun copy(element: E): AbstractList<E>".to_owned(),
        params: "element: E".to_owned(),
        param_counts: (1, 1), // 1 required param
        type_params: vec!["E".to_owned()],
        extension_receiver: String::new(),
        extension_receiver_type: String::new(),
        container: Some("AbstractList".to_owned()),
        doc: String::new(),
        trailing_lambda: false,
        deprecated: false,
    }];
    let jar_file_data = std::sync::Arc::new(crate::types::FileData {
        symbols: jar_symbols,
        source_set: crate::types::SourceSet::Library,
        ..Default::default()
    });
    idx.jar_files.insert(jar_uri.to_string(), jar_file_data);
    idx.jar_definitions.insert(
        "copy".to_owned(),
        vec![tower_lsp::lsp_types::Location {
            uri: jar_uri,
            range: Default::default(),
        }],
    );

    // Now run diagnostics on the data class file
    let doc = parse_live(
        data_class_src,
        crate::indexer::live_tree::lang_for_path(data_class_uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &data_class_uri, &doc);
    assert!(
        diagnostics.is_empty(),
        "data class copy() should not be confused by JAR copy(); got: {diagnostics:?}"
    );
}

#[test]
fn data_class_copy_with_cross_file_unrelated_copy_fn() {
    // An unrelated copy() function in another source file should not
    // interfere with data class copy() resolution.
    let (uri, idx, src) = setup(&[
        ("/other.kt", "fun copy() = TODO()"),
        (
            "/a.kt",
            concat!(
                "data class Foo(val x: Int, val y: String)\n",
                "fun test() {\n",
                "    val foo = Foo(1, \"a\")\n",
                "    foo.copy(x = 2)\n",
                "}\n",
            ),
        ),
    ]);
    let doc = parse_live(
        &src,
        crate::indexer::live_tree::lang_for_path(uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &uri, &doc);
    assert!(
        diagnostics.is_empty(),
        "data class copy() should not be confused by unrelated copy() in other file; got: {diagnostics:?}"
    );
}

#[test]
fn copy_inside_custom_receiver_lambda_no_false_positive() {
    // Unqualified copy() inside a custom receiver lambda (not a scope function like apply/run)
    // should not produce false positives when a JAR copy(element: E) with (1, 1) params
    // is "reachable" due to fail-open import checking for JAR files.
    let idx = Indexer::new();

    // Data class in a different package — import for the TYPE exists, but not for `copy`
    let state_src = "package com.app.state\ndata class MyState(val count: Int)\n";
    let state_uri = Url::parse("file:///com/app/state/MyState.kt").unwrap();
    idx.index_content(&state_uri, state_src);

    // Usage file: different package, only imports the type
    let usage_src = concat!(
        "package com.app.reducer\n",
        "import com.app.state.MyState\n",
        "fun MyState.update(block: MyState.() -> MyState) = block(this)\n",
        "fun test() {\n",
        "    val s = MyState(0)\n",
        "    s.update { copy() }\n",
        "}\n",
    );
    let usage_uri = Url::parse("file:///com/app/reducer/Reducer.kt").unwrap();
    idx.index_content(&usage_uri, usage_src);
    idx.store_live_tree(&usage_uri, usage_src);

    // JAR copy(element: E) — 1 required param — would cause a false positive if
    // it were selected as the sole match for the unqualified `copy()` call.
    let jar_uri = Url::parse("jar:file:///some/jar.jar!/AbstractList.kt").unwrap();
    let jar_symbols = vec![crate::types::SymbolEntry {
        name: "copy".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::FUNCTION,
        visibility: crate::types::Visibility::Public,
        range: Default::default(),
        selection_range: Default::default(),
        detail: "fun copy(element: E): AbstractList<E>".to_owned(),
        params: "element: E".to_owned(),
        param_counts: (1, 1),
        type_params: vec!["E".to_owned()],
        extension_receiver: String::new(),
        extension_receiver_type: String::new(),
        container: Some("AbstractList".to_owned()),
        doc: String::new(),
        trailing_lambda: false,
        deprecated: false,
    }];
    let jar_file_data = std::sync::Arc::new(crate::types::FileData {
        symbols: jar_symbols,
        source_set: crate::types::SourceSet::Library,
        ..Default::default()
    });
    idx.jar_files.insert(jar_uri.to_string(), jar_file_data);
    idx.jar_definitions.insert(
        "copy".to_owned(),
        vec![tower_lsp::lsp_types::Location {
            uri: jar_uri,
            range: Default::default(),
        }],
    );

    let doc = parse_live(
        usage_src,
        crate::indexer::live_tree::lang_for_path(usage_uri.path()).unwrap(),
    )
    .unwrap();
    let diagnostics = call_arg_diagnostics(&idx, &usage_uri, &doc);
    assert!(
        diagnostics.is_empty(),
        "copy() inside a custom receiver lambda must not produce false positives; got: {diagnostics:?}"
    );
}

fn jar_symbol(name: &str, detail: &str, container: &str) -> crate::sidecar::SidecarSymbol {
    crate::sidecar::SidecarSymbol {
        name: name.to_owned(),
        kind: "fun".to_owned(),
        container: container.to_owned(),
        detail: detail.to_owned(),
        doc: String::new(),
        type_params: Vec::new(),
        extension_receiver_type: String::new(),
        trailing_lambda: false,
        deprecated: false,
        pkg: String::new(),
        top_level: container.is_empty(),
    }
}

/// Regression (mirrors the real `androidx.compose.foundation.layout.WindowInsets`
/// factory overloads): a multi-arity JAR function resolves to `Overloaded`, so the
/// call-arg diagnostic skips it. Reproduces the nowinandroid `WindowInsets(0,0,0,0)`
/// false positive, which was caused by stale `(0,0)` JAR param counts + the testDemo
/// extension leaking in. Now the JAR overloads carry real arities and win.
#[test]
fn jar_multi_overload_call_is_not_diagnosed() {
    let idx = Indexer::new();
    let symbols = vec![
        jar_symbol(
            "WindowInsets",
            "fun WindowInsets(): WindowInsets",
            "WindowInsetsKt",
        ),
        jar_symbol(
            "WindowInsets",
            "fun WindowInsets(left: Int, top: Int, right: Int, bottom: Int): WindowInsets",
            "WindowInsetsKt",
        ),
        jar_symbol(
            "WindowInsets",
            "fun WindowInsets(left: Dp, top: Dp, right: Dp, bottom: Dp): WindowInsets",
            "WindowInsetsKt",
        ),
    ];
    crate::indexer::jar::populate_from_symbols(
        &idx,
        "/home/test/.gradle/foundation-layout.jar".as_ref(),
        &symbols,
    );
    let src = concat!(
        "class Screen {\n",
        "  fun build() {\n",
        "    val w = WindowInsets(0, 0, 0, 0)\n",
        "  }\n",
        "}\n",
    );
    let u = uri("/Screen.kt");
    idx.index_content(&u, src);
    let doc = parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let diags = call_arg_diagnostics(&idx, &u, &doc);
    assert!(
        diags.iter().all(|d| !d.message.contains("WindowInsets")),
        "overloaded JAR factory must not be diagnosed: {diags:?}"
    );
}

/// A single-overload JAR function whose real declaration has default parameters
/// (e.g. `fun pad(a: Int, b: Int = 0)`) must not produce a "too few args" false
/// positive when called with fewer args. The sidecar `detail` omits defaults, so
/// `required` is clamped to 0 for library symbols — only over-supply is flagged.
#[test]
fn jar_single_overload_with_defaults_allows_fewer_args() {
    let idx = Indexer::new();
    let symbols = vec![jar_symbol(
        "pad",
        "fun pad(a: Int, b: Int): Modifier",
        "PadKt",
    )];
    crate::indexer::jar::populate_from_symbols(
        &idx,
        "/home/test/.gradle/lib.jar".as_ref(),
        &symbols,
    );
    let src = concat!(
        "class Screen {\n",
        "  fun build() {\n",
        "    val m = pad(1)\n",
        "  }\n",
        "}\n",
    );
    let u = uri("/Screen.kt");
    idx.index_content(&u, src);
    let doc = parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let diags = call_arg_diagnostics(&idx, &u, &doc);
    assert!(
        diags.iter().all(|d| !d.message.contains("pad")),
        "library call relying on a default must not be flagged 'too few': {diags:?}"
    );
}

/// Over-supplying arguments to a library function is still flagged — `total`
/// (the parameter-list length) is reliable from the sidecar, only `required` isn't.
#[test]
fn jar_call_with_too_many_args_is_still_diagnosed() {
    let idx = Indexer::new();
    let symbols = vec![jar_symbol(
        "pad",
        "fun pad(a: Int, b: Int): Modifier",
        "PadKt",
    )];
    crate::indexer::jar::populate_from_symbols(
        &idx,
        "/home/test/.gradle/lib.jar".as_ref(),
        &symbols,
    );
    let src = concat!(
        "class Screen {\n",
        "  fun build() {\n",
        "    val m = pad(1, 2, 3)\n",
        "  }\n",
        "}\n",
    );
    let u = uri("/Screen.kt");
    idx.index_content(&u, src);
    let doc = parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let diags = call_arg_diagnostics(&idx, &u, &doc);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("pad") && d.message.contains("at most 2")),
        "over-supplying a library call must be flagged: {diags:?}"
    );
}

/// While JAR indexing is in flight the index is partial, so call-arg diagnostics
/// are suppressed to avoid flashing a false positive against the wrong overload.
/// They resume (and are republished by the actor) once indexing reaches a terminal
/// phase.
#[test]
fn diagnostics_suppressed_while_jars_loading() {
    use crate::indexer::jar_phase::JarPhase;
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "fun greet(name: String, age: Int) {}\n",
            "fun main() {\n",
            "    greet(\"Alice\")\n",
            "}\n",
        ),
    )]);

    *idx.jar_phase.lock().unwrap() = JarPhase::InProgress;
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "diagnostics must be suppressed while JARs load: {diags:?}"
    );

    *idx.jar_phase.lock().unwrap() = JarPhase::Ready { count: 1 };
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        !diags.is_empty(),
        "diagnostics must resume once JAR indexing is done"
    );
}

/// Regression: a qualified call to a *member* method defined in another file
/// (reached via the receiver type, not an import) must be diagnosed. Previously
/// the method's file was rejected by import-reachability because the caller only
/// imports the class, never its methods.
#[test]
fn qualified_member_method_cross_file_is_diagnosed() {
    let (uri, idx, src) = setup(&[
        ("/repo.kt", "package data\ninterface Repo {\n    fun save(id: String)\n}\n"),
        (
            "/main.kt",
            "package app\nimport data.Repo\nclass T(val repo: Repo) {\n    fun go() {\n        repo.save()\n    }\n}\n",
        ),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("save") && d.message.contains("expected 1")),
        "cross-file member method call must be diagnosed: {diags:?}"
    );
}
