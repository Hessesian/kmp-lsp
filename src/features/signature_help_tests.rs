// Tests for src/features/signature_help.rs
// See: https://github.com/Hessesian/kotlin-lsp/issues/124

use tower_lsp::lsp_types::{Position, Url};

use crate::features::signature_help::compute_signature_help;
use crate::indexer::Indexer;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

/// Set up indexer with `src` indexed and `live_lines` pointing at the same content.
/// No `live_trees` — simulates the did_change path where `set_live_lines` was called
/// synchronously but the actor's `spawn_live_tree_update` hasn't completed yet.
fn setup_with_live_lines(path: &str, src: &str) -> (Url, Indexer) {
    let u = uri(path);
    let idx = Indexer::new();
    idx.index_content(&u, src);
    idx.set_live_lines(&u, src);
    (u, idx)
}

/// Reproduce the Zed race: actor processed FileOpened (live_trees = original content),
/// then did_change updated live_lines to new content, but live_trees was NOT cleared.
fn setup_with_stale_live_tree(path: &str, original: &str, new_content: &str) -> (Url, Indexer) {
    let u = uri(path);
    let idx = Indexer::new();
    idx.index_content(&u, original);
    idx.store_live_tree(&u, original); // actor stored tree of original content
    idx.set_live_lines(&u, new_content); // did_change updated live_lines but NOT live_trees
    (u, idx)
}

// ── Basic scenarios ───────────────────────────────────────────────────────────

#[test]
fn signature_help_cursor_after_open_paren() {
    // Realistic file with auto-paired parens (how editors work):
    //
    // line 0: fun greet(name: String, age: Int) {}
    // line 1: fun main() {
    // line 2:     greet()    ← cursor at col 10, right after `(`, before `)`
    // line 3: }
    //
    // "    greet()" → 4 spaces + "greet" (5) + "(" (1) = col 10 is on `)`.
    // This is the position editors send signatureHelp when user just typed `(`.
    let src = "fun greet(name: String, age: Int) {}\nfun main() {\n    greet()\n}";
    let (u, idx) = setup_with_live_lines("/Foo.kt", src);
    let pos = Position {
        line: 2,
        character: 10,
    };
    let result = compute_signature_help(&u, pos, &idx);
    assert!(
        result.is_some(),
        "expected signature help after `(`, got None"
    );
    let sig = result.unwrap();
    assert_eq!(sig.signatures.len(), 1);
    assert!(
        sig.signatures[0].label.contains("greet"),
        "label should contain function name, got: {:?}",
        sig.signatures[0].label
    );
    assert_eq!(
        sig.active_parameter,
        Some(0),
        "cursor right after `(` should be on first parameter"
    );
}

#[test]
fn signature_help_cursor_after_comma() {
    // line 2:     greet("hello", )  ← cursor at col 19, after `hello", ` and before `)`
    // "    greet(\"hello\", )" = 4 + 5 + 1 + 7 + 1 + 1 + 1 = 20 chars.
    // Cursor at col 19 = space after `,`, before `)`.
    let src = "fun greet(name: String, age: Int) {}\nfun main() {\n    greet(\"hello\", )\n}";
    let (u, idx) = setup_with_live_lines("/Foo.kt", src);
    let pos = Position {
        line: 2,
        character: 19,
    };
    let result = compute_signature_help(&u, pos, &idx);
    assert!(
        result.is_some(),
        "expected signature help after `,`, got None"
    );
    let sig = result.unwrap();
    assert_eq!(
        sig.active_parameter,
        Some(1),
        "cursor after first arg should be on second parameter"
    );
}

#[test]
fn signature_help_single_param_function() {
    // "    show()" → "    show(" = 9 chars → col 9 is on `)`.
    let src = "fun show(message: String) {}\nfun main() {\n    show()\n}";
    let (u, idx) = setup_with_live_lines("/Foo.kt", src);
    let pos = Position {
        line: 2,
        character: 9,
    };
    let result = compute_signature_help(&u, pos, &idx);
    assert!(
        result.is_some(),
        "expected signature help for single-param function"
    );
    let sig = result.unwrap();
    assert!(sig.signatures[0]
        .parameters
        .as_ref()
        .is_some_and(|p| p.len() == 1));
}

#[test]
fn signature_help_outside_call_returns_none() {
    let src = "fun greet(name: String) {}\nval x = 42\n";
    let (u, idx) = setup_with_live_lines("/Foo.kt", src);
    let pos = Position {
        line: 1,
        character: 4,
    };
    let result = compute_signature_help(&u, pos, &idx);
    assert!(
        result.is_none(),
        "expected None when cursor is not inside a call"
    );
}

// ── Zed regression: stale live_tree ──────────────────────────────────────────

/// Regression test for https://github.com/Hessesian/kotlin-lsp/issues/124
///
/// Documents the bug state: when `live_trees` holds stale CST from did_open
/// and `live_lines` has newer content (from did_change), `live_doc_or_parse`
/// returns the stale tree which has no `call_expression` at the cursor → None.
///
/// Timeline that breaks Zed (and any fast editor):
///
///   1. did_open  → actor processes FileOpened → store_live_tree(original)
///   2. did_change → set_live_lines(new with call)   [synchronous in backend]
///   3. textDocument/signatureHelp arrives
///   4. cst_call_info → live_doc_or_parse → live_doc() returns STALE tree
///      (original content, no call_expression at cursor) → None
///
/// The fix is in `did_change` (src/backend/mod.rs): call `remove_live_tree`
/// before `set_live_lines`. This test exercises the mechanism at unit level by
/// constructing the stale state directly, verifying that the stale tree blocks
/// signature help. The companion test below verifies that clearing it unblocks.
#[test]
fn regression_124_stale_live_tree_blocks_signature_help() {
    let original = "fun greet(name: String, age: Int) {}\nfun main() {}\n";
    let with_call = "fun greet(name: String, age: Int) {}\nfun main() {\n    greet()\n}";

    let (u, idx) = setup_with_stale_live_tree("/Foo.kt", original, with_call);
    let pos = Position {
        line: 2,
        character: 10,
    };
    let result = compute_signature_help(&u, pos, &idx);

    // Stale tree has no call_expression at the cursor → cst_call_info returns None.
    // The fix in did_change (remove_live_tree before set_live_lines) prevents this
    // stale state from ever occurring in production.
    assert!(
        result.is_none(),
        "stale live_tree must block signature help — this is the bug condition"
    );
}

/// Confirm the fix direction: clearing live_trees on did_change lets
/// live_doc_or_parse re-parse from fresh live_lines and find the call.
#[test]
fn regression_124_cleared_live_tree_allows_signature_help() {
    let original = "fun greet(name: String, age: Int) {}\nfun main() {}\n";
    let with_call = "fun greet(name: String, age: Int) {}\nfun main() {\n    greet()\n}";

    let u = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(&u, original);
    idx.store_live_tree(&u, original); // actor stored original tree
    idx.remove_live_tree(&u); // fix: did_change clears stale tree
    idx.set_live_lines(&u, with_call); // fresh live_lines

    let pos = Position {
        line: 2,
        character: 10,
    };
    let result = compute_signature_help(&u, pos, &idx);
    assert!(
        result.is_some(),
        "with stale tree cleared, signature help must succeed"
    );
}
