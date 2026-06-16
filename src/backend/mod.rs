use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;
use tower_lsp::lsp_types::*;
use tower_lsp::{async_trait, Client, LanguageServer};

use tower_lsp::jsonrpc::Result;

use crate::indexer::Indexer;
use crate::semantic_tokens;
use crate::workspace::Event;

pub(crate) mod actions;
pub(crate) mod capabilities;
pub(crate) mod commands;
pub(crate) mod cursor;
pub(crate) mod format;
pub(crate) mod git_watcher;
pub(crate) mod handlers;
pub(crate) mod helpers;
pub(crate) mod init;
pub(crate) mod nav;
pub(crate) mod panic_guard;
pub(crate) mod progress;
pub(crate) mod rename;

// Re-export for external callers that use the original crate paths.
pub(crate) use panic_guard::panic_safe;
pub(crate) use progress::LspProgressReporter;

pub(crate) struct Backend {
    pub(super) client: Client,
    pub(super) indexer: Arc<Indexer>,
    event_tx: mpsc::Sender<Event>,
    /// True if the client advertised `snippetSupport: true` during initialize.
    /// Used to decide whether to send `InsertTextFormat::SNIPPET` in completions.
    pub(super) snippet_support: Arc<AtomicBool>,
    /// Per-URI sequence counter for server-side completion debounce.
    pub(super) completion_seq: Arc<dashmap::DashMap<String, u64>>,
    /// Per-URI sequence counter for server-side semantic-tokens debounce.
    /// Rapid didChange events cause nvim to send a semantic-tokens request per
    /// keystroke. Each response is a large data array that blocks nvim's Lua
    /// thread. Coalescing bursts to a single response prevents the popup from
    /// being delayed by a queue of redundant token payloads.
    pub(super) semtok_seq: Arc<dashmap::DashMap<String, u64>>,
}

impl Backend {
    pub(crate) fn new(
        client: Client,
        indexer: Arc<Indexer>,
        event_tx: mpsc::Sender<Event>,
    ) -> Self {
        // Spawn the background enrichment worker and install its handle.
        let handle =
            crate::indexer::enrich::spawn_enrichment_worker(Arc::clone(&indexer), client.clone());
        indexer.set_enrichment_handle(handle);

        Self {
            client,
            indexer,
            event_tx,
            snippet_support: Arc::new(AtomicBool::new(false)),
            completion_seq: Arc::new(dashmap::DashMap::new()),
            semtok_seq: Arc::new(dashmap::DashMap::new()),
        }
    }
}

#[async_trait]
impl LanguageServer for Backend {
    // ── lifecycle ────────────────────────────────────────────────────────────

    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let supports_snippets = Self::detect_snippet_support(&params);
        self.snippet_support
            .store(supports_snippets, Ordering::Relaxed);
        log::info!("client snippet support: {supports_snippets}");

        let resolved_workspace_root = Self::resolve_workspace_root(&params);
        let workspace_pinned = resolved_workspace_root.is_some();
        if let Some(workspace_root) = resolved_workspace_root {
            // Pre-register the indexing progress token BEFORE returning InitializeResult.
            // Clients such as Serena call _indexing_complete.wait() immediately after
            // receiving InitializeResult. The event starts SET, so if workDoneProgress/create
            // only arrives later (from the scan worker's begin()), wait() returns early.
            // Registering here forces the client to clear the event synchronously during
            // the initialize round-trip, guaranteeing wait() blocks until indexing ends.
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                self.client
                    .send_request::<tower_lsp::lsp_types::request::WorkDoneProgressCreate>(
                        WorkDoneProgressCreateParams {
                            token: NumberOrString::String("kmp-lsp/indexing".to_owned()),
                        },
                    ),
            )
            .await;
            self.configure_initialized_workspace(&params, &workspace_root, workspace_pinned)
                .await;
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "kmp-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: capabilities::server_capabilities(),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "kmp-lsp ready")
            .await;
        // NOTE: dynamic capability registration via client.register_capability() is intentionally
        // omitted here. tower-lsp 0.20 panics when the oneshot receiver created by pending.wait()
        // is dropped before the client's response arrives — a race that occurs because tower-lsp
        // fires `initialized` as a fire-and-forget notification (no coroutine keepalive). When
        // the client (e.g. Zed) responds quickly, pending.rs:35 finds a dropped receiver and
        // calls tx.send(r).expect("receiver already dropped"), killing the server process.
        //
        // Clients that natively watch files (Zed, Helix) send workspace/didChangeWatchedFiles
        // without dynamic registration; our did_change_watched_files handler processes those.

        // Watch .git/HEAD (resolved to actual commit SHA) and trigger a full reindex
        // when it changes — branch switches swap many files at once without sending
        // per-file workspace/didChangeWatchedFiles notifications.
        if let Some(root) = self.indexer.workspace_root.get() {
            git_watcher::spawn_git_head_watcher(
                root,
                Arc::clone(&self.indexer),
                self.client.clone(),
            );
        }
    }

    async fn shutdown(&self) -> Result<()> {
        // Spawn cache write in background so the LSP shutdown response is sent
        // immediately. The process stays alive until the `exit` notification
        // arrives, giving the write enough time to complete for typical caches.
        let idx = Arc::clone(&self.indexer);
        tokio::task::spawn_blocking(move || idx.save_cache_to_disk());
        Ok(())
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        panic_safe("execute_command", self.execute_command_impl(params)).await
    }

    // ── document sync ────────────────────────────────────────────────────────

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let _ = self
            .event_tx
            .send(Event::FileOpened {
                uri: params.text_document.uri,
                language_id: params.text_document.language_id,
                content: params.text_document.text,
            })
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Update live_lines synchronously so that any subsequent request
        // (e.g. completion) on the same transport sees the latest content,
        // even before the actor processes the event.
        if let Some(change) = params.content_changes.last() {
            // Clear the stale live_tree first so that live_doc_or_parse falls through
            // to re-parse from fresh live_lines. Without this, a signatureHelp request
            // that arrives before the actor's spawn_live_tree_update completes would
            // see the CST from the *previous* did_open and return None.
            // See: https://github.com/Hessesian/kmp-lsp/issues/124
            self.indexer.remove_live_tree(&params.text_document.uri);
            self.indexer
                .set_live_lines(&params.text_document.uri, &change.text);
        }
        let _ = self
            .event_tx
            .send(Event::FileChanged {
                uri: params.text_document.uri,
                changes: params.content_changes,
            })
            .await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let _ = self
            .event_tx
            .send(Event::FileClosed {
                uri: params.text_document.uri,
            })
            .await;
    }

    // ── textDocument/didSave ─────────────────────────────────────────────────

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let _ = self
            .event_tx
            .send(Event::FileSaved {
                uri: params.text_document.uri,
            })
            .await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // Re-index any *.kt / *.java file that changed on disk.
        // This fires after workspace/rename edits are applied to closed files,
        // keeping the in-memory symbol index consistent.
        for change in params.changes {
            if change.typ == FileChangeType::DELETED {
                // Remove from index; definition map cleanup is handled lazily.
                if self
                    .event_tx
                    .send(Event::FileDeleted {
                        uri: change.uri.clone(),
                    })
                    .await
                    .is_err()
                {
                    log::warn!("FileDeleted event dropped: workspace actor channel closed");
                }
                continue;
            }
            let uri = change.uri;
            let idx = Arc::clone(&self.indexer);
            let sem = idx.parse_sem();
            tokio::task::spawn(async move {
                if let Ok(path) = uri.to_file_path() {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        if let Ok(permit) = sem.acquire_owned().await {
                            tokio::task::spawn_blocking(move || {
                                let _permit = permit;
                                idx.index_content(&uri, &content);
                            })
                            .await
                            .ok();
                        }
                    }
                }
            });
        }
    }

    // ── textDocument/definition ──────────────────────────────────────────────

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        panic_safe("goto_definition", self.goto_definition_impl(params)).await
    }

    async fn goto_declaration(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        panic_safe("goto_declaration", self.goto_definition_impl(params)).await
    }

    async fn goto_implementation(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        panic_safe("goto_implementation", self.goto_implementation_impl(params)).await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        panic_safe("completion", self.completion_impl(params)).await
    }

    async fn completion_resolve(&self, item: CompletionItem) -> Result<CompletionItem> {
        panic_safe("completion_resolve", async {
            Ok(crate::features::completion::resolve_completion_item(
                item,
                self.indexer.as_ref(),
            ))
        })
        .await
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        panic_safe("hover", self.hover_impl(params)).await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        panic_safe("references", self.references_impl(params)).await
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        panic_safe("document_highlight", self.document_highlight_impl(params)).await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        panic_safe("document_symbol", self.document_symbol_impl(params)).await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        panic_safe("inlay_hint", self.inlay_hint_impl(params)).await
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        panic_safe("workspace_symbol", self.symbol_impl(params)).await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        panic_safe("signature_help", self.signature_help_impl(params)).await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        panic_safe("prepare_rename", self.prepare_rename_impl(params)).await
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        panic_safe("rename", self.rename_impl(params)).await
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        panic_safe("folding_range", self.folding_range_impl(params)).await
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let Some(lines) = self.indexer.lines_for(uri) else {
            log::debug!("on_type_formatting: no lines for {uri}");
            return Ok(None);
        };
        let ch = &params.ch;
        log::debug!(
            "on_type_formatting: ch={ch:?} pos=({},{}) prev={:?} cur={:?}",
            position.line,
            position.character,
            lines.get(position.line.saturating_sub(1) as usize),
            lines.get(position.line as usize),
        );
        let result = crate::features::on_type_formatting::compute_on_type_formatting(
            &lines,
            position,
            ch,
            &params.options,
        );
        log::debug!(
            "on_type_formatting: result has {} edits",
            result.as_ref().map_or(0, |v| v.len())
        );
        Ok(result)
    }

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<Vec<CodeActionOrCommand>>> {
        panic_safe("code_action", self.code_action_impl(params)).await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri_key = params.text_document.uri.to_string();
        let seq = {
            let mut e = self.semtok_seq.entry(uri_key.clone()).or_insert(0);
            *e += 1;
            *e
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if self.semtok_seq.get(&uri_key).map(|v| *v) != Some(seq) {
            return Ok(None);
        }
        panic_safe("semantic_tokens_full", async {
            let language = crate::Language::from_path(&uri_key);
            let Some(doc) = self.indexer.live_doc_or_parse(&params.text_document.uri) else {
                return Ok(None);
            };
            let parsed_uri = params.text_document.uri;
            let indexer = std::sync::Arc::clone(&self.indexer);
            Ok(tokio::task::spawn_blocking(move || {
                Some(SemanticTokensResult::Tokens(semantic_tokens::full_tokens(
                    &indexer,
                    &parsed_uri,
                    &doc,
                    language,
                )))
            })
            .await
            .unwrap_or(None))
        })
        .await
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
        let uri_key = params.text_document.uri.to_string();
        let seq = {
            let mut e = self.semtok_seq.entry(uri_key.clone()).or_insert(0);
            *e += 1;
            *e
        };
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if self.semtok_seq.get(&uri_key).map(|v| *v) != Some(seq) {
            return Ok(None);
        }
        panic_safe("semantic_tokens_range", async {
            let language = crate::Language::from_path(&uri_key);
            let Some(doc) = self.indexer.live_doc_or_parse(&params.text_document.uri) else {
                return Ok(None);
            };
            let parsed_uri = params.text_document.uri;
            let range = params.range;
            let indexer = std::sync::Arc::clone(&self.indexer);
            Ok(tokio::task::spawn_blocking(move || {
                Some(SemanticTokensRangeResult::Tokens(
                    semantic_tokens::range_tokens(&indexer, &parsed_uri, &doc, language, &range),
                ))
            })
            .await
            .unwrap_or(None))
        })
        .await
    }
}
