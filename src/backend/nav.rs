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

        // Rewrite `jar:…!/Foo.kt` targets into extracted `file://` ones the editor
        // can actually open (in-memory library sources aren't navigable otherwise).
        Ok(def::find_definition(&ctx, &*self.indexer, uri, position)
            .await
            .map(|response| crate::jar_extract::rewrite_jar_definitions(&self.indexer, response)))
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

        Ok(
            imp::find_implementation(&ctx.word, &*self.indexer, uri, position.line)
                .await
                .map(|response| {
                    crate::jar_extract::rewrite_jar_definitions(&self.indexer, response)
                }),
        )
    }
}
