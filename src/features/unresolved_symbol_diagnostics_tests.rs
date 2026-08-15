use super::*;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{Position, Url};

/// Find the one outcome for `name` at 0-indexed `line` — fixtures below
/// sometimes have the same name appear more than once on different lines
/// (a declaration site, a self-shadowed bare call, an explicit-receiver
/// call), so matching by name alone would be ambiguous.
fn outcome_at<'a>(outcomes: &'a [ReferenceOutcome], name: &str, line: u32) -> &'a ReferenceOutcome {
    outcomes
        .iter()
        .find(|o| o.name == name && o.line == line)
        .unwrap_or_else(|| {
            panic!("no reference outcome named `{name}` at line {line}, got: {outcomes:?}")
        })
}

#[test]
fn member_call_on_jar_receiver_resolves_cst_resolved() {
    use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;

    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    // Deliberately no same-file `collect` declaration here — that collision
    // scenario is `same_file_shape_mismatched_self_declaration_is_filtered_candidate_not_gap`'s
    // own job below. `resolve_qualified`'s uppercase-root branch tries
    // `resolve_extension_in_scope` before the member/jar lookup and returns
    // immediately on any name match there, with no arity check — so a
    // same-file same-named extension would short-circuit before this jar
    // member is ever reached, regardless of this test's call shape. This
    // fixture isolates the claim this test actually makes: an unambiguous
    // JAR-member reference resolves via `resolve_identity` alone.
    let src = "package com.example\n\
               import kotlinx.coroutines.flow.Flow\n\
               fun useTriggers(triggers: Flow<String>) {\n\
                   triggers.collect { trigger -> println(trigger) }\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let jar_uri_str = "jar:file:///fake-coroutines.jar!/Flow.kt".to_string();
    let jar_uri = Url::parse(&jar_uri_str).unwrap();
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
            uri: jar_uri.clone(),
            range: type_range,
        });

    let doc = crate::indexer::live_tree::parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let outcomes = collect_resolution_outcomes(&idx, &uri, &doc);
    let outcome = outcome_at(&outcomes, "collect", 3);
    match &outcome.outcome {
        ResolutionOutcome::Success { tier, locations } => {
            assert_eq!(*tier, SuccessTier::CstResolved);
            assert_eq!(locations.len(), 1);
            assert_eq!(locations[0].uri, jar_uri);
        }
        other => panic!("expected Success/CstResolved, got: {other:?}"),
    }
}

#[test]
fn same_file_shape_mismatched_self_declaration_is_filtered_candidate_not_gap() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    let src = "package com.example\n\
               class Flow<T>\n\
               class CoroutineScope\n\
               fun <T> Flow<T>.collect(scope: CoroutineScope, block: (T) -> Unit) {\n\
               }\n\
               fun useTriggers(triggers: Flow<String>, scope: CoroutineScope) {\n\
                   triggers.collect { trigger -> println(trigger) }\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let doc = crate::indexer::live_tree::parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let outcomes = collect_resolution_outcomes(&idx, &uri, &doc);
    let outcome = outcome_at(&outcomes, "collect", 6);
    assert!(
        matches!(outcome.outcome, ResolutionOutcome::FilteredCandidate),
        "expected FilteredCandidate (a same-file wrong-arity self-declaration exists), got: {:?}",
        outcome.outcome
    );
}

#[test]
fn undeclared_member_reference_is_gap() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Foo.kt").unwrap();
    let src = "package com.example\n\
               class Foo\n\
               fun useFoo(foo: Foo) {\n\
                   foo.totallyUndeclaredMethod()\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let doc = crate::indexer::live_tree::parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let outcomes = collect_resolution_outcomes(&idx, &uri, &doc);
    let outcome = outcome_at(&outcomes, "totallyUndeclaredMethod", 3);
    assert!(
        matches!(outcome.outcome, ResolutionOutcome::Gap),
        "expected Gap, got: {:?}",
        outcome.outcome
    );
}

#[test]
fn bare_call_to_top_level_function_resolves_name_scan_success() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Helper.kt").unwrap();
    let src = "package com.example\n\
               fun helper(): Int = 5\n\
               fun useHelper() {\n\
                   val result = helper()\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let doc = crate::indexer::live_tree::parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let outcomes = collect_resolution_outcomes(&idx, &uri, &doc);
    let outcome = outcome_at(&outcomes, "helper", 3);
    match &outcome.outcome {
        ResolutionOutcome::Success { tier, locations } => {
            assert_eq!(*tier, SuccessTier::NameScan);
            assert_eq!(locations.len(), 1);
        }
        other => panic!("expected Success/NameScan, got: {other:?}"),
    }
}

#[test]
fn collect_resolution_outcomes_survives_a_pathologically_deep_expression() {
    let n = 5_000; // several times MAX_CST_DESCENT_DEPTH (512)
    let mut src = String::from("package app\nfun f() {\n    val x = 1");
    for _ in 0..n {
        src.push_str("+1");
    }
    src.push_str("\n}\n");
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Deep.kt").unwrap();
    idx.index_content(&uri, &src);
    idx.store_live_tree(&uri, &src);
    let doc = crate::indexer::live_tree::parse_live(&src, tree_sitter_kotlin::language()).unwrap();
    let _ = collect_resolution_outcomes(&idx, &uri, &doc);
}
