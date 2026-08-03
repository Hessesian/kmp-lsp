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
        if params.command == "kmp-lsp/reindex" {
            if self.indexer.workspace_root.get().is_none() {
                self.client
                    .show_message(MessageType::WARNING, "kmp-lsp: no workspace root set")
                    .await;
                return Ok(None);
            }
            // Routed through the actor (not a direct reset_index_state() +
            // index_workspace() call) so handle_reindex's Tier-1/materialization
            // clearing and spawn_jar_indexing() re-crawl actually run — a direct
            // call here bypassed both, silently freezing JAR/library data at
            // whatever it was during LSP startup for the rest of the session.
            if self.event_tx.send(Event::Reindex).await.is_ok() {
                self.client
                    .show_message(MessageType::INFO, "kmp-lsp: reindexing workspace…")
                    .await;
            } else {
                self.client
                    .show_message(
                        MessageType::ERROR,
                        "kmp-lsp: reindex failed — the workspace actor is not running",
                    )
                    .await;
            }
        } else if params.command == "kmp-lsp/clearCache" {
            let arg = params
                .arguments
                .first()
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let target_root = if let Some(p) = arg {
                let pb = std::path::PathBuf::from(p);
                if !pb.is_dir() {
                    self.client
                        .show_message(
                            MessageType::WARNING,
                            format!("kmp-lsp/clearCache: not a directory: {}", pb.display()),
                        )
                        .await;
                    return Ok(None);
                }
                pb
            } else {
                let current_root_opt = self.indexer.workspace_root.get();
                match current_root_opt {
                    Some(r) => r,
                    None => {
                        self.client
                            .show_message(
                                MessageType::WARNING,
                                "kmp-lsp/clearCache: no workspace root set and no path provided",
                            )
                            .await;
                        return Ok(None);
                    }
                }
            };
            let cache_path = workspace_cache_path(&target_root);
            if let Some(cache_dir) = cache_path.parent() {
                match std::fs::remove_dir_all(cache_dir) {
                    Ok(_) => {
                        log::info!("Cleared workspace cache directory: {}", cache_dir.display());
                        // Reindex immediately so the next scan is a cold scan
                        // that discovers all files instead of relying on stale cache.
                        // Canonicalize both sides so a relative / symlinked / differently
                        // normalized path still matches the active root.
                        let canon =
                            |p: &std::path::Path| p.canonicalize().unwrap_or(p.to_path_buf());
                        let is_current_root = self
                            .indexer
                            .workspace_root
                            .get()
                            .is_some_and(|r| canon(&r) == canon(&target_root));
                        if is_current_root {
                            // Routed through the actor for the same reason as
                            // `kmp-lsp/reindex` above: a direct reset_index_state()
                            // + index_workspace() call here never re-triggered JAR
                            // indexing, freezing library data at LSP-startup state.
                            if self.event_tx.send(Event::Reindex).await.is_ok() {
                                self.client
                                    .show_message(
                                        MessageType::INFO,
                                        "kmp-lsp: cache cleared, reindexing workspace…",
                                    )
                                    .await;
                            } else {
                                self.client
                                    .show_message(
                                        MessageType::ERROR,
                                        "kmp-lsp: cache cleared, but reindex failed — the \
                                         workspace actor is not running",
                                    )
                                    .await;
                            }
                        } else {
                            self.client
                                .show_message(
                                    MessageType::INFO,
                                    format!("kmp-lsp: cleared cache for {}", target_root.display()),
                                )
                                .await;
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to remove cache dir {}: {}", cache_dir.display(), e);
                        self.client
                            .show_message(
                                MessageType::WARNING,
                                format!("kmp-lsp: failed to clear cache: {}", e),
                            )
                            .await;
                    }
                }
            } else {
                self.client
                    .show_message(
                        MessageType::WARNING,
                        "kmp-lsp/clearCache: cache path parent missing",
                    )
                    .await;
            }
        }
        Ok(None)
    }
}
