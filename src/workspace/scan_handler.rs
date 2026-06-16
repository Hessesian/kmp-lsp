use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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
    scan_queue: Mutex<ScanQueue>,
    scan_done_tx: mpsc::UnboundedSender<()>,
    /// Guard that prevents concurrent Gradle-cache crawls.
    jar_indexing_in_progress: Arc<AtomicBool>,
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
            scan_queue: Mutex::new(ScanQueue::new()),
            scan_done_tx,
            jar_indexing_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns `true` while a background index scan is in flight.
    pub(crate) fn is_scanning(&self) -> bool {
        self.scan_queue.lock().unwrap().is_in_progress()
    }

    /// Returns true while JAR indexing is in progress.
    #[allow(dead_code)]
    pub(crate) fn is_jar_indexing(&self) -> bool {
        self.jar_indexing_in_progress.load(Ordering::Acquire)
    }

    /// Called by the actor when `scan_done_rx` fires.
    ///
    /// Marks the current scan complete and starts any pending follow-up.
    pub(crate) fn on_scan_completed(&self) {
        let maybe_next = {
            let mut queue = self.scan_queue.lock().unwrap();
            queue.completed();
            queue.try_start()
        };
        if let Some(args) = maybe_next {
            self.execute_scan(args);
        }
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
        self.enqueue_scan(ScanArgs {
            root: data.root,
            kind: ScanKind::Prioritized {
                initial_paths: Vec::new(),
            },
            completion_tx,
            reset_before_scan: false,
            expected_generation: 0,
        });
        self.spawn_jar_indexing();
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
            reset_before_scan: true,
            expected_generation: 0,
        });
        // JAR symbols survive reindex (separate maps), but re-scan in case new
        // dependencies were added since last indexing.
        self.spawn_jar_indexing();
    }

    pub(crate) async fn handle_change_root(&self, root: PathBuf) {
        // Workspace is changing — clear JAR symbols from the old project.
        self.indexer.clear_jar_index();
        let config = Config {
            root,
            explicit_source_paths: Vec::new(),
            ignore_patterns: Vec::new(),
            pin_workspace: true,
        };
        let data = self.apply_config(config).await;
        self.enqueue_scan(ScanArgs {
            root: data.root,
            kind: ScanKind::Full,
            completion_tx: None,
            reset_before_scan: true,
            expected_generation: 0,
        });
        // Re-index JARs for the new workspace root.
        self.spawn_jar_indexing();
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
        log::info!(
            "Auto-detected workspace root (now pinned): {}",
            data.root.display()
        );
        self.enqueue_scan(ScanArgs {
            root: data.root,
            kind: ScanKind::Prioritized {
                initial_paths: opened_file_path.into_iter().collect(),
            },
            completion_tx: None,
            reset_before_scan: true,
            expected_generation: 0,
        });
        self.spawn_jar_indexing();
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
        self.write_workspace_source_roots(&data.root);
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

    fn write_workspace_source_roots(&self, root: &std::path::Path) {
        let roots: Vec<String> = crate::workspace_json::load_source_paths(root)
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if !roots.is_empty() {
            log::info!("workspace sourceRoots for rg scoping: {:?}", roots);
        }
        match self.indexer.workspace_source_roots.write() {
            Ok(mut guard) => *guard = roots,
            Err(error) => log::warn!("Actor: failed to write workspace_source_roots: {error}"),
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

    /// Enqueue a scan request. If a scan is in progress the generation is
    /// bumped to invalidate it; the new request replaces any earlier pending
    /// one (last-write-wins). Starts the scan immediately when the queue is idle.
    fn enqueue_scan(&self, args: ScanArgs) {
        let maybe_args = {
            let mut queue = self.scan_queue.lock().unwrap();
            if queue.is_in_progress() {
                self.indexer.workspace_root.bump_generation();
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
    fn execute_scan(&self, args: ScanArgs) {
        let indexer = Arc::clone(&self.indexer);
        let reporter = Arc::clone(&self.reporter);
        let scan_done_tx = self.scan_done_tx.clone();
        tokio::spawn(async move {
            let ScanArgs {
                root,
                kind,
                completion_tx,
                expected_generation,
                reset_before_scan,
                ..
            } = args;

            if indexer.workspace_root.generation() != expected_generation {
                let _ = scan_done_tx.send(());
                return;
            }

            if reset_before_scan {
                indexer.reset_index_state();
            }

            match kind {
                ScanKind::Prioritized { initial_paths } => {
                    Arc::clone(&indexer)
                        .index_workspace_prioritized(&root, initial_paths, reporter)
                        .await;
                }
                ScanKind::Full => {
                    Arc::clone(&indexer)
                        .index_workspace_full(&root, reporter)
                        .await;
                }
            }

            let _ = scan_done_tx.send(());
            if indexer.workspace_root.generation() == expected_generation {
                if let Some(tx) = completion_tx {
                    let _ = tx.send(());
                }
            }
        });
    }

    /// Spawn a blocking task that scans the Gradle cache and indexes JAR/AAR
    /// symbols via the sidecar.  Runs in the background after `initialize` returns
    /// so it never blocks LSP startup.  Coalesces: if a scan is already running,
    /// this call is a no-op (the running scan will have picked up the current state).
    fn spawn_jar_indexing(&self) {
        use crate::indexer::jar_phase::JarPhase;

        // Cheap non-blocking check: skip if sidecar is unavailable.
        match self.indexer.jar_sidecar.try_lock() {
            Ok(guard) if guard.is_none() => return,
            Err(_) => {
                // Lock held by running scan — already in progress, coalesce.
                return;
            }
            _ => {}
        }
        // Coalesce: only one Gradle crawl at a time.
        if self
            .jar_indexing_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        // Transition to InProgress before spawning.
        if let Ok(mut phase) = self.indexer.jar_phase.lock() {
            *phase = JarPhase::InProgress;
        }

        let indexer = Arc::clone(&self.indexer);
        let in_progress = Arc::clone(&self.jar_indexing_in_progress);

        // Capture current workspace generation so stale tasks don't overwrite state.
        let expected_gen = indexer
            .workspace_root
            .generation_atomic()
            .load(Ordering::Acquire);
        tokio::task::spawn_blocking(move || {
            // ── Compiled-JAR first (sidecar path, populates jar_files / jar_definitions) ──
            let paths = crate::indexer::jar::scan_gradle_jars(None);

            // Check generation before doing any JAR indexing work.
            let current_gen = indexer
                .workspace_root
                .generation_atomic()
                .load(Ordering::Acquire);
            if current_gen != expected_gen {
                in_progress.store(false, Ordering::Release);
                return;
            }

            let mut sidecar = indexer
                .jar_sidecar
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let compiled_total = crate::indexer::jar::index_jars(&indexer, &paths, &mut sidecar);

            // Check generation again before continuing to sources-JAR work.
            let current_gen = indexer
                .workspace_root
                .generation_atomic()
                .load(Ordering::Acquire);
            if current_gen != expected_gen {
                in_progress.store(false, Ordering::Release);
                return;
            }

            // ── Sources-JAR second (auto-mount, populates main files / definitions) ──
            // Runs LAST so that when both pipelines contribute the same FQN to
            // `qualified` / `extension_by_receiver`, the sources-JAR entry (real
            // line numbers from tree-sitter) wins over the compiled-JAR entry
            // (synthetic line indices from the sidecar).
            let sources_total = crate::indexer::jar::index_sources_jars(&indexer, None, None);

            if paths.is_empty() && sources_total == 0 {
                if let Ok(mut phase) = indexer.jar_phase.lock() {
                    *phase = JarPhase::Ready { count: 0 };
                }
                in_progress.store(false, Ordering::Release);
                return;
            }

            log::info!(
                "jar: found {} compiled JARs/AARs in Gradle cache",
                paths.len()
            );

            // Check generation once more before recording terminal phase.
            let current_gen = indexer
                .workspace_root
                .generation_atomic()
                .load(Ordering::Acquire);
            if current_gen != expected_gen {
                in_progress.store(false, Ordering::Release);
                return;
            }

            let total = sources_total + compiled_total;
            let final_phase = if sidecar.is_none() && compiled_total > 0 {
                // Sidecar died mid-index; sources may still be available.
                JarPhase::Failed(format!(
                    "sidecar died mid-index; {total} symbols partially loaded ({sources_total} from sources, {compiled_total} from compiled)"
                ))
            } else {
                JarPhase::Ready { count: total }
            };
            if let Ok(mut phase) = indexer.jar_phase.lock() {
                *phase = final_phase;
            }
            // Invalidate the completion cache so the next request returns JAR
            // symbols (launch, collect, etc.) without requiring a retype.
            indexer.invalidate_completion_cache();
            in_progress.store(false, Ordering::Release);
        });
    }
}

#[cfg(test)]
#[path = "scan_handler_tests.rs"]
mod tests;
