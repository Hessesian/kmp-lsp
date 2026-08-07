//! Observable state of the JAR symbol indexing pipeline.
//!
//! This is *separate* from the concurrency guard (`jar_indexing_in_progress:
//! AtomicBool`) which prevents duplicate spawns.  `JarPhase` is what callers
//! (hover, completion, diagnostics) read to decide how to behave when JAR
//! symbols are absent.

/// Observable phase of the JAR symbol indexing pipeline.
///
/// Stored as `Arc<Mutex<JarPhase>>` on `Indexer` so `ScanHandler` can
/// transition it from inside a `spawn_blocking` task.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JarPhase {
    /// The `kmp-jar-indexer` sidecar binary/JAR was not found at process
    /// startup.  JAR symbols will never be available in this session.
    Unavailable,
    /// Sidecar is present; indexing has not been triggered yet.
    Pending,
    /// A `spawn_blocking` task is currently running.
    InProgress,
    /// Indexing completed.  `count` is the total number of symbols loaded;
    /// may be zero when no Gradle JARs were discovered (distinguishes "done
    /// but empty" from `Pending`).
    Ready { count: usize },
    /// The sidecar died mid-index.  Partial symbols may still be available
    /// in `jar_definitions`/`jar_files`.
    Failed(String),
}

impl JarPhase {
    /// True while JAR symbols are still being loaded — either not yet requested
    /// (`Pending`) or actively indexing (`InProgress`). Hover uses this to show a
    /// "still indexing" hint instead of an empty popup when a symbol that lives in
    /// a JAR hasn't been indexed yet, so the user knows to retry once it's done.
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, JarPhase::Pending | JarPhase::InProgress)
    }
}

/// Whether `Indexer::jar_phase`'s poisoning has already been reported this
/// process. `std::sync::Mutex` poisons on any panic while the lock is held,
/// and every `if let Ok(mut phase) = jar_phase.lock()` transition site in
/// `indexer.rs`/`scan_handler.rs` currently has no `else` — once poisoned,
/// each becomes a permanent silent no-op, and the JAR-indexing state machine
/// (Pending → InProgress → Ready/Failed) freezes wherever it was, with hover
/// and completion never learning why JAR symbols stopped updating. Gated to
/// fire once (not per call site, not per throttle count) since every hit
/// after the first describes the exact same poisoning.
static JAR_PHASE_POISON_REPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Report that `jar_phase.lock()` returned `Err` (poisoned) at `site`. See
/// [`JAR_PHASE_POISON_REPORTED`] for why this only ever logs once.
pub(crate) fn report_jar_phase_lock_poisoned(site: &str) {
    if JAR_PHASE_POISON_REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    log::warn!(
        "jar_phase mutex is poisoned (first observed in {site}) — a prior JAR-phase update \
         panicked while holding the lock. Every future phase transition will silently no-op \
         from here on, freezing JAR-derived hover/completion at whatever phase they were in \
         when this fired."
    );
}
