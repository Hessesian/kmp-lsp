use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Tracks overall indexing progress across workspace scan and JAR indexing.
/// Sends `$/progress` notifications to the editor.
///
/// Lifecycle:
/// 1. `begin()` — sent when workspace scan starts
/// 2. `report()` — sent periodically with current progress
/// 3. `end()` — sent when BOTH workspace scan AND JAR indexing are complete
///
/// The tracker is shared between the workspace scan task and JAR indexing task.
/// Each task signals completion via `workspace_done()` / `jar_done()`.
/// The `End` notification is sent only when both are done.
pub(crate) struct ProgressTracker<R: crate::indexer::ProgressReporter + 'static> {
    reporter: Arc<R>,
    token: tower_lsp::lsp_types::NumberOrString,
    workspace_done: AtomicBool,
    jar_done: AtomicBool,
    jar_in_progress: AtomicBool,
}

impl<R: crate::indexer::ProgressReporter + 'static> ProgressTracker<R> {
    pub(crate) fn new(reporter: Arc<R>) -> Self {
        Self {
            reporter,
            token: tower_lsp::lsp_types::NumberOrString::String("kmp-lsp/indexing".into()),
            workspace_done: AtomicBool::new(false),
            jar_done: AtomicBool::new(true), // true = no JAR work pending
            jar_in_progress: AtomicBool::new(false),
        }
    }

    /// Signal that workspace indexing has started.
    pub(crate) async fn begin(&self, message: &str) {
        self.workspace_done.store(false, Ordering::Release);
        self.reporter.begin(&self.token, message).await;
    }

    /// Signal that workspace scan completed.
    pub(crate) async fn workspace_done(&self, message: &str) {
        self.workspace_done.store(true, Ordering::Release);
        self.reporter.end(&self.token, message).await;
        self.check_complete().await;
    }

    /// Signal that JAR indexing has started.
    pub(crate) fn jar_started(&self) {
        self.jar_in_progress.store(true, Ordering::Release);
        self.jar_done.store(false, Ordering::Release);
    }

    /// Signal that JAR indexing completed.
    pub(crate) async fn jar_done(&self, message: &str) {
        self.jar_done.store(true, Ordering::Release);
        self.jar_in_progress.store(false, Ordering::Release);
        self.reporter.end(&self.token, message).await;
        self.check_complete().await;
    }

    /// Check if both workspace and JAR are done, and send final End if so.
    async fn check_complete(&self) {
        if self.workspace_done.load(Ordering::Acquire) && self.jar_done.load(Ordering::Acquire) {
            // Both done — the End notifications were already sent by the individual
            // done() calls. Nothing more to do.
        }
    }

    /// Returns true while JAR indexing is in progress.
    pub(crate) fn is_jar_in_progress(&self) -> bool {
        self.jar_in_progress.load(Ordering::Acquire)
    }
}
