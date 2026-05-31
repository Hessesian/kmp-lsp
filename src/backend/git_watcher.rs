use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::MessageType;
use tower_lsp::Client;

use crate::indexer::Indexer;

use super::progress::LspProgressReporter;

/// Resolves the actual git directory from a workspace root.
///
/// Handles both regular repos (`.git/` is a directory) and worktrees/submodules
/// where `.git` is a file containing `gitdir: /path/to/actual/git/dir`.
/// Returns `None` when no git directory can be found.
fn resolve_git_dir(root: &Path) -> Option<PathBuf> {
    let git_path = root.join(".git");
    if git_path.is_dir() {
        return Some(git_path);
    }
    // Worktree or submodule: `.git` is a file with `gitdir: <path>`.
    if git_path.is_file() {
        let content = std::fs::read_to_string(&git_path).ok()?;
        let git_dir_str = content.trim().strip_prefix("gitdir:")?;
        let git_dir = PathBuf::from(git_dir_str.trim());
        let resolved = if git_dir.is_absolute() {
            git_dir
        } else {
            root.join(git_dir)
        };
        if resolved.is_dir() {
            return Some(resolved);
        }
    }
    None
}

/// Resolves the "refs root" directory for a given git dir.
///
/// In worktrees, the per-worktree git dir (`.git/worktrees/<name>/`) has a
/// `commondir` file pointing to the main git dir where all refs live.
/// In regular repos and submodules, refs are directly under `git_dir`.
fn refs_dir(git_dir: &Path) -> PathBuf {
    let commondir_file = git_dir.join("commondir");
    if let Ok(content) = std::fs::read_to_string(&commondir_file) {
        let rel = content.trim();
        let candidate = if rel.starts_with('/') {
            PathBuf::from(rel)
        } else {
            git_dir.join(rel)
        };
        if candidate.is_dir() {
            return candidate;
        }
    }
    git_dir.to_path_buf()
}

/// Resolves the current git commit SHA from `git_dir/HEAD`.
///
/// For a symbolic ref (`ref: refs/heads/main`), reads the pointed-to ref file
/// from the refs root (which may differ from `git_dir` in worktrees).
/// For a detached HEAD, returns the raw SHA from HEAD itself.
/// Returns `None` if HEAD cannot be read.
fn read_git_commit(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(ref_path) = head.strip_prefix("ref: ") {
        // Symbolic ref — resolve to actual commit SHA.
        // Use refs_dir() so worktree git dirs look in the common git dir for refs.
        let sha = std::fs::read_to_string(refs_dir(git_dir).join(ref_path))
            .ok()
            .map(|s| s.trim().to_string());
        // If the ref file doesn't exist yet (empty branch), fall back to
        // the symbolic ref itself so we still detect the branch name change.
        sha.or_else(|| Some(head.to_string()))
    } else {
        // Detached HEAD.
        Some(head.to_string())
    }
}

/// Spawns a background task that polls `.git/HEAD` every 2 seconds.
/// When the resolved commit SHA changes (branch switch or new commit), clears
/// the in-memory index and triggers a full workspace reindex.
pub(super) fn spawn_git_head_watcher(root: PathBuf, indexer: Arc<Indexer>, client: Client) {
    let Some(git_dir) = resolve_git_dir(&root) else {
        return;
    };
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
                    "kmp-lsp: branch changed, reindexing workspace…",
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
