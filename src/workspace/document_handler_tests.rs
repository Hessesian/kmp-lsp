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
fn promote_file_imports_attempts_every_import_not_just_the_first_five() {
    // Regression: a hard cap of MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION (5)
    // shared across the whole import list permanently starved every cold
    // import beyond the first five — and file-open import promotion is the
    // designated safety net the zero-IPC inference sites rely on ("uncached
    // JARs are covered by file-open import promotion"). A real Compose file
    // imports 20-50 names; with a partially wiped disk cache, `padding` et
    // al. never materialized and chained-call completion
    // (`Modifier.padding().padd…`) returned nothing. didOpen promotion runs
    // inside spawn_blocking, not on a user-facing request, so it is NOT
    // budget-capped: every explicitly imported name must get a real attempt.
    let idx = Indexer::new();
    let import_names = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf",
    ];
    let mut jar_ids = Vec::new();
    for name in import_names {
        let jar_id = idx.jar_table.intern(&format!("/fake/{name}.jar"));
        idx.jar_bare_names
            .entry(name.to_owned())
            .or_default()
            .push(jar_id);
        jar_ids.push(jar_id);
    }
    let imports: String = import_names
        .iter()
        .map(|name| format!("import lib.{name}\n"))
        .collect();
    let file_uri = uri("/ManyImports.kt");
    idx.index_content(&file_uri, &format!("{imports}\nfun Screen() {{}}"));

    super::promote_file_imports(&idx, &file_uri);

    for (name, jar_id) in import_names.iter().zip(&jar_ids) {
        assert!(
            idx.materialization_failed.contains(jar_id) || idx.materialized.contains(jar_id),
            "import `{name}` never got a materialization attempt — the import \
             list must not be budget-starved"
        );
    }
}

#[tokio::test]
async fn jar_ready_republish_promotes_imports_of_already_open_files() {
    // Cold-start ordering gap: helix opens the file IMMEDIATELY, so didOpen's
    // import promotion runs against an EMPTY Tier-1 manifest (the jar scan
    // hasn't produced jar_bare_names yet), finds no candidates, and never
    // runs again — the file's imports stay unmaterialized for the whole
    // session and chained-call completion silently returns nothing. The
    // republish pass that fires when the jar scan completes must re-attempt
    // import promotion for every open file, now that Tier-1 exists.
    let indexer = Arc::new(Indexer::new());
    let handler = DocumentHandler::new(Arc::clone(&indexer), None);
    let file_uri = Url::parse("file:///app/Screen.kt").unwrap();
    let content = "import lib.padding\n\nfun screen() {}";
    indexer.index_content(&file_uri, content);
    indexer.store_live_tree(&file_uri, content);

    // Tier-1 manifest arrives only AFTER the file was opened.
    let jar_id = indexer.jar_table.intern("/fake/foundation.jar");
    indexer
        .jar_bare_names
        .entry("padding".to_owned())
        .or_default()
        .push(jar_id);

    handler.republish_open_file_diagnostics();

    // The republish work runs on spawned tasks — poll for the attempt.
    let mut attempted = false;
    for _ in 0..200 {
        if indexer.materialization_failed.contains(&jar_id)
            || indexer.materialized.contains(&jar_id)
        {
            attempted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        attempted,
        "when the jar scan completes, open files' imports must get a \
         (re-)promotion attempt — didOpen ran before Tier-1 existed"
    );
}

#[test]
fn promote_file_imports_no_op_for_unindexed_uri() {
    let idx = Indexer::new();
    let file_uri = uri("/NeverOpened.kt");

    // Must not panic when the URI has no FileData yet.
    super::promote_file_imports(&idx, &file_uri);
}
