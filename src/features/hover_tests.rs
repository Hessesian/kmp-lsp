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

/// The follow-up reported bug, once the self-shadow above was suppressed:
/// hover showed nothing at all, because nothing ever tried "this bare call
/// is really `this.collect(...)` against the enclosing extension function's
/// own receiver" — the real target is `Flow`'s own JAR-indexed interface
/// member (reached only via implicit-receiver resolution, mirroring the
/// matching `goto_definition_resolves_implicit_receiver_call_to_jar_member`
/// end-to-end test).
#[test]
fn hover_resolves_implicit_receiver_call_to_jar_member() {
    use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;

    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    let src = "package com.example\n\
               import kotlinx.coroutines.flow.Flow\n\
               class CoroutineScope\n\
               fun <T : Any> Flow<T>.collect(scope: CoroutineScope, block: (T) -> Unit) {\n\
                   collect(block)\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let jar_uri_str = "jar:file:///fake-coroutines.jar!/Flow.kt".to_string();
    let type_range = tower_lsp::lsp_types::Range {
        start: Position::new(0, 0),
        end: Position::new(0, 4),
    };
    let member_range = tower_lsp::lsp_types::Range {
        start: Position::new(1, 0),
        end: Position::new(1, 7),
    };
    let member = SymbolEntry {
        name: "collect".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::METHOD,
        visibility: Visibility::Public,
        range: member_range,
        selection_range: member_range,
        detail: "suspend fun collect(collector: FlowCollector<T>)".to_owned(),
        container: Some("Flow".to_owned()),
        params: "collector: FlowCollector<T>".to_owned(),
        param_counts: (1, 1),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    let flow_type = SymbolEntry {
        name: "Flow".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::INTERFACE,
        visibility: Visibility::Public,
        range: type_range,
        selection_range: type_range,
        detail: "interface Flow<T>".to_owned(),
        container: None,
        params: String::new(),
        param_counts: (0, 0),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    idx.jar_files.insert(
        jar_uri_str.clone(),
        Arc::new(FileData {
            symbols: vec![flow_type, member],
            source_set: SourceSet::Library,
            package: Some("kotlinx.coroutines.flow".to_owned()),
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );
    idx.jar_definitions
        .entry("Flow".to_owned())
        .or_default()
        .push(tower_lsp::lsp_types::Location {
            uri: Url::parse(&jar_uri_str).unwrap(),
            range: type_range,
        });

    let col = src.lines().nth(4).unwrap().find("collect").unwrap() as u32;
    let position = Position::new(4, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("FlowCollector"),
        "hover must show Flow's own collect member signature, got: {text:?}"
    );
    assert!(
        !text.contains("scope: CoroutineScope"),
        "hover must not show the enclosing self declaration's own \
         signature, got: {text:?}"
    );
}

/// A second, distinct manifestation of the same self-shadow bug, reported
/// after the implicit-receiver fix above shipped: an *explicit*-receiver call
/// (`triggers.collect { trigger -> ... }`, trailing-lambda-only) reaches
/// `contextual_receiver_hover` via `ctx.contextual` — populated for *any*
/// qualified reference via smart-cast narrowing, not just `it`/`this`/named
/// lambda params — which had no arity awareness at all (mirrors
/// `goto_definition_resolves_explicit_receiver_call_to_jar_member_not_self`).
#[test]
fn hover_resolves_explicit_receiver_call_to_jar_member_not_self() {
    use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;

    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    let src = "package com.example\n\
               import kotlinx.coroutines.flow.Flow\n\
               class CoroutineScope\n\
               fun <T : Any> Flow<T>.collect(scope: CoroutineScope, block: (T) -> Unit) {\n\
                   collect(block)\n\
               }\n\
               fun useTriggers(triggers: Flow<String>) {\n\
                   triggers.collect { trigger -> println(trigger) }\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let jar_uri_str = "jar:file:///fake-coroutines.jar!/Flow.kt".to_string();
    let type_range = tower_lsp::lsp_types::Range {
        start: Position::new(0, 0),
        end: Position::new(0, 4),
    };
    let member_range = tower_lsp::lsp_types::Range {
        start: Position::new(1, 0),
        end: Position::new(1, 7),
    };
    let member = SymbolEntry {
        name: "collect".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::METHOD,
        visibility: Visibility::Public,
        range: member_range,
        selection_range: member_range,
        detail: "suspend fun collect(collector: FlowCollector<T>)".to_owned(),
        container: Some("Flow".to_owned()),
        params: "collector: FlowCollector<T>".to_owned(),
        param_counts: (1, 1),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    let flow_type = SymbolEntry {
        name: "Flow".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::INTERFACE,
        visibility: Visibility::Public,
        range: type_range,
        selection_range: type_range,
        detail: "interface Flow<T>".to_owned(),
        container: None,
        params: String::new(),
        param_counts: (0, 0),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    idx.jar_files.insert(
        jar_uri_str.clone(),
        Arc::new(FileData {
            symbols: vec![flow_type, member],
            source_set: SourceSet::Library,
            package: Some("kotlinx.coroutines.flow".to_owned()),
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );
    idx.jar_definitions
        .entry("Flow".to_owned())
        .or_default()
        .push(tower_lsp::lsp_types::Location {
            uri: Url::parse(&jar_uri_str).unwrap(),
            range: type_range,
        });

    let col = src.lines().nth(7).unwrap().find("collect").unwrap() as u32;
    let position = Position::new(7, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("FlowCollector"),
        "hover must show Flow's own collect member signature, got: {text:?}"
    );
    assert!(
        !text.contains("scope: CoroutineScope"),
        "hover must not show the collect(scope, block) self-declaration's \
         own signature, got: {text:?}"
    );
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
