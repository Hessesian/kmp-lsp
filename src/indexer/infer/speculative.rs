//! Marker-insertion speculative parse for cursor-context queries.
//!
//! Mid-typing states like `foo.` parse with tree-sitter ERROR nodes. The fix
//! is rust-analyzer's: insert a fake identifier at the cursor
//! ([`COMPLETION_MARKER`], their `intellijRulezz`), reparse incrementally, and
//! read the now-well-formed tree. The speculative [`LiveDoc`] is request-local
//! and never stored.
//!
//! This module now hosts both transient-healed-doc constructors: marker
//! insertion (dot-completion receiver derivation, above) and append-only
//! brace repair (lambda resolution, [`lambda_doc_at`], below). They are
//! co-located because both answer "give me a parseable view of a mid-typing
//! buffer" — not merged because the transforms differ (surgical insertion vs.
//! trailing-brace append).

use std::sync::Arc;

use tower_lsp::lsp_types::Url;
use tree_sitter::{InputEdit, Parser, Point};

use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte, LiveDoc};
use crate::indexer::{Indexer, NodeExt};
use crate::queries::{KIND_LAMBDA_LIT, KIND_NAV_EXPR, KIND_NAV_SUFFIX};
use crate::types::CursorPos;

use super::cst_lambda::cursor_node_at;

/// Fake identifier inserted at the cursor. Any valid Kotlin identifier that
/// will never collide with real code works; the homage is intentional.
pub(crate) const COMPLETION_MARKER: &str = "kmpLspRulezz";

/// Byte offset + tree-sitter point for `cursor` within `bytes`.
///
/// Iterates `split_inclusive('\n')` so the running offset counts the REAL
/// separator bytes — `str::lines()` silently drops a `\r`, which would drift
/// the offset by one byte per line on CRLF files and corrupt the `InputEdit`.
/// Mirrors `cursor_node_at`'s end-of-file posture: a cursor on the phantom
/// line after a trailing `\n` maps to the end of the content; anything
/// further is `None`.
fn insertion_site(bytes: &[u8], cursor: CursorPos) -> Option<(usize, Point)> {
    let source = std::str::from_utf8(bytes).ok()?;
    let mut offset = 0usize;
    let mut row = 0usize;
    for raw_line in source.split_inclusive('\n') {
        if row == cursor.line {
            let content = raw_line
                .strip_suffix('\n')
                .map_or(raw_line, |no_lf| no_lf.strip_suffix('\r').unwrap_or(no_lf));
            let col = utf16_col_to_byte(content, cursor.utf16_col).min(content.len());
            return Some((offset + col, Point { row, column: col }));
        }
        offset += raw_line.len();
        row += 1;
    }
    if cursor.line == row && source.ends_with('\n') {
        return Some((source.len(), Point { row, column: 0 }));
    }
    None
}

/// Parse a copy of `base` with [`COMPLETION_MARKER`] inserted at `cursor`.
///
/// Returns the speculative doc and the marker's byte offset, or `None` when
/// the cursor lies outside the content or the parser fails.
pub(crate) fn speculative_doc(
    base: &LiveDoc,
    lang: tree_sitter::Language,
    cursor: CursorPos,
) -> Option<(LiveDoc, usize)> {
    let (offset, point) = insertion_site(&base.bytes, cursor)?;

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
    // Per-thread parser reuse (same rationale as `parser.rs`): `Parser::new()`
    // allocates internal state, and this runs on the completion hot path.
    // `set_language` per call keeps the single instance language-agnostic.
    thread_local! {
        static SPECULATIVE_PARSER: std::cell::RefCell<Parser> =
            std::cell::RefCell::new(Parser::new());
    }
    let tree = SPECULATIVE_PARSER.with(|parser| {
        let mut parser = parser.borrow_mut();
        parser.set_language(&lang).ok()?;
        parser.parse(&bytes, Some(&tree))
    })?;
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

/// Upper bound on closing braces appended during broken-syntax brace repair
/// in [`repaired_doc_at`].
const MAX_BRACE_REPAIRS: usize = 8;

/// The parse tree an `it`/`this` resolution ran against.
///
/// The two variants keep a repaired-tree answer from silently masquerading as
/// a normal one: any consumer that cares which tree produced the answer must
/// match. [`crate::indexer::infer::it_this::find_it_element_type`]
/// treats both the same because the resolution algorithm is identical either
/// way.
pub(crate) enum ResolutionDoc {
    /// The tree from [`Indexer::live_doc_or_parse`] — authoritative.
    Parsed(Arc<LiveDoc>),
    /// An append-only brace-repaired transient reparse (never cached into
    /// `live_trees`): the original tree had an ERROR node at/above the cursor
    /// and no enclosing `lambda_literal`.
    Repaired(LiveDoc),
}

impl ResolutionDoc {
    pub(crate) fn doc(&self) -> &LiveDoc {
        match self {
            ResolutionDoc::Parsed(doc) => doc,
            ResolutionDoc::Repaired(doc) => doc,
        }
    }
}

/// Typed observation of the cursor's tree, deciding whether broken-syntax
/// brace repair is permitted.
enum LambdaTreeGate {
    /// The ancestor chain contains a `lambda_literal`, or the whole tree is
    /// error-free — resolve on this tree; its answer (including `None`) is
    /// authoritative.
    Resolvable,
    /// No enclosing `lambda_literal` and the tree contains a parse error —
    /// the missing lambda may be unrepresentable (unclosed `{`); repair is
    /// permitted. Tree-wide, not chain-only: comments are tree-sitter extras
    /// that attach outside the ERROR node, so a cursor remapped onto a
    /// trailing comment has no ERROR ancestor even though the lambda is
    /// unclosed.
    BrokenSyntax,
}

fn lambda_tree_gate(node: tree_sitter::Node<'_>, tree_has_error: bool) -> LambdaTreeGate {
    let mut cursor = Some(node);
    while let Some(current) = cursor {
        if current.kind() == KIND_LAMBDA_LIT {
            return LambdaTreeGate::Resolvable;
        }
        cursor = current.parent();
    }
    if tree_has_error {
        LambdaTreeGate::BrokenSyntax
    } else {
        LambdaTreeGate::Resolvable
    }
}

/// Whether `lang`'s grammar can ever produce a `lambda_literal` node.
///
/// [`build_repair_candidates`]'s self-verification requires an enclosing one
/// to accept a candidate — when a grammar has no such node kind at all,
/// every repair attempt is guaranteed to fail regardless of input, so
/// [`lambda_doc_at`] skips repair entirely rather than burning
/// [`MAX_BRACE_REPAIRS`] full re-parses on a foregone conclusion. Verified
/// directly against each grammar's own `node-types.json` (never assume this
/// from a bare "not Kotlin" check): Java has no `lambda_literal` node kind at
/// all, but Swift's closure-expression grammar defines its own (distinct)
/// `lambda_literal` node — so this must stay a per-language grammar query,
/// not a hardcoded language list.
fn grammar_has_lambda_literal(lang: &tree_sitter::Language) -> bool {
    lang.id_for_node_kind(KIND_LAMBDA_LIT, true) != 0
}

/// Build the sequence of up to [`MAX_BRACE_REPAIRS`] append-only brace-repair
/// candidate reparses for `doc`.
///
/// Appending `\n}` at end-of-file shifts no existing byte offsets, so any
/// `pos` valid in `doc` remains valid in every candidate. Depends only on
/// `doc`'s content, not on any particular cursor position — safe to compute
/// once per file and reuse across every position checked against it (see
/// [`repair_candidates_for`]). Stops early if a reparse fails.
fn build_repair_candidates(doc: &LiveDoc, lang: &tree_sitter::Language) -> Vec<LiveDoc> {
    let Ok(base) = std::str::from_utf8(&doc.bytes) else {
        return Vec::new();
    };
    let mut source = base.to_owned();
    let mut candidates = Vec::with_capacity(MAX_BRACE_REPAIRS);
    for _ in 0..MAX_BRACE_REPAIRS {
        source.push_str("\n}");
        let Some(candidate) = parse_live(&source, lang.clone()) else {
            break;
        };
        candidates.push(candidate);
    }
    candidates
}

/// [`build_repair_candidates`], memoized per `uri` on `indexer`.
///
/// The candidate sequence is file-content-derived, not cursor-derived, so
/// every identifier's repair attempt within the same file reuses the same
/// up-to-[`MAX_BRACE_REPAIRS`] re-parses instead of rebuilding them from
/// scratch each time — the dominant cost `classify_cursor` pays when called
/// once per identifier across a whole file with any parse error.
/// `store_live_tree`/`remove_live_tree` clear this cache alongside
/// `live_trees` so a stale candidate list can never survive an edit.
fn repair_candidates_for(
    indexer: &Indexer,
    uri: &Url,
    doc: &LiveDoc,
    lang: &tree_sitter::Language,
) -> Arc<Vec<LiveDoc>> {
    if let Some(cached) = indexer.repair_candidates.get(uri.as_str()) {
        return Arc::clone(&*cached);
    }
    let candidates = Arc::new(build_repair_candidates(doc, lang));
    indexer
        .repair_candidates
        .insert(uri.to_string(), Arc::clone(&candidates));
    candidates
}

/// Whether the cursor at `pos` verifies against `candidate` — has an
/// enclosing `lambda_literal`.
fn verify_repair_candidate(candidate: &LiveDoc, pos: CursorPos) -> bool {
    cursor_node_at(candidate, pos)
        .and_then(|node| node.enclosing_lambda_literal())
        .is_some()
}

/// Pick the tree to resolve `it`/`this` against at `pos`.
///
/// Normally the tree from [`Indexer::live_doc_or_parse`]. When that tree has
/// an ERROR node at/above the cursor and no enclosing `lambda_literal`, the
/// syntax is broken in a way the CST cannot represent — tree-sitter forms no
/// `lambda_literal` for an unclosed `{`; the brace opens an ERROR node instead
/// (`it` in `items.forEach { it.name` parses as `simple_identifier` →
/// `navigation_expression` → `statements` → ERROR → `source_file`). In that
/// case resolve against an append-only brace repair (see
/// [`repair_candidates_for`]).
pub(crate) fn lambda_doc_at(indexer: &Indexer, uri: &Url, pos: CursorPos) -> Option<ResolutionDoc> {
    let doc = indexer.live_doc_or_parse(uri)?;
    let node = cursor_node_at(&doc, pos)?;
    let tree_has_error = doc.tree.root_node().has_error();
    match lambda_tree_gate(node, tree_has_error) {
        LambdaTreeGate::Resolvable => Some(ResolutionDoc::Parsed(doc)),
        LambdaTreeGate::BrokenSyntax => {
            let lang = lang_for_path(uri.path())?;
            if !grammar_has_lambda_literal(&lang) {
                return None;
            }
            let candidates = repair_candidates_for(indexer, uri, &doc, &lang);
            candidates
                .iter()
                .find(|candidate| verify_repair_candidate(candidate, pos))
                .cloned()
                .map(ResolutionDoc::Repaired)
        }
    }
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
            if p.kind() == KIND_NAV_EXPR {
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
    fn crlf_line_endings_keep_the_insertion_offset_exact() {
        // `str::lines()` drops the `\r`; the offset walk must count it, or
        // every line after the first drifts the InputEdit by one byte.
        let base = kotlin_doc("val a = 1\r\nval b = 2\r\nval x = foo.\r\n");
        let cursor = CursorPos {
            line: 2,
            utf16_col: 12,
        };
        let (doc, marker_byte) =
            speculative_doc(&base, tree_sitter_kotlin::language(), cursor).unwrap();
        let text = std::str::from_utf8(&doc.bytes).unwrap();
        assert_eq!(
            &text[marker_byte..marker_byte + COMPLETION_MARKER.len()],
            COMPLETION_MARKER
        );
        assert!(
            text.contains("val x = foo.kmpLspRulezz\r\n"),
            "text: {text:?}"
        );
        let node = receiver_node_for_marker(&doc, marker_byte).expect("receiver");
        assert_eq!(node.utf8_text(&doc.bytes).unwrap(), "foo");
        // Incremental reparse must agree with a fresh parse of the same bytes.
        let fresh = kotlin_doc(text);
        assert_eq!(
            doc.tree.root_node().to_sexp(),
            fresh.tree.root_node().to_sexp()
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

    #[test]
    fn nested_call_args_do_not_confuse_the_receiver() {
        let (kind, text) = receiver_of("fun f() { productFlow(trigger.isRefresh()).| }").unwrap();
        assert_eq!(kind, "call_expression");
        assert_eq!(text, "productFlow(trigger.isRefresh())");
    }

    #[test]
    fn double_safe_call_chain_receiver() {
        let (kind, text) = receiver_of("fun f() { a?.b?.| }").unwrap();
        assert_eq!(kind, "navigation_expression");
        assert_eq!(text, "a?.b");
    }

    // ─── brace-repair grammar gating + memoization ──────────────────────────

    #[test]
    fn kotlin_grammar_has_a_lambda_literal_node_kind() {
        assert!(grammar_has_lambda_literal(&tree_sitter_kotlin::language()));
    }

    #[test]
    fn swift_grammar_has_a_lambda_literal_node_kind() {
        // Swift's own closure-expression grammar defines a distinct
        // `lambda_literal` node — this must NOT be treated as "not Kotlin,
        // so skip repair," which would silently break Swift lambda/closure
        // resolution mid-typing.
        assert!(grammar_has_lambda_literal(
            &tree_sitter_swift_bundled::language()
        ));
    }

    #[test]
    fn java_grammar_has_no_lambda_literal_node_kind() {
        assert!(!grammar_has_lambda_literal(&tree_sitter_java::language()));
    }

    #[test]
    fn lambda_doc_at_repairs_an_unclosed_lambda() {
        let indexer = Indexer::new();
        let uri = Url::parse("file:///t/Unclosed.kt").unwrap();
        // No closing braces at all — both the function body's `{` and the
        // lambda's `{` are open at EOF, matching lambda_doc_at's own doc
        // comment example. Append-only repair supplies exactly what's
        // missing (one `}` for the lambda, one more for the function).
        let src = "fun f(items: List<String>) {\n    items.forEach { it.name\n";
        indexer.store_live_tree(&uri, src);
        let Some(line) = src.lines().nth(1) else {
            panic!("fixture must have a line 1");
        };
        let Some(col) = line.find("it.name") else {
            panic!("fixture line 1 must contain `it.name`");
        };
        let pos = CursorPos {
            line: 1,
            utf16_col: col,
        };

        let Some(resolution) = lambda_doc_at(&indexer, &uri, pos) else {
            panic!("expected a repaired resolution");
        };
        assert!(
            matches!(resolution, ResolutionDoc::Repaired(_)),
            "expected the ERROR-node/no-lambda-literal case to go through repair"
        );
    }

    #[test]
    fn lambda_doc_at_skips_repair_for_broken_java() {
        let indexer = Indexer::new();
        let uri = Url::parse("file:///t/Broken.java").unwrap();
        // A stray unmatched closing brace gives the tree a parse error
        // somewhere; Java has no `lambda_literal` node kind at all, so
        // repair could never verify regardless of where the cursor sits.
        let src = "class Broken {\n    void f() {\n        int x = 1;\n    }\n}\n}\n";
        indexer.store_live_tree(&uri, src);
        let Some(doc) = indexer.live_doc_or_parse(&uri) else {
            panic!("expected a live doc for the just-stored uri");
        };
        assert!(
            doc.tree.root_node().has_error(),
            "fixture must actually have a parse error for this test to be meaningful"
        );

        let pos = CursorPos {
            line: 2,
            utf16_col: 12,
        }; // inside `int x = 1;`, nowhere near the stray brace
        assert!(lambda_doc_at(&indexer, &uri, pos).is_none());
        assert!(
            indexer.repair_candidates.get(uri.as_str()).is_none(),
            "Java must never populate the repair-candidates cache — repair is skipped \
             before any candidate is ever built"
        );
    }

    #[test]
    fn repair_candidates_are_memoized_across_calls_for_the_same_file() {
        let indexer = Indexer::new();
        let uri = Url::parse("file:///t/Broken.kt").unwrap();
        let src = "fun f(items: List<String>) {\n    items.forEach { it.name\n}\n";
        indexer.store_live_tree(&uri, src);
        let Some(doc) = indexer.live_doc_or_parse(&uri) else {
            panic!("expected a live doc for the just-stored uri");
        };
        let lang = tree_sitter_kotlin::language();

        let first = repair_candidates_for(&indexer, &uri, &doc, &lang);
        let second = repair_candidates_for(&indexer, &uri, &doc, &lang);
        assert!(
            Arc::ptr_eq(&first, &second),
            "second call should reuse the cached candidates, not rebuild them"
        );
        assert!(
            !first.is_empty(),
            "expected at least one repair candidate to be built for this unclosed-lambda fixture"
        );
    }

    #[test]
    fn storing_a_new_live_tree_clears_the_stale_repair_candidate_cache() {
        let indexer = Indexer::new();
        let uri = Url::parse("file:///t/Broken.kt").unwrap();
        let src = "fun f(items: List<String>) {\n    items.forEach { it.name\n}\n";
        indexer.store_live_tree(&uri, src);
        let Some(doc) = indexer.live_doc_or_parse(&uri) else {
            panic!("expected a live doc for the just-stored uri");
        };
        let lang = tree_sitter_kotlin::language();
        let _ = repair_candidates_for(&indexer, &uri, &doc, &lang);
        assert!(indexer.repair_candidates.get(uri.as_str()).is_some());

        // Re-store the same URI with different (now well-formed) content.
        indexer.store_live_tree(&uri, "fun f() {}\n");
        assert!(
            indexer.repair_candidates.get(uri.as_str()).is_none(),
            "a fresh store_live_tree call must invalidate any memoized repair \
             candidates from the previous content"
        );
    }
}
