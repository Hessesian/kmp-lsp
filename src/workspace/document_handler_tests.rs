use std::sync::Arc;

use tower_lsp::lsp_types::Url;

use crate::indexer::Indexer;

use super::DocumentHandler;
use crate::workspace::file_change_handler::FileChangeHandler;

/// Build a `file://` URL rooted under the system temp directory.
fn uri(name: &str) -> Url {
    let path = std::env::temp_dir().join(name.trim_start_matches('/'));
    Url::from_file_path(path).unwrap()
}

#[tokio::test]
async fn handle_file_closed_clears_live_document_state() {
    let indexer = Arc::new(Indexer::new());
    let handler = DocumentHandler::new(Arc::clone(&indexer), None);
    let mut file_change_handler = FileChangeHandler::new(Arc::clone(&indexer), None);
    let uri = Url::parse("file:///workspace/Main.kt").unwrap();
    let content = "fun main() = Unit";

    indexer.set_live_lines(&uri, content);
    indexer.store_live_tree(&uri, content);

    handler
        .handle_file_closed(&mut file_change_handler, uri.clone())
        .await;

    assert!(!indexer.live_lines.contains_key(uri.as_str()));
    assert!(indexer.live_doc(&uri).is_none());
}

// ── promote_file_imports (Task 10) ──────────────────────────────────────────

#[test]
fn opening_a_file_eagerly_promotes_its_own_imports() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/fake/compose.jar");
    idx.jar_bare_names
        .entry("Column".to_owned())
        .or_default()
        .push(jar_id);
    let file_uri = uri("/Screen.kt");
    idx.index_content(
        &file_uri,
        "import androidx.compose.foundation.layout.Column\n\nfun Screen() { Column {} }",
    );

    super::promote_file_imports(&idx, &file_uri);

    assert!(
        idx.materialization_failed.contains(&jar_id) || idx.materialized.contains(&jar_id),
        "opening a file must eagerly attempt materialization for every JAR \
         its own ImportEntry list references, before any diagnostics pass runs"
    );
}

#[test]
fn promote_file_imports_skips_star_imports() {
    // Wildcard imports are deferred to v2 (no package-keyed Tier-1 yet) —
    // promoting them here would be a silent no-op at best, and at worst
    // would require iterating every bare name in the package, which is not
    // what a star import's ImportEntry carries. Confirm the star entry is
    // skipped rather than passed to `ensure_jar_materialized`, by proving a
    // jar registered only under the star import's local_name ("*") is left
    // untouched.
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/fake/star.jar");
    idx.jar_bare_names
        .entry("*".to_owned())
        .or_default()
        .push(jar_id);
    let file_uri = uri("/Screen.kt");
    idx.index_content(
        &file_uri,
        "import androidx.compose.foundation.layout.*\n\nfun Screen() {}",
    );

    super::promote_file_imports(&idx, &file_uri);

    assert!(
        !idx.materialized.contains(&jar_id) && !idx.materialization_failed.contains(&jar_id),
        "a star import's local_name (\"*\") must never be passed to \
         ensure_jar_materialized"
    );
}

#[test]
fn promote_file_imports_no_op_for_unindexed_uri() {
    let idx = Indexer::new();
    let file_uri = uri("/NeverOpened.kt");

    // Must not panic when the URI has no FileData yet.
    super::promote_file_imports(&idx, &file_uri);
}
