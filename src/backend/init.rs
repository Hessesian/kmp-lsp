//! Workspace initialization and configuration.
//!
//! Extracted from `backend/mod.rs` as part of codebase refactoring.

use std::path::{Path, PathBuf};

use crate::types::InlayHintConfig;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tower_lsp::lsp_types::*;

use super::Backend;
use crate::indexer::IgnoreMatcher;

impl Backend {
    pub(super) fn detect_snippet_support(params: &InitializeParams) -> bool {
        params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.completion.as_ref())
            .and_then(|completion| completion.completion_item.as_ref())
            .and_then(|completion_item| completion_item.snippet_support)
            .unwrap_or(false)
    }

    pub(super) fn resolve_workspace_root(params: &InitializeParams) -> Option<PathBuf> {
        if std::env::var("KOTLIN_LSP_PREFER_CONFIG_ROOT").is_ok() {
            // Copilot CLI mode: config file overrides client rootUri so
            // kotlin_lsp_set_workspace works correctly.
            Self::workspace_root_from_environment()
                .or_else(Self::workspace_root_from_config)
                .or_else(|| Self::workspace_root_from_client(params))
        } else {
            // Editor mode: always honour the client's rootUri.
            Self::workspace_root_from_environment()
                .or_else(|| Self::workspace_root_from_client(params))
                .or_else(Self::workspace_root_from_config)
        }
    }

    fn workspace_root_from_environment() -> Option<PathBuf> {
        std::env::var("KOTLIN_LSP_WORKSPACE_ROOT")
            .ok()
            .map(PathBuf::from)
            .filter(|workspace_root| workspace_root.is_dir())
    }

    fn workspace_root_from_client(params: &InitializeParams) -> Option<PathBuf> {
        Self::initialize_root_uri(params)
            .and_then(|root_uri| root_uri.to_file_path().ok())
            .filter(|workspace_root| workspace_root.is_dir())
            .map(|workspace_root| Self::walk_up_to_git_root(&workspace_root))
    }

    fn initialize_root_uri(params: &InitializeParams) -> Option<Url> {
        params.root_uri.clone().or_else(|| {
            params
                .workspace_folders
                .as_deref()
                .and_then(|workspace_folders| workspace_folders.first())
                .map(|workspace_folder| workspace_folder.uri.clone())
        })
    }

    fn walk_up_to_git_root(workspace_root: &Path) -> PathBuf {
        let mut current_directory = workspace_root;
        loop {
            if current_directory.join(".git").exists() {
                return current_directory.to_path_buf();
            }
            match current_directory.parent() {
                Some(parent_directory) => current_directory = parent_directory,
                None => return workspace_root.to_path_buf(),
            }
        }
    }

    fn workspace_root_from_config() -> Option<PathBuf> {
        let home_directory = std::env::var("HOME")
            .ok()
            .unwrap_or_else(|| "/tmp".to_string());
        let config_file = Path::new(&home_directory).join(".config/kotlin-lsp/workspace");
        std::fs::read_to_string(config_file)
            .ok()
            .map(|workspace_root| PathBuf::from(workspace_root.trim()))
            .filter(|workspace_root| workspace_root.is_dir())
    }

    pub(super) fn configure_initialized_workspace(
        &self,
        params: &InitializeParams,
        workspace_root: &Path,
        workspace_pinned: bool,
    ) {
        self.set_workspace_root(workspace_root.to_path_buf());
        if workspace_pinned {
            self.indexer.workspace_pinned.store(true, Ordering::Relaxed);
        }
        self.apply_initialization_options(params.initialization_options.as_ref(), workspace_root);
        self.spawn_workspace_indexing(workspace_root.to_path_buf(), Vec::new());
    }

    fn apply_initialization_options(
        &self,
        initialization_options: Option<&serde_json::Value>,
        workspace_root: &Path,
    ) {
        if let Some(ignore_patterns) =
            Self::collect_indexing_option_strings(initialization_options, "ignorePatterns")
        {
            log::info!("ignorePatterns: {:?}", ignore_patterns);
            match self.indexer.ignore_matcher.write() {
                Ok(mut ignore_matcher) => {
                    *ignore_matcher = Some(Arc::new(IgnoreMatcher::new(
                        ignore_patterns,
                        workspace_root,
                    )));
                }
                Err(error) => {
                    log::warn!("Failed to update ignore matcher: {error}");
                }
            }
        }

        let all_source_paths =
            Self::collect_all_source_paths(initialization_options, workspace_root);
        let rg_source_roots = Self::collect_workspace_source_roots(workspace_root);

        if !all_source_paths.is_empty() {
            log::info!("sourcePaths (combined): {:?}", all_source_paths);
            match self.indexer.source_paths_raw.write() {
                Ok(mut source_paths_raw) => {
                    *source_paths_raw = all_source_paths;
                }
                Err(error) => {
                    log::warn!("Failed to update source paths: {error}");
                }
            }
        }

        if !rg_source_roots.is_empty() {
            log::info!(
                "workspace sourceRoots for rg scoping: {:?}",
                rg_source_roots
            );
            match self.indexer.workspace_source_roots.write() {
                Ok(mut roots) => {
                    *roots = rg_source_roots;
                }
                Err(error) => {
                    log::warn!("Failed to update workspace_source_roots: {error}");
                }
            }
        }

        // Parse inlay hint configuration from initialization options.
        let inlay_config = InlayHintConfig::from_init_opts(initialization_options);
        match self.inlay_hint_config.write() {
            Ok(mut g) => *g = inlay_config,
            Err(_) => log::error!("inlay_hint_config lock poisoned"),
        }
    }

    /// Build the combined source-paths list used for indexing:
    /// explicit `sourcePaths` + workspace.json modules + build-layout auto-detection
    /// + external library directories (~/.kotlin-lsp/sources, Android SDK).
    fn collect_all_source_paths(
        initialization_options: Option<&serde_json::Value>,
        workspace_root: &Path,
    ) -> Vec<String> {
        let mut paths: Vec<String> =
            Self::collect_indexing_option_strings(initialization_options, "sourcePaths")
                .unwrap_or_default();

        let workspace_json_paths = crate::workspace_json::load_source_paths(workspace_root);
        for path in &workspace_json_paths {
            let s = path.to_string_lossy().into_owned();
            if !paths.contains(&s) {
                paths.push(s);
            }
        }

        if workspace_json_paths.is_empty() {
            for path in crate::workspace_json::detect_build_layout_source_paths(workspace_root) {
                let s = path.to_string_lossy().into_owned();
                if !paths.contains(&s) {
                    paths.push(s);
                }
            }
        }

        if let Some(configured) =
            crate::workspace_json::load_configured_source_paths(workspace_root)
        {
            for p in configured {
                let s = p.to_string_lossy().into_owned();
                if !paths.contains(&s) {
                    paths.push(s);
                }
            }
        } else {
            #[allow(deprecated)]
            if let Some(home) = std::env::home_dir() {
                let default_sources = home.join(".kotlin-lsp").join("sources");
                if default_sources.is_dir() {
                    let s = default_sources.to_string_lossy().into_owned();
                    if !paths.contains(&s) {
                        paths.push(s);
                    }
                }
            }
        }

        for path in crate::workspace_json::detect_android_sdk_source_paths(workspace_root) {
            let s = path.to_string_lossy().into_owned();
            if !paths.contains(&s) {
                paths.push(s);
            }
        }

        paths
    }

    /// Collect workspace source roots for rg scoping.
    ///
    /// Only workspace.json JetBrains module sourceRoots are included — these are paths
    /// the user explicitly configured for the project layout. `initializationOptions.sourcePaths`
    /// is intentionally excluded: it is an additive indexing override for stubs/generated code,
    /// not a scope restriction, and including it would make references/rename skip the rest of
    /// the workspace for any project that adds a single `buildSrc/src` to sourcePaths.
    fn collect_workspace_source_roots(workspace_root: &Path) -> Vec<String> {
        crate::workspace_json::load_source_paths(workspace_root)
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    /// Reload workspace source roots from `workspace_root`'s workspace.json and store them.
    /// Call whenever the active workspace root changes so stale source roots from the previous
    /// project are not used for rg scoping in the new project.
    pub(super) fn reload_workspace_source_roots(&self, workspace_root: &Path) {
        let roots = Self::collect_workspace_source_roots(workspace_root);
        match self.indexer.workspace_source_roots.write() {
            Ok(mut guard) => *guard = roots,
            Err(e) => log::warn!("Failed to update workspace_source_roots: {e}"),
        }
    }

    fn collect_indexing_option_strings(
        initialization_options: Option<&serde_json::Value>,
        option_name: &str,
    ) -> Option<Vec<String>> {
        let option_values = initialization_options?
            .get("indexingOptions")?
            .get(option_name)?
            .as_array()?;
        let collected_values: Vec<String> = option_values
            .iter()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect();
        (!collected_values.is_empty()).then_some(collected_values)
    }
}
