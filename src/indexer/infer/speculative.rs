//! Marker-insertion speculative parse for cursor-context queries.
//!
//! Mid-typing states like `foo.` parse with tree-sitter ERROR nodes. The fix
//! is rust-analyzer's: insert a fake identifier at the cursor
//! ([`COMPLETION_MARKER`], their `intellijRulezz`), reparse incrementally, and
//! read the now-well-formed tree. The speculative [`LiveDoc`] is request-local
//! and never stored.

use tree_sitter::{InputEdit, Parser, Point};

use crate::indexer::live_tree::{utf16_col_to_byte, LiveDoc};
use crate::queries::{KIND_NAV_EXPR, KIND_NAV_SUFFIX};
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

/// Number of ancestor hops from the marker before giving up. The marker sits
/// in `navigation_suffix` one or two levels up in well-formed trees; the
/// allowance covers ERROR-node wrapping in broken mid-edit states.
const MAX_ASCENT: usize = 6;

/// From the marker identifier, find the `navigation_expression` whose suffix
/// contains the marker and return its receiver subtree (the left child).
///
/// Returns `None` when the marker is not the member position of a navigation
/// (bare word, string literal, comment) — the caller treats that as bare-word
/// completion. Comments and plain string content swallow the marker: the
/// covering node is a comment/string token with no `navigation_suffix`
/// ancestor of its own, so the ascent returns `None` naturally (interpolation
/// `${...}` forms real expression nodes and resolves like ordinary code).
#[allow(dead_code)] // wiring seam — consumed by derive_dot_receiver (follow-up commit)
pub(crate) fn receiver_node_for_marker(
    doc: &LiveDoc,
    marker_byte: usize,
) -> Option<tree_sitter::Node<'_>> {
    let marker_end = marker_byte + COMPLETION_MARKER.len();
    let node = doc
        .tree
        .root_node()
        .descendant_for_byte_range(marker_byte, marker_end)?;
    let mut cur = node;
    for _ in 0..MAX_ASCENT {
        let parent = cur.parent()?;
        if parent.kind() == KIND_NAV_SUFFIX {
            // The suffix we ascended through must be the marker's own, not an
            // outer chain segment's (a marker inside a receiver never
            // dot-completes on the outer chain).
            if parent.start_byte() > marker_end || parent.end_byte() < marker_byte {
                return None;
            }
            let nav = parent.parent()?;
            if nav.kind() != KIND_NAV_EXPR {
                return None;
            }
            return nav.child(0);
        }
        cur = parent;
    }
    None
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

    // ─── receiver extraction ─────────────────────────────────────────────────

    /// Derive the receiver's `(kind, text)` for a source with a `|` cursor.
    fn receiver_of(src_with_caret: &str) -> Option<(String, String)> {
        let caret = src_with_caret.find('|').expect("caret");
        let src: String = src_with_caret.replace('|', "");
        let line = src_with_caret[..caret].matches('\n').count();
        let line_start = src_with_caret[..caret].rfind('\n').map_or(0, |p| p + 1);
        let col = src_with_caret[line_start..caret].encode_utf16().count();
        let base = kotlin_doc(&src);
        let (doc, marker) = speculative_doc(
            &base,
            tree_sitter_kotlin::language(),
            CursorPos {
                line,
                utf16_col: col,
            },
        )?;
        let node = receiver_node_for_marker(&doc, marker)?;
        let text = node.utf8_text(&doc.bytes).unwrap().to_owned();
        Some((node.kind().to_owned(), text))
    }

    #[test]
    fn extracts_a_simple_identifier_receiver() {
        let (kind, text) = receiver_of("fun f() { foo.| }").unwrap();
        assert_eq!((kind.as_str(), text.as_str()), ("simple_identifier", "foo"));
    }

    #[test]
    fn extracts_a_call_receiver() {
        let (kind, text) = receiver_of("fun f() { productFlow(a, b).| }").unwrap();
        assert_eq!(kind, "call_expression");
        assert_eq!(text, "productFlow(a, b)");
    }

    #[test]
    fn extracts_a_dotted_chain_receiver() {
        let (kind, text) = receiver_of("fun f() { foo.bar.| }").unwrap();
        assert_eq!(kind, "navigation_expression");
        assert_eq!(text, "foo.bar");
    }

    #[test]
    fn safe_call_receiver_works() {
        let (_, text) = receiver_of("fun f() { nullable?.| }").unwrap();
        assert_eq!(text, "nullable");
    }

    #[test]
    fn multiline_fluent_chain_receiver_spans_lines() {
        let (kind, text) = receiver_of(
            "fun f() {\n    val m = Modifier\n        .fillMaxSize() // grab it.\n        .|\n}",
        )
        .unwrap();
        // The chain tail is a call: `call_expression(nav(Modifier, .fillMaxSize), args)`.
        assert_eq!(kind, "call_expression");
        assert!(text.contains("Modifier"), "text: {text}");
        assert!(text.contains("fillMaxSize()"), "text: {text}");
    }

    #[test]
    fn cursor_inside_a_string_literal_finds_no_receiver() {
        assert!(receiver_of("fun f() { val s = \"foo.|\" }").is_none());
    }

    #[test]
    fn cursor_inside_a_line_comment_finds_no_receiver() {
        assert!(receiver_of("fun f() { g() } // foo.|").is_none());
    }

    #[test]
    fn interpolation_receiver_is_found() {
        let (_, text) = receiver_of("fun f(user: User) { val s = \"${user.|}\" }").unwrap();
        assert_eq!(text, "user");
    }

    #[test]
    fn unclosed_paren_state_still_finds_the_receiver() {
        let (_, text) = receiver_of("fun f() { g(bar.| }").unwrap();
        assert_eq!(text, "bar");
    }

    #[test]
    fn unclosed_lambda_state_still_finds_the_receiver() {
        let (_, text) = receiver_of("fun f() { items.filter { it.| }").unwrap();
        assert_eq!(text, "it");
    }

    #[test]
    fn super_receiver_is_extracted() {
        let (kind, _) = receiver_of("class A { fun f() { super.| } }").unwrap();
        assert_eq!(kind, "super_expression");
    }

    #[test]
    fn labeled_this_receiver_is_extracted() {
        let (kind, text) = receiver_of("fun f() { items.forEach { this@forEach.| } }").unwrap();
        assert_eq!(kind, "this_expression");
        assert_eq!(text, "this@forEach");
    }

    #[test]
    fn bare_word_context_has_no_receiver() {
        assert!(receiver_of("fun f() { Modif| }").is_none());
    }

    #[test]
    fn marker_inside_a_chain_receiver_does_not_complete_on_the_outer_chain() {
        // Cursor mid-chain: `foo.|.bar` — the marker's own suffix is the one
        // after `foo`, not `.bar`; the receiver must be `foo` alone.
        let (_, text) = receiver_of("fun f() { foo.|.bar }").unwrap();
        assert_eq!(text, "foo");
    }
}
