use tower_lsp::lsp_types::Url;

use crate::indexer::live_tree::parse_live;
use crate::indexer::Indexer;

use super::{full_diagnostics, semantic_diagnostics};
use crate::features::fill_when::{when_diagnostics, when_diagnostics_with_doc};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

const ENUM_SRC: &str = "enum class Color {\n    RED, GREEN, BLUE\n}\n";

const PARTIAL_WHEN: &str =
    "fun handle(c: Color) {\n    when (c) {\n        Color.RED -> println(\"red\")\n    }\n}\n";

/// The pull path (`textDocument/diagnostic`) resolves its document via
/// `live_doc_or_parse`, which returns an unstored parse for files that were
/// never opened. The old stored-tree-only `when_diagnostics` goes blind
/// there; the doc-based core must still flag the missing branches.
#[test]
fn when_with_doc_flags_missing_branches_without_a_stored_live_tree() {
    let indexer = Indexer::new();
    indexer.index_content(&uri("/Color.kt"), ENUM_SRC);
    indexer.index_content(&uri("/main.kt"), PARTIAL_WHEN);
    // Deliberately no `store_live_tree`: the file was never opened.
    let target = uri("/main.kt");

    assert!(
        when_diagnostics(&indexer, &target).is_empty(),
        "stored-tree variant has no live tree to read"
    );

    let doc = parse_live(PARTIAL_WHEN, tree_sitter_kotlin::LANGUAGE.into()).unwrap();
    let diags = when_diagnostics_with_doc(&indexer, &target, &doc);
    assert_eq!(diags.len(), 1, "expected one when diagnostic: {diags:?}");
    assert!(
        diags[0].message.contains("GREEN"),
        "message: {}",
        diags[0].message
    );
}

/// The shared coordinator must include the unused-import check: the CLI
/// `diagnose` path previously omitted it and reported a clean bill.
#[test]
fn semantic_diagnostics_flags_an_unused_import() {
    let source =
        "package app\n\nimport com.example.lib.Unused\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let indexer = Indexer::new();
    let target = uri("/main.kt");
    indexer.index_content(&target, source);

    let doc = parse_live(source, tree_sitter_kotlin::LANGUAGE.into()).unwrap();
    let diags = semantic_diagnostics(&indexer, &target, &doc);
    assert!(
        diags
            .iter()
            .any(|diag| diag.message.contains("Unused import")),
        "expected an unused-import diagnostic: {diags:?}"
    );
}

/// Pull diagnostics must serve files the editor never opened (oh-my-pi
/// diagnoses arbitrary workspace paths): index-only state plus an
/// on-demand parse is enough for the pure-CST checks.
#[test]
fn full_diagnostics_serves_a_never_opened_file() {
    let source =
        "package app\n\nimport com.example.lib.Unused\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let indexer = Indexer::new();
    let target = uri("/main.kt");
    indexer.index_content(&target, source);
    // No live lines, no live tree: never opened.

    let diags = full_diagnostics(&indexer, &target);
    assert!(
        diags
            .iter()
            .any(|diag| diag.message.contains("Unused import")),
        "expected an unused-import diagnostic via pull: {diags:?}"
    );
}
