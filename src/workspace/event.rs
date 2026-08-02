//! [`Event`] — the sealed set of workspace-level state mutations.
//!
//! Every write to workspace state goes through one of these variants.
//! Adding a new variant produces a compile error in [`super::Actor::handle_event`]
//! until the handler is implemented — this is the key correctness invariant.

use std::path::PathBuf;

use tokio::sync::oneshot;
use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, Url};

use super::Config;

/// All workspace-level mutations, serialised through [`Actor`].
pub(crate) enum Event {
    /// Configure the workspace and start an initial scan.
    ///
    /// Must be the first event sent to a fresh actor. Subsequent `Initialize`
    /// events switch the root and restart the scan, discarding old source paths.
    Initialize {
        config: Config,
        completion_tx: Option<oneshot::Sender<()>>,
    },

    /// Re-scan the current workspace from scratch.
    ///
    /// Equivalent to the `kmp-lsp/reindex` execute-command. Keeps the
    /// long-lived `Indexer` so live-document state is preserved.
    // Not constructed by production code today: `kmp-lsp/reindex`
    // (backend/commands.rs) and the git-HEAD watcher (backend/git_watcher.rs)
    // both call the indexer directly instead of routing through the actor.
    // `Actor::handle_event` still matches this variant exhaustively, so
    // wiring either caller through `Event::Reindex` is a drop-in change.
    #[allow(dead_code)]
    Reindex,

    /// Switch to a new workspace root and restart the scan.
    ///
    /// Source paths are re-resolved via `Config::resolve_sources` for
    /// the new root. Existing explicit `source_paths_raw` are discarded because
    /// they were relative to the old root.
    // Constructed only by tests (`actor_tests.rs`) today; no production
    // caller switches workspace roots via the actor yet.
    #[allow(dead_code)]
    ChangeRoot { root: PathBuf },

    /// Store live document state and schedule indexing for a newly opened file.
    FileOpened {
        uri: Url,
        language_id: String,
        content: String,
    },

    /// Update live document state and debounce re-indexing after edits.
    FileChanged {
        uri: Url,
        changes: Vec<TextDocumentContentChangeEvent>,
    },

    /// Re-index the current on-disk content for a saved file.
    FileSaved { uri: Url },

    /// Drop live document state for a closed file.
    FileClosed { uri: Url },

    /// Remove a file from the in-memory index after a workspace-watched deletion.
    FileDeleted { uri: Url },
}
