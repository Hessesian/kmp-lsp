use super::cursor::CursorContext;
use super::Backend;
use crate::inlay_hints::compute_inlay_hints;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

impl Backend {
    pub(super) async fn hover_impl(&self, params: HoverParams) -> Result<Option<Hover>> {
        let pp = params.text_document_position_params;
        let uri = &pp.text_document.uri;
        let position = pp.position;
        let workspace = self.indexer.as_ref();

        let context_started = std::time::Instant::now();
        let Some(ctx) = CursorContext::build(&self.indexer, uri, position) else {
            return Ok(None);
        };
        let context_build_ms = context_started.elapsed().as_millis();
        let hover_started = std::time::Instant::now();
        let result = crate::features::hover::compute_hover(workspace, &ctx, uri, position);
        let compute_hover_ms = hover_started.elapsed().as_millis();
        if context_build_ms + compute_hover_ms > crate::backend::panic_guard::SLOW_THRESHOLD_MS {
            log::warn!(
                "SLOW hover: CursorContext::build {context_build_ms}ms + compute_hover {compute_hover_ms}ms (line {})",
                position.line
            );
        }
        Ok(result)
    }

    pub(super) async fn references_impl(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        // Ensure the file is in the index before scope resolution — `did_open`
        // is async (actor-queued), so the index may not have it yet.
        //
        // Skip this for library / extracted JAR sources (the user did go-to-definition
        // into a dependency's source and invoked find-references there): indexing the
        // extracted `file://` copy would duplicate every library definition, since the
        // symbols are already indexed under their original `jar:` URI. Cursor-word
        // resolution still works via `lines_for`'s on-disk fallback, and the reference
        // scope is derived from the JAR symbol index rather than this (library) file.
        if !self.indexer.is_library_uri(uri) && !crate::jar_extract::is_extracted_jar_source(uri) {
            self.indexer.ensure_indexed(uri);
        }

        let Some(ctx) = CursorContext::build(&self.indexer, uri, position) else {
            return Ok(None);
        };

        let locations = crate::features::references::find_references_with_qualifier(
            &ctx.word,
            ctx.qualifier.as_deref(),
            uri,
            position,
            params.context.include_declaration,
            &self.indexer,
        )
        .await;

        Ok((!locations.is_empty()).then_some(locations))
    }

    pub(super) async fn document_symbol_impl(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let mut symbols = self.indexer.file_symbols(uri);
        if symbols.is_empty() {
            if let Ok(path) = uri.to_file_path() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    self.indexer.index_content(uri, &content);
                    symbols = self.indexer.file_symbols(uri);
                }
            }
        }
        Ok(crate::features::symbols::compute_document_symbols(symbols))
    }

    pub(super) async fn inlay_hint_impl(
        &self,
        params: InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let range = params.range;
        let compute_started = std::time::Instant::now();
        let hints = compute_inlay_hints(&self.indexer, uri, range);
        let compute_ms = compute_started.elapsed().as_millis();
        if compute_ms > crate::backend::panic_guard::SLOW_THRESHOLD_MS {
            log::warn!(
                "SLOW inlay compute: {compute_ms}ms ({} hints, range {}..{})",
                hints.len(),
                range.start.line,
                range.end.line
            );
        }
        Ok(if hints.is_empty() { None } else { Some(hints) })
    }

    pub(super) async fn symbol_impl(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let results = crate::features::workspace_symbols::compute_workspace_symbols(
            params.query,
            self.indexer.as_ref(),
        )
        .await;
        Ok((!results.is_empty()).then_some(results))
    }

    pub(super) async fn signature_help_impl(
        &self,
        params: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let result = crate::features::signature_help::compute_signature_help(
            uri,
            pos,
            self.indexer.as_ref(),
        );
        log::debug!(
            "signatureHelp uri={} line={} char={} → {}",
            uri,
            pos.line,
            pos.character,
            if result.is_some() { "Some" } else { "None" }
        );
        Ok(result)
    }

    pub(super) async fn folding_range_impl(
        &self,
        params: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        Ok(crate::features::folding::compute_folding_ranges(
            uri,
            self.indexer.as_ref(),
        ))
    }

    // ── textDocument/documentHighlight ───────────────────────────────────────

    pub(super) async fn document_highlight_impl(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        Ok(crate::features::highlight::compute_document_highlight(
            uri,
            pos,
            self.indexer.as_ref(),
        ))
    }

    // ── textDocument/diagnostic (pull) ───────────────────────────────────────
    //
    // Pull-based clients (oh-my-pi's "LSP diagnostics") ask for diagnostics
    // on demand instead of waiting for pushed `publishDiagnostics`. This
    // serves the exact same [`crate::features::diagnostics::full_diagnostics`]
    // set the push path publishes, so pull can never report a clean bill the
    // push path would contradict. CPU-bound work runs in `spawn_blocking`;
    // a join failure degrades to an empty report rather than an RPC error.
    //
    // Cold-start race: omp lazily spawns the server on this very request,
    // so the workspace scan is typically still in flight and the index is
    // partial. Push compensates by republishing when the scan completes;
    // pull has no second chance, so wait bounded for the scan before
    // answering rather than returning a syntax-only clean bill. The deadline
    // stays inside omp's own default request timeout (20s): waiting longer
    // would turn a slow scan into a client-side timeout (no answer at all)
    // instead of a fast fallback answer. On expiry fall through to whatever
    // is available.
    pub(super) async fn diagnostic_impl(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        const SCAN_WAIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(18);
        const SCAN_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
        let scan_wait_start = std::time::Instant::now();
        while self
            .indexer
            .indexing_in_progress
            .load(std::sync::atomic::Ordering::Acquire)
        {
            if scan_wait_start.elapsed() >= SCAN_WAIT_DEADLINE {
                log::debug!("diagnostic: workspace scan still in progress after 18s; answering with available diagnostics");
                break;
            }
            tokio::time::sleep(SCAN_POLL_INTERVAL).await;
        }
        let uri = params.text_document.uri;
        let indexer = std::sync::Arc::clone(&self.indexer);
        let items = tokio::task::spawn_blocking(move || {
            crate::features::diagnostics::full_diagnostics(&indexer, &uri)
        })
        .await
        .unwrap_or_default();
        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            }),
        ))
    }
}

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
