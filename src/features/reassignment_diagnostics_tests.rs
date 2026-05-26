use tower_lsp::lsp_types::Url;

use crate::indexer::live_tree::parse_live;
use crate::indexer::Indexer;

use super::reassignment_diagnostics;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn setup(source: &str) -> (Url, Indexer, String) {
    let idx = Indexer::new();
    let u = uri("/test.kt");
    idx.index_content(&u, source);
    idx.store_live_tree(&u, source);
    (u, idx, source.to_string())
}

fn run_diagnostics(
    idx: &Indexer,
    uri: &Url,
    source: &str,
) -> Vec<tower_lsp::lsp_types::Diagnostic> {
    let doc = parse_live(source, tree_sitter_kotlin::language()).unwrap();
    reassignment_diagnostics(idx, uri, &doc)
}

#[test]
fn val_reassignment_is_error() {
    let (uri, idx, src) = setup(concat!(
        "fun test() {\n",
        "    val x = 1\n",
        "    x = 2\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected 1 error: {diags:?}");
    assert_eq!(
        diags[0].message, "Val cannot be reassigned",
        "wrong message: {}",
        diags[0].message
    );
    assert_eq!(
        diags[0].range.start.line, 2,
        "error should be on line 2 (x = 2), got line {}",
        diags[0].range.start.line
    );
}

#[test]
fn var_reassignment_no_error() {
    let (uri, idx, src) = setup(concat!(
        "fun test() {\n",
        "    var x = 1\n",
        "    x = 2\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "var reassignment should not be an error: {diags:?}"
    );
}

#[test]
fn val_init_not_error() {
    let (uri, idx, src) = setup(concat!("fun test() {\n", "    val x = 1\n", "}\n",));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "val initialization should not be an error: {diags:?}"
    );
}

#[test]
fn function_param_reassignment_is_error() {
    let (uri, idx, src) = setup(concat!(
        "fun greet(name: String) {\n",
        "    name = \"Bob\"\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "function param reassignment should be error: {diags:?}"
    );
    assert_eq!(diags[0].message, "Val cannot be reassigned");
}

#[test]
fn lambda_param_reassignment_is_error() {
    let (uri, idx, src) = setup(concat!(
        "fun test() {\n",
        "    listOf(1, 2).forEach { item ->\n",
        "        item = 3\n",
        "    }\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "lambda param reassignment should be error: {diags:?}"
    );
    assert_eq!(diags[0].message, "Val cannot be reassigned");
}

#[test]
fn class_val_param_reassignment_is_error() {
    let (uri, idx, src) = setup(concat!(
        "class User(val name: String) {\n",
        "    fun rename() {\n",
        "        name = \"Alice\"\n",
        "    }\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "class val param reassignment should be error: {diags:?}"
    );
    assert_eq!(diags[0].message, "Val cannot be reassigned");
}

#[test]
fn class_var_param_reassignment_no_error() {
    let (uri, idx, src) = setup(concat!(
        "class User(var name: String) {\n",
        "    fun rename() {\n",
        "        name = \"Alice\"\n",
        "    }\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "class var param reassignment should not be error: {diags:?}"
    );
}

#[test]
fn shadowing_innermost_val_shadows_outer_var() {
    let (uri, idx, src) = setup(concat!(
        "fun test() {\n",
        "    var x = 1\n",
        "    run {\n",
        "        val x = 2\n",
        "        x = 3\n",
        "    }\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "innermost val shadows outer var, reassignment should error: {diags:?}"
    );
    assert_eq!(diags[0].message, "Val cannot be reassigned");
}

#[test]
fn shadowing_innermost_var_shadows_outer_val() {
    let (uri, idx, src) = setup(concat!(
        "fun test() {\n",
        "    val x = 1\n",
        "    run {\n",
        "        var x = 2\n",
        "        x = 3\n",
        "    }\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "innermost var shadows outer val, reassignment should NOT error: {diags:?}"
    );
}

#[test]
fn navigation_assignment_not_handled() {
    let (uri, idx, src) = setup(concat!(
        "class User(var name: String)\n",
        "fun test() {\n",
        "    val user = User(\"Bob\")\n",
        "    user.name = \"Alice\"\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "navigation expression (user.name) should not be handled: {diags:?}"
    );
}

#[test]
fn multiple_val_reassignments_multiple_errors() {
    let (uri, idx, src) = setup(concat!(
        "fun test(a: Int, b: Int) {\n",
        "    val x = 1\n",
        "    a = 10\n",
        "    x = 20\n",
        "    b = 30\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 3, "expected 3 errors (a, x, b), got {diags:?}");
}

#[test]
fn top_level_val_reassignment() {
    let (uri, idx, src) = setup(concat!(
        "val x = 1\n",
        "fun test() {\n",
        "    x = 2\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "top-level val reassignment should be error: {diags:?}"
    );
    assert_eq!(diags[0].message, "Val cannot be reassigned");
}

#[test]
fn top_level_var_reassignment() {
    let (uri, idx, src) = setup(concat!(
        "var x = 1\n",
        "fun test() {\n",
        "    x = 2\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "top-level var reassignment should NOT be error: {diags:?}"
    );
}

#[test]
fn it_lambda_param_reassignment() {
    let (uri, idx, src) = setup(concat!(
        "fun test() {\n",
        "    listOf(1, 2).forEach {\n",
        "        println(it)\n",
        "        it = 3\n",
        "    }\n",
        "}\n",
    ));
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "implicit 'it' lambda param reassignment should be error: {diags:?}"
    );
    assert_eq!(diags[0].message, "Val cannot be reassigned");
}
