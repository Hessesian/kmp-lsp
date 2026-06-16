//! Completion feature — pipeline and helpers extracted from `Indexer`.
//!
//! # CompletionItem data keys
//!
//! The completion pipeline writes a small JSON blob into `CompletionItem.data`
//! so that `completionItem/resolve` can look up the full signature + doc comment.
//! The `DATA_*` constants are defined in `resolver::complete` (the write side)
//! and re-exported here for use by `resolve_completion_item` (the read side).

use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionResponse, Documentation,
    MarkupContent, MarkupKind, Position, Url,
};

use crate::indexer::resolution::{enrich_at_line, IndexRead, ResolveOptions, SubstitutionContext};
use crate::indexer::Indexer;
use crate::indexer::{
    find_it_element_type, find_named_lambda_param_type, is_lambda_param, last_ident_in,
};
use crate::resolver::complete::{
    complete_symbol, complete_symbol_with_context, is_annotation_context,
};
use crate::types::CursorPos;

use crate::features::traits::SignatureIndex;
use crate::indexer::split_params_at_depth_zero;

use super::completion_context::{CompletionContext, ScopeContext};
use super::traits::CompletionIndex;

// Re-export so callers only need to import from one place.
pub(crate) use crate::resolver::complete::{DATA_CALLING_URI, DATA_COL, DATA_LINE, DATA_URI};

const IT: &str = "it";

/// Compute completions at `position` in `uri`.
///
/// Returns the LSP `CompletionResponse` (possibly incomplete), or `None` when
/// there are no items and the workspace is fully indexed.
pub(crate) fn compute_completions(
    uri: &Url,
    position: Position,
    snippets: bool,
    index: &impl CompletionIndex,
) -> Option<CompletionResponse> {
    let (mut items, hit_cap) = index.completions(uri, position, snippets);
    // Always return None for an empty list — returning {items:[], isIncomplete:true}
    // causes clients like nvim-cmp to clear the popup and fall back to buffer source.
    if items.is_empty() {
        return None;
    }
    let still_indexing = index.is_indexing_in_progress();
    // Pre-select the best match so the editor highlights it without an extra keystroke.
    if let Some(first) = items.first_mut() {
        first.preselect = Some(true);
    }
    // When hit_cap is true the list was truncated — tell the client to re-request
    // on every keystroke. Also mark incomplete while indexing is in progress so
    // JAR symbols (launch, collect, etc.) surface automatically when ready.
    Some(CompletionResponse::List(CompletionList {
        is_incomplete: hit_cap || still_indexing,
        items,
    }))
}

/// Enrich a completion item with signature + doc comment on `completionItem/resolve`.
///
/// Reads `uri`, `line`, `col`, and optionally `calling_uri` from the item's
/// custom `data` blob written by the completion pipeline.
pub(crate) fn resolve_completion_item<I: IndexRead>(
    item: CompletionItem,
    index: &I,
) -> CompletionItem {
    let mut item = item;
    if let Some(ref data) = item.data {
        if let (Some(uri), Some(line)) = (
            data.get(DATA_URI).and_then(|v| v.as_str()),
            data.get(DATA_LINE).and_then(|v| v.as_u64()),
        ) {
            let col = data.get(DATA_COL).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let calling_uri = data.get(DATA_CALLING_URI).and_then(|v| v.as_str());

            let subst_ctx = match calling_uri {
                Some(cu) if cu != uri => SubstitutionContext::CrossFile {
                    calling_uri: cu,
                    cursor_line: None,
                },
                _ => SubstitutionContext::None,
            };

            if let Some(info) = enrich_at_line(
                index,
                uri,
                line as u32,
                col,
                subst_ctx,
                &ResolveOptions::completion(),
            ) {
                if !info.signature.is_empty() {
                    item.detail = Some(info.signature);
                }
                if !info.doc.is_empty() {
                    item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: info.doc,
                    }));
                }
            }
        }
    }
    item
}

// ─── pipeline ────────────────────────────────────────────────────────────────

/// Full completion pipeline. Called by `Indexer::completions` (inherent method).
pub(crate) fn run_completions(
    index: &Indexer,
    uri: &Url,
    position: Position,
    snippets: bool,
) -> (Vec<CompletionItem>, bool) {
    let gen = index.workspace_root.generation();
    let epoch = index
        .completion_epoch
        .load(std::sync::atomic::Ordering::Acquire);

    index.ensure_indexed(uri);

    let Some(line) = line_for_position(index, uri, position.line) else {
        return (vec![], false);
    };
    let before = before_cursor(&line, position.character);
    let (prefix, before_prefix) = split_prefix(before);
    let cache_key = completion_cache_key(uri, before_prefix, position.line);
    if let Some((cached, hit_cap)) = cache_hit(index, &cache_key) {
        return (cached, hit_cap);
    }

    if index.workspace_root.generation() != gen {
        return (vec![], false);
    }

    let annotation_only = is_annotation_context(before, prefix);
    let lines = index.lines_for(uri).unwrap_or_default();
    let ctx = CompletionContext::analyse(
        before_prefix,
        position,
        index,
        uri,
        lines.as_ref(),
        annotation_only,
    );

    if let Some(ref recv) = ctx.receiver {
        let recv_str = recv.as_str();
        if ctx.scope.is_scope_receiver(recv_str)
            || is_lambda_param(recv_str, before, index, uri, position.line as usize)
        {
            return (
                complete_lambda_dot(
                    index,
                    recv_str,
                    &ctx.scope,
                    CompletionSite {
                        before,
                        position,
                        uri,
                    },
                    snippets,
                    prefix,
                ),
                false,
            );
        }
    }

    let (mut items, hit_cap) = complete_symbol_with_context(
        index,
        prefix,
        ctx.receiver.clone(),
        uri,
        snippets,
        ctx.annotation_only,
        Some(position.line),
    );

    if ctx.receiver.is_none() {
        add_lambda_param_completions(&mut items, &ctx.scope, prefix);
        add_named_arg_completions(index, &mut items, uri, prefix, ctx.call_info.as_ref());
    }

    store_in_cache(index, cache_key, &items, hit_cap, epoch);
    (items, hit_cap)
}

// ─── pipeline helpers ─────────────────────────────────────────────────────────

/// Build the cache key for a completion request.
///
/// The key always omits the prefix so that subsequent keystrokes (e.g. `l` →
/// `la` → `lau`) hit the cache and the client fuzzy-filters the result list.
/// One server round-trip per context (uri + line + preceding text), then instant
/// for every additional character typed.
fn completion_cache_key(uri: &Url, before_prefix: &str, line: u32) -> String {
    format!("{}|{}|{}", uri.as_str(), before_prefix, line)
}

/// Return cached `(items, hit_cap)` if the last completion key matches, `None` otherwise.
fn cache_hit(index: &Indexer, key: &str) -> Option<(Vec<CompletionItem>, bool)> {
    let guard = index.last_completion.lock().ok()?;
    let (ref k, _, ref cached, hit_cap) = *(*guard).as_ref()?;
    (k.as_str() == key).then(|| (cached.clone(), hit_cap))
}

/// Persist the latest completion result for subsequent identical requests.
///
/// Only stores if `epoch` still matches the current `completion_epoch` — guards
/// against an in-flight request storing stale results after JAR indexing
/// incremented the epoch and cleared the cache.
fn store_in_cache(
    index: &Indexer,
    key: String,
    items: &[CompletionItem],
    hit_cap: bool,
    epoch: u64,
) {
    if let Ok(mut guard) = index.last_completion.lock() {
        if index
            .completion_epoch
            .load(std::sync::atomic::Ordering::Acquire)
            == epoch
        {
            *guard = Some((key, String::new(), items.to_vec(), hit_cap));
        }
    }
}

struct CompletionSite<'a> {
    before: &'a str,
    position: Position,
    uri: &'a Url,
}

/// Run dot-completion for a lambda receiver (`it.`, `this.`, `this@label.`, or named param).
///
/// Returns a type-hint placeholder item when the type is known but no members
/// matched yet (gives the user a visible signal of what type was inferred).
fn complete_lambda_dot(
    index: &Indexer,
    recv: &str,
    scope: &ScopeContext,
    site: CompletionSite<'_>,
    snippets: bool,
    prefix: &str,
) -> Vec<CompletionItem> {
    let Some(elem_type) = scope
        .resolve_receiver(recv)
        .or_else(|| scope.named_param_type(recv))
        .map(str::to_owned)
        .or_else(|| {
            resolve_named_lambda_param_type(index, recv, site.before, site.position, site.uri)
        })
        .or_else(|| {
            (recv == IT)
                .then(|| find_it_element_type(site.before, index, site.uri))
                .flatten()
        })
    else {
        return vec![];
    };
    let (items, _) = complete_symbol(
        index,
        prefix,
        Some(&elem_type),
        site.uri,
        snippets,
        Some(site.position.line),
    );
    if items.is_empty() {
        vec![type_hint_item(recv, &elem_type)]
    } else {
        items
    }
}

/// A placeholder `CompletionItem` showing the inferred type when no members matched.
fn type_hint_item(recv: &str, elem_type: &str) -> CompletionItem {
    CompletionItem {
        label: format!("{recv}: {elem_type}"),
        kind: Some(CompletionItemKind::TYPE_PARAMETER),
        detail: Some(format!("Inferred type: {elem_type}")),
        sort_text: Some("~hint".into()),
        ..Default::default()
    }
}

// ─── private helpers ─────────────────────────────────────────────────────────

/// Returns the text of line `line_idx` for `uri`, preferring live lines.
fn line_for_position(index: &Indexer, uri: &Url, line_idx: u32) -> Option<String> {
    let line_index = line_idx as usize;
    if let Some(ll) = index.live_lines.get(uri.as_str()) {
        return ll.get(line_index).cloned();
    }
    index
        .files
        .get(uri.as_str())?
        .lines
        .get(line_index)
        .cloned()
}

fn resolve_named_lambda_param_type(
    index: &Indexer,
    recv: &str,
    before: &str,
    position: Position,
    uri: &Url,
) -> Option<String> {
    find_named_lambda_param_type(
        before,
        recv,
        index,
        uri,
        CursorPos {
            line: position.line as usize,
            utf16_col: position.character as usize,
        },
    )
}

/// Appends lambda-parameter completions for bare-word (non-dot) completion.
fn add_lambda_param_completions(
    items: &mut Vec<CompletionItem>,
    scope: &ScopeContext,
    prefix: &str,
) {
    use crate::features::text_utils::starts_with_ignore_ascii_case;

    let prefix_lower = prefix.to_lowercase();
    for (param_name, _) in scope
        .lambda_scopes
        .iter()
        .rev()
        .flat_map(|lambda_scope| lambda_scope.named_params.iter())
    {
        if starts_with_ignore_ascii_case(param_name, &prefix_lower)
            && !items.iter().any(|item| item.label == *param_name)
        {
            items.push(CompletionItem {
                label: param_name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                sort_text: Some(format!("005:{param_name}")),
                ..Default::default()
            });
        }
    }
}

// ─── named argument completions ───────────────────────────────────────────────

/// Append `name =` completion items for the active call expression.
///
/// Prefers `CompletionContext::call_info` when available, and falls back to
/// `call_info_at` so callers without a precomputed context keep the old
/// behavior for multiline and qualified calls.
fn add_named_arg_completions(
    index: &Indexer,
    items: &mut Vec<CompletionItem>,
    uri: &Url,
    prefix: &str,
    call_info: Option<&crate::features::completion_context::CallInfo>,
) {
    let Some(call_info) = call_info else {
        return;
    };
    let Some(params_text) = index.find_fun_signature_with_receiver(
        uri,
        &call_info.callee,
        call_info.qualifier.as_deref(),
    ) else {
        return;
    };
    let expected_name = call_info.expected_name.as_deref();
    let raw = params_text.trim_matches(|c| c == '(' || c == ')');
    let prefix_lower = prefix.to_lowercase();
    for name in param_names_from_sig(raw) {
        use crate::features::text_utils::starts_with_ignore_ascii_case;
        if !prefix_lower.is_empty() && !starts_with_ignore_ascii_case(&name, &prefix_lower) {
            continue;
        }
        if items.iter().any(|i| i.label == format!("{name} =")) {
            continue;
        }
        let sort_prefix = if expected_name == Some(name.as_str()) {
            "000"
        } else {
            "001"
        };
        items.push(CompletionItem {
            label: format!("{name} ="),
            filter_text: Some(name.clone()),
            insert_text: Some(format!("{name} = ")),
            kind: Some(CompletionItemKind::FIELD),
            sort_text: Some(format!("{sort_prefix}:{name}")),
            ..Default::default()
        });
    }
}

/// Extract parameter names from a flattened signature string like
/// `"name: String, age: Int"` or `"@Ann vararg items: T"`.
///
/// Skips `this` (extension receivers) and any part that has no `:`.
pub(crate) fn param_names_from_sig(params_text: &str) -> Vec<String> {
    split_params_at_depth_zero(params_text)
        .into_iter()
        .filter_map(|part| {
            let part = part.trim();
            let colon = part.find(':')?;
            // Everything before the colon is modifiers + name; take the last word.
            let before_colon = part[..colon].trim();
            let name = before_colon.split_whitespace().next_back()?;
            // Strip any remaining leading non-ident chars (e.g. bare `@` residue).
            let name = name.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_');
            if name.is_empty() || name == "this" {
                return None;
            }
            Some(name.to_owned())
        })
        .collect()
}

// ─── pure string helpers ──────────────────────────────────────────────────────

/// Returns a slice of `line` up to the UTF-16 column `utf16_col`.
fn before_cursor(line: &str, utf16_col: u32) -> &str {
    let target = utf16_col as usize;
    let mut utf16 = 0usize;
    let mut byte_end = line.len();
    for (bi, ch) in line.char_indices() {
        if utf16 >= target {
            byte_end = bi;
            break;
        }
        utf16 += ch.len_utf16();
    }
    &line[..byte_end]
}

/// Splits `before` into the trailing identifier fragment (`prefix`) and
/// everything that precedes it (`before_prefix`).
fn split_prefix(before: &str) -> (&str, &str) {
    let prefix = last_ident_in(before);
    let before_prefix = &before[..before.len() - prefix.len()];
    (prefix, before_prefix)
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;
