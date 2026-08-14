//! `rename` feature — shared rename logic behind thin backend adapters.
//!
//! Entry points: [`prepare_rename_impl`] and [`rename_impl`]. The backend
//! adapter only unwraps LSP params; this module handles CST-verified local
//! (`local_scope_occurrences`) and cross-file (`verified_references_for`)
//! rename, refusal-as-error on ambiguous/library/override identities, and
//! edit construction.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::features::references::verified_references_for;
use crate::features::text_utils::is_keyword_for_file;
use crate::indexer::{
    classify_cursor, local_scope_occurrences, resolve_identity, Indexer, NavigationSource,
};

pub(crate) async fn prepare_rename_impl(
    indexer: &Indexer,
    uri: &Url,
    pos: Position,
) -> Result<Option<PrepareRenameResponse>> {
    let (word, range) = match indexer.word_and_range_at(uri, pos) {
        Some(word_and_range) => word_and_range,
        None => return Ok(None),
    };

    if word.len() <= 1 || is_keyword_for_file(&word, uri.path()) {
        return Ok(None);
    }

    Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
        range,
        placeholder: word,
    }))
}

fn workspace_edit_from_locations(locations: &[Location], new_name: &str) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for location in locations {
        changes
            .entry(location.uri.clone())
            .or_default()
            .push(TextEdit {
                range: location.range,
                new_text: new_name.to_owned(),
            });
    }
    for edits in changes.values_mut() {
        edits.sort_by_key(|edit| Reverse(edit.range.start));
    }
    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Build a user-facing rename refusal. Rename refusals are errors BY DESIGN
/// (see the design doc's Global Constraints) — never `Ok(None)` — so the
/// reason surfaces to the user (e.g. the Helix status line) instead of
/// silently doing nothing. `panic_safe` (the only wrapper between this
/// function and the LSP transport, see `src/backend/panic_guard.rs`) forwards
/// any `Result::Err` unchanged, so no adapter change is needed for this to
/// reach the client.
///
/// `InvalidRequest` matches the only existing `tower_lsp::jsonrpc::Error`
/// construction in this codebase (`panic_guard.rs`'s raw struct literal for
/// panics, which uses `InternalError` — not appropriate here since this is an
/// expected business-rule refusal, not a bug). No other user-facing refusal
/// code exists in this codebase to match instead.
fn refusal(reason: &str) -> tower_lsp::jsonrpc::Error {
    tower_lsp::jsonrpc::Error {
        code: tower_lsp::jsonrpc::ErrorCode::InvalidRequest,
        message: reason.to_owned().into(),
        data: None,
    }
}

pub(crate) async fn rename_impl(
    indexer: &Arc<Indexer>,
    uri: &Url,
    pos: Position,
    new_name: &str,
) -> Result<Option<WorkspaceEdit>> {
    // Local fast path: a real CST subtree walk over the enclosing
    // function/lambda body. Never refuses, never crosses the scope boundary.
    if let Some(locations) = local_scope_occurrences(indexer, uri, pos) {
        return Ok(Some(workspace_edit_from_locations(&locations, new_name)));
    }

    let Some(symbol) = classify_cursor(indexer, uri, pos) else {
        return Ok(None);
    };

    let identity = resolve_identity(&symbol, indexer, uri);
    let NavigationSource::CstResolved(definitions) = identity else {
        return Err(refusal(
            "identity is ambiguous — could not resolve a single definition",
        ));
    };
    if definitions.len() != 1 {
        return Err(refusal(
            "identity is ambiguous — matches more than one definition",
        ));
    }
    if indexer.is_library_uri(&definitions[0].uri) {
        return Err(refusal(
            "defined in a library — cannot rename a library symbol",
        ));
    }

    let qualifier = None; // rename's cursor site has no dot-qualifier context today; matches prior behavior.
    let (verified, _query_declaring_type, _query_declaring_type_uri) = verified_references_for(
        &symbol.name,
        qualifier,
        uri,
        pos,
        true,
        indexer,
        usize::MAX,
        true, // detect_reverse_overrides: rename needs proven_overrides populated
    )
    .await;

    if !verified.proven_overrides.is_empty() {
        return Err(refusal(
            "renaming an override relationship is not supported — rename the exact \
             declaration you need",
        ));
    }

    let edit_locations: Vec<Location> = verified
        .kept
        .into_iter()
        .map(|source| match source {
            NavigationSource::CstResolved(location) => location,
            NavigationSource::NameScan(location) => location,
        })
        .collect();
    if edit_locations.is_empty() {
        return Ok(None);
    }

    Ok(Some(workspace_edit_from_locations(
        &edit_locations,
        new_name,
    )))
}

#[cfg(test)]
#[path = "rename_tests.rs"]
mod tests;
