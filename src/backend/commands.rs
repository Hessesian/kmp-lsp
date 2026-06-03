//! LSP command execution — `execute_command` handler extracted from
//! `backend/mod.rs` for readability.

use std::sync::Arc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use super::progress::LspProgressReporter;
use super::Backend;
use crate::indexer::workspace_cache_path;

impl Backend {
    /// Execute a workspace command (reindex, clearCache).
    pub(super) async fn execute_command_impl(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        if params.command == "kotlin-lsp/reindex" {
            let root = self
                .indexer
                .workspace_root
                .read()
                .expect("workspace_root lock poisoned")
                .clone();
            let Some(root) = root else {
                self.client
                    .show_message(MessageType::WARNING, "kotlin-lsp: no workspace root set")
                    .await;
                return Ok(None);
            };
            let idx = Arc::clone(&self.indexer);
            let client = self.client.clone();
            idx.reset_index_state();
            tokio::spawn(async move {
                idx.index_workspace(&root, Arc::new(LspProgressReporter(client)))
                    .await;
            });
            self.client
                .show_message(MessageType::INFO, "kotlin-lsp: reindexing workspace…")
                .await;
        } else if params.command == "kotlin-lsp/clearCache" {
            // Optional arg: path to workspace root. If absent, clear current root's cache.
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
                            format!("kotlin-lsp/clearCache: not a directory: {}", pb.display()),
                        )
                        .await;
                    return Ok(None);
                }
                pb
            } else {
                // Acquire current root upfront and drop the lock before any await.
                let current_root_opt = {
                    self.indexer
                        .workspace_root
                        .read()
                        .expect("workspace_root lock poisoned")
                        .clone()
                };
                match current_root_opt {
                    Some(r) => r,
                    None => {
                        self.client
                            .show_message(
                                MessageType::WARNING,
                                "kotlin-lsp/clearCache: no workspace root set and no path provided",
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
                        self.client
                            .show_message(
                                MessageType::INFO,
                                format!("kotlin-lsp: cleared cache for {}", target_root.display()),
                            )
                            .await;
                    }
                    Err(e) => {
                        log::warn!("Failed to remove cache dir {}: {}", cache_dir.display(), e);
                        self.client
                            .show_message(
                                MessageType::WARNING,
                                format!("kotlin-lsp: failed to clear cache: {}", e),
                            )
                            .await;
                    }
                }
            } else {
                self.client
                    .show_message(
                        MessageType::WARNING,
                        "kotlin-lsp/clearCache: cache path parent missing",
                    )
                    .await;
            }
        }
        Ok(None)
    }
}
