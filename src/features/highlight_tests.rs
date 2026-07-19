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
