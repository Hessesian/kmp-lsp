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
            position.line,
            params.context.include_declaration,
            &*self.indexer,
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
}

#[cfg(test)]
#[path = "handlers_tests.rs"]
mod tests;
