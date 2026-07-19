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
