//! Tests for [`find_definition`] — CST-resolved-first, NameScan-fallback.

use tower_lsp::lsp_types::{GotoDefinitionResponse, Position, Url};

use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::Indexer;

/// House decoy: a call-expression receiver (`getUser().save()`). The
/// string/word-based qualifier extraction (`word_and_qualifier_at`) only
/// captures a simple identifier immediately before the dot — it can't
/// capture `getUser()` as a qualifier at all (the char before the dot is
/// `)`, not an identifier char), so today's path treats `save` as a BARE,
/// receiver-less reference and falls through to an unqualified same-name
/// scan that can't distinguish `User.save` from `Admin.save`. The CST path
/// walks the actual `navigation_expression` and resolves the call
/// receiver's return type directly, so it must land on `User.save` only.
#[tokio::test]
async fn goto_definition_resolves_call_expression_receiver_via_cst() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/D.kt").unwrap();
    let src = "class User { fun save() {} }\n\
               class Admin { fun save() {} }\n\
               fun getUser(): User = User()\n\
               fun f() { getUser().save() }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(3).unwrap().find("save").unwrap() as u32;
    let ctx = CursorContext::build(&idx, &uri, Position::new(3, col)).unwrap();
    let response = find_definition(&ctx, &idx, &uri, Position::new(3, col))
        .await
        .unwrap();
    let loc = match response {
        GotoDefinitionResponse::Scalar(l) => l,
        other => panic!("expected a single location, got {other:?}"),
    };
    assert_eq!(
        loc.range.start.line, 0,
        "must jump to User.save, not Admin.save"
    );
}

/// The reported bug: `fun <T> Flow<T>.collect(scope, block) { collect(block) }`
/// — the inner 1-arg `collect(block)` must not goto-definition back to the
/// enclosing 2-required-arg declaration just because it's the only same-named
/// symbol in the file. Exercises the whole path: `call_shape_at_callee`'s CST
/// classification, `find_definition_for_call`, and `resolve_callee_definition`'s
/// arity filter together — not just the resolver internals directly.
#[tokio::test]
async fn goto_definition_does_not_resolve_wrong_arity_call_to_enclosing_self() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    let src = "class CoroutineScope\n\
               fun <T : Any> Flow<T>.collect(scope: CoroutineScope, block: (T) -> Unit) {\n\
                   collect(block)\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(2).unwrap().find("collect").unwrap() as u32;
    let position = Position::new(2, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();
    let response = find_definition(&ctx, &idx, &uri, position).await;
    if let Some(GotoDefinitionResponse::Scalar(loc)) = &response {
        assert_ne!(
            loc.range.start.line, 1,
            "must not jump back to the enclosing declaration itself, got: {response:?}"
        );
    }
    if let Some(GotoDefinitionResponse::Array(locs)) = &response {
        assert!(
            !locs.iter().any(|loc| loc.range.start.line == 1),
            "must not include the enclosing declaration among the results, got: {response:?}"
        );
    }
}

/// Goto-definition runs constantly on mid-edit, ERROR-recovered buffers.
/// `call_shape_at_callee` is the first caller of `call_shape_of` that starts
/// from an arbitrary live cursor position rather than a node already known to
/// enclose complete syntax — verify an unterminated call argument list (the
/// user is still typing) degrades safely rather than miscounting and wrongly
/// excluding the one legitimate candidate.
#[tokio::test]
async fn goto_definition_on_unterminated_call_still_resolves() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Incomplete.kt").unwrap();
    let src = "fun greet(name: String) {}\n\
               fun test() {\n\
                   greet(name\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(2).unwrap().find("greet").unwrap() as u32;
    let position = Position::new(2, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();
    let response = find_definition(&ctx, &idx, &uri, position).await;
    match response {
        Some(GotoDefinitionResponse::Scalar(loc)) => {
            assert_eq!(
                loc.range.start.line, 0,
                "an unterminated call must still resolve its one legitimate \
                 candidate, not be wrongly excluded by a miscounted shape"
            );
        }
        other => panic!("expected a single resolved location, got: {other:?}"),
    }
}
