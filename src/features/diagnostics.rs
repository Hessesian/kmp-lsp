//! Shared diagnostics coordinator: one source of truth for every diagnostics
//! surface (push `publishDiagnostics` on didOpen/didChange/republish, pull
//! `textDocument/diagnostic`, CLI `diagnose`).
//!
//! Previously each surface assembled its own list of per-feature calls, and
//! the lists drifted: the CLI never ran `unused-import` or `missing-package`,
//! and the pull request path did not exist at all, so pull-based clients
//! (oh-my-pi's "LSP diagnostics") always saw a clean bill. Every surface
//! below funnels through [`semantic_diagnostics`] / [`full_diagnostics`].

use std::sync::atomic::Ordering;

use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::backend::helpers::syntax_diagnostics;
use crate::features::call_arg_diagnostics::call_arg_diagnostics;
use crate::features::code_actions::missing_package_diagnostic;
use crate::features::fill_when::when_diagnostics_with_doc;
use crate::features::missing_import_diagnostics::missing_import_diagnostics;
use crate::features::nullable_call_diagnostics::nullable_dot_call_diagnostics;
use crate::features::unused_import_diagnostics::unused_import_diagnostics;
use crate::indexer::live_tree::LiveDoc;
use crate::indexer::Indexer;

/// The five CST/index diagnostics that share one parsed [`LiveDoc`].
///
/// `when` runs on the caller-provided document (not `indexer.live_doc`),
/// so pull requests for not-currently-open files still get `when` coverage.
pub(crate) fn semantic_diagnostics(indexer: &Indexer, uri: &Url, doc: &LiveDoc) -> Vec<Diagnostic> {
    let mut diagnostics = when_diagnostics_with_doc(indexer, uri, doc);
    diagnostics.extend(call_arg_diagnostics(indexer, uri, doc));
    diagnostics.extend(nullable_dot_call_diagnostics(indexer, uri, doc));
    diagnostics.extend(missing_import_diagnostics(indexer, uri, doc));
    diagnostics.extend(unused_import_diagnostics(doc));
    diagnostics
}

/// Full diagnostics for one URI, as served by `textDocument/diagnostic`.
///
/// Ensures the file is indexed, then combines syntax errors from the index
/// with [`semantic_diagnostics`] over the current content plus the
/// missing-package hint. While the workspace scan is still in progress the
/// index is partial, so (mirroring the push path) only syntax + package
/// diagnostics are returned.
pub(crate) fn full_diagnostics(indexer: &Indexer, uri: &Url) -> Vec<Diagnostic> {
    indexer.ensure_indexed(uri);

    let mut diagnostics = indexer
        .files
        .get(uri.as_str())
        .map(|file_data| syntax_diagnostics(&file_data.syntax_errors))
        .unwrap_or_default();

    let indexing = indexer.indexing_in_progress.load(Ordering::Acquire);

    if !indexing {
        if let Some(doc) = indexer.live_doc_or_parse(uri) {
            diagnostics.extend(semantic_diagnostics(indexer, uri, &doc));
        }
    }

    let lines: Vec<String> = indexer
        .mem_lines_for(uri.as_str())
        .as_ref()
        .map(|lines| lines.as_ref().clone())
        .unwrap_or_default();
    if let Some(package_diagnostic) = missing_package_diagnostic(&lines, uri) {
        diagnostics.push(package_diagnostic);
    }

    diagnostics
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
