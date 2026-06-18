use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::*;

use super::Backend;
use crate::workspace::{Config, Event};

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
        if std::env::var("KMP_LSP_PREFER_CONFIG_ROOT").is_ok() {
            // Copilot CLI mode: config file overrides client rootUri so
            // kmp_lsp_set_workspace works correctly.
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
        std::env::var("KMP_LSP_WORKSPACE_ROOT")
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
        let config_file = crate::util::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".config/kmp-lsp/workspace");
        std::fs::read_to_string(config_file)
            .ok()
            .map(|workspace_root| PathBuf::from(workspace_root.trim()))
            .filter(|workspace_root| workspace_root.is_dir())
    }

    pub(super) async fn configure_initialized_workspace(
        &self,
        params: &InitializeParams,
        workspace_root: &Path,
        workspace_pinned: bool,
    ) {
        let (explicit_source_paths, ignore_patterns, jar_paths) =
            self.apply_initialization_options(params.initialization_options.as_ref());
        if self
            .event_tx
            .send(Event::Initialize {
                config: Config {
                    root: workspace_root.to_path_buf(),
                    explicit_source_paths,
                    ignore_patterns,
                    jar_paths,
                    pin_workspace: workspace_pinned,
                },
                completion_tx: None,
            })
            .await
            .is_err()
        {
            log::error!(
                "configure_initialized_workspace: workspace actor channel closed; \
                 indexing will not start"
            );
        }
    }

    fn apply_initialization_options(
        &self,
        initialization_options: Option<&serde_json::Value>,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let ignore_patterns =
            Self::collect_indexing_option_strings(initialization_options, "ignorePatterns")
                .unwrap_or_default();
        if !ignore_patterns.is_empty() {
            log::info!("ignorePatterns: {:?}", ignore_patterns);
        }

        let explicit_source_paths =
            Self::collect_indexing_option_strings(initialization_options, "sourcePaths")
                .unwrap_or_default();
        if !explicit_source_paths.is_empty() {
            log::info!("sourcePaths: {:?}", explicit_source_paths);
        }

        let jar_paths = Self::collect_indexing_option_strings(initialization_options, "jarPaths")
            .unwrap_or_default();
        if !jar_paths.is_empty() {
            log::info!("jarPaths: {:?}", jar_paths);
        }

        (explicit_source_paths, ignore_patterns, jar_paths)
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
