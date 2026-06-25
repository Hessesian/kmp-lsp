use tower_lsp::lsp_types::Url;

use crate::indexer::live_tree::parse_live;
use crate::indexer::Indexer;

use super::nullable_dot_call_diagnostics;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn setup(sources: &[(&str, &str)]) -> (Url, Indexer, String) {
    let idx = Indexer::new();
    let mut last_uri = uri("/test.kt");
    let mut last_src = String::new();
    for (path, src) in sources {
        let file_uri = uri(path);
        idx.index_content(&file_uri, src);
        idx.store_live_tree(&file_uri, src);
        last_uri = file_uri;
        last_src = (*src).to_string();
    }
    (last_uri, idx, last_src)
}

fn run_diagnostics(
    idx: &Indexer,
    uri: &Url,
    source: &str,
) -> Vec<tower_lsp::lsp_types::Diagnostic> {
    let doc = parse_live(source, tree_sitter_kotlin::language()).unwrap();
    nullable_dot_call_diagnostics(idx, uri, &doc)
}

#[test]
fn member_access_on_nullable_receiver_without_safe_call_warns() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("loadData"));
}

#[test]
fn member_access_on_nullable_receiver_with_safe_call_is_clean() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo?.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "safe call should not warn: {diags:?}");
}

#[test]
fn member_access_on_nullable_receiver_with_non_null_assertion_is_clean() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo!!.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "!! assertion should not warn: {diags:?}");
}

#[test]
fn member_access_on_non_nullable_receiver_is_clean() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository) {\n",
            "    repo.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "non-nullable receiver should not warn: {diags:?}"
    );
}

#[test]
fn extension_on_non_nullable_receiver_warns_when_called_on_nullable() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository\n",
            "fun Repository.loadData() {}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("loadData"));
}

#[test]
fn extension_on_nullable_receiver_is_clean() {
    // `fun Repository?.loadDataOrDefault()` is itself safe to call on a
    // nullable receiver without `?.` — matches Kotlin's `String?.isNullOrEmpty()`.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository\n",
            "fun Repository?.loadDataOrDefault() {}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.loadDataOrDefault()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "extension declared on a nullable receiver should not warn: {diags:?}"
    );
}

#[test]
fn unresolved_member_on_nullable_receiver_is_skipped() {
    // Neither a member nor a known extension — avoid guessing.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.unknownMethod()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "unresolved member should not warn: {diags:?}"
    );
}

#[test]
fn chained_receiver_is_skipped() {
    // `a.b.c` — multi-level chains are out of scope for this MVP.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Inner {\n",
            "    fun loadData() {}\n",
            "}\n",
            "class Outer {\n",
            "    val inner: Inner? = null\n",
            "}\n",
            "fun caller(outer: Outer) {\n",
            "    outer.inner.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "chained receiver should be skipped: {diags:?}"
    );
}
