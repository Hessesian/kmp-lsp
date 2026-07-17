# CST Receiver Derivation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Derive dot-completion receivers from the CST via marker-insertion speculative parse, deleting the byte-scanners (`ReceiverExpr::parse`, `join_fluent_chain_continuation`, `resolve_dotted_receiver_type`).

**Architecture:** Insert a fake identifier (`kmpLspRulezz`) at the cursor, incremental-reparse a copy of the live doc, ascend from the marker to the enclosing `navigation_expression`, and resolve the receiver subtree's type via `CstQuery::expr_type` while the speculative tree is alive. A small owned `DotReceiver` value flows downstream; member collection is unchanged. Spec: `docs/superpowers/specs/2026-07-17-cst-receiver-derivation-design.md`.

**Tech Stack:** Rust, tree-sitter (incremental `Tree::edit` + reparse), existing `indexer/infer` engine (`InferDeps`, `CstQuery`).

## Global Constraints

- No string fallback for receiver *derivation*. The retained text-keyed *type* fallbacks are exactly: smart-cast (`infer_receiver_type_at`), variable lookup + fn-type extraction, uppercase→type-name, `function_return_type`, `infer_callable_param_return_type`.
- Smart-cast must keep working: for a receiver that is a **simple identifier**, do NOT resolve via `expr_type` at analysis time (its var-type lookup would shadow smart-cast narrowing); leave `resolved: None` and let the ladder run.
- The speculative parse runs only when `before_prefix.trim_end().ends_with('.')` — bare-word completions never pay the reparse.
- Existing suites (`resolver/tests.rs`, `features/completion_tests.rs`, `features/completion_context_tests.rs`) must pass; intentional behavior changes are listed in the PR description.
- Gates per commit: `cargo test` + pre-commit clippy. Final: both clippy profiles, e2e smoke, live probe.
- Branch: `refactor/cst-receiver-derivation`, PR → `refactor/unified-resolution`.

---

### Task 1: Speculative parse unit (`speculative.rs`)

**Files:**
- Create: `src/indexer/infer/speculative.rs`
- Modify: `src/indexer/infer/mod.rs` (add `pub(crate) mod speculative;` — check existing `mod` list style)
- Test: inline `#[cfg(test)]` in `speculative.rs` (module has no Indexer dependency — pure LiveDoc in/out)

**Interfaces:**
- Consumes: `LiveDoc { bytes, tree }` (`src/indexer/live_tree.rs`), `utf16_col_to_byte` (same file), `CursorPos { line, utf16_col }` (`src/types.rs`), `tree_sitter_kotlin::language()`.
- Produces (later tasks rely on these exact signatures):
  - `pub(crate) const COMPLETION_MARKER: &str = "kmpLspRulezz";`
  - `pub(crate) fn speculative_doc(base: &LiveDoc, lang: tree_sitter::Language, cursor: CursorPos) -> Option<(LiveDoc, usize)>` — the speculative doc plus the marker's byte offset.
  - `pub(crate) fn receiver_node_for_marker(doc: &LiveDoc, marker_byte: usize) -> Option<tree_sitter::Node<'_>>` (Task 2 adds this; declared here so the module doc mentions both).

- [ ] **Step 1: Write failing tests** (in `speculative.rs`'s test module)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::live_tree::parse_live;
    use crate::types::CursorPos;

    fn kotlin_doc(src: &str) -> crate::indexer::live_tree::LiveDoc {
        parse_live(src, tree_sitter_kotlin::language()).unwrap()
    }

    #[test]
    fn trailing_dot_state_parses_cleanly_with_the_marker() {
        // `val x = foo.` alone parses with an ERROR node; the marker heals it.
        let base = kotlin_doc("val x = foo.\n");
        let cursor = CursorPos { line: 0, utf16_col: 12 };
        let (doc, marker_byte) = speculative_doc(&base, tree_sitter_kotlin::language(), cursor)
            .expect("speculative parse");
        assert_eq!(marker_byte, 12);
        let text = std::str::from_utf8(&doc.bytes).unwrap();
        assert_eq!(text, "val x = foo.kmpLspRulezz\n");
        // The healed tree contains a navigation_expression covering foo.<marker>.
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
        assert!(found_nav, "expected navigation_expression ancestor, tree: {}",
            doc.tree.root_node().to_sexp());
    }

    #[test]
    fn incremental_reparse_matches_a_fresh_parse() {
        let src = "class A {\n    fun f() {\n        val m = Modifier\n            .fillMaxSize()\n            .\n    }\n}\n";
        let base = kotlin_doc(src);
        let cursor = CursorPos { line: 4, utf16_col: 13 };
        let (doc, _) =
            speculative_doc(&base, tree_sitter_kotlin::language(), cursor).unwrap();
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
        let cursor = CursorPos { line: 1, utf16_col: 0 };
        let (doc, marker_byte) =
            speculative_doc(&base, tree_sitter_kotlin::language(), cursor).unwrap();
        assert_eq!(marker_byte, 13);
        assert!(std::str::from_utf8(&doc.bytes).unwrap().ends_with("kmpLspRulezz"));
    }

    #[test]
    fn cursor_beyond_content_returns_none() {
        let base = kotlin_doc("val x = 1\n");
        let cursor = CursorPos { line: 7, utf16_col: 0 };
        assert!(speculative_doc(&base, tree_sitter_kotlin::language(), cursor).is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib indexer::infer::speculative 2>&1 | tail -5`
Expected: compile error — `speculative_doc` not defined.

- [ ] **Step 3: Implement**

```rust
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
        return Some((source.len(), Point { row: cursor.line, column: 0 }));
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
```

Register the module in `src/indexer/infer/mod.rs` next to the existing `mod chain;` etc.:

```rust
pub(crate) mod speculative;
```

(If `LiveDoc` construction or `utf16_col_to_byte` visibility differs, adjust — both are `pub(crate)` today.)

- [ ] **Step 4: Run tests**

Run: `cargo test --lib indexer::infer::speculative 2>&1 | tail -5`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add src/indexer/infer/speculative.rs src/indexer/infer/mod.rs
git commit -m "feat(infer): marker-insertion speculative parse for cursor states"
```

---

### Task 2: Receiver extraction from the speculative tree

**Files:**
- Modify: `src/indexer/infer/speculative.rs`
- Test: same file's test module

**Interfaces:**
- Consumes: `speculative_doc` (Task 1), node-kind constants `KIND_NAV_EXPR = "navigation_expression"`, `KIND_NAV_SUFFIX = "navigation_suffix"` from `src/queries.rs`.
- Produces: `pub(crate) fn receiver_node_for_marker(doc: &LiveDoc, marker_byte: usize) -> Option<tree_sitter::Node<'_>>` — the receiver subtree (left child of the nav-expr whose suffix contains the marker). Node kinds later tasks branch on: `simple_identifier`, `navigation_expression`, `call_expression`, `super_expression`, `this_expression`.

- [ ] **Step 1: Write failing tests**

```rust
    // Helper for the extraction tests: derive the receiver's (kind, text).
    fn receiver_of(src_with_caret: &str) -> Option<(String, String)> {
        // `|` marks the cursor; strip it to build the real source.
        let caret = src_with_caret.find('|').expect("caret");
        let src: String = src_with_caret.replace('|', "");
        let line = src_with_caret[..caret].matches('\n').count();
        let line_start = src_with_caret[..caret].rfind('\n').map_or(0, |p| p + 1);
        let col = src_with_caret[line_start..caret].encode_utf16().count();
        let base = kotlin_doc(&src);
        let (doc, marker) = speculative_doc(
            &base,
            tree_sitter_kotlin::language(),
            CursorPos { line, utf16_col: col },
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
        assert_eq!(kind, "navigation_expression");
        assert!(text.contains("Modifier"));
        assert!(text.contains("fillMaxSize()"));
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
        let (kind, text) =
            receiver_of("fun f() { items.forEach { this@forEach.| } }").unwrap();
        assert_eq!(kind, "this_expression");
        assert_eq!(text, "this@forEach");
    }

    #[test]
    fn bare_word_context_has_no_receiver() {
        assert!(receiver_of("fun f() { Modif| }").is_none());
    }
```

(If a grammar shape differs from an assertion — e.g. the unclosed-paren state genuinely forms no nav-expr — the test documents reality: change the assertion to match observed behavior and note it. These are characterization tests; the *string-literal*, *comment*, and *bare-word* `None` cases are requirements, not characterizations.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib indexer::infer::speculative 2>&1 | tail -5`
Expected: compile error — `receiver_node_for_marker` not defined.

- [ ] **Step 3: Implement**

```rust
use crate::queries::{KIND_NAV_EXPR, KIND_NAV_SUFFIX};

/// Number of ancestor hops from the marker before giving up. The marker sits
/// in `navigation_suffix` one or two levels up in well-formed trees; the
/// allowance covers ERROR-node wrapping in broken mid-edit states.
const MAX_ASCENT: usize = 6;

/// From the marker identifier, find the `navigation_expression` whose suffix
/// contains the marker and return its receiver subtree (the left child).
///
/// Returns `None` when the marker is not the member position of a navigation
/// (bare word, string literal, comment) — the caller treats that as bare-word
/// completion.
pub(crate) fn receiver_node_for_marker(
    doc: &LiveDoc,
    marker_byte: usize,
) -> Option<tree_sitter::Node<'_>> {
    let marker_end = marker_byte + COMPLETION_MARKER.len();
    let node = doc
        .tree
        .root_node()
        .descendant_for_byte_range(marker_byte, marker_end)?;
    // Comments and string content swallow the marker: the covering node is a
    // comment/string token, never an identifier — and no nav_suffix ancestor
    // will contain the marker as its member, so the ascent below returns None
    // naturally (interpolation `${...}` DOES form real expression nodes and
    // resolves like ordinary code).
    let mut cur = node;
    for _ in 0..MAX_ASCENT {
        let parent = cur.parent()?;
        if parent.kind() == KIND_NAV_SUFFIX {
            let nav = parent.parent()?;
            if nav.kind() != KIND_NAV_EXPR {
                return None;
            }
            // The receiver is the nav-expr's first child (the subtree before
            // the suffix). Guard: the suffix we ascended through must be the
            // marker's own, not an outer chain segment's.
            if parent.start_byte() > marker_end || parent.end_byte() < marker_byte {
                return None;
            }
            return nav.child(0);
        }
        cur = parent;
    }
    None
}
```

- [ ] **Step 4: Run tests, adjust characterizations**

Run: `cargo test --lib indexer::infer::speculative 2>&1 | tail -8`
Expected: all pass. For any characterization mismatch (unclosed-delimiter shapes), print `to_sexp()` in the failure, adjust the assertion to observed reality, and record the shape in a comment. The string/comment/bare-word `None` tests must pass as written.

- [ ] **Step 5: Commit**

```bash
git add src/indexer/infer/speculative.rs
git commit -m "feat(infer): extract dot-completion receiver subtree from the speculative tree"
```

---

### Task 3: `DotReceiver` type + derivation entry

**Files:**
- Modify: `src/resolver/complete.rs` (add `DotReceiver` next to `ReceiverExpr` — deletion of `ReceiverExpr` comes in Task 6)
- Modify: `src/features/completion_context.rs` (add `derive_dot_receiver`)
- Modify: `src/indexer/infer/mod.rs` if re-exports are needed (`speculative::{speculative_doc, receiver_node_for_marker, COMPLETION_MARKER}` — follow the existing re-export style at the top of `src/indexer.rs` / `mod.rs`)
- Test: `src/features/completion_context_tests.rs`

**Interfaces:**
- Consumes: Task 1/2 functions; `Indexer::live_doc_or_parse`, `lang_for_path` (`src/indexer/live_tree.rs`), `CstQuery::new(node, doc, index, uri, ResolveIo::NoRg).expr_type()` (`src/indexer/infer/mod.rs:146-197`), `Resolution`/`ResolvedType` (same file).
- Produces:

```rust
// in src/resolver/complete.rs
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DotReceiver {
    /// `it`, `this`, `this@label` — resolved by `ScopeContext`, routed to
    /// `complete_lambda_dot` before member collection.
    Scope(String),
    Super,
    /// Any other receiver expression.
    Expr {
        /// Receiver text — for a call receiver, the callee text (the final
        /// argument list is implied by `is_call`, mirroring the old chain
        /// normalization). Feeds the retained text-keyed type fallbacks.
        text: String,
        /// CST-derived: the receiver subtree was a call_expression.
        is_call: bool,
        /// Type resolved by `CstQuery::expr_type` at analysis time. `None`
        /// for simple identifiers (smart-cast must get first look) and for
        /// CST-unresolvable receivers.
        resolved: Option<String>,
    },
}

impl DotReceiver {
    /// Plain variable / type-name receiver (the `complete_symbol` entry).
    pub(crate) fn expr(text: &str) -> Self {
        Self::Expr { text: text.to_owned(), is_call: false, resolved: None }
    }
    pub(crate) fn text(&self) -> &str { /* Scope(s)/Expr{text} → s, Super → "super" */ }
}
```

```rust
// in src/features/completion_context.rs
pub(crate) fn derive_dot_receiver(
    index: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<DotReceiver>
```

- [ ] **Step 1: Write failing tests** (in `completion_context_tests.rs`, following its existing fixture style — look at how sibling tests build an `Indexer` and open a doc; reuse that helper)

```rust
// Sketch — adapt fixture setup to the file's existing helpers:
#[test]
fn derives_a_simple_identifier_receiver_with_no_early_resolution() {
    let (index, uri) = fixture_with_open_doc("fun f() { val user = User()\n user. }", ...);
    let recv = derive_dot_receiver(&index, &uri, Position::new(1, 7)).unwrap();
    assert_eq!(
        recv,
        DotReceiver::Expr { text: "user".into(), is_call: false, resolved: None }
    );
}

#[test]
fn derives_and_resolves_a_chain_receiver() {
    // Fixture: class Theme { val colors: Palette }, val theme = Theme()
    // cursor after `theme.colors.`
    let recv = derive_dot_receiver(&index, &uri, pos).unwrap();
    match recv {
        DotReceiver::Expr { is_call: false, resolved: Some(t), .. } => {
            assert_eq!(t, "Palette")
        }
        other => panic!("expected resolved chain, got {other:?}"),
    }
}

#[test]
fn derives_a_call_receiver_with_callee_text() {
    // cursor after `productFlow(x).`
    let recv = derive_dot_receiver(&index, &uri, pos).unwrap();
    match recv {
        DotReceiver::Expr { text, is_call: true, .. } => assert_eq!(text, "productFlow"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn classifies_scope_and_super_receivers() {
    // `it.` → Scope("it"); `this@forEach.` → Scope("this@forEach"); `super.` → Super
}

#[test]
fn multiline_fluent_chain_derives_a_receiver() {
    // The Compose idiom across 3 lines; assert Expr with text containing "Modifier".
}

#[test]
fn no_receiver_for_bare_word_or_string_interior() {
    // `Modif|` → None; cursor inside "foo.|" string literal → None.
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib features::completion_context 2>&1 | tail -5`
Expected: compile error — `derive_dot_receiver` / `DotReceiver` not defined.

- [ ] **Step 3: Implement**

`DotReceiver` in `src/resolver/complete.rs` as specified in Interfaces. Then:

```rust
// src/features/completion_context.rs
use crate::indexer::infer::speculative::{receiver_node_for_marker, speculative_doc};
use crate::indexer::live_tree::lang_for_path;
use crate::queries::{KIND_CALL_EXPR, KIND_SIMPLE_IDENT, KIND_SUPER_EXPR, KIND_THIS_EXPR};
use crate::resolver::complete::DotReceiver;

const IT: &str = "it";   // already defined at the top of this file
const THIS: &str = "this";

/// Derive the dot-completion receiver at `position` from the CST.
///
/// Marker-insertion speculative parse (see `indexer/infer/speculative.rs`);
/// the receiver's type is resolved here, while the speculative tree is alive,
/// so only an owned `DotReceiver` escapes.
pub(crate) fn derive_dot_receiver(
    index: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<DotReceiver> {
    let base = index.live_doc_or_parse(uri)?;
    let lang = lang_for_path(uri.path())?;
    let cursor = CursorPos {
        line: position.line as usize,
        utf16_col: position.character as usize,
    };
    let (doc, marker_byte) = speculative_doc(&base, lang, cursor)?;
    let node = receiver_node_for_marker(&doc, marker_byte)?;
    let text = node.utf8_text(&doc.bytes).ok()?.to_owned();

    if node.kind() == KIND_SUPER_EXPR || text == "super" {
        return Some(DotReceiver::Super);
    }
    if node.kind() == KIND_THIS_EXPR || text == IT || text == THIS {
        return Some(DotReceiver::Scope(text));
    }

    let is_call = node.kind() == KIND_CALL_EXPR;
    // Call receivers: the fallback ladder keys on the callee text (the final
    // `(...)` is implied by `is_call`), matching the old chain normalization.
    let text = if is_call {
        node.child(0)
            .and_then(|callee| callee.utf8_text(&doc.bytes).ok())
            .map(str::to_owned)
            .unwrap_or(text)
    } else {
        text
    };
    // Simple identifiers stay unresolved here: `expr_type`'s declared-variable
    // lookup would shadow smart-cast narrowing, which must get first look in
    // the downstream ladder.
    let resolved = if node.kind() == KIND_SIMPLE_IDENT {
        None
    } else {
        match CstQuery::new(node, &doc, index, uri, ResolveIo::NoRg).expr_type() {
            Resolution::Resolved(t) => Some(t.as_type_str().to_owned()),
            _ => None,
        }
    };
    Some(DotReceiver::Expr { text, is_call, resolved })
}
```

Add `KIND_SUPER_EXPR = "super_expression"` to `src/queries.rs` if missing (grep first — `KIND_THIS_EXPR` exists). Import `Resolution`, `ResolvedType` per the existing imports at the top of `completion_context.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib features::completion_context 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/resolver/complete.rs src/features/completion_context.rs src/features/completion_context_tests.rs src/queries.rs
git commit -m "feat(complete): DotReceiver derived from the speculative CST"
```

---

### Task 4: Rewire the completion pipeline onto `DotReceiver`

**Files:**
- Modify: `src/features/completion.rs` (`run_completions` at ~156-238; delete the `joined_chain` block at 185-190)
- Modify: `src/features/completion_context.rs` (`CompletionContext::analyse` at 70-88)
- Modify: `src/resolver/complete.rs` (`complete_symbol` 301-318, `complete_symbol_with_context` 322-336, `complete_dot_expr` 701-753)
- Test: existing suites drive this task — no new tests; run them.

**Interfaces:**
- Consumes: `derive_dot_receiver`, `DotReceiver` (Task 3).
- Produces:
  - `CompletionContext::analyse(position, index, uri, annotation_only, wants_receiver: bool) -> Self` — `before_prefix` param replaced by `wants_receiver` (the `.`-gate computed by the caller); `receiver: Option<DotReceiver>`.
  - `complete_symbol_with_context(indexer, prefix, dot_receiver: Option<DotReceiver>, from_uri, snippets, annotation_only, cursor_line)` — same name, receiver type changed.
  - `complete_symbol` keeps its `Option<&str>` signature (external callers unchanged) and wraps via `DotReceiver::expr`.

- [ ] **Step 1: Rewire `analyse`**

```rust
impl CompletionContext {
    /// Single analysis pass for a cache miss. `wants_receiver` is the caller's
    /// dot-gate: the text before the completion prefix ends with `.` (after
    /// trailing-whitespace trim), so a speculative parse can pay off.
    pub(crate) fn analyse(
        position: Position,
        index: &Indexer,
        uri: &Url,
        annotation_only: bool,
        wants_receiver: bool,
    ) -> Self {
        let scope = ScopeContext::build(position, index, uri);
        let call_info = build_call_info(position, index, uri);
        Self {
            receiver: wants_receiver
                .then(|| derive_dot_receiver(index, uri, position))
                .flatten(),
            annotation_only,
            scope,
            call_info,
        }
    }
}
```

`CompletionContext.receiver` field type becomes `Option<DotReceiver>`.

- [ ] **Step 2: Rewire `run_completions`**

Replace lines 184-214 (the `lines` fetch stays — `complete_lambda_dot` uses it):

```rust
    let annotation_only = is_annotation_context(before, prefix);
    let lines = index.lines_for(uri).unwrap_or_default();
    let wants_receiver = before_prefix.trim_end().ends_with('.');
    let ctx = CompletionContext::analyse(position, index, uri, annotation_only, wants_receiver);

    if let Some(ref recv) = ctx.receiver {
        let recv_str = recv.text();
        let scope_recv = matches!(recv, DotReceiver::Scope(_));
        if scope_recv || is_lambda_param(recv_str, before, index, uri, position.line as usize) {
            return (
                complete_lambda_dot(/* unchanged args, recv_str */),
                false,
            );
        }
    }
```

`DotReceiver::text()` (Task 3) returns the scope/expr text (`"super"` for `Super`). The `is_lambda_param` routing stays — it already delegates to the CST-backed `lambda_params_at_col`; only its receiver *source* changed.

- [ ] **Step 3: Rewire `complete_symbol_with_context` and `complete_dot_expr`**

```rust
pub(crate) fn complete_symbol(
    indexer: &Indexer,
    prefix: &str,
    dot_receiver: Option<&str>,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
) -> (Vec<CompletionItem>, bool) {
    complete_symbol_with_context(
        indexer,
        prefix,
        dot_receiver.map(DotReceiver::expr),
        from_uri,
        snippets,
        false,
        cursor_line,
    )
}
```

In `complete_dot_expr`, replace the `expr.as_str() == "super"` check:

```rust
fn complete_dot_expr(
    indexer: &Indexer,
    expr: &DotReceiver,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
) -> Vec<CompletionItem> {
    if matches!(expr, DotReceiver::Super) || expr.text() == "super" {
        return complete_super(indexer, from_uri, snippets);
    }
    // … rest unchanged; resolve_dot_receiver_type now takes &DotReceiver (Task 5
    // rewrites its body; this task only changes the parameter type and threads
    // `expr` through).
```

For this task, make `resolve_dot_receiver_type` compile against `DotReceiver` with **old semantics**: destructure `DotReceiver::Expr { text, is_call, resolved }` (treat `Scope(s)` text as a variable name — pre-existing behavior for the `complete_symbol` path) and use `resolved` first:

```rust
    if let DotReceiver::Expr { resolved: Some(t), .. } = expr {
        return Some(ReceiverType::from_raw(t.clone()));
    }
```

then the existing `is_call` / non-call ladders operating on `expr.text()` — keep `resolve_dotted_receiver_type` calls in place for now (deleted in Task 5).

- [ ] **Step 4: Fix compile fallout mechanically**

Run: `cargo build 2>&1 | grep -E "^error" | head -20`
The `#[cfg(test)] complete_dot` wrapper (complete.rs:685) wraps via `DotReceiver::expr(receiver)`. Update `resolver/tests.rs` / `completion_tests.rs` compile errors by constructing `DotReceiver` where they built `ReceiverExpr` (30 references — mostly `ReceiverExpr::parse` unit tests, which Task 6 deletes; for now only fix what blocks compilation, e.g. imports).

- [ ] **Step 5: Run the full suites**

Run: `cargo test --lib 2>&1 | tail -15`
Expected: failures ONLY in tests that (a) unit-test `ReceiverExpr::parse` / `join_fluent_chain_continuation` internals (deleted in Task 6 — `#[ignore]` nothing; leave them failing only if Task 6 lands in the same PR, otherwise fix order: do Step 6 first), or (b) expose real regressions — fix those now. Multiline-chain completion tests (e.g. resolver/tests.rs ~4843) must pass via the new path.

- [ ] **Step 6: Commit**

```bash
git add -A src/
git commit -m "refactor(complete): completion pipeline consumes CST-derived DotReceiver"
```

---

### Task 5: New type-resolution ladder; delete `resolve_dotted_receiver_type`

**Files:**
- Modify: `src/resolver/complete.rs` (`resolve_dot_receiver_type` 760-815; delete `resolve_dotted_receiver_type` ~819-880)
- Test: `src/resolver/tests.rs` — existing dot-completion tests are the net; add one RED test for the resolved-fast-path.

**Interfaces:**
- Consumes: `DotReceiver` (Task 3); retained fallbacks: `infer_receiver_type_at` (`resolver/infer.rs:227`), `infer_receiver_type` + `ReceiverKind::Variable` (`resolver/infer.rs:96,157`), `extract_fn_type_return` (`complete.rs:858`), `Indexer::function_return_type`, `infer_callable_param_return_type` (`resolver/infer_lines.rs`).
- Produces: `fn resolve_dot_receiver_type(indexer, expr: &DotReceiver, from_uri, cursor_line) -> Option<ReceiverType>` with the ladder below.

- [ ] **Step 1: Write the failing fast-path test**

```rust
#[test]
fn cst_resolved_receiver_type_wins_over_the_text_ladder() {
    let (indexer, uri) = /* minimal fixture: no `Palette` variable exists */;
    let recv = DotReceiver::Expr {
        text: "theme.colors".into(),
        is_call: false,
        resolved: Some("Palette".into()),
    };
    // Fixture declares `class Palette { fun swap() {} }` in the same package.
    let items = complete_dot_expr_test_entry(&indexer, &recv, &uri);
    assert!(items.iter().any(|i| i.label.starts_with("swap")));
}
```

(Use the file's existing fixture helpers; expose a test-visible entry if `complete_dot_expr` is private — the existing `#[cfg(test)] complete_dot` pattern shows the convention.)

- [ ] **Step 2: Run to verify failure** — `cargo test --lib cst_resolved_receiver 2>&1 | tail -3` (fails if Task 4's interim wiring didn't already implement the fast path; if it passes, note it and move on — the test still pins the contract).

- [ ] **Step 3: Rewrite the ladder**

```rust
fn resolve_dot_receiver_type(
    indexer: &Indexer,
    expr: &DotReceiver,
    from_uri: &Url,
    cursor_line: Option<u32>,
) -> Option<ReceiverType> {
    let (text, is_call, resolved) = match expr {
        DotReceiver::Expr { text, is_call, resolved } => (text.as_str(), *is_call, resolved.as_deref()),
        // Scope receivers are routed to complete_lambda_dot before member
        // collection; reaching here means a plain-text receiver from the
        // complete_symbol entry — treat as a non-call expression.
        DotReceiver::Scope(s) => (s.as_str(), false, None),
        DotReceiver::Super => return None,
    };

    // CST-resolved type from analysis time is authoritative.
    if let Some(t) = resolved {
        return Some(ReceiverType::from_raw(t.to_owned()));
    }

    if is_call {
        // Call receiver the CST engine couldn't type: global fn return type,
        // then callable-param inference (`val make: () -> Foo` + `make().`).
        if let Some(ret) = indexer.function_return_type(text, from_uri) {
            return Some(ReceiverType::from_raw(ret.into_inner()));
        }
        let file = ensure_file_data(indexer, from_uri)?;
        let ret = infer_callable_param_return_type(&file.lines, text)?;
        return Some(ReceiverType::from_raw(ret));
    }

    // Smart-cast narrowing gets first look for plain receivers.
    if let Some(line) = cursor_line {
        let pos = Position::new(line, 0);
        if let Some(rt) = infer_receiver_type_at(indexer, text, from_uri, pos) {
            return Some(rt);
        }
    }
    if let Some(rt) = infer_receiver_type(indexer, ReceiverKind::Variable(text), from_uri) {
        if let Some(ret) = extract_fn_type_return(&rt.raw) {
            return Some(ReceiverType::from_raw(ret));
        }
        return Some(rt);
    }
    if text.starts_with_uppercase() {
        return Some(ReceiverType::from_raw(text.to_string()));
    }
    if let Some(ret) = indexer.function_return_type(text, from_uri) {
        return Some(ReceiverType::from_raw(ret.into_inner()));
    }
    let file = ensure_file_data(indexer, from_uri)?;
    let ret = infer_callable_param_return_type(&file.lines, text)?;
    Some(ReceiverType::from_raw(ret))
}
```

Delete `resolve_dotted_receiver_type` and its now-unused helpers (`infer_variable_type_raw` import stays if others use it — check with `cargo build`).

- [ ] **Step 4: Run the full suite; extend chain.rs where the net catches gaps**

Run: `cargo test --lib 2>&1 | tail -15`
The spec budgets for this: dotted-chain completion tests failing here mean `expr_type`'s nav path missed something the deleted string walker handled (most likely: type-name-rooted chains — though `infer_ident_type`'s uppercase-known-type fallback should cover companions). Fix by extending `chain.rs` / `expr_type.rs` root resolution — never by resurrecting the string walker. Timebox: if a gap needs design (not mechanics), stop and flag rather than bolt heuristics into the CST engine.

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "refactor(complete): receiver type ladder consumes CST resolution; drop string chain walker"
```

---

### Task 6: Delete the byte-scanners and their tests

**Files:**
- Modify: `src/resolver/complete.rs` — delete `ReceiverExpr` (struct + `parse` + `variable` + `as_str`, lines 163-294)
- Modify: `src/features/completion.rs` — delete `join_fluent_chain_continuation`, `MAX_FLUENT_CHAIN_LINES`, `strip_line_comment` (lines 528-602)
- Modify: `src/resolver/tests.rs`, `src/features/completion_tests.rs` — delete their unit tests (~30 references)
- Test: full suite.

**Interfaces:**
- Consumes: everything already migrated in Tasks 4-5. This task is pure deletion.
- Produces: nothing new — `grep -rn "ReceiverExpr\|join_fluent" src/` must return zero hits.

- [ ] **Step 1: Delete, then hunt stragglers**

Run: `grep -rn "ReceiverExpr\|join_fluent\|MAX_FLUENT_CHAIN_LINES" src/ --include='*.rs'`
Expected: no hits after deletion.

- [ ] **Step 2: Audit the deleted tests for behavior coverage** — for each deleted `ReceiverExpr::parse` / `join_fluent` unit test, confirm an equivalent end-to-end case exists in the Task 2/3 characterization tests (chain-with-args, `?.`, comment-in-chain, multiline idiom are all there). Port any case that isn't (as a `derive_dot_receiver` or `receiver_of` test, not a scanner test).

- [ ] **Step 3: Full gates**

Run: `cargo test 2>&1 | tail -5 && cargo clippy --all-targets 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add -A src/
git commit -m "refactor(complete): delete receiver byte-scanners — CST derivation is the only path"
```

---

### Task 7: Live probe + PR

**Files:**
- Scratchpad: adapt `lsp_probe.py` (see ledger wave-7d / the 2026-07-16 entry for the didOpen/didChange/completion scripting pattern; **BIN must point at this worktree's `target/debug/kmp-lsp`**, never `~/.cargo/bin`).

- [ ] **Step 1: Build and probe against the real project**

Run: `cargo build 2>&1 | tail -2`, then the probe with three scenarios on real Moneta files:
1. Multiline `Modifier` chain continuation (`.padd|` on a fresh line) → expect Modifier-extension items.
2. A `?.` receiver → expect member items.
3. A broken mid-edit state (unclosed brace above the cursor) → expect dot-completion still works (or degrades to bare-word, never panics/garbage).

Expected: parity or better vs. the installed binary on the same scenarios.

- [ ] **Step 2: Full final gates**

Run: `cargo test 2>&1 | tail -3 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -2 && cargo clippy --release --all-targets -- -D warnings 2>&1 | tail -2`

- [ ] **Step 3: Push, open PR → `refactor/unified-resolution`**

PR description: what was deleted (three scanners, ~230 lines), what replaced it (marker-insertion speculative parse + CST resolution), intentional behavior changes (no phantom receivers inside strings/comments; `foo. |` whitespace-dot now completes; interpolation receivers now work), and the characterization-test inventory. Note: `gh pr merge --delete-branch` fails while the main repo holds the base branch — merge without it.

- [ ] **Step 4: Ledger + memory**

Append the slice entry to `.superpowers/sdd/progress.md`; update the `cst-resolution-unification` memory (this is a slice of slices 2-6; note what remains).

## Self-review notes

- Spec coverage: Goals 1-3 = Tasks 1-6; Goal 4 (suites pass) = Tasks 4-6 step gates; spec's testing section = Task 2/3 characterization tests + Task 7 probe. Non-goals respected: `is_lambda_param` retained (already CST-backed), smart-cast/uppercase/fn-type/function-return/callable-param fallbacks retained in Task 5's ladder.
- Type consistency: `DotReceiver` defined once (Task 3 Interfaces), consumed by Tasks 4-5 with identical field names; `speculative_doc`/`receiver_node_for_marker` signatures match between Tasks 1-3.
- Known judgment points for the implementer: characterization assertions in Task 2 may need adjusting to observed grammar shapes (only the string/comment/bare-word `None` cases are hard requirements); Task 5 Step 4 may require chain.rs extension — stop-and-flag if it's design-shaped.
