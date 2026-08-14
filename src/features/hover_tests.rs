use super::*;
use crate::indexer::Indexer;

fn hover_text(hover: &Hover) -> String {
    match &hover.contents {
        HoverContents::Markup(markup) => markup.value.clone(),
        _ => String::new(),
    }
}

/// Same reported bug as goto-definition's regression test, reached through
/// hover's separate `resolve_symbol_info` path instead of
/// `find_definition`: hovering the inner `collect(block)` must not show
/// the enclosing 2-required-arg declaration's own signature.
#[test]
fn hover_does_not_show_wrong_arity_self_reference() {
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

    let hover = compute_hover(&idx, &ctx, &uri, position);
    if let Some(hover) = hover {
        let text = hover_text(&hover);
        assert!(
            !text.contains("scope: CoroutineScope"),
            "hover must not show the enclosing self declaration's own \
             signature, got: {text:?}"
        );
    }
}

/// Genuine same-arity self-recursion must still hover to itself — the
/// arity filter must not become a blanket "never show a same-file match."
#[test]
fn hover_shows_same_arity_self_recursion() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Factorial.kt").unwrap();
    let src = "fun factorial(n: Int): Int {\n\
                   return factorial(n - 1)\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(1).unwrap().find("factorial").unwrap() as u32;
    let position = Position::new(1, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("factorial"),
        "same-arity self-recursion must still hover to itself, got: {text:?}"
    );
}
