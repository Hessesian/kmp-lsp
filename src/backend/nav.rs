use std::sync::Arc;

use super::cursor::CursorContext;
use super::Backend;
use crate::features::definition as def;
use crate::features::implementation as imp;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
impl Backend {
    pub(super) async fn goto_definition_impl(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pp = params.text_document_position_params;
        let uri = &pp.text_document.uri;
        let position = pp.position;

        let Some(ctx) = CursorContext::build(&self.indexer, uri, position) else {
            return Ok(None);
        };

        let response = def::find_definition(&ctx, &*self.indexer, uri, position).await;
        Ok(self.rewrite_jar_targets_off_thread(response).await)
    }

    pub(super) async fn goto_implementation_impl(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pp = params.text_document_position_params;
        let uri = &pp.text_document.uri;
        let position = pp.position;

        let Some(ctx) = CursorContext::build(&self.indexer, uri, position) else {
            return Ok(None);
        };

        let response =
            imp::find_implementation(&ctx.word, &*self.indexer, uri, position.line).await;
        Ok(self.rewrite_jar_targets_off_thread(response).await)
    }

    /// Rewrite `jar:…!/Foo.kt` definition targets into extracted `file://` ones the
    /// editor can actually open. The extraction does zip + disk I/O, so it runs on a
    /// blocking thread to avoid stalling the Tokio executor on the request path.
    async fn rewrite_jar_targets_off_thread(
        &self,
        response: Option<GotoDefinitionResponse>,
    ) -> Option<GotoDefinitionResponse> {
        let response = response?;
        let indexer = Arc::clone(&self.indexer);
        tokio::task::spawn_blocking(move || {
            crate::jar_extract::rewrite_jar_definitions(&indexer, response)
        })
        .await
        .ok()
    }
}
