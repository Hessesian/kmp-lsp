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
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file:///t{path}")).unwrap()
    }

    // `#[tokio::test]` + `.await` rather than `futures::executor::block_on`:
    // `verified_references_for`'s rg search runs on `tokio::task::spawn_blocking`,
    // which panics ("no reactor running") without a real Tokio runtime driving
    // it -- `block_on` alone doesn't provide one. Matches the convention already
    // used by every other async feature test in this codebase (e.g.
    // `definition_tests.rs`, `references_tests.rs`).
    #[tokio::test]
    async fn cross_file_rename_refuses_on_override_from_the_interface_side() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() { override fun save() {} }\n\
                      fun caller(user: User) { user.save() }\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        // cursor on the interface's own declaration
        let column = source.lines().next().unwrap().find("save").unwrap() as u32;
        let result = rename_impl(&indexer, &file_uri, Position::new(0, column), "persist").await;
        assert!(
            result.is_err(),
            "renaming an interface member with a real override must refuse, got {result:?}"
        );
    }

    #[tokio::test]
    async fn cross_file_rename_refuses_on_override_from_the_concrete_side() {
        let source = "open class User { fun save() {} }\n\
                      class DerivedUser : User() { override fun save() {} }\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        // cursor on the OVERRIDE's own declaration -- the symmetric direction
        let column = source.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let result = rename_impl(&indexer, &file_uri, Position::new(1, column), "persist").await;
        assert!(
            result.is_err(),
            "renaming FROM the override side must ALSO refuse -- symmetric with \
             the interface side, got {result:?}"
        );
    }

    #[tokio::test]
    async fn cross_file_rename_renames_a_clean_no_override_multi_call_site_member() {
        let source = "class Logger { fun log(message: String) {} }\n\
                      fun a(logger: Logger) { logger.log(\"a\") }\n\
                      fun b(logger: Logger) { logger.log(\"b\") }\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let column = source.lines().next().unwrap().find("log").unwrap() as u32;
        let result = rename_impl(&indexer, &file_uri, Position::new(0, column), "write")
            .await
            .expect("no override, no ambiguity -- must succeed")
            .expect("must produce an edit");
        let edits = result
            .changes
            .expect("must have changes")
            .remove(&file_uri)
            .expect("must edit this file");
        assert_eq!(edits.len(), 3, "declaration + 2 call sites, got {edits:?}");
    }

    #[tokio::test]
    async fn local_rename_uses_the_cst_fast_path_and_never_refuses() {
        let source = "fun run() {\n    val total = 0\n    print(total)\n}\n";
        let file_uri = uri("/D.kt");
        let indexer = std::sync::Arc::new(Indexer::new());
        indexer.index_content(&file_uri, source);
        indexer.store_live_tree(&file_uri, source);

        let result = rename_impl(&indexer, &file_uri, Position::new(1, 8), "sum")
            .await
            .expect("a local rename must never refuse")
            .expect("must produce an edit");
        let edits = result.changes.expect("must have changes");
        assert_eq!(edits.get(&file_uri).map(Vec::len), Some(2));
    }
}
