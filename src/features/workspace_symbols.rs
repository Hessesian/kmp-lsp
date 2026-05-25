//! Workspace symbol search — index-first with rg cold-start fallback.
//!
//! Entry point: [`compute_workspace_symbols`].
//! Bounds: `SymbolIndex + SearchAccess`.

use std::sync::Arc;

use crate::rg;
use crate::types::{FileData, SymbolEntry};
use tower_lsp::lsp_types::{Location, SymbolInformation, Url};

use super::traits::{SearchAccess, SymbolIndex};

/// Maximum results returned from the index scan.
const WORKSPACE_SYMBOL_CAP: usize = 512;

/// Timeout for the rg cold-start fallback — prevents runaway scans on large repos.
const RG_FALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Search workspace symbols by `query`, returning up to `WORKSPACE_SYMBOL_CAP` results.
///
/// Index-first: scans all indexed files for matching symbols.
/// rg fallback: fired **only on cold start** (index not yet populated) so that
/// editors get results before the initial scan completes.  When the index already
/// has files, an empty result means the symbol is genuinely absent — no rg scan.
/// The fallback is capped at [`RG_FALLBACK_TIMEOUT`] to prevent runaway scans on
/// large repos.
pub(crate) async fn compute_workspace_symbols(
    query_str: String,
    index: &(impl SymbolIndex + SearchAccess),
) -> Vec<SymbolInformation> {
    let query = WorkspaceSymbolQuery::new(query_str);
    let (mut results, index_populated) = collect_index_symbols(&query, index);
    if results.is_empty() && !index_populated {
        results = rg_symbol_search(&query, index).await;
    }
    results
}

// ─── Index scan ──────────────────────────────────────────────────────────────

/// Returns `(matching_symbols, index_populated)`.
///
/// `index_populated` is `true` when at least one file has been indexed,
/// regardless of whether any symbol matched the query.
fn collect_index_symbols(
    query: &WorkspaceSymbolQuery,
    index: &impl SymbolIndex,
) -> (Vec<SymbolInformation>, bool) {
    let mut results = Vec::new();
    let mut index_populated = false;
    let mut f = |uri_str: &str, data: &Arc<FileData>| {
        index_populated = true;
        let Some(uri) = parse_uri(uri_str) else {
            return true;
        };
        for symbol in &data.symbols {
            if !query.matches(symbol) {
                continue;
            }
            results.push(symbol_information(&uri, symbol));
            if results.len() >= WORKSPACE_SYMBOL_CAP {
                return false;
            }
        }
        true
    };
    index.for_each_indexed_file(&mut f);
    results.sort_by(|a, b| a.name.cmp(&b.name));
    (results, index_populated)
}

// ─── rg fallback ─────────────────────────────────────────────────────────────

async fn rg_symbol_search(
    query: &WorkspaceSymbolQuery,
    index: &impl SearchAccess,
) -> Vec<SymbolInformation> {
    if !query.allows_rg_fallback() {
        return vec![];
    }
    let (workspace_root, source_roots, ignore_matcher) = index.rg_scope_for_path(None);
    let name = query.name.clone();
    let task = tokio::task::spawn_blocking(move || {
        rg::rg_find_definition(
            &name,
            workspace_root.as_deref(),
            &source_roots,
            ignore_matcher.as_deref(),
        )
    });
    let locations = match tokio::time::timeout(RG_FALLBACK_TIMEOUT, task).await {
        Ok(Ok(locs)) => locs,
        Ok(Err(_)) | Err(_) => {
            log::warn!("workspace_symbols: rg cold-start fallback timed out or failed");
            vec![]
        }
    };

    locations
        .into_iter()
        .map(|loc| rg_symbol_information(query.name.clone(), loc))
        .collect()
}

// ─── Query type ──────────────────────────────────────────────────────────────

/// Parsed and lowercased workspace symbol query.
#[derive(Clone)]
struct WorkspaceSymbolQuery {
    raw: String,
    qualifier: Option<String>,
    name: String,
}

impl WorkspaceSymbolQuery {
    fn new(query: String) -> Self {
        let raw = query.to_lowercase();
        if let Some(dot) = raw.rfind('.') {
            return Self {
                qualifier: Some(raw[..dot].to_owned()),
                name: raw[dot + 1..].to_owned(),
                raw,
            };
        }
        Self {
            name: raw.clone(),
            raw,
            qualifier: None,
        }
    }

    fn matches(&self, symbol: &SymbolEntry) -> bool {
        if self.raw.is_empty() {
            return true;
        }
        let name = symbol.name.to_lowercase();
        if let Some(qualifier) = self.qualifier.as_deref() {
            return name.contains(&self.name) && symbol.detail.to_lowercase().contains(qualifier);
        }
        name.contains(&self.raw)
    }

    fn allows_rg_fallback(&self) -> bool {
        !self.raw.is_empty() && self.qualifier.is_none()
    }
}

// ─── LSP conversion helpers ──────────────────────────────────────────────────

fn parse_uri(uri_str: &str) -> Option<Url> {
    Url::parse(uri_str)
        .ok()
        .or_else(|| Url::from_file_path(uri_str).ok())
}

#[allow(deprecated)] // `deprecated` superseded by `tags` in LSP 3.16+
fn symbol_information(uri: &Url, symbol: &SymbolEntry) -> SymbolInformation {
    SymbolInformation {
        name: symbol.name.clone(),
        kind: symbol.kind,
        tags: None,
        deprecated: None,
        location: Location {
            uri: uri.clone(),
            range: symbol.selection_range,
        },
        container_name: (!symbol.detail.is_empty()).then(|| symbol.detail.clone()),
    }
}

#[allow(deprecated)]
fn rg_symbol_information(name: String, location: Location) -> SymbolInformation {
    SymbolInformation {
        name,
        kind: tower_lsp::lsp_types::SymbolKind::FILE,
        tags: None,
        deprecated: None,
        location,
        container_name: Some("rg fallback".to_string()),
    }
}
