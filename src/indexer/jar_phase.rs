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
    /// True while JAR symbols have not yet been requested.
    /// Once indexing starts (`InProgress`), symbols are progressively
    /// available and hover/completion should not show a loading message.
    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, JarPhase::Pending)
    }
}
