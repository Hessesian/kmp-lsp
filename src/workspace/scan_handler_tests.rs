use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};

use crate::indexer::{Indexer, NoopReporter};
use crate::workspace::phase::State;
use crate::workspace::Config;

use super::ScanHandler;

fn make_handler(indexer: Arc<Indexer>) -> ScanHandler<NoopReporter> {
    let (scan_done_tx, _scan_done_rx) = mpsc::unbounded_channel();
    ScanHandler::new(
        indexer,
        Arc::new(NoopReporter),
        Arc::new(RwLock::new(State::Uninitialized)),
        scan_done_tx,
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
