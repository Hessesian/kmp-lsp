use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot, RwLock};

use crate::indexer::{Indexer, ProgressReporter};
use crate::rg::IgnoreMatcher;

use super::phase::{ReadyState, State};
use super::scan_queue::{ScanArgs, ScanKind, ScanQueue};
use super::Config;

/// A short, bounded attempt to lock `jar_sidecar` — used by on-demand
/// materialization (Task 8) so a hover/completion request never blocks
/// indefinitely behind the startup crawl or another in-flight materialization.
/// Returns `None` (degrade to Tier-1-only for this request) rather than
/// waiting. See design §Concurrency.
pub(crate) fn try_lock_sidecar_bounded(
    indexer: &crate::indexer::Indexer,
) -> Option<std::sync::MutexGuard<'_, Option<crate::sidecar::SidecarHandle>>> {
    // `try_lock` is genuinely non-blocking (fails immediately if contended,
    // rather than spinning) — the "bounded" framing in the design becomes
    // "immediate or nothing" here, which is the simplest correct instance of
    // "don't block the interactive path" and avoids inventing a timeout
    // mechanism this codebase doesn't otherwise use.
    //
    // A plain `.ok()` would treat "poisoned" the same as "contended" —
    // unlike a contended lock, a poisoned one never recovers on its own, so
    // that would permanently disable on-demand promotion after a single
    // recoverable panic elsewhere while holding this lock. Every other
    // `jar_sidecar` lock site in this codebase recovers via
    // `unwrap_or_else(|e| e.into_inner())`; match that here.
    match indexer.jar_sidecar.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

pub(crate) struct ScanHandler<R: ProgressReporter + 'static> {
    indexer: Arc<Indexer>,
    reporter: Arc<R>,
    state: Arc<RwLock<State>>,
    /// `Arc` so the jar-pipeline task can poll "is a workspace scan still
    /// running" from its blocking thread — see `wait_for_jvm_sources_gate`.
    scan_queue: Arc<Mutex<ScanQueue>>,
    scan_done_tx: mpsc::UnboundedSender<()>,
    /// Signals the actor when background JAR indexing reaches a terminal phase so
    /// it can recompute diagnostics for files diagnosed against a partial index.
    jar_done_tx: mpsc::UnboundedSender<()>,
    /// Guard that prevents concurrent Gradle-cache crawls.
    jar_indexing_in_progress: Arc<AtomicBool>,
    /// Compiled jar/aar path specs from LSP init options (`indexingOptions.jarPaths`),
    /// indexed alongside the Gradle cache. Set by `apply_config`; merged with
    /// `workspace.json`'s `jarPaths` in `spawn_jar_indexing`.
    configured_jar_paths: Mutex<Vec<String>>,
}

impl<R: ProgressReporter + 'static> ScanHandler<R> {
    pub(crate) fn new(
        indexer: Arc<Indexer>,
        reporter: Arc<R>,
        state: Arc<RwLock<State>>,
        scan_done_tx: mpsc::UnboundedSender<()>,
        jar_done_tx: mpsc::UnboundedSender<()>,
    ) -> Self {
        Self {
            indexer,
            reporter,
            state,
            scan_queue: Arc::new(Mutex::new(ScanQueue::new())),
            scan_done_tx,
            jar_done_tx,
            jar_indexing_in_progress: Arc::new(AtomicBool::new(false)),
            configured_jar_paths: Mutex::new(Vec::new()),
        }
    }

    /// Returns `true` while a background index scan is in flight.
    pub(crate) fn is_scanning(&self) -> bool {
        self.scan_queue.lock().unwrap().is_in_progress()
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
        // Tier-1 (`jar_qualified`/`jar_bare_names`) and materialization
        // (`materialized`/`materialization_failed`) state must not survive a
        // reindex — a stale entry would point at a JarId whose manifest is
        // about to be rebuilt from scratch by `spawn_jar_indexing` below, and
        // a stale `materialized` flag would suppress re-materialization for a
        // JAR that could have changed on disk since it was last read. This
        // mirrors the discipline `jar.rs::clear_jar_maps` already applies to
        // `jar_files`/`jar_definitions`.
        //
        // `jar_table` itself is NOT cleared — JarIds stay stable across a
        // reindex (append-only growth), the same invariant `FileTable`
        // already relies on for `FileId`. Only the maps keyed BY JarId reset;
        // a JAR whose path is interned again during the new crawl gets its
        // existing JarId back (`JarTable::intern` is idempotent), so nothing
        // downstream needs to know a reindex happened.
        self.indexer.materialized.clear();
        self.indexer.materialization_failed.clear();
        self.indexer.jar_qualified.clear();
        self.indexer.jar_bare_names.clear();
        self.indexer.jar_extension_receivers.clear();
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
            // Root-switch configs carry no init-options jars; the new root's
            // workspace.json `jarPaths` is read per-scan in spawn_jar_indexing.
            jar_paths: Vec::new(),
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
            // Root-switch configs carry no init-options jars; the new root's
            // workspace.json `jarPaths` is read per-scan in spawn_jar_indexing.
            jar_paths: Vec::new(),
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
        // Recover a poisoned lock so init-options `jarPaths` are still applied
        // after an unrelated panic (consistent with the other locks here).
        *self
            .configured_jar_paths
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = config.jar_paths.clone();
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
        } else {
            crate::indexer::jar_phase::report_jar_phase_lock_poisoned(
                "ScanHandler::spawn_jar_indexing (-> InProgress)",
            );
        }

        let indexer = Arc::clone(&self.indexer);
        let in_progress = Arc::clone(&self.jar_indexing_in_progress);
        let jar_done_tx = self.jar_done_tx.clone();
        let scan_queue = Arc::clone(&self.scan_queue);

        // Init-options `jarPaths` specs (cheap string clone). The actual filesystem
        // expansion (reading workspace.json, walking dirs) happens inside the
        // spawn_blocking task below so it never blocks a Tokio worker thread.
        let init_jar_specs: Vec<String> = self
            .configured_jar_paths
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());

        // Capture current workspace generation so stale tasks don't overwrite state.
        let expected_gen = indexer
            .workspace_root
            .generation_atomic()
            .load(Ordering::Acquire);
        tokio::task::spawn_blocking(move || {
            // Bail before any filesystem work if this task is already superseded
            // (a newer scan bumped the generation).
            if indexer
                .workspace_root
                .generation_atomic()
                .load(Ordering::Acquire)
                != expected_gen
            {
                abandon_stale_jar_scan(&indexer, &in_progress, &jar_done_tx);
                return;
            }
            // ── Compiled-JAR first (sidecar path, populates jar_files / jar_definitions) ──
            // The Gradle-cache pipeline only matters for a workspace with
            // JVM sources — a Swift-only project cannot reference anything
            // in it, and unconditionally running the pipeline there cost a
            // 1.28M-name Tier-1 manifest over 755 JARs plus a 1.66M-symbol
            // sources-JAR pass (observed live on an iOS repo). Gated on the
            // INDEX, not on build-file markers — see
            // `workspace_has_jvm_sources` for why. This task races the
            // workspace scan that populates that index (every caller
            // enqueues the scan first, so the queue is already non-idle
            // here), so the gate WAITS: it opens the moment the scan
            // indexes the first JVM source, and returns a negative verdict
            // only once the queue drains. Explicitly configured `jarPaths`
            // below stay unaffected.
            let generation_ok = || {
                indexer
                    .workspace_root
                    .generation_atomic()
                    .load(Ordering::Acquire)
                    == expected_gen
            };
            // Recover from a poisoned queue lock rather than panicking: a
            // panic here would strand `jar_indexing_in_progress` as true and
            // wedge the jar pipeline for the rest of the process. Reading
            // `is_in_progress` off a poisoned queue is harmless.
            let workspace_uses_gradle_cache = match crate::indexer::jar::wait_for_jvm_sources_gate(
                &indexer,
                || {
                    scan_queue
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .is_in_progress()
                },
                generation_ok,
                std::time::Duration::from_millis(100),
            ) {
                Some(verdict) => verdict,
                None => {
                    abandon_stale_jar_scan(&indexer, &in_progress, &jar_done_tx);
                    return;
                }
            };
            let gradle_paths = if workspace_uses_gradle_cache {
                crate::indexer::jar::scan_gradle_jars(None)
            } else {
                log::info!(
                    "jar: no Kotlin/Java sources in the workspace — skipping the \
                     Gradle-cache JAR pipeline (configured jarPaths still honored)"
                );
                Vec::new()
            };
            let gradle_count = gradle_paths.len();
            let mut paths = gradle_paths;

            // Android SDK's compiled `android.jar`, discovered independently of the
            // Gradle-cache gate above — an Android SDK can be installed on a machine
            // regardless of whether this particular workspace's JVM-sources gate is
            // open, and `android.jar` never lives in the Gradle module cache anyway.
            if let Some(root) = indexer.workspace_root.get() {
                for jar in crate::workspace_json::detect_android_sdk_jar_path(&root) {
                    if !paths.contains(&jar) {
                        paths.push(jar);
                    }
                }
            }

            // Explicitly-configured jars (workspace.json `jarPaths` + init-options
            // `jarPaths`), so non-Gradle projects (Make/Bazel/manual) get symbols too.
            // Filesystem I/O (read workspace.json, walk dirs) runs here off-thread.
            if let Some(root) = indexer.workspace_root.get() {
                let mut configured = crate::workspace_json::load_configured_jar_paths(&root);
                configured.extend(crate::workspace_json::resolve_jar_path_specs(
                    &init_jar_specs,
                    &root,
                ));
                for jar in configured {
                    if !paths.contains(&jar) {
                        paths.push(jar);
                    }
                }
            }

            // Check generation before doing any JAR indexing work.
            let current_gen = indexer
                .workspace_root
                .generation_atomic()
                .load(Ordering::Acquire);
            if current_gen != expected_gen {
                abandon_stale_jar_scan(&indexer, &in_progress, &jar_done_tx);
                return;
            }

            crate::indexer::jar::clear_jar_maps(&indexer);
            let (compiled_total, sidecar_alive) = {
                let mut sidecar = indexer
                    .jar_sidecar
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let compiled_total =
                    crate::indexer::jar::build_jar_manifest(&indexer, &paths, &mut sidecar);
                (compiled_total, sidecar.is_some())
                // `sidecar` MutexGuard drops here, at the end of this block —
                // released before index_sources_jars runs, so an on-demand
                // materialization request only ever contends with the
                // compiled-JAR phase, never the (much longer) sources-JAR
                // phase that follows it.
            };
            log::info!(
                "jar: manifested {compiled_total} names from {} compiled JARs (Tier 1 only — \
                 full materialization deferred to first real use)",
                paths.len()
            );

            // Check generation again before continuing to sources-JAR work.
            let current_gen = indexer
                .workspace_root
                .generation_atomic()
                .load(Ordering::Acquire);
            if current_gen != expected_gen {
                abandon_stale_jar_scan(&indexer, &in_progress, &jar_done_tx);
                return;
            }

            // ── Sources-JAR second (auto-mount, populates main files / definitions) ──
            // Runs LAST so that when both pipelines contribute the same FQN to
            // `qualified` / `extension_by_receiver`, the sources-JAR entry (real
            // line numbers from tree-sitter) wins over the compiled-JAR entry
            // (synthetic line indices from the sidecar). Same Gradle gate as
            // the compiled pipeline — sources JARs come from the same cache.
            let sources_total = if workspace_uses_gradle_cache {
                crate::indexer::jar::index_sources_jars(&indexer, None, None)
            } else {
                0
            };

            if paths.is_empty() && sources_total == 0 {
                if let Ok(mut phase) = indexer.jar_phase.lock() {
                    *phase = JarPhase::Ready { count: 0 };
                } else {
                    crate::indexer::jar_phase::report_jar_phase_lock_poisoned(
                        "ScanHandler::spawn_jar_indexing (-> Ready{0}, no jars found)",
                    );
                }
                in_progress.store(false, Ordering::Release);
                let _ = jar_done_tx.send(());
                return;
            }

            log::info!(
                "jar: indexing {} compiled JARs/AARs ({} from Gradle cache, {} configured)",
                paths.len(),
                gradle_count,
                paths.len() - gradle_count
            );

            // Check generation once more before recording terminal phase.
            let current_gen = indexer
                .workspace_root
                .generation_atomic()
                .load(Ordering::Acquire);
            if current_gen != expected_gen {
                abandon_stale_jar_scan(&indexer, &in_progress, &jar_done_tx);
                return;
            }

            let total = sources_total + compiled_total;
            let final_phase = if !sidecar_alive && compiled_total > 0 {
                // Sidecar died mid-index; sources may still be available.
                JarPhase::Failed(format!(
                    "sidecar died mid-index; {total} symbols partially loaded ({sources_total} from sources, {compiled_total} from compiled)"
                ))
            } else {
                JarPhase::Ready { count: total }
            };
            if let Ok(mut phase) = indexer.jar_phase.lock() {
                *phase = final_phase;
            } else {
                crate::indexer::jar_phase::report_jar_phase_lock_poisoned(
                    "ScanHandler::spawn_jar_indexing (-> Ready/Failed, terminal)",
                );
            }
            // Invalidate the completion cache so the next request returns JAR
            // symbols (launch, collect, etc.) without requiring a retype.
            indexer.invalidate_completion_cache();
            in_progress.store(false, Ordering::Release);
            // Wake the actor to recompute diagnostics now that JAR symbols exist.
            let _ = jar_done_tx.send(());
        });
    }
}

/// Clean up after a background JAR scan abandons because the workspace generation
/// changed mid-scan. A newer scan may have been coalesced away while this one held
/// the in-flight guard, so we must not leave `jar_phase` stuck in a loading state —
/// that would keep `call_arg_diagnostics` suppressed indefinitely. Move the phase
/// out of `Pending`/`InProgress` and wake the actor to republish diagnostics.
fn abandon_stale_jar_scan(
    indexer: &Indexer,
    in_progress: &AtomicBool,
    jar_done_tx: &mpsc::UnboundedSender<()>,
) {
    use crate::indexer::jar_phase::JarPhase;
    in_progress.store(false, Ordering::Release);
    if let Ok(mut phase) = indexer.jar_phase.lock() {
        if matches!(*phase, JarPhase::InProgress | JarPhase::Pending) {
            *phase = JarPhase::Ready {
                count: indexer.jar_definitions.len(),
            };
        }
    } else {
        // Worse here than at the other transition sites: this function's own
        // doc comment says leaving `jar_phase` stuck in Pending/InProgress
        // "would keep `call_arg_diagnostics` suppressed indefinitely" — a
        // poisoned lock produces exactly that stuck state, silently.
        crate::indexer::jar_phase::report_jar_phase_lock_poisoned(
            "ScanHandler::abandon_stale_jar_scan",
        );
    }
    let _ = jar_done_tx.send(());
}

#[cfg(test)]
#[path = "scan_handler_tests.rs"]
mod tests;
