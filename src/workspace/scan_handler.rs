use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot, RwLock};

use crate::indexer::{Indexer, ProgressReporter};
use crate::rg::IgnoreMatcher;

use super::phase::{ReadyState, State};
use super::scan_queue::{ScanArgs, ScanKind, ScanQueue};
use super::Config;

pub(crate) struct ScanHandler<R: ProgressReporter + 'static> {
    indexer: Arc<Indexer>,
    reporter: Arc<R>,
    state: Arc<RwLock<State>>,
    is_scanning: Arc<AtomicBool>,
    scan_done_tx: mpsc::UnboundedSender<()>,
}

impl<R: ProgressReporter + 'static> ScanHandler<R> {
    pub(crate) fn new(
        indexer: Arc<Indexer>,
        reporter: Arc<R>,
        state: Arc<RwLock<State>>,
        scan_done_tx: mpsc::UnboundedSender<()>,
    ) -> Self {
        Self {
            indexer,
            reporter,
            state,
            is_scanning: Arc::new(AtomicBool::new(false)),
            scan_done_tx,
        }
    }

    /// Returns `true` while a background index scan is in flight.
    pub(crate) fn is_scanning(&self) -> bool {
        self.is_scanning.load(Ordering::Acquire)
    }

    pub(crate) fn state_stream(&self) -> Arc<RwLock<State>> {
        Arc::clone(&self.state)
    }

    pub(crate) async fn handle_initialize(
        &self,
        config: Config,
        completion_tx: Option<oneshot::Sender<()>>,
    ) {
        let data = self.apply_config(config).await;
        self.spawn_scan(data.root, Vec::new(), completion_tx).await;
    }

    pub(crate) async fn handle_reindex(&self) {
        let Some(root) = self.current_root() else {
            log::warn!("Actor: Reindex received but no workspace root is set");
            return;
        };
        // `reset_index_state()` is deferred into the scan task so it never
        // races with a concurrently running scan.
        self.enqueue_scan(ScanArgs {
            root,
            kind: ScanKind::Full,
            completion_tx: None,
            expected_generation: 0,
            reset_before_scan: true,
        });
    }

    pub(crate) async fn handle_change_root(&self, root: PathBuf) {
        let config = Config {
            root,
            explicit_source_paths: Vec::new(),
            ignore_patterns: Vec::new(),
            pin_workspace: true,
        };
        let data = self.apply_config(config).await;
        self.indexer.reset_index_state();
        self.spawn_full_scan(data.root).await;
    }

    pub(crate) async fn switch_workspace_root_for_opened_document(
        &self,
        workspace_root: PathBuf,
        opened_file_path: Option<PathBuf>,
    ) {
        let config = Config {
            root: workspace_root,
            explicit_source_paths: Vec::new(),
            ignore_patterns: Vec::new(),
            pin_workspace: true,
        };
        let data = self.apply_config(config).await;
        self.indexer.reset_index_state();
        log::info!(
            "Auto-detected workspace root (now pinned): {}",
            data.root.display()
        );
        self.spawn_scan(data.root, opened_file_path.into_iter().collect(), None)
            .await;
    }

    /// Apply a [`Config`] to the indexer and transition the phase state.
    ///
    /// The single write path shared by Initialize, ChangeRoot, and
    /// switch_workspace_root_for_opened_document. Returns the resolved
    /// [`ReadyState`] so callers can extract the root for subsequent scans.
    async fn apply_config(&self, config: Config) -> ReadyState {
        let data = ReadyState::from_config(&config);
        self.set_root(data.root.clone());
        self.apply_ignore_patterns(&config.ignore_patterns, &data.root);
        self.indexer
            .workspace_pinned
            .store(config.pin_workspace, std::sync::atomic::Ordering::Relaxed);
        self.write_source_paths(data.source_paths.clone());
        self.state.write().await.set_state(data.clone());
        data
    }

    pub(crate) fn current_root(&self) -> Option<PathBuf> {
        self.indexer.workspace_root.get()
    }

    fn set_root(&self, root: PathBuf) {
        self.indexer.workspace_root.set(root);
    }

    fn write_source_paths(&self, paths: Vec<String>) {
        match self.indexer.source_paths_raw.write() {
            Ok(mut guard) => *guard = paths,
            Err(error) => log::warn!("Actor: failed to write source_paths_raw: {error}"),
        }
    }

    fn apply_ignore_patterns(&self, patterns: &[String], root: &Path) {
        match self.indexer.ignore_matcher.write() {
            Ok(mut guard) => {
                *guard = (!patterns.is_empty())
                    .then(|| Arc::new(IgnoreMatcher::new(patterns.to_vec(), root)));
            }
            Err(error) => log::warn!("Actor: failed to write ignore_matcher: {error}"),
        }
    }

    async fn spawn_scan(
        &self,
        root: PathBuf,
        initial_paths: Vec<PathBuf>,
        completion_tx: Option<oneshot::Sender<()>>,
    ) {
        let indexer = Arc::clone(&self.indexer);
        let reporter = Arc::clone(&self.reporter);
        let is_scanning = Arc::clone(&self.is_scanning);
        let scan_done_tx = self.scan_done_tx.clone();
        is_scanning.store(true, Ordering::Release);
        tokio::spawn(async move {
            indexer
                .index_workspace_prioritized(&root, initial_paths, reporter)
                .await;
            is_scanning.store(false, Ordering::Release);
            let _ = scan_done_tx.send(());
            if let Some(completion_tx) = completion_tx {
                let _ = completion_tx.send(());
            }
            let gen = self.indexer.workspace_root.generation();
            let args = ScanArgs {
                expected_generation: gen,
                ..args
            };
            queue.request(args);
            queue.try_start()
        };
        if let Some(args) = maybe_args {
            self.execute_scan(args);
        }
    }

    /// Spawn the tokio task for a single scan. Bails out early if the scan
    /// has been superseded (generation mismatch) before or after indexing.
    ///
    /// The `ScanDoneGuard` RAII type guarantees `scan_done_tx` is always
    /// signalled on task completion or panic, keeping the queue unblocked.
    fn execute_scan(&self, args: ScanArgs) {
        let indexer = Arc::clone(&self.indexer);
        let reporter = Arc::clone(&self.reporter);
        let is_scanning = Arc::clone(&self.is_scanning);
        let scan_done_tx = self.scan_done_tx.clone();
        is_scanning.store(true, Ordering::Release);
        tokio::spawn(async move {
            indexer.index_workspace_full(&root, reporter).await;
            is_scanning.store(false, Ordering::Release);
            let _ = scan_done_tx.send(());
        });
    }
}

#[cfg(test)]
#[path = "scan_handler_tests.rs"]
mod tests;
