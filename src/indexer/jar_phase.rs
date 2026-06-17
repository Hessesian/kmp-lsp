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
