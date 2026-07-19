//! Document highlight feature — marks same-symbol occurrences in a file.

use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Position, Range, Url};

use super::text_utils::{utf16_column, word_byte_offsets};

/// The narrowest enclosing function/lambda body (or the whole file if
/// neither exists) containing `node` — the boundary document-highlight
/// searches within. Coarser than full lexical scoping (doesn't distinguish
/// nested shadowing), but it NEVER crosses into an unrelated function, which
/// is the bug this exists to fix.
fn enclosing_body(node: tree_sitter::Node<'_>) -> tree_sitter::Node<'_> {
    let mut cur = node;
    while let Some(parent) = cur.parent() {
        if matches!(
            parent.kind(),
            k if k == crate::queries::KIND_FUN_DECL || k == crate::queries::KIND_LAMBDA_LIT
        ) {
            return parent;
        }
        cur = parent;
    }
    cur
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

    let scope_range: Option<Range> = (|| {
        let cursor = crate::types::CursorPos {
            line: pos.line as usize,
            utf16_col: pos.character as usize,
        };
        let doc = index.live_doc_or_parse(uri)?;
        let node = crate::indexer::cursor_node_at(&doc, cursor)?;
        let body = enclosing_body(node);
        Some(Range::new(
            Position::new(body.start_position().row as u32, 0),
            Position::new(body.end_position().row as u32, u32::MAX),
        ))
    })();

    let decl_lines: std::collections::HashSet<u32> = index
        .definition_locations(&name)
        .into_iter()
        .filter(|loc| loc.uri == *uri)
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
