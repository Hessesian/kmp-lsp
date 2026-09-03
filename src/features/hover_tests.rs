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

/// Regression: hovering a smart-cast-narrowed field access (`event.event`
/// inside `is Event.OverdraftInput -> when (event.event) { ... }`) must show
/// the *specific* nested variant's own field, not a same-named sibling's.
///
/// `CursorContext::build` correctly narrows the root to `Event.OverdraftInput`
/// (confirmed via smart-cast inference), but the member lookup
/// (`resolve_qualified`'s uppercase-qualifier branch, in `resolver/resolve.rs`)
/// only anchored the search on the *outer* type's (`Event`'s) own declaration
/// line, then took whichever same-named field was textually closest —
/// `RegularInput`'s `event` field, declared before `OverdraftInput`'s in the
/// same sealed interface, not the one actually being hovered. `RegularInput`
/// is the decoy that makes this test fail without the fix; a single-variant
/// fixture would pass by accident regardless.
#[test]
fn hover_on_smart_cast_narrowed_field_shows_the_specific_variant_not_a_sibling() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Reducer.kt").unwrap();
    let src = "\
sealed interface Event {
    data class RegularInput(val event: RegularEvent) : Event
    data class OverdraftInput(val event: OverdraftEvent) : Event
}
sealed interface RegularEvent {
    object RegularOnClick : RegularEvent
}
sealed interface OverdraftEvent {
    object OverdraftOnClick : OverdraftEvent
}
class Reducer {
    fun reduce(event: Event) {
        when (event) {
            is Event.OverdraftInput -> when (event.event) {
                is OverdraftEvent.OverdraftOnClick -> println(\"1\")
            }
            else -> {}
        }
    }
}
";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let target_line = src
        .lines()
        .position(|line| line.contains("when (event.event)"))
        .unwrap();
    // The second `event` — the field access, not the smart-cast root.
    let col = src
        .lines()
        .nth(target_line)
        .unwrap()
        .rfind("event")
        .unwrap() as u32;
    let position = Position::new(target_line as u32, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("OverdraftInput") && !text.contains("RegularInput"),
        "must show OverdraftInput's own `event` field, not RegularEvent's \
         sibling field found by proximity to Event's own declaration, got: {text:?}"
    );
}

/// Real corpus case that motivated PR #304 (Moneta's `FormatUtil.java`),
/// reached through hover's own resolution pipeline instead of goto-def's.
/// `resolve_qualified` now hands every same-named overload to its caller
/// (`find_all_names_scoped_to_container`, see PR #304), in `file_data.symbols`
/// order — reverse source order for Java, so the 3-arg (last-declared)
/// overload sorts first. Before this fix, `locate_symbol`
/// (`src/indexer/resolution.rs`) picked `.into_iter().next()` with no
/// shape-awareness at all, so hovering the CALL to the 2-arg overload always
/// showed the 3-arg overload's own signature instead.
#[test]
fn hover_on_qualified_call_shows_the_called_overload_not_an_arbitrary_one() {
    let idx = Indexer::new();
    let java_uri = Url::parse("file:///app/FormatUtil.java").unwrap();
    idx.index_content(
        &java_uri,
        concat!(
            "package app;\n",
            "public class FormatUtil {\n",
            "    public static String formatAmount(java.math.BigDecimal a) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b, boolean c) { return null; }\n",
            "}\n",
        ),
    );

    let uri = Url::parse("file:///app/Caller.kt").unwrap();
    let src = "package app\n\
               class Caller {\n\
                   fun run(amount: java.math.BigDecimal) {\n\
                       FormatUtil.formatAmount(amount, 2)\n\
                   }\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(3).unwrap().find("formatAmount").unwrap() as u32;
    let position = Position::new(3, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("int b") && !text.contains("boolean c"),
        "hovering the 2-arg call must show the 2-arg overload's own \
         signature, not the 3-arg (or 1-arg) overload's, got: {text:?}"
    );
}

/// The sibling gap left open by PR #304: a qualified reference with NO
/// derivable call shape (here, a bare property-style reference — no call
/// parens at all) that still resolves to more than one overload candidate
/// must decline rather than silently showing an arbitrary overload's docs.
#[test]
fn hover_on_ambiguous_qualified_reference_without_call_declines_rather_than_guessing() {
    let idx = Indexer::new();
    let java_uri = Url::parse("file:///app/FormatUtil.java").unwrap();
    idx.index_content(
        &java_uri,
        concat!(
            "package app;\n",
            "public class FormatUtil {\n",
            "    public static String formatAmount(java.math.BigDecimal a) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b, boolean c) { return null; }\n",
            "}\n",
        ),
    );

    let uri = Url::parse("file:///app/Caller.kt").unwrap();
    let src = "package app\n\
               class Caller {\n\
                   val ref = FormatUtil.formatAmount\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(2).unwrap().find("formatAmount").unwrap() as u32;
    let position = Position::new(2, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position);
    assert!(
        hover.is_none(),
        "an ambiguous qualified reference with no derivable call shape must \
         decline rather than showing an arbitrary overload's docs, got: {:?}",
        hover.map(|h| hover_text(&h))
    );
}
