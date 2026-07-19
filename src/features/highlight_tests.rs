use super::compute_document_highlight;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{Position, Url};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///t{path}")).unwrap()
}

fn indexed_with_live(path: &str, src: &str) -> (Url, Indexer) {
    let u = uri(path);
    let idx = Indexer::new();
    idx.index_content(&u, src);
    idx.store_live_tree(&u, src);
    idx.set_live_lines(&u, src);
    (u, idx)
}

/// Confirmed bug: a local variable named `total` in one function must not
/// highlight an unrelated local ALSO named `total` in a different function.
#[test]
fn highlight_does_not_cross_function_boundaries() {
    let src = "fun a() {\n    val total = 1\n    println(total)\n}\n\
               fun b() {\n    val total = 2\n    println(total)\n}\n";
    let (u, idx) = indexed_with_live("/H.kt", src);
    // cursor on `total` inside fn a() (line 2, the println use).
    let highlights = compute_document_highlight(&u, Position::new(2, 14), &idx).unwrap();
    assert_eq!(
        highlights.len(),
        2,
        "must highlight only fn a()'s two occurrences, not fn b()'s — got {highlights:?}"
    );
    assert!(highlights.iter().all(|h| h.range.start.line <= 2));
}

/// Confirmed bug (task-6 review finding): a top-level function called from
/// MULTIPLE functions in the same file must NOT have its highlight narrowed
/// to the enclosing function of the click site — that silently drops the
/// declaration site and any call sites in other functions.
#[test]
fn highlight_does_not_narrow_top_level_function_used_across_functions() {
    let src = "fun helper() {\n    println(1)\n}\n\
               fun a() {\n    helper()\n}\n\
               fun b() {\n    helper()\n}\n";
    let (u, idx) = indexed_with_live("/H.kt", src);
    // cursor on `helper()` call inside fn a() (line 4, col 6).
    let highlights = compute_document_highlight(&u, Position::new(4, 6), &idx).unwrap();
    assert_eq!(
        highlights.len(),
        3,
        "must highlight declaration + both call sites, not just the click site — got {highlights:?}"
    );
    assert!(
        highlights.iter().any(|h| h.range.start.line == 0),
        "declaration on line 0 must not be dropped — got {highlights:?}"
    );
    assert!(
        highlights.iter().any(|h| h.range.start.line == 7),
        "call site in fn b() on line 7 must not be dropped — got {highlights:?}"
    );
}
