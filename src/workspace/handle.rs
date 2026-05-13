//! `WorkspaceHandle` — phase-gated read access to the shared workspace.
//!
//! The backend holds a `WorkspaceHandle` instead of a raw `Arc<Indexer>`.
//! It uses `ensure_indexed` (adapter-level warming) before calling feature
//! functions, and `is_ready` to skip queries during early startup.

use std::sync::{Arc, RwLock};

use tokio::sync::mpsc;
use tower_lsp::lsp_types::Url;

use crate::indexer::Indexer;
use crate::workspace::event::Event;
use crate::workspace::phase::State;

/// Phase-gated handle to the shared workspace, held by the Backend.
///
/// Read path: `is_ready` → `ensure_indexed` → feature fn via capability traits
/// Write path: `event_tx.send(Event::…)` → Actor → Indexer mutation
#[allow(dead_code)]
pub(crate) struct WorkspaceHandle {
    pub(crate) indexer: Arc<Indexer>,
    phase: Arc<RwLock<State>>,
    pub(crate) event_tx: mpsc::Sender<Event>,
}

#[allow(dead_code)]
impl WorkspaceHandle {
    pub(crate) fn new(
        indexer: Arc<Indexer>,
        phase: Arc<RwLock<State>>,
        event_tx: mpsc::Sender<Event>,
    ) -> Self {
        Self {
            indexer,
            phase,
            event_tx,
        }
    }

    /// `true` once the workspace has completed its first scan and is ready
    /// to serve feature queries.
    pub(crate) fn is_ready(&self) -> bool {
        self.phase
            .read()
            .map(|s| s.ready().is_some())
            .unwrap_or(false)
    }

    /// Ensure the file at `uri` is indexed before a feature query.
    /// This is the adapter-level warm step — capability traits are pure reads.
    pub(crate) fn ensure_indexed(&self, uri: &Url) {
        self.indexer.ensure_indexed(uri);
    }
}
