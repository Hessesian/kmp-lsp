//! Marker-insertion speculative parse for cursor-context queries.
//!
//! Mid-typing states like `foo.` parse with tree-sitter ERROR nodes. The fix
//! is rust-analyzer's: insert a fake identifier at the cursor
//! ([`COMPLETION_MARKER`], their `intellijRulezz`), reparse incrementally, and
//! read the now-well-formed tree. The speculative [`LiveDoc`] is request-local
//! and never stored.

use tree_sitter::{InputEdit, Parser, Point};

use crate::indexer::live_tree::{utf16_col_to_byte, LiveDoc};
use crate::types::CursorPos;

/// Fake identifier inserted at the cursor. Any valid Kotlin identifier that
/// will never collide with real code works; the homage is intentional.
#[allow(dead_code)] // wiring seam — consumed by derive_dot_receiver (follow-up commit)
pub(crate) const COMPLETION_MARKER: &str = "kmpLspRulezz";

/// Byte offset + tree-sitter point for `cursor` within `bytes`.
///
/// Mirrors `cursor_node_at`'s end-of-file posture: a cursor on the phantom
/// line after a trailing `\n` maps to the end of the content; anything
/// further is `None`.
fn cursor_byte_and_point(bytes: &[u8], cursor: CursorPos) -> Option<(usize, Point)> {
    let source = std::str::from_utf8(bytes).ok()?;
    let mut offset = 0usize;
    for (row, line_text) in source.lines().enumerate() {
        if row == cursor.line {
            let col = utf16_col_to_byte(line_text, cursor.utf16_col).min(line_text.len());
            return Some((offset + col, Point { row, column: col }));
        }
        offset += line_text.len() + 1; // '\n'
    }
    if cursor.line == source.lines().count() && source.ends_with('\n') {
        return Some((
            source.len(),
            Point {
                row: cursor.line,
                column: 0,
            },
        ));
    }
    None
}

/// Parse a copy of `base` with [`COMPLETION_MARKER`] inserted at `cursor`.
///
/// Returns the speculative doc and the marker's byte offset, or `None` when
/// the cursor lies outside the content or the parser fails.
#[allow(dead_code)] // wiring seam — consumed by derive_dot_receiver (follow-up commit)
pub(crate) fn speculative_doc(
    base: &LiveDoc,
    lang: tree_sitter::Language,
    cursor: CursorPos,
) -> Option<(LiveDoc, usize)> {
    let (offset, point) = cursor_byte_and_point(&base.bytes, cursor)?;

    let mut bytes = Vec::with_capacity(base.bytes.len() + COMPLETION_MARKER.len());
    bytes.extend_from_slice(&base.bytes[..offset]);
    bytes.extend_from_slice(COMPLETION_MARKER.as_bytes());
    bytes.extend_from_slice(&base.bytes[offset..]);

    // Incremental reparse: clone the immutable base tree, record the insertion,
    // and let tree-sitter reuse everything outside the edited range.
    let mut tree = base.tree.clone();
    tree.edit(&InputEdit {
        start_byte: offset,
        old_end_byte: offset,
        new_end_byte: offset + COMPLETION_MARKER.len(),
        start_position: point,
        old_end_position: point,
        new_end_position: Point {
            row: point.row,
            column: point.column + COMPLETION_MARKER.len(),
        },
    });
    let mut parser = Parser::new();
    parser.set_language(&lang).ok()?;
    let tree = parser.parse(&bytes, Some(&tree))?;
    Some((LiveDoc { bytes, tree }, offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::live_tree::parse_live;

    fn kotlin_doc(src: &str) -> LiveDoc {
        parse_live(src, tree_sitter_kotlin::language()).unwrap()
    }

    #[test]
    fn trailing_dot_state_parses_cleanly_with_the_marker() {
        // `val x = foo.` alone parses with an ERROR node; the marker heals it.
        let base = kotlin_doc("val x = foo.\n");
        let cursor = CursorPos {
            line: 0,
            utf16_col: 12,
        };
        let (doc, marker_byte) =
            speculative_doc(&base, tree_sitter_kotlin::language(), cursor).expect("parse");
        assert_eq!(marker_byte, 12);
        let text = std::str::from_utf8(&doc.bytes).unwrap();
        assert_eq!(text, "val x = foo.kmpLspRulezz\n");
        let node = doc
            .tree
            .root_node()
            .descendant_for_byte_range(marker_byte, marker_byte + 1)
            .unwrap();
        let mut cur = node;
        let mut found_nav = false;
        while let Some(p) = cur.parent() {
            if p.kind() == "navigation_expression" {
                found_nav = true;
                break;
            }
            cur = p;
        }
        assert!(
            found_nav,
            "expected navigation_expression ancestor, tree: {}",
            doc.tree.root_node().to_sexp()
        );
    }

    #[test]
    fn incremental_reparse_matches_a_fresh_parse() {
        let src = "class A {\n    fun f() {\n        val m = Modifier\n            .fillMaxSize()\n            .\n    }\n}\n";
        let base = kotlin_doc(src);
        let cursor = CursorPos {
            line: 4,
            utf16_col: 13,
        };
        let (doc, _) = speculative_doc(&base, tree_sitter_kotlin::language(), cursor).unwrap();
        let fresh = kotlin_doc(std::str::from_utf8(&doc.bytes).unwrap());
        assert_eq!(
            doc.tree.root_node().to_sexp(),
            fresh.tree.root_node().to_sexp(),
            "InputEdit coordinates are wrong if these diverge"
        );
    }

    #[test]
    fn cursor_at_eof_inserts_at_end() {
        let base = kotlin_doc("val x = foo.\n");
        // Line 1 = the editor's phantom final line after the trailing newline.
        let cursor = CursorPos {
            line: 1,
            utf16_col: 0,
        };
        let (doc, marker_byte) =
            speculative_doc(&base, tree_sitter_kotlin::language(), cursor).unwrap();
        assert_eq!(marker_byte, 13);
        assert!(std::str::from_utf8(&doc.bytes)
            .unwrap()
            .ends_with("kmpLspRulezz"));
    }

    #[test]
    fn cursor_beyond_content_returns_none() {
        let base = kotlin_doc("val x = 1\n");
        let cursor = CursorPos {
            line: 7,
            utf16_col: 0,
        };
        assert!(speculative_doc(&base, tree_sitter_kotlin::language(), cursor).is_none());
    }
}
