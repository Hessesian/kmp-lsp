//! Background symbol enrichment service.
//!
//! When the inlay-hints hot path encounters an unindexed symbol, it submits
//! the symbol name to an [`EnrichmentHandle`].  A background task runs
//! `rg_find_definition` + `index_content`, then signals the editor to
//! re-render inlay hints via `workspace/inlayHint/refresh`.
//!
//! Design invariants:
//! - **Never blocks the hot path** — submit is a non-blocking channel send.
//! - **Dedup** — a `DashSet` prevents re-enqueuing symbols already pending,
//!   in-flight, or recently failed (negative cache with cooldown).
//! - **Generation-stamped** — enrichments captured before a workspace switch
//!   are silently dropped.
//! - **Debounced refresh** — at most one refresh per `REFRESH_DEBOUNCE_MS`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::mpsc;

use crate::indexer::Indexer;

/// How long to wait after the last enrichment before sending a refresh.
const REFRESH_DEBOUNCE_MS: u64 = 200;

/// Cooldown before retrying a symbol that rg couldn't find.
const MISS_COOLDOWN: Duration = Duration::from_secs(60);

// ─── EnrichmentHandle ────────────────────────────────────────────────────────

/// Cheap, cloneable handle that hot-path code uses to submit enrichment requests.
///
/// Constructed by [`spawn_enrichment_worker`]. When no worker is running
/// (CLI mode, tests), use [`EnrichmentHandle::noop`] — sends are silently dropped.
#[derive(Clone)]
pub(crate) struct EnrichmentHandle {
    tx: Option<mpsc::UnboundedSender<EnrichRequest>>,
    /// Dedup + negative cache. Key = symbol name.
    /// Value = `None` while pending/in-flight, `Some(instant)` for miss cooldown.
    seen: Arc<DashMap<String, Option<Instant>>>,
}

struct EnrichRequest {
    symbol: String,
    generation: u64,
}

impl EnrichmentHandle {
    /// Create a no-op handle that silently discards all requests.
    pub(crate) fn noop() -> Self {
        Self {
            tx: None,
            seen: Arc::new(DashMap::new()),
        }
    }

    /// Submit a symbol for background enrichment.
    ///
    /// Returns immediately. Duplicate/recently-failed symbols are silently skipped.
    pub(crate) fn submit(&self, symbol: &str, generation: u64) {
        let Some(ref tx) = self.tx else { return };

        // Check dedup / negative cache.
        if let Some(entry) = self.seen.get(symbol) {
            match *entry {
                None => {
                    log::trace!("enrich: skip {symbol} (already pending)");
                    return;
                }
                Some(miss_at) if miss_at.elapsed() < MISS_COOLDOWN => {
                    log::trace!("enrich: skip {symbol} (miss cooldown)");
                    return;
                }
                _ => {
                    log::debug!("enrich: retry {symbol} (cooldown expired)");
                }
            }
        }

        log::debug!("enrich: submit {symbol} (gen={generation})");
        self.seen.insert(symbol.to_owned(), None);

        let _ = tx.send(EnrichRequest {
            symbol: symbol.to_owned(),
            generation,
        });
    }

    /// Clear dedup state (called on workspace switch / reindex).
    pub(crate) fn clear(&self) {
        let count = self.seen.len();
        self.seen.clear();
        if count > 0 {
            log::debug!("enrich: cleared {count} dedup entries");
        }
    }
}

// ─── Worker ──────────────────────────────────────────────────────────────────

/// Spawn the background enrichment worker.  Returns the handle for hot-path callers.
///
/// The worker runs until `indexer` is dropped (or the process exits).
/// `client` is used to send `workspace/inlayHint/refresh` after enrichment.
pub(crate) fn spawn_enrichment_worker(
    indexer: Arc<Indexer>,
    client: tower_lsp::Client,
) -> EnrichmentHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let seen = Arc::new(DashMap::new());
    let handle = EnrichmentHandle {
        tx: Some(tx),
        seen: Arc::clone(&seen),
    };
    log::info!("enrich: background worker started");
    tokio::spawn(enrichment_loop(indexer, client, rx, seen));
    handle
}

async fn enrichment_loop(
    indexer: Arc<Indexer>,
    client: tower_lsp::Client,
    mut rx: mpsc::UnboundedReceiver<EnrichRequest>,
    seen: Arc<DashMap<String, Option<Instant>>>,
) {
    // Debounce: accumulate enrichments, then refresh once.
    let debounce = Duration::from_millis(REFRESH_DEBOUNCE_MS);
    let mut dirty = false;
    let mut last_enrich = Instant::now();

    loop {
        // Wait for next request or debounce timeout.
        let request = if dirty {
            let remaining = debounce.saturating_sub(last_enrich.elapsed());
            tokio::select! {
                biased;
                req = rx.recv() => req,
                _ = tokio::time::sleep(remaining) => {
                    // Debounce expired — send refresh.
                    send_inlay_hint_refresh(&client).await;
                    dirty = false;
                    continue;
                }
            }
        } else {
            rx.recv().await
        };

        let Some(req) = request else {
            log::debug!("enrich: channel closed, worker exiting");
            break;
        };

        // Generation check — drop stale requests.
        let current_gen = indexer.workspace_root.generation();
        if req.generation != current_gen {
            log::debug!(
                "enrich: drop stale request for {} (req_gen={}, current_gen={})",
                req.symbol,
                req.generation,
                current_gen
            );
            seen.remove(&req.symbol);
            continue;
        }

        // Run rg + index in a blocking task to avoid starving the async runtime.
        log::debug!("enrich: rg lookup for {}", req.symbol);
        let idx = Arc::clone(&indexer);
        let symbol = req.symbol.clone();
        let start = Instant::now();
        let found = tokio::task::spawn_blocking(move || enrich_symbol(&idx, &symbol))
            .await
            .unwrap_or(false);
        let elapsed = start.elapsed();

        if found {
            log::info!(
                "enrich: indexed {} ({:.0?}), scheduling refresh",
                req.symbol,
                elapsed
            );
            dirty = true;
            last_enrich = Instant::now();
            seen.remove(&req.symbol);
        } else {
            log::debug!(
                "enrich: miss for {} ({:.0?}), cooldown {}s",
                req.symbol,
                elapsed,
                MISS_COOLDOWN.as_secs()
            );
            seen.insert(req.symbol, Some(Instant::now()));
        }
    }
}

/// Run `rg_find_definition` for `symbol` and index any discovered files.
/// Returns `true` if at least one new file was indexed.
fn enrich_symbol(indexer: &Indexer, symbol: &str) -> bool {
    let (root, source_roots, matcher) = indexer.rg_scope_for_path(None);
    let matcher_ref = matcher.as_deref();
    let locs = crate::rg::rg_find_definition(symbol, root.as_deref(), &source_roots, matcher_ref);

    log::trace!(
        "enrich: rg returned {} location(s) for {symbol}",
        locs.len()
    );

    let mut indexed_any = false;
    for loc in &locs {
        if indexer.files.contains_key(loc.uri.as_str()) {
            log::trace!("enrich: already indexed {}", loc.uri);
            continue;
        }
        // Prefer live buffer content if the file is open.
        let content = if let Some(live) = indexer.live_lines.get(loc.uri.as_str()) {
            log::trace!("enrich: using live buffer for {}", loc.uri);
            live.join("\n")
        } else if let Ok(path) = loc.uri.to_file_path() {
            match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("enrich: failed to read {}: {e}", path.display());
                    continue;
                }
            }
        } else {
            continue;
        };
        log::debug!("enrich: indexing {} for symbol {symbol}", loc.uri);
        indexer.index_content(&loc.uri, &content);
        indexed_any = true;
    }
    indexed_any
}

/// Send `workspace/inlayHint/refresh` to the client.
///
/// This is a server-initiated request (LSP 3.17+). Failures are silently ignored
/// since some clients don't support it.
async fn send_inlay_hint_refresh(client: &tower_lsp::Client) {
    log::info!("enrich: sending workspace/inlayHint/refresh");
    let result: Result<(), _> = client.send_request::<InlayHintRefreshRequest>(()).await;
    match result {
        Ok(()) => log::debug!("enrich: inlayHint/refresh acknowledged"),
        Err(e) => log::debug!("enrich: inlayHint/refresh failed: {e}"),
    }
}

/// LSP request type for `workspace/inlayHint/refresh`.
struct InlayHintRefreshRequest;

impl tower_lsp::lsp_types::request::Request for InlayHintRefreshRequest {
    type Params = ();
    type Result = ();
    const METHOD: &'static str = "workspace/inlayHint/refresh";
}

#[cfg(test)]
#[path = "enrich_tests.rs"]
mod tests;
