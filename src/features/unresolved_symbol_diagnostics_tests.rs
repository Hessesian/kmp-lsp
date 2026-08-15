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

fn test_location(line: u32) -> tower_lsp::lsp_types::Location {
    tower_lsp::lsp_types::Location {
        uri: Url::parse("file:///t/Target.kt").unwrap(),
        range: tower_lsp::lsp_types::Range {
            start: Position::new(line, 0),
            end: Position::new(line, 5),
        },
    }
}

#[test]
fn recall_summary_keeps_member_lane_separate_from_bare_lane() {
    let mut agg = ResolutionAccuracyAggregator::default();
    agg.add(
        "A.kt",
        &ReferenceOutcome {
            name: "foo".to_owned(),
            receiver_type: Some("Bar".to_owned()),
            line: 0,
            col: 0,
            outcome: ResolutionOutcome::Success {
                tier: SuccessTier::CstResolved,
                locations: vec![test_location(0)],
            },
        },
    );
    agg.add(
        "A.kt",
        &ReferenceOutcome {
            name: "missing".to_owned(),
            receiver_type: Some("Bar".to_owned()),
            line: 1,
            col: 0,
            outcome: ResolutionOutcome::Gap,
        },
    );
    agg.add(
        "A.kt",
        &ReferenceOutcome {
            name: "helper".to_owned(),
            receiver_type: None,
            line: 2,
            col: 0,
            outcome: ResolutionOutcome::Success {
                tier: SuccessTier::NameScan,
                locations: vec![test_location(0)],
            },
        },
    );

    let recall = agg.recall();
    assert_eq!(recall.member_total, 2);
    assert_eq!(recall.member_cst_resolved, 1);
    assert!((recall.member_recall_pct() - 50.0).abs() < 0.01);
    assert_eq!(recall.bare_total, 1);
    assert_eq!(recall.bare_success, 1);
    assert_eq!(recall.bare_recall_pct(), 100.0);

    // The `missing` outcome above has `receiver_type: Some("Bar")`, i.e. a
    // member-ref Gap — it must land in `top_member_gaps`, not `top_bare_gaps`.
    let member_gaps = agg.top_member_gaps(10);
    assert_eq!(member_gaps.len(), 1);
    assert_eq!(member_gaps[0].0, "missing");
    assert_eq!(member_gaps[0].1.count, 1);
    assert_eq!(member_gaps[0].1.sample_location, "A.kt:2:1");
    assert_eq!(recall.member_gap_total, 1);
    assert_eq!(recall.bare_gap_total, 0);

    assert!(agg.top_bare_gaps(10).is_empty());
}

#[test]
fn cache_candidate_grouping_separates_stable_from_unstable_keys() {
    let mut agg = ResolutionAccuracyAggregator::default();
    let stable_loc = test_location(10);
    for line in 0..5u32 {
        agg.add(
            "A.kt",
            &ReferenceOutcome {
                name: "widelyUsed".to_owned(),
                receiver_type: Some("Bar".to_owned()),
                line,
                col: 0,
                outcome: ResolutionOutcome::Success {
                    tier: SuccessTier::CstResolved,
                    locations: vec![stable_loc.clone()],
                },
            },
        );
    }
    for (line, loc_line) in [(0u32, 1u32), (1, 2)] {
        agg.add(
            "B.kt",
            &ReferenceOutcome {
                name: "shadowed".to_owned(),
                receiver_type: None,
                line,
                col: 0,
                outcome: ResolutionOutcome::Success {
                    tier: SuccessTier::NameScan,
                    locations: vec![test_location(loc_line)],
                },
            },
        );
    }

    let candidates = agg.cache_candidates(10);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].name, "widelyUsed");
    assert_eq!(candidates[0].count, 5);

    let unstable = agg.unstable_hot_keys(10);
    assert_eq!(unstable.len(), 1);
    assert_eq!(unstable[0].name, "shadowed");
    assert_eq!(unstable[0].count, 2);
    assert_eq!(unstable[0].distinct_locations, 2);
}

/// `CacheCandidate.location` must be the resolved target's location, not the
/// reference site where the symbol was first seen — `sample_location`-style
/// formatting (`"{file}:{line}:{col}"` of the *reference*) would silently
/// pass the old assertion shape, so this pins the actual resolved-location
/// string instead. Fixture picks a reference site (`A.kt:1:1`, from
/// `outcome.line`/`col` both `0`) that's clearly distinct from the resolved
/// target (`file:///t/Target.kt`, line 11) so the test fails pre-fix
/// (would've asserted on `"A.kt:1:1"`) and passes post-fix.
#[test]
fn cache_candidate_location_is_the_resolved_target_not_the_reference_site() {
    let mut agg = ResolutionAccuracyAggregator::default();
    let target = test_location(10); // resolved target: Target.kt, line 11 (0-indexed 10)
    agg.add(
        "A.kt",
        &ReferenceOutcome {
            name: "widelyUsed".to_owned(),
            receiver_type: Some("Bar".to_owned()),
            line: 0,
            col: 0,
            outcome: ResolutionOutcome::Success {
                tier: SuccessTier::CstResolved,
                locations: vec![target.clone()],
            },
        },
    );

    let candidates = agg.cache_candidates(10);
    assert_eq!(candidates.len(), 1);
    let location = &candidates[0].location;
    assert!(
        location.contains("Target.kt") && location.contains("10"),
        "expected the resolved target's location (Target.kt, line 10), got: {location}"
    );
    assert!(
        !location.contains("A.kt"),
        "location must not be the reference site (A.kt), got: {location}"
    );
}

#[test]
fn filtered_candidate_outcome_appears_in_top_filtered_candidates() {
    let mut agg = ResolutionAccuracyAggregator::default();
    agg.add(
        "A.kt",
        &ReferenceOutcome {
            name: "ambiguousMember".to_owned(),
            receiver_type: Some("Bar".to_owned()),
            line: 4,
            col: 2,
            outcome: ResolutionOutcome::FilteredCandidate,
        },
    );

    let recall = agg.recall();
    assert_eq!(recall.member_total, 1);
    assert_eq!(recall.filtered_candidate_total, 1);

    let filtered = agg.top_filtered_candidates(10);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].0, "ambiguousMember");
    assert_eq!(filtered[0].1.count, 1);
    assert_eq!(filtered[0].1.sample_location, "A.kt:5:3");
}

#[test]
fn package_header_segments_are_not_flagged_as_gaps() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Pkg.kt").unwrap();
    // `com`/`example` are package-header path segments, not references —
    // walking into `package_header` would classify them as bare references
    // that never resolve, producing spurious Gaps for every file.
    let src = "package com.example\n\
               import kotlin.collections.List\n\
               fun useIt(list: List<Int>) {\n\
                   println(list)\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let doc = crate::indexer::live_tree::parse_live(src, tree_sitter_kotlin::language()).unwrap();
    let outcomes = collect_resolution_outcomes(&idx, &uri, &doc);
    assert!(
        !outcomes
            .iter()
            .any(|o| o.name == "com" || o.name == "example"),
        "package-header segments must not appear as classified outcomes at all, got: {outcomes:?}"
    );
}
