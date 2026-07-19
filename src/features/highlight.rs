//! Document highlight feature — marks same-symbol occurrences in a file.

use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Position, Range, Url};

use super::text_utils::{utf16_column, word_byte_offsets};

/// The narrowest enclosing function/lambda body containing `node` — the
/// boundary document-highlight searches within. Coarser than full lexical
/// scoping (doesn't distinguish nested shadowing), but it NEVER crosses into
/// an unrelated function, which is the bug this exists to fix.
///
/// Returns `None` when `node` has no enclosing function/lambda ancestor
/// (e.g. it's a top-level declaration, or nested only inside a class body) —
/// callers use this to tell "genuinely local" from "possibly file-wide"
/// apart, since narrowing to a body is only safe in the former case.
fn enclosing_body(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if matches!(
            parent.kind(),
            k if k == crate::queries::KIND_FUN_DECL || k == crate::queries::KIND_LAMBDA_LIT
        ) {
            return Some(parent);
        }
        cur = parent;
    }
    None
}

/// Compute all highlight ranges for the symbol under `pos` in `uri`.
///
/// Definition sites are marked as `Write`; all other occurrences as `Read`.
/// Returns `None` when the cursor is not on a word or the file has no lines.
///
/// When the CST can resolve the cursor to a node inside a function/lambda
/// body, the search is scoped to that body — this is what stops a local
/// variable in one function from highlighting an unrelated same-named local
/// in another function. When the CST can't resolve a node (or `index` isn't
/// a concrete `Indexer`, which the sole production caller always passes),
/// this falls through to today's whole-file scan exactly.
pub(crate) fn compute_document_highlight(
    uri: &Url,
    pos: Position,
    index: &crate::indexer::Indexer,
) -> Option<Vec<DocumentHighlight>> {
    let (name, _) = index.word_and_qualifier_at(uri, pos)?;
    let lines = index.mem_lines_for(uri.as_str())?;

    let decl_locations: Vec<_> = index
        .definition_locations(&name)
        .into_iter()
        .filter(|loc| loc.uri == *uri)
        .collect();

    // Narrowing to the enclosing function/lambda body is only safe when
    // EVERY declaration site of `name` is itself nested inside some
    // function/lambda body. A top-level function or class member can
    // legitimately be referenced from anywhere in the file — narrowing to
    // the click site's enclosing function would silently drop both the
    // declaration and any call sites in other functions (task-6 review
    // finding). If any declaration site has no enclosing body, skip
    // narrowing entirely and fall back to a whole-file scan for this name.
    let any_top_level_decl = decl_locations.iter().any(|loc| {
        (|| -> Option<bool> {
            let doc = index.live_doc_or_parse(uri)?;
            let node = crate::indexer::cursor_node_at(&doc, loc.range.start.into())?;
            // `node` here is the declaration's OWN name identifier (the
            // `selection_range` the index stores). For a `function_declaration`,
            // that identifier is a *direct* child of the function_declaration
            // node it names — the Kotlin `KOTLIN_DEFINITIONS` query captures it
            // positionally, e.g. `(function_declaration (simple_identifier) @name)`
            // (see src/queries.rs patterns 7-10), with no field annotation to
            // distinguish it from an unrelated identifier. But a Kotlin
            // `function_declaration`'s only direct `simple_identifier` child IS
            // its own name (parameters live inside a separate
            // `function_value_parameters` child) — so `node`'s immediate parent
            // being a FUN_DECL unambiguously means `node` is that function's own
            // name, not a use nested in its body. Climbing from the bare name
            // would otherwise immediately "find" the function as its own
            // enclosing scope, mistaking a top-level `fun foo()` for being
            // nested inside itself. Start the enclosing-scope search from the
            // declaration node instead of the name in that case.
            let decl_node = match node.parent() {
                Some(p) if p.kind() == crate::queries::KIND_FUN_DECL => p,
                _ => node,
            };
            Some(enclosing_body(decl_node).is_none())
        })()
        .unwrap_or(false)
    });

    let scope_range: Option<Range> = if any_top_level_decl {
        None
    } else {
        (|| {
            let cursor = crate::types::CursorPos {
                line: pos.line as usize,
                utf16_col: pos.character as usize,
            };
            let doc = index.live_doc_or_parse(uri)?;
            let node = crate::indexer::cursor_node_at(&doc, cursor)?;
            let body = enclosing_body(node)?;
            Some(Range::new(
                Position::new(body.start_position().row as u32, 0),
                Position::new(body.end_position().row as u32, u32::MAX),
            ))
        })()
    };

    let decl_lines: std::collections::HashSet<u32> = decl_locations
        .iter()
        .map(|loc| loc.range.start.line)
        .collect();

    let mut highlights = Vec::new();

    for (line_idx, line) in lines.iter().enumerate() {
        let line_idx = line_idx as u32;
        if let Some(ref scope) = scope_range {
            if line_idx < scope.start.line || line_idx > scope.end.line {
                continue;
            }
        }
        for abs in word_byte_offsets(line, &name) {
            let col = utf16_column(&line[..abs]);
            let col_end = col + utf16_column(&name);
            let range = Range::new(
                Position::new(line_idx, col),
                Position::new(line_idx, col_end),
            );
            let kind = if decl_lines.contains(&line_idx) {
                DocumentHighlightKind::WRITE
            } else {
                DocumentHighlightKind::READ
            };
            highlights.push(DocumentHighlight {
                range,
                kind: Some(kind),
            });
        }
    }

    if highlights.is_empty() {
        None
    } else {
        Some(highlights)
    }
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
