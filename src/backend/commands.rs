use std::path::{Path, PathBuf};

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use super::Backend;
use crate::indexer::workspace_cache_path;
use crate::workspace::Event;

impl Backend {
    pub(super) async fn execute_command_impl(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            "kmp-lsp/reindex" => self.handle_reindex_command().await,
            "kmp-lsp/clearCache" => self.handle_clear_cache_command(params).await,
            _ => Ok(None),
        }
    }

    async fn handle_reindex_command(&self) -> Result<Option<serde_json::Value>> {
        if self.indexer.workspace_root.get().is_none() {
            self.client
                .show_message(MessageType::WARNING, "kmp-lsp: no workspace root set")
                .await;
            return Ok(None);
        }
        self.send_reindex_event(
            "kmp-lsp: reindexing workspace…",
            "kmp-lsp: reindex failed — the workspace actor is not running",
        )
        .await;
        Ok(None)
    }

    async fn handle_clear_cache_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        let Some(target_root) = self.resolve_clear_cache_target_root(&params).await else {
            return Ok(None);
        };

        let cache_path = workspace_cache_path(&target_root);
        let Some(cache_dir) = cache_path.parent() else {
            self.client
                .show_message(
                    MessageType::WARNING,
                    "kmp-lsp/clearCache: cache path parent missing",
                )
                .await;
            return Ok(None);
        };

        // Run on the blocking pool: `remove_dir_all` is synchronous I/O and a
        // large cache directory could otherwise stall a tokio worker thread.
        let cache_dir_owned = cache_dir.to_path_buf();
        let remove_result =
            tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&cache_dir_owned))
                .await
                .unwrap_or_else(|join_error| Err(std::io::Error::other(join_error)));

        // clearCache must be idempotent: clearing an already-clear (or
        // never-created) cache directory is success, not failure.
        if let Err(error) = remove_result {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!(
                    "Failed to remove cache dir {}: {}",
                    cache_dir.display(),
                    error
                );
                self.client
                    .show_message(
                        MessageType::WARNING,
                        format!("kmp-lsp: failed to clear cache: {error}"),
                    )
                    .await;
                return Ok(None);
            }
        }
        log::info!("Cleared workspace cache directory: {}", cache_dir.display());

        if !self.is_current_workspace_root(&target_root) {
            self.client
                .show_message(
                    MessageType::INFO,
                    format!("kmp-lsp: cleared cache for {}", target_root.display()),
                )
                .await;
            return Ok(None);
        }

        // Reindex immediately so the next scan is a cold scan that discovers
        // all files instead of relying on stale cache.
        self.send_reindex_event(
            "kmp-lsp: cache cleared, reindexing workspace…",
            "kmp-lsp: cache cleared, but reindex failed — the workspace actor is not running",
        )
        .await;
        Ok(None)
    }

    /// Resolve `clearCache`'s target root: an explicit path argument
    /// (validated as a directory), or the current workspace root when no
    /// argument is given. Shows a `WARNING` and returns `None` for every
    /// failure case — the caller just propagates `None` up as `Ok(None)`.
    async fn resolve_clear_cache_target_root(
        &self,
        params: &ExecuteCommandParams,
    ) -> Option<PathBuf> {
        let explicit_path = params
            .arguments
            .first()
            .and_then(|value| value.as_str())
            .map(PathBuf::from);

        let Some(explicit_path) = explicit_path else {
            let Some(current_root) = self.indexer.workspace_root.get() else {
                self.client
                    .show_message(
                        MessageType::WARNING,
                        "kmp-lsp/clearCache: no workspace root set and no path provided",
                    )
                    .await;
                return None;
            };
            return Some(current_root);
        };

        if !explicit_path.is_dir() {
            self.client
                .show_message(
                    MessageType::WARNING,
                    format!(
                        "kmp-lsp/clearCache: not a directory: {}",
                        explicit_path.display()
                    ),
                )
                .await;
            return None;
        }
        Some(explicit_path)
    }

    /// Whether `root` (canonicalized on both sides, so a relative/symlinked/
    /// differently-normalized path still matches) is the currently active
    /// workspace root.
    fn is_current_workspace_root(&self, root: &Path) -> bool {
        let canonicalize = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.indexer
            .workspace_root
            .get()
            .is_some_and(|current_root| canonicalize(&current_root) == canonicalize(root))
    }

    /// Send `Event::Reindex` through the actor and show the resulting
    /// success/failure message. Shared by `kmp-lsp/reindex` and
    /// `kmp-lsp/clearCache` (on the current root). Must go through the
    /// actor rather than calling `Indexer::index_workspace_full` directly —
    /// see that method's doc comment for why.
    async fn send_reindex_event(&self, success_message: &str, failure_message: &str) {
        if self.event_tx.send(Event::Reindex).await.is_ok() {
            self.client
                .show_message(MessageType::INFO, success_message)
                .await;
        } else {
            self.client
                .show_message(MessageType::ERROR, failure_message)
                .await;
        }
    }
}
