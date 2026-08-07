use super::{cache_hit, completion_cache_key, param_names_from_sig, split_prefix, store_in_cache};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{CompletionItem, Url};

/// Regression for a TOCTOU race between a completion request and a
/// concurrent, non-JAR file reindex sharing the single-slot `last_completion`
/// cache: `run_completions` captures `completion_epoch` at the START of the
/// request (before computing its result); `index_content` (the debounced
/// live-edit reindex path) clears `last_completion` when content changes but
/// — before this fix — never bumped `completion_epoch`, only JAR-population
/// paths did (`Indexer::invalidate_completion_cache`). So a request that
/// started computing against stale (pre-edit) data, and only finishes after
/// a concurrent reindex has already cleared the cache, would pass
/// `store_in_cache`'s epoch check (unchanged) and re-populate the
/// just-cleared cache with its stale result — exactly the shape of the
/// "newly added member doesn't show up" symptom reported from live,
/// keystroke-interleaved editing (not reproducible via a single sequential
/// request well after a single finished reindex).
///
/// This test reproduces the race SHAPE directly (not just sequential
/// store-then-fetch, which can't tell "invalidated correctly" apart from
/// "invalidated then reclobbered"): capture the epoch as of before an edit,
/// perform the edit (a real reindex), then attempt to store a result computed
/// under the pre-edit epoch — the store must be rejected.
#[test]
fn store_in_cache_rejects_stale_write_racing_a_concurrent_reindex() {
    let indexer = Indexer::new();
    let uri = Url::parse("file:///test/Race.kt").unwrap();
    indexer.index_content(&uri, "class Widget {\n    fun existing() {}\n}\n");

    // Simulate the epoch a completion request captures at its own start,
    // before it begins computing (mirrors `run_completions`'s `let epoch = …`).
    let epoch_at_request_start = indexer
        .completion_epoch
        .load(std::sync::atomic::Ordering::Acquire);

    // Concurrently, the user's edit lands and the debounced reindex runs —
    // content actually changes, so `last_completion` is cleared.
    indexer.index_content(
        &uri,
        "class Widget {\n    fun existing() {}\n    fun freshlyAdded() {}\n}\n",
    );

    // The original (now-stale) request finally finishes computing against the
    // OLD symbol table and tries to persist its result under the epoch it
    // captured before the reindex.
    let stale_items = vec![CompletionItem {
        label: "existing".to_string(),
        ..Default::default()
    }];
    let key = completion_cache_key(&uri, "    w.", 1);
    store_in_cache(
        &indexer,
        key.clone(),
        &stale_items,
        false,
        epoch_at_request_start,
    );

    assert!(
        cache_hit(&indexer, &key).is_none(),
        "a completion result computed before a concurrent reindex must not \
         be allowed to re-populate the cache after that reindex cleared it"
    );
}

#[test]
fn split_prefix_after_dot() {
    let (prefix, before_prefix) = split_prefix("foo.bar");
    assert_eq!(prefix, "bar");
    assert_eq!(before_prefix, "foo.");
}

#[test]
fn split_prefix_bare() {
    let (prefix, before_prefix) = split_prefix("someIdent");
    assert_eq!(prefix, "someIdent");
    assert_eq!(before_prefix, "");
}

// ── param_names_from_sig ──────────────────────────────────────────────────────

#[test]
fn param_names_basic() {
    assert_eq!(
        param_names_from_sig("name: String, age: Int"),
        vec!["name", "age"]
    );
}

#[test]
fn param_names_with_defaults() {
    assert_eq!(
        param_names_from_sig(
            "text: String, modifier: Modifier = Modifier, color: Color = Color.Unspecified"
        ),
        vec!["text", "modifier", "color"]
    );
}

#[test]
fn param_names_with_annotation() {
    assert_eq!(
        param_names_from_sig("@Composable content: @Composable () -> Unit"),
        vec!["content"]
    );
}

#[test]
fn param_names_vararg() {
    assert_eq!(param_names_from_sig("vararg items: String"), vec!["items"]);
}

#[test]
fn param_names_skips_this() {
    // Extension receiver `this@Foo` should not produce a named arg
    assert_eq!(param_names_from_sig("this: Foo, value: Int"), vec!["value"]);
}

#[test]
fn param_names_empty() {
    let result = param_names_from_sig("");
    assert!(result.is_empty());
}
