use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Coordinates progress notifications between workspace scan and JAR indexing.
///
/// The LSP `$/progress` protocol expects:
/// - One `Begin` when work starts
/// - Zero or more `Report` updates
/// - One `End` when work completes
///
/// Since we have two parallel work streams (workspace scan + JAR indexing),
/// we need to coordinate so that `End` is sent only when BOTH complete.
///
/// Design:
/// - Workspace scan sends `Begin` at start
/// - JAR indexing does NOT send its own `Begin`/`End` — it's considered part of the same work
/// - Workspace scan waits for JAR to finish before sending `End`
/// - If JAR is already done when workspace finishes, send `End` immediately
pub(crate) struct ProgressCoordinator {
    /// Number of pending JAR indexing tasks (0 or 1).
    pending_jars: AtomicUsize,
}

impl ProgressCoordinator {
    pub(crate) fn new() -> Self {
        Self {
            pending_jars: AtomicUsize::new(0),
        }
    }

    /// Called when JAR indexing starts. Increments the pending count.
    pub(crate) fn jar_started(&self) {
        self.pending_jars.fetch_add(1, Ordering::AcqRel);
    }

    /// Called when JAR indexing completes. Decrements the pending count.
    /// Returns true if this was the last pending JAR.
    pub(crate) fn jar_done(&self) -> bool {
        let prev = self.pending_jars.fetch_sub(1, Ordering::AcqRel);
        prev == 1 // was 1, now 0 — no more pending
    }

    /// Returns true if there are pending JAR indexing tasks.
    pub(crate) fn has_pending_jars(&self) -> bool {
        self.pending_jars.load(Ordering::Acquire) > 0
    }
}
