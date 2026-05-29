use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::MessageType;
use tower_lsp::Client;

use crate::indexer::Indexer;

use super::progress::LspProgressReporter;

/// Resolves the current git commit SHA from `.git/HEAD`.
///
/// For a symbolic ref (`ref: refs/heads/main`), reads the pointed-to ref file.
/// For a detached HEAD, returns the raw SHA from HEAD itself.
/// Returns `None` if the git directory doesn't exist or files can't be read.
fn read_git_commit(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(ref_path) = head.strip_prefix("ref: ") {
        // Symbolic ref — resolve to actual commit SHA.
        std::fs::read_to_string(git_dir.join(ref_path))
            .ok()
            .map(|s| s.trim().to_string())
            // If the ref file doesn't exist yet (empty branch), fall back to
            // the symbolic ref itself so we still detect the branch name change.
            .or_else(|| Some(head.to_string()))
    } else {
        // Detached HEAD.
        Some(head.to_string())
    }
}

/// Spawns a background task that polls `.git/HEAD` every 2 seconds.
/// When the resolved commit SHA changes (branch switch or new commit), clears
/// the in-memory index and triggers a full workspace reindex.
pub(super) fn spawn_git_head_watcher(root: PathBuf, indexer: Arc<Indexer>, client: Client) {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return;
    }
    let mut last_commit = read_git_commit(&git_dir).unwrap_or_default();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let current = read_git_commit(&git_dir).unwrap_or_default();
            if current.is_empty() || current == last_commit {
                continue;
            }
            last_commit = current;
            log::info!("git HEAD changed — triggering workspace reindex");
            client
                .show_message(
                    MessageType::INFO,
                    "kotlin-lsp: branch changed, reindexing workspace…",
                )
                .await;
            let idx = Arc::clone(&indexer);
            let root_clone = root.clone();
            let client_clone = client.clone();
            idx.reset_index_state();
            tokio::spawn(async move {
                idx.index_workspace(&root_clone, Arc::new(LspProgressReporter(client_clone)))
                    .await;
            });
        }
    });
}
