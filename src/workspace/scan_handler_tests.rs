use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};

use crate::indexer::{Indexer, NoopReporter};
use crate::workspace::phase::State;
use crate::workspace::Config;

use super::ScanHandler;

fn make_handler(indexer: Arc<Indexer>) -> ScanHandler<NoopReporter> {
    let (scan_done_tx, _scan_done_rx) = mpsc::unbounded_channel();
    let (jar_done_tx, _jar_done_rx) = mpsc::unbounded_channel();
    ScanHandler::new(
        indexer,
        Arc::new(NoopReporter),
        Arc::new(RwLock::new(State::Uninitialized)),
        scan_done_tx,
        jar_done_tx,
    )
}

#[tokio::test]
async fn handle_initialize_updates_root_and_source_paths() {
    let indexer = Arc::new(Indexer::new());
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    // Opt out of real external sources so the test doesn't scan ~/.kmp-lsp/sources.
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();
    let handler = make_handler(Arc::clone(&indexer));

    handler
        .handle_initialize(
            Config {
                root: root.clone(),
                explicit_source_paths: vec!["/some/lib".to_string()],
                ignore_patterns: Vec::new(),
                jar_paths: Vec::new(),
                pin_workspace: false,
            },
            None,
        )
        .await;

    assert_eq!(
        indexer.workspace_root.get().as_deref(),
        Some(root.as_path())
    );
    let state = handler.state_stream();
    let source_paths = state
        .read()
        .await
        .ready()
        .map(|ready| ready.source_paths.clone())
        .unwrap_or_default();
    assert!(source_paths.contains(&"/some/lib".to_string()));
    assert!(indexer
        .source_paths_raw
        .read()
        .unwrap()
        .contains(&"/some/lib".to_string()));
}

#[test]
fn indexer_new_jar_phase_is_unavailable_in_tests() {
    // In #[cfg(test)], jar_sidecar is always None, so phase starts as Unavailable.
    let indexer = Indexer::new();
    let phase = indexer.jar_phase.lock().unwrap().clone();
    assert_eq!(
        phase,
        crate::indexer::jar_phase::JarPhase::Unavailable,
        "test Indexer must start as Unavailable (no sidecar in tests)"
    );
}

#[test]
fn clear_jar_index_resets_phase_to_unavailable_when_no_sidecar() {
    // In tests, jar_sidecar is None, so clear_jar_index should keep Unavailable.
    let indexer = Indexer::new();
    // Manually set to Ready to check reset behaviour.
    *indexer.jar_phase.lock().unwrap() = crate::indexer::jar_phase::JarPhase::Ready { count: 42 };
    indexer.clear_jar_index();
    let phase = indexer.jar_phase.lock().unwrap().clone();
    assert_eq!(
        phase,
        crate::indexer::jar_phase::JarPhase::Unavailable,
        "clear_jar_index should reset to Unavailable when sidecar is None"
    );
}

#[test]
fn jar_phase_is_loading_helpers() {
    use crate::indexer::jar_phase::JarPhase;
    assert!(JarPhase::Pending.is_loading());
    assert!(JarPhase::InProgress.is_loading());
    assert!(!JarPhase::Unavailable.is_loading());
    assert!(!JarPhase::Ready { count: 0 }.is_loading());
    assert!(!JarPhase::Failed("oops".to_owned()).is_loading());
}

/// Regression: a stale JAR scan that abandons on a generation change must not
/// leave `jar_phase` stuck in a loading state (which would keep call-arg
/// diagnostics suppressed forever via the `is_loading()` gate). It moves the
/// phase out of loading and fires `jar_done` so the actor republishes.
#[test]
fn abandon_stale_jar_scan_clears_loading_signals() {
    use crate::indexer::jar_phase::JarPhase;
    use std::sync::atomic::{AtomicBool, Ordering};

    let indexer = Arc::new(Indexer::new());
    *indexer.jar_phase.lock().unwrap() = JarPhase::InProgress;
    let in_progress = AtomicBool::new(true);
    let (jar_done_tx, mut jar_done_rx) = mpsc::unbounded_channel();

    super::abandon_stale_jar_scan(&indexer, &in_progress, &jar_done_tx);

    assert!(
        !indexer.jar_phase.lock().unwrap().is_loading(),
        "phase must leave the loading state so diagnostics resume"
    );
    assert!(
        !in_progress.load(Ordering::Acquire),
        "in-flight guard cleared"
    );
    assert!(
        jar_done_rx.try_recv().is_ok(),
        "jar_done must fire to republish"
    );
}

/// Regression: on-demand materialization (Task 8) must never block behind
/// the startup crawl's jar_sidecar lock. A bounded attempt must return
/// `None` immediately when the lock is already held elsewhere, rather than
/// waiting for it to be released.
#[test]
fn try_lock_sidecar_bounded_returns_none_when_held() {
    let idx = Indexer::new();
    let held_guard = idx.jar_sidecar.lock().unwrap_or_else(|e| e.into_inner());
    // While `held_guard` is alive, a bounded attempt from "another caller"
    // must return None quickly rather than blocking this test forever.
    let attempt = crate::workspace::scan_handler::try_lock_sidecar_bounded(&idx);
    assert!(
        attempt.is_none(),
        "bounded lock attempt must not block/succeed while the sidecar is held elsewhere"
    );
    drop(held_guard);
    let attempt_after_release = crate::workspace::scan_handler::try_lock_sidecar_bounded(&idx);
    assert!(
        attempt_after_release.is_some(),
        "must succeed once the lock is free"
    );
}

/// Task 11: `handle_reindex` must reset Tier-1 (`jar_qualified`/`jar_bare_names`)
/// and materialization (`materialized`/`materialization_failed`) state so stale
/// JAR data from a previous session doesn't leak across a reindex — the same
/// discipline `jar.rs::clear_jar_maps` already applies to `jar_files`/
/// `jar_definitions`. `jar_table` itself is untouched: JarIds stay stable
/// across a reindex (append-only growth), matching the invariant `FileTable`
/// already relies on for `FileId`.
#[tokio::test]
async fn handle_reindex_resets_tier1_and_materialization_state() {
    let indexer = Arc::new(Indexer::new());
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    // Opt out of real external sources so the test doesn't scan ~/.kmp-lsp/sources.
    std::fs::write(root.join("workspace.json"), r#"{"sourcePaths":[]}"#).unwrap();
    indexer.workspace_root.set(root);
    let handler = make_handler(Arc::clone(&indexer));

    let jar_id = indexer.jar_table.intern("/fake/lib.jar");
    indexer.materialized.insert(jar_id);
    indexer.materialization_failed.insert(jar_id);
    indexer.jar_qualified.insert("SomeType".to_string(), jar_id);
    indexer
        .jar_bare_names
        .entry("SomeType".to_string())
        .or_default()
        .push(jar_id);

    handler.handle_reindex().await;

    assert!(
        !indexer.materialized.contains(&jar_id),
        "reindex must reset materialized state"
    );
    assert!(
        !indexer.materialization_failed.contains(&jar_id),
        "reindex must reset materialization_failed state"
    );
    assert!(
        indexer.jar_qualified.is_empty(),
        "reindex must clear stale Tier-1 FQN data"
    );
    assert!(
        indexer.jar_bare_names.is_empty(),
        "reindex must clear stale Tier-1 bare-name data"
    );
    assert!(
        indexer.jar_table.path(jar_id).is_some(),
        "jar_table itself must survive reindex — JarIds stay stable"
    );
}

/// Task 12 (the flip): after the compiled-JAR phase of the crawl runs, a JAR
/// nothing has referenced yet must have Tier 1 data (`jar_bare_names`/
/// `jar_qualified`) but empty Tier 2 (`jar_definitions`) — that's the entire
/// memory-saving point of `scan_handler.rs`'s crawl block now calling
/// `build_jar_manifest` instead of `index_jars`.
///
/// This can't be driven through `spawn_jar_indexing`/`handle_initialize`
/// directly: `Indexer::new()` hardcodes `jar_sidecar` to `None` under
/// `#[cfg(test)]` (see `indexer_new_jar_phase_is_unavailable_in_tests`
/// above), and `spawn_jar_indexing` bails out immediately whenever
/// `jar_sidecar` is `None` (src/workspace/scan_handler.rs, the
/// `Ok(guard) if guard.is_none() => return` check) — so the background crawl
/// thread never even spawns in this test binary. Only a real sidecar child
/// process, which `tests/lsp_smoke.rs::smoke_completion_from_compiled_jar`
/// spawns via a real `--stdio` server, can drive the crawl end-to-end; that
/// test is the actual regression gate for the flip. This test instead
/// exercises `populate_tier1_from_manifest` — the exact routine
/// `build_jar_manifest` (now wired into the crawl) delegates to on both its
/// cache-hit and sidecar-response paths — directly, pinning the Tier
/// 1/Tier 2 separation contract the flip depends on.
#[test]
fn crawl_no_longer_eagerly_materializes_every_jar() {
    let idx = Indexer::new();
    let jar_id = idx.jar_table.intern("/gradle/caches/scan-fixture-1.0.jar");
    let names = vec![crate::indexer::jar_manifest_cache::JarManifestName {
        name: "ScanFixtureType".to_owned(),
        kind: "class".to_owned(),
        container: None,
        package: Some("com.scanfixture.pkg".to_owned()),
        extension_receiver: None,
    }];

    crate::indexer::jar::populate_tier1_from_manifest(&idx, jar_id, &names);

    assert!(
        idx.jar_definitions.is_empty(),
        "the crawl must not eagerly populate jar_definitions (Tier 2) for \
         any jar — that's the whole point of the flip"
    );
    assert!(
        !idx.jar_bare_names.is_empty(),
        "the crawl MUST populate jar_bare_names (Tier 1) for every jar — \
         cheap, always-eager"
    );
}
