//! Integration tests for `kotlin-lsp diagnose`.
//!
//! These tests invoke the compiled binary with complex Kotlin patterns that
//! previously triggered false positives or missed real errors. Each test:
//!   1. Writes Kotlin fixtures to a temp directory (with workspace.json).
//!   2. Calls `kotlin-lsp diagnose --root <tmpdir> <file>`.
//!   3. Asserts expected diagnostics appear (or don't).

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_kotlin-lsp");

fn write_fixture(dir: &Path, rel_path: &str, content: &str) {
    let full = dir.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, content).unwrap();
}

/// Run `kotlin-lsp diagnose --root <root> <file>` and return stdout lines.
fn diagnose(root: &Path, rel_path: &str) -> Vec<String> {
    let file = root.join(rel_path);
    let out = Command::new(BIN)
        .args(["diagnose", "--root"])
        .arg(root)
        .arg(&file)
        .output()
        .expect("failed to spawn kotlin-lsp");
    assert!(
        out.status.success(),
        "diagnose failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .lines()
        .filter(|l| !l.is_empty() && *l != "No diagnostics.")
        .map(|l| l.to_string())
        .collect()
}

// ── No false positives ───────────────────────────────────────────────────────

/// Trailing lambda should not trigger missing-arg diagnostic.
#[test]
fn no_false_positive_trailing_lambda() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Api.kt",
        concat!(
            "fun <T> runCatching(block: () -> T): Result<T> = TODO()\n",
            "fun test() {\n",
            "    runCatching { 42 }\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Api.kt");
    assert!(
        diags.is_empty(),
        "trailing lambda should not trigger diagnostic; got: {diags:?}"
    );
}

/// Higher-order function params (function types) should not be counted as
/// missing arguments at the call site when passed as trailing lambda.
#[test]
fn no_false_positive_higher_order_trailing_lambda() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Vm.kt",
        concat!(
            "fun setState(reducer: (String) -> String) {}\n",
            "fun test() {\n",
            "    setState { copy -> copy }\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Vm.kt");
    assert!(
        diags.is_empty(),
        "setState with trailing lambda should be clean; got: {diags:?}"
    );
}

/// Named arguments should not trigger missing-arg diagnostic.
#[test]
fn no_false_positive_named_args() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Config.kt",
        concat!(
            "fun configure(host: String, port: Int = 8080, ssl: Boolean = false) {}\n",
            "fun test() {\n",
            "    configure(host = \"localhost\")\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Config.kt");
    assert!(
        diags.is_empty(),
        "named args should skip diagnostic; got: {diags:?}"
    );
}

/// Default parameters — calling with fewer than total but >= required.
#[test]
fn no_false_positive_default_params() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Defaults.kt",
        concat!(
            "fun greet(name: String, greeting: String = \"Hello\", punctuation: String = \"!\") {}\n",
            "fun test() {\n",
            "    greet(\"World\")\n",
            "    greet(\"World\", \"Hi\")\n",
            "    greet(\"World\", \"Hi\", \".\")\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Defaults.kt");
    assert!(
        diags.is_empty(),
        "default params should allow fewer args; got: {diags:?}"
    );
}

/// Extension function with CoroutineContext.cancel(cause) pattern —
/// default parameter in extension function should not false-positive.
#[test]
fn no_false_positive_extension_fun_default_param() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Coroutines.kt",
        concat!(
            "class CancellationException(msg: String)\n",
            "class CoroutineContext\n",
            "fun CoroutineContext.cancel(cause: CancellationException? = null) {}\n",
            "fun test(ctx: CoroutineContext) {\n",
            "    ctx.cancel()\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Coroutines.kt");
    assert!(
        diags.is_empty(),
        "extension fun with default param should be clean; got: {diags:?}"
    );
}

/// Zero-arg function called with zero args should be clean.
#[test]
fn no_false_positive_zero_arg_function() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Zero.kt",
        concat!(
            "fun refresh() {}\n",
            "fun test() {\n",
            "    refresh()\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Zero.kt");
    assert!(
        diags.is_empty(),
        "zero-arg function should be clean; got: {diags:?}"
    );
}

/// Complex generic function type params should not confuse counting.
/// `copy(products = products.plus(key to map(result)).toImmutableMap())`
#[test]
fn no_false_positive_complex_nested_calls() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Vm.kt",
        concat!(
            "fun <T> setState(reducer: T.() -> T) {}\n",
            "data class State(val products: Map<String, List<String>>)\n",
            "fun test() {\n",
            "    setState { copy(products = mapOf(\"a\" to listOf(\"b\"))) }\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Vm.kt");
    assert!(
        diags.is_empty(),
        "nested calls inside trailing lambda should be clean; got: {diags:?}"
    );
}

/// Overloaded functions should not trigger diagnostic (ambiguous arity).
#[test]
fn no_false_positive_overloaded_functions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Overload.kt",
        concat!(
            "fun show(message: String) {}\n",
            "fun show(message: String, duration: Int) {}\n",
            "fun test() {\n",
            "    show(\"hello\")\n",
            "    show(\"hello\", 3000)\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Overload.kt");
    assert!(
        diags.is_empty(),
        "overloaded functions should be skipped; got: {diags:?}"
    );
}

/// Vararg functions should not trigger diagnostic.
#[test]
fn no_false_positive_vararg() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Vararg.kt",
        concat!(
            "fun listOf(vararg items: String): List<String> = TODO()\n",
            "fun test() {\n",
            "    listOf(\"a\", \"b\", \"c\", \"d\")\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Vararg.kt");
    assert!(diags.is_empty(), "vararg should be skipped; got: {diags:?}");
}

// ── True positives ───────────────────────────────────────────────────────────

/// Too few arguments should be detected.
#[test]
fn true_positive_too_few_args() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Missing.kt",
        concat!(
            "fun connect(host: String, port: Int) {}\n",
            "fun test() {\n",
            "    connect(\"localhost\")\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Missing.kt");
    assert!(!diags.is_empty(), "should detect missing argument");
    assert!(
        diags[0].contains("expected 2") && diags[0].contains("found 1"),
        "message should mention expected/found; got: {:?}",
        diags[0]
    );
}

/// Too many arguments should be detected.
#[test]
fn true_positive_too_many_args() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/TooMany.kt",
        concat!(
            "fun ping(host: String) {}\n",
            "fun test() {\n",
            "    ping(\"a\", \"b\")\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/TooMany.kt");
    assert!(!diags.is_empty(), "should detect extra argument");
    assert!(
        diags[0].contains("expected at most 1") && diags[0].contains("found 2"),
        "message should mention expected/found; got: {:?}",
        diags[0]
    );
}

/// Cross-file: function defined in another file, called with wrong arity.
#[test]
fn true_positive_cross_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Api.kt",
        "fun fetchUser(id: String, token: String): String = TODO()\n",
    );
    write_fixture(
        root,
        "src/Main.kt",
        concat!("fun main() {\n", "    fetchUser(\"123\")\n", "}\n",),
    );
    let diags = diagnose(root, "src/Main.kt");
    assert!(
        !diags.is_empty(),
        "cross-file missing arg should be detected"
    );
    assert!(
        diags[0].contains("fetchUser") && diags[0].contains("expected 2"),
        "should name function and expected count; got: {:?}",
        diags[0]
    );
}

// ── Complex patterns (regression cases) ──────────────────────────────────────

/// @JvmStatic annotation on same line as fun should not confuse param parsing.
#[test]
fn no_false_positive_jvm_static_same_line() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Companion.kt",
        concat!(
            "class Factory {\n",
            "    companion object {\n",
            "        @JvmStatic fun create(name: String): Factory = TODO()\n",
            "    }\n",
            "}\n",
            "fun test() {\n",
            "    Factory.create(\"test\")\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Companion.kt");
    assert!(
        diags.is_empty(),
        "@JvmStatic same-line should not confuse params; got: {diags:?}"
    );
}

/// Constructor with default params — data class pattern.
#[test]
fn no_false_positive_constructor_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Model.kt",
        concat!(
            "data class Config(\n",
            "    val host: String,\n",
            "    val port: Int = 8080,\n",
            "    val ssl: Boolean = false,\n",
            "    val timeout: Long = 5000L\n",
            ")\n",
            "fun test() {\n",
            "    Config(\"localhost\")\n",
            "    Config(\"localhost\", 9090)\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Model.kt");
    assert!(
        diags.is_empty(),
        "constructor with defaults should allow partial args; got: {diags:?}"
    );
}

/// Multiple chained scope functions should not interfere with each other.
#[test]
fn no_false_positive_chained_scope_functions() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Chain.kt",
        concat!(
            "fun <T> T.also(block: (T) -> Unit): T = TODO()\n",
            "fun <T, R> T.let(block: (T) -> R): R = TODO()\n",
            "fun test(value: String?) {\n",
            "    value?.let { it.length }?.also { println(it) }\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Chain.kt");
    assert!(
        diags.is_empty(),
        "chained scope functions should be clean; got: {diags:?}"
    );
}

/// Function type parameter with complex generics should not break counting.
#[test]
fn no_false_positive_function_type_param() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(
        root,
        "src/Generic.kt",
        concat!(
            "fun <T> transform(input: T, mapper: (T) -> List<Map<String, T>>): List<Map<String, T>> = TODO()\n",
            "fun test() {\n",
            "    transform(\"hello\") { listOf(mapOf(\"key\" to it)) }\n",
            "}\n",
        ),
    );
    let diags = diagnose(root, "src/Generic.kt");
    assert!(
        diags.is_empty(),
        "complex generic function type should be handled; got: {diags:?}"
    );
}
