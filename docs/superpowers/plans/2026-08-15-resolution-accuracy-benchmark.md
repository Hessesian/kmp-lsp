# Resolution-Accuracy Benchmark Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a CLI benchmark (`kmp-lsp resolution-accuracy <root>`) that measures what fraction of references in a Kotlin/Java workspace the resolver can actually find (recall), and surfaces which symbols get resolved identically over and over (cache-candidate signal), per `docs/superpowers/specs/2026-08-15-resolution-accuracy-benchmark-design.md`.

**Architecture:** A shared, pure classification function (`collect_resolution_outcomes`, in a new `src/features/unresolved_symbol_diagnostics.rs`, positioned like its `missing_import_diagnostics`/`unused_import_diagnostics` siblings for later live-diagnostic reuse) walks every identifier in one file's CST, classifies each via the existing `classify_cursor`/`resolve_identity` pipeline, and buckets the result into `Success` / `FilteredCandidate` / `Gap`. A pure `ResolutionAccuracyAggregator` accumulates these across a whole workspace scan into a recall summary and a cache-candidate report. A new CLI subcommand (`src/cli/resolution_accuracy_poc.rs`, mirroring `missing_import_poc.rs`) owns all I/O: building the index, warming the JAR index, walking workspace files, and printing the report.

**Tech Stack:** Rust, tree-sitter (Kotlin/Java grammars), `tower_lsp::lsp_types`, existing `Indexer`/`classify_cursor`/`resolve_identity` infrastructure — no new dependencies.

## Global Constraints

- CLI-only this pass — no live-diagnostic wiring (per the design's "Risks / deferred" section and the user's explicit sequencing: CLI first, LSP later).
- Classification must go through `classify_cursor` + `resolve_identity` directly, never `find_definition`'s full pipeline — its `rg`-grep fallback would mask real resolver regressions (see design's "Classification algorithm" section).
- Any new hand-rolled recursive CST descent must be bounded via `crate::util::MAX_CST_DESCENT_DEPTH` + `crate::util::report_cst_depth_exceeded!`, matching the codebase-wide convention from the PR #257–#259 stack-overflow hardening pass.
- Follow the exact sibling-file structure already established by `missing_import_diagnostics.rs` / `cli/missing_import_poc.rs` (shared detection function in `features/`, I/O-only CLI wrapper in `cli/`).
- `tree-sitter` node `start_position().column` is a byte offset, not a UTF-16 code unit offset (LSP's `Position.character`). Use it directly as an approximation — the same simplification `missing_import_diagnostics.rs`'s own candidate collection already relies on (see `src/features/missing_import_diagnostics.rs:286-289`). Do not attempt a "more correct" UTF-16 conversion; that would be unbuilt-upon scope creep on a benchmark tool.

---

### Task 1: Core classification — identifier walk + `collect_resolution_outcomes`

**Files:**
- Create: `src/features/unresolved_symbol_diagnostics.rs`
- Create: `src/features/unresolved_symbol_diagnostics_tests.rs`
- Modify: `src/features/mod.rs` (register the new module)

**Interfaces:**
- Consumes: `crate::indexer::{classify_cursor, resolve_identity, Indexer, NavigationSource, SymbolAtCursor, SymbolRole}` (all already `pub(crate)`, re-exported at `src/indexer.rs:36-45`); `crate::indexer::live_tree::LiveDoc`; `crate::queries::{KIND_SIMPLE_IDENT, KIND_TYPE_IDENT}`; `crate::util::{MAX_CST_DESCENT_DEPTH, report_cst_depth_exceeded}`.
- Produces (used by Task 2 and Task 3):
  - `pub(crate) enum SuccessTier { CstResolved, NameScan }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub(crate) enum ResolutionOutcome { Success { tier: SuccessTier, locations: Vec<Location> }, FilteredCandidate, Gap }` (derives `Debug, Clone`)
  - `pub(crate) struct ReferenceOutcome { pub name: String, pub receiver_type: Option<String>, pub line: u32, pub col: u32, pub outcome: ResolutionOutcome }` (derives `Debug, Clone`)
  - `pub(crate) fn collect_resolution_outcomes(indexer: &Indexer, uri: &Url, doc: &LiveDoc) -> Vec<ReferenceOutcome>`

- [ ] **Step 1: Write the failing tests**

The design doc's Testing section calls for "a bare local-variable reference → `Success/NameScan`" as the fourth case. This plan uses a bare *call to a top-level function* instead
(`bare_call_to_top_level_function_resolves_name_scan_success` below) — `resolve_identity`'s
`Reference { receiver_type: None, .. }` arm handles every bare reference identically regardless of
whether the referent is a local `val` or a top-level declaration (same `find_definition_qualified(name, None, uri)` call either way), so this exercises the identical code path without depending on unverified assumptions about whether local variable declarations are indexed the same way top-level ones are.

Create `src/features/unresolved_symbol_diagnostics_tests.rs`:

```rust
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
        .unwrap_or_else(|| panic!("no reference outcome named `{name}` at line {line}, got: {outcomes:?}"))
}

#[test]
fn member_call_on_jar_receiver_resolves_cst_resolved() {
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
    let outcome = outcome_at(&outcomes, "collect", 7);
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
```

Add the test wiring at the bottom of `src/features/unresolved_symbol_diagnostics.rs` (create the file with just this for now):

```rust
#[cfg(test)]
#[path = "unresolved_symbol_diagnostics_tests.rs"]
mod tests;
```

Register the module in `src/features/mod.rs` — insert alphabetically between `traits_impl` and `unused_import_diagnostics`:

```rust
pub(crate) mod unresolved_symbol_diagnostics;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --bin kmp-lsp unresolved_symbol_diagnostics -- --nocapture`
Expected: compile FAILURE — `collect_resolution_outcomes`, `ReferenceOutcome`, `ResolutionOutcome`, `SuccessTier` don't exist yet.

- [ ] **Step 3: Implement the classification logic**

Replace `src/features/unresolved_symbol_diagnostics.rs`'s content (keep the `mod tests` block at the bottom) with:

```rust
//! Resolution-accuracy benchmark: classify and resolve every `Reference`
//! identifier in a file, bucketing the result into `Success` /
//! `FilteredCandidate` / `Gap`.
//!
//! Shared with the `resolution-accuracy` CLI subcommand
//! (`cli::resolution_accuracy_poc`), which runs this over an entire
//! workspace to measure recall — see that module for the aggregate
//! methodology. Not wired into a live diagnostic yet; named and positioned
//! like its `missing_import_diagnostics`/`unused_import_diagnostics`
//! siblings so that reuse doesn't need restructuring later.

use tower_lsp::lsp_types::{Location, Position, Url};
use tree_sitter::Node;

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::{
    classify_cursor, resolve_identity, Indexer, NavigationSource, SymbolAtCursor, SymbolRole,
};
use crate::queries::{KIND_SIMPLE_IDENT, KIND_TYPE_IDENT};

/// Which resolution path produced a `Success` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuccessTier {
    /// Precise, receiver-verified — `resolve_identity`'s `CstResolved` path.
    CstResolved,
    /// Found by name only. Reachable here exclusively for bare references:
    /// `resolve_identity`'s `Reference { receiver_type: Some(_), .. }` arm
    /// only ever returns `CstResolved` for a non-empty result (see
    /// `classify_reference` below), so a member reference never lands here.
    NameScan,
}

/// The outcome of attempting to resolve one `Reference`-classified identifier.
#[derive(Debug, Clone)]
pub(crate) enum ResolutionOutcome {
    Success {
        tier: SuccessTier,
        locations: Vec<Location>,
    },
    /// The receiver-typed lookup came back empty, but an untyped lookup by
    /// the same name found *something* elsewhere in the workspace. Ambiguous
    /// by design: could be a correct self-shadow-style suppression (the
    /// arity-filtering this session's fixes rely on), or a genuine resolver
    /// miss — reported separately, not scored as a plain failure.
    FilteredCandidate,
    /// No candidate found anywhere by this name — the actionable bucket.
    Gap,
}

/// One classified-and-resolved reference.
#[derive(Debug, Clone)]
pub(crate) struct ReferenceOutcome {
    pub name: String,
    /// `Some` for a member reference (`x.foo()`); `None` for a bare
    /// reference (local var, unqualified call). Matches
    /// `SymbolRole::Reference::receiver_type`.
    pub receiver_type: Option<String>,
    pub line: u32,
    pub col: u32,
    pub outcome: ResolutionOutcome,
}

/// Find every `simple_identifier`/`type_identifier` node's start position in
/// `root`, via depth-bounded recursion (see `crate::util::MAX_CST_DESCENT_DEPTH`)
/// — a pathological input can nest far deeper than any real Kotlin/Java
/// syntax, and an unbounded walk would overflow the stack rather than degrade.
fn collect_identifier_positions(node: Node, out: &mut Vec<Position>, depth: usize) {
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("collect_identifier_positions", node);
        return;
    }
    if matches!(node.kind(), KIND_SIMPLE_IDENT | KIND_TYPE_IDENT) {
        let point = node.start_position();
        // `column` is a byte offset (tree-sitter), used directly as an
        // approximation of the UTF-16 offset `classify_cursor` expects —
        // same simplification `missing_import_diagnostics.rs`'s own
        // candidate collection already relies on.
        out.push(Position::new(point.row as u32, point.column as u32));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifier_positions(child, out, depth + 1);
    }
}

/// Classify and resolve every `Reference`-role identifier in `doc`.
///
/// `Declaration` and `ImportSegment` roles are skipped — declarations
/// resolve by construction, imports are the `missing-imports`/
/// `unused-imports` benchmarks' own territory.
///
/// The caller is responsible for `uri` having a live tree stored
/// (`indexer.store_live_tree`) before calling this — `classify_cursor`
/// resolves through `indexer.live_doc_or_parse(uri)` internally, the same
/// requirement `collect_missing_import_flags` documents.
pub(crate) fn collect_resolution_outcomes(
    indexer: &Indexer,
    uri: &Url,
    doc: &LiveDoc,
) -> Vec<ReferenceOutcome> {
    let mut positions = Vec::new();
    collect_identifier_positions(doc.tree.root_node(), &mut positions, 0);

    let mut outcomes = Vec::new();
    for position in positions {
        let Some(symbol) = classify_cursor(indexer, uri, position) else {
            continue;
        };
        let receiver_type = match &symbol.role {
            SymbolRole::Reference { receiver_type, .. } => receiver_type.clone(),
            SymbolRole::Declaration { .. } | SymbolRole::ImportSegment => continue,
        };
        let name = symbol.name.clone();
        let outcome = classify_reference(indexer, uri, &symbol, receiver_type.clone());
        outcomes.push(ReferenceOutcome {
            name,
            receiver_type,
            line: position.line,
            col: position.character,
            outcome,
        });
    }
    outcomes
}

/// Resolve one `Reference`-role symbol to a `ResolutionOutcome`.
fn classify_reference(
    indexer: &Indexer,
    uri: &Url,
    symbol: &SymbolAtCursor,
    receiver_type: Option<String>,
) -> ResolutionOutcome {
    let success = match resolve_identity(symbol, indexer, uri) {
        NavigationSource::CstResolved(defs) if !defs.is_empty() => {
            Some((SuccessTier::CstResolved, defs.0))
        }
        NavigationSource::NameScan(defs) if !defs.is_empty() => {
            Some((SuccessTier::NameScan, defs.0))
        }
        _ => None,
    };
    match success {
        Some((tier, locations)) => ResolutionOutcome::Success { tier, locations },
        None if receiver_type.is_some() => {
            let probe = indexer.find_definition_qualified(&symbol.name, None, uri);
            if probe.is_empty() {
                ResolutionOutcome::Gap
            } else {
                ResolutionOutcome::FilteredCandidate
            }
        }
        None => ResolutionOutcome::Gap,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --bin kmp-lsp unresolved_symbol_diagnostics -- --nocapture`
Expected: PASS (5 tests: `member_call_on_jar_receiver_resolves_cst_resolved`,
`same_file_shape_mismatched_self_declaration_is_filtered_candidate_not_gap`,
`undeclared_member_reference_is_gap`,
`bare_call_to_top_level_function_resolves_name_scan_success`,
`collect_resolution_outcomes_survives_a_pathologically_deep_expression`).

Also run the full suite to confirm nothing else broke: `cargo test --bin kmp-lsp` — expected all pass (1743 pre-existing + 5 new).

- [ ] **Step 5: Commit**

```bash
git add src/features/unresolved_symbol_diagnostics.rs src/features/unresolved_symbol_diagnostics_tests.rs src/features/mod.rs
git commit -m "feat(resolution-accuracy): classify+resolve every reference into Success/FilteredCandidate/Gap"
```

---

### Task 2: Aggregator — recall summary + cache-candidate grouping

**Files:**
- Modify: `src/features/unresolved_symbol_diagnostics.rs` (append)
- Modify: `src/features/unresolved_symbol_diagnostics_tests.rs` (append)

**Interfaces:**
- Consumes: `ReferenceOutcome`, `ResolutionOutcome`, `SuccessTier` from Task 1 (same file).
- Produces (used by Task 3):
  - `pub(crate) struct RecallSummary { pub member_total: usize, pub member_cst_resolved: usize, pub bare_total: usize, pub bare_success: usize }` (derives `Debug, Clone, Copy, Default`) with `member_recall_pct(&self) -> f64`, `bare_recall_pct(&self) -> f64`
  - `pub(crate) struct NamedSample { pub count: usize, pub sample_location: String }` (derives `Debug, Clone`)
  - `pub(crate) struct CacheCandidate { pub name: String, pub receiver_type: Option<String>, pub count: usize, pub location: String }` (derives `Debug, Clone`)
  - `pub(crate) struct UnstableHotKey { pub name: String, pub receiver_type: Option<String>, pub count: usize, pub distinct_locations: usize }` (derives `Debug, Clone`)
  - `pub(crate) struct ResolutionAccuracyAggregator` (derives `Debug, Default`) with:
    - `pub(crate) fn add(&mut self, file_label: &str, outcome: &ReferenceOutcome)`
    - `pub(crate) fn recall(&self) -> RecallSummary`
    - `pub(crate) fn top_filtered_candidates(&self, n: usize) -> Vec<(String, NamedSample)>`
    - `pub(crate) fn top_gaps(&self, n: usize) -> Vec<(String, NamedSample)>`
    - `pub(crate) fn cache_candidates(&self, n: usize) -> Vec<CacheCandidate>`
    - `pub(crate) fn unstable_hot_keys(&self, n: usize) -> Vec<UnstableHotKey>`

- [ ] **Step 1: Write the failing tests**

Append to `src/features/unresolved_symbol_diagnostics_tests.rs`:

```rust
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
fn recall_summary_counts_member_and_bare_lanes_separately() {
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

    let gaps = agg.top_gaps(10);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].0, "missing");
    assert_eq!(gaps[0].1.count, 1);
    assert_eq!(gaps[0].1.sample_location, "A.kt:2");
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --bin kmp-lsp unresolved_symbol_diagnostics -- --nocapture`
Expected: compile FAILURE — `ResolutionAccuracyAggregator` doesn't exist yet.

- [ ] **Step 3: Implement the aggregator**

Append to `src/features/unresolved_symbol_diagnostics.rs` (before the `#[cfg(test)]` block at the bottom), and add `use std::collections::{BTreeMap, HashMap, HashSet};` to the top-of-file `use` block:

```rust
/// One name's occurrence count plus a representative location, for the
/// `FilteredCandidate`/`Gap` top-N reports.
#[derive(Debug, Clone)]
pub(crate) struct NamedSample {
    pub count: usize,
    pub sample_location: String,
}

/// Aggregate recall counts across a workspace scan.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RecallSummary {
    pub member_total: usize,
    pub member_cst_resolved: usize,
    pub bare_total: usize,
    pub bare_success: usize,
}

impl RecallSummary {
    /// `CstResolved / total` for member refs, as a percentage. `0.0` when
    /// there were no member refs to score (an empty corpus, not a failure).
    pub(crate) fn member_recall_pct(&self) -> f64 {
        percent(self.member_cst_resolved, self.member_total)
    }

    /// `Success / total` for bare refs, as a percentage.
    pub(crate) fn bare_recall_pct(&self) -> f64 {
        percent(self.bare_success, self.bare_total)
    }
}

fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64 / total as f64) * 100.0
    }
}

/// A `(name, receiver_type)` symbol resolved repeatedly to the exact same
/// location — a candidate for a resolver-level memoization cache.
#[derive(Debug, Clone)]
pub(crate) struct CacheCandidate {
    pub name: String,
    pub receiver_type: Option<String>,
    pub count: usize,
    pub location: String,
}

/// A `(name, receiver_type)` symbol resolved often but to *different*
/// locations across occurrences — not cacheable by this simple a key.
#[derive(Debug, Clone)]
pub(crate) struct UnstableHotKey {
    pub name: String,
    pub receiver_type: Option<String>,
    pub count: usize,
    pub distinct_locations: usize,
}

#[derive(Debug, Default)]
struct CacheGroupState {
    count: usize,
    locations: HashSet<String>,
    sample_location: String,
}

/// `Location`'s own `uri`/`range` fields, flattened to a string key —
/// `Location` implements `Eq`/`PartialEq` but not `Hash`, so it can't sit in
/// a `HashSet` directly.
fn location_key(location: &Location) -> String {
    format!(
        "{}#{}:{}-{}:{}",
        location.uri,
        location.range.start.line,
        location.range.start.character,
        location.range.end.line,
        location.range.end.character
    )
}

/// Accumulates `ReferenceOutcome`s across a workspace scan, one file at a
/// time, into a recall summary and a repeat-resolution/cache-candidate
/// report — avoids holding every reference from a large corpus in memory
/// simultaneously.
#[derive(Debug, Default)]
pub(crate) struct ResolutionAccuracyAggregator {
    recall: RecallSummary,
    filtered_candidate: BTreeMap<String, NamedSample>,
    gap: BTreeMap<String, NamedSample>,
    cache: HashMap<(String, Option<String>), CacheGroupState>,
}

impl ResolutionAccuracyAggregator {
    /// Record one reference's outcome. `file_label` is a display-friendly
    /// path (workspace-relative, e.g. `"src/Foo.kt"`) used only for sample
    /// locations in the reports below.
    pub(crate) fn add(&mut self, file_label: &str, outcome: &ReferenceOutcome) {
        let sample_location = format!("{file_label}:{}", outcome.line + 1);
        let is_member = outcome.receiver_type.is_some();
        match &outcome.outcome {
            ResolutionOutcome::Success { tier, locations } => {
                if is_member {
                    self.recall.member_total += 1;
                    if *tier == SuccessTier::CstResolved {
                        self.recall.member_cst_resolved += 1;
                    }
                } else {
                    self.recall.bare_total += 1;
                    self.recall.bare_success += 1;
                }
                let key = (outcome.name.clone(), outcome.receiver_type.clone());
                let group = self.cache.entry(key).or_insert_with(|| CacheGroupState {
                    count: 0,
                    locations: HashSet::new(),
                    sample_location: sample_location.clone(),
                });
                group.count += 1;
                for location in locations {
                    group.locations.insert(location_key(location));
                }
            }
            ResolutionOutcome::FilteredCandidate => {
                self.recall.member_total += 1;
                let entry = self
                    .filtered_candidate
                    .entry(outcome.name.clone())
                    .or_insert_with(|| NamedSample {
                        count: 0,
                        sample_location: sample_location.clone(),
                    });
                entry.count += 1;
            }
            ResolutionOutcome::Gap => {
                if is_member {
                    self.recall.member_total += 1;
                } else {
                    self.recall.bare_total += 1;
                }
                let entry = self.gap.entry(outcome.name.clone()).or_insert_with(|| NamedSample {
                    count: 0,
                    sample_location: sample_location.clone(),
                });
                entry.count += 1;
            }
        }
    }

    pub(crate) fn recall(&self) -> RecallSummary {
        self.recall
    }

    /// Top `n` `FilteredCandidate` names by occurrence count, ties broken by name.
    pub(crate) fn top_filtered_candidates(&self, n: usize) -> Vec<(String, NamedSample)> {
        top_n(&self.filtered_candidate, n)
    }

    /// Top `n` `Gap` names by occurrence count, ties broken by name.
    pub(crate) fn top_gaps(&self, n: usize) -> Vec<(String, NamedSample)> {
        top_n(&self.gap, n)
    }

    /// Top `n` cache candidates (singleton resolved location) by occurrence count.
    pub(crate) fn cache_candidates(&self, n: usize) -> Vec<CacheCandidate> {
        let mut candidates: Vec<CacheCandidate> = self
            .cache
            .iter()
            .filter(|(_, group)| group.locations.len() == 1)
            .map(|((name, receiver_type), group)| CacheCandidate {
                name: name.clone(),
                receiver_type: receiver_type.clone(),
                count: group.count,
                location: group.sample_location.clone(),
            })
            .collect();
        candidates.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        candidates.truncate(n);
        candidates
    }

    /// Top `n` high-frequency but location-unstable keys, by occurrence count.
    pub(crate) fn unstable_hot_keys(&self, n: usize) -> Vec<UnstableHotKey> {
        let mut hot: Vec<UnstableHotKey> = self
            .cache
            .iter()
            .filter(|(_, group)| group.locations.len() > 1)
            .map(|((name, receiver_type), group)| UnstableHotKey {
                name: name.clone(),
                receiver_type: receiver_type.clone(),
                count: group.count,
                distinct_locations: group.locations.len(),
            })
            .collect();
        hot.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        hot.truncate(n);
        hot
    }
}

fn top_n(map: &BTreeMap<String, NamedSample>, n: usize) -> Vec<(String, NamedSample)> {
    let mut entries: Vec<(String, NamedSample)> =
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    entries.sort_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(n);
    entries
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --bin kmp-lsp unresolved_symbol_diagnostics -- --nocapture`
Expected: PASS (7 tests total in this module now).

Also run: `cargo clippy --all-targets -- -D warnings` — expected clean (no unused-field warnings; every field is read by at least one method already in this same commit).

- [ ] **Step 5: Commit**

```bash
git add src/features/unresolved_symbol_diagnostics.rs src/features/unresolved_symbol_diagnostics_tests.rs
git commit -m "feat(resolution-accuracy): add recall + cache-candidate aggregator"
```

---

### Task 3: CLI POC — workspace walk + report printing

**Files:**
- Create: `src/cli/resolution_accuracy_poc.rs`

**Interfaces:**
- Consumes: `crate::features::unresolved_symbol_diagnostics::{collect_resolution_outcomes, ResolutionAccuracyAggregator}` (Tasks 1–2); `crate::indexer::live_tree::{lang_for_path, parse_live}`; `crate::indexer::Indexer`; `crate::indexer::jar::{scan_gradle_jars, index_jars}`; `super::run::build_index`.
- Produces (used by Task 4): `pub(crate) async fn run_resolution_accuracy(root: &Path)`

- [ ] **Step 1: Write the file**

No new unit tests in this task — it's I/O orchestration mirroring the already-tested `missing_import_poc.rs` pattern exactly (workspace walk, JAR warming, aggregation via Task 2's already-tested `ResolutionAccuracyAggregator`). Verified via the manual smoke run in Task 5.

Create `src/cli/resolution_accuracy_poc.rs`:

```rust
//! POC: resolution-accuracy recall benchmark over a workspace.
//!
//! Runs [`collect_resolution_outcomes`] (shared with a future live
//! diagnostic — see `features::unresolved_symbol_diagnostics`) over every
//! indexed workspace file and prints an aggregate recall summary plus a
//! repeat-resolution/cache-candidate report.
//!
//! Unlike the missing-imports/unused-imports precision POCs (run against a
//! project you know compiles, so every flag is a false positive), there is
//! no compiler ground truth for recall — treat this as a trend metric:
//! compare the same corpus before/after a resolver change, not an absolute
//! score. `Gap` names are the actionable bucket; `FilteredCandidate` is
//! ambiguous by design (see `ResolutionOutcome`'s doc) and needs
//! spot-checking, not blind trust.

use std::path::Path;

use tower_lsp::lsp_types::Url;

use crate::features::unresolved_symbol_diagnostics::{
    collect_resolution_outcomes, ResolutionAccuracyAggregator,
};
use crate::indexer::live_tree::{lang_for_path, parse_live};
use crate::indexer::Indexer;

/// Feed one already-indexed workspace file's reference outcomes into `aggregator`.
fn scan_file(
    indexer: &Indexer,
    uri: &Url,
    source: &str,
    file_label: &str,
    aggregator: &mut ResolutionAccuracyAggregator,
) {
    let Some(lang) = lang_for_path(uri.path()) else {
        return;
    };
    let Some(doc) = parse_live(source, lang) else {
        return;
    };
    indexer.store_live_tree(uri, source);
    let outcomes = collect_resolution_outcomes(indexer, uri, &doc);
    indexer.remove_live_tree(uri);
    for outcome in &outcomes {
        aggregator.add(file_label, outcome);
    }
}

/// Run the benchmark over every indexed workspace `.kt`/`.java` file under
/// `root` and print an aggregate recall + cache-candidate summary.
pub(crate) async fn run_resolution_accuracy(root: &Path) {
    eprintln!("Indexing {}...", root.display());
    let index = super::run::build_index(root, true).await;
    eprintln!(
        "Indexed: {} files, {} symbols",
        index.files.len(),
        index.definitions.len()
    );

    // Member-ref recall depends on library-typed receivers resolving —
    // without warming the compiled-JAR index, every library type looks
    // unresolvable and member recall would be an artifact of missing setup,
    // not resolver quality. Same requirement `missing_import_poc` documents.
    let gradle_paths = crate::indexer::jar::scan_gradle_jars(None);
    if !gradle_paths.is_empty() {
        let mut sidecar = index.jar_sidecar.lock().unwrap_or_else(|e| e.into_inner());
        let n = crate::indexer::jar::index_jars(&index, &gradle_paths, &mut sidecar);
        eprintln!(
            "Indexed {n} compiled-jar symbols from {} gradle jars",
            gradle_paths.len()
        );
    } else {
        eprintln!("(no gradle jars found — library-typed receivers will look unresolved)");
    }

    let mut uris: Vec<String> = index
        .files
        .iter()
        .map(|e| e.key().clone())
        .filter(|u| u.starts_with("file://") && !index.library_uris.contains(u))
        .filter(|u| u.ends_with(".kt") || u.ends_with(".java"))
        .collect();
    uris.sort();

    let mut aggregator = ResolutionAccuracyAggregator::default();
    let mut total_files = 0usize;
    for uri_str in &uris {
        let Ok(uri) = Url::parse(uri_str) else {
            continue;
        };
        let Ok(path) = uri.to_file_path() else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        total_files += 1;
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        scan_file(&index, &uri, &source, &rel, &mut aggregator);
    }

    print_report(total_files, &aggregator);
}

fn print_report(total_files: usize, aggregator: &ResolutionAccuracyAggregator) {
    let recall = aggregator.recall();
    eprintln!("\n──────── resolution-accuracy summary ────────");
    eprintln!("files scanned          : {total_files}");
    eprintln!(
        "member refs             : {} ({} CstResolved, {:.1}% recall)",
        recall.member_total,
        recall.member_cst_resolved,
        recall.member_recall_pct()
    );
    eprintln!(
        "bare refs               : {} ({} resolved, {:.1}% recall)",
        recall.bare_total,
        recall.bare_success,
        recall.bare_recall_pct()
    );

    eprintln!("\ntop Gap names (actionable — no candidate found anywhere):");
    for (name, sample) in aggregator.top_gaps(20) {
        eprintln!(
            "  {:>4}  {name:<32} e.g. {}",
            sample.count, sample.sample_location
        );
    }

    eprintln!("\ntop FilteredCandidate names (ambiguous — spot-check, not a plain miss):");
    for (name, sample) in aggregator.top_filtered_candidates(20) {
        eprintln!(
            "  {:>4}  {name:<32} e.g. {}",
            sample.count, sample.sample_location
        );
    }

    eprintln!("\ntop cache candidates (same symbol resolved to the same place, repeatedly):");
    for candidate in aggregator.cache_candidates(20) {
        let receiver = candidate.receiver_type.as_deref().unwrap_or("<bare>");
        eprintln!(
            "  {:>4}  {receiver}.{:<24} -> {}",
            candidate.count, candidate.name, candidate.location
        );
    }

    eprintln!(
        "\nhigh-frequency but unstable (not a simple cache key — same name, different targets):"
    );
    for hot in aggregator.unstable_hot_keys(10) {
        let receiver = hot.receiver_type.as_deref().unwrap_or("<bare>");
        eprintln!(
            "  {:>4}  {receiver}.{:<24} -> {} distinct locations",
            hot.count, hot.name, hot.distinct_locations
        );
    }
}
```

- [ ] **Step 2: Confirm it compiles**

This file isn't reachable yet (no `mod` declaration, no subcommand wiring) — Task 4 wires it in. Skip straight to Task 4; do not attempt to build this file in isolation.

- [ ] **Step 3: Commit**

```bash
git add src/cli/resolution_accuracy_poc.rs
git commit -m "feat(resolution-accuracy): add CLI workspace-walk + report printing"
```

---

### Task 4: Wire the `resolution-accuracy` subcommand

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `src/cli/args.rs`
- Modify: `src/cli/run.rs`

**Interfaces:**
- Consumes: `super::resolution_accuracy_poc::run_resolution_accuracy` (Task 3).
- Produces: a working `kmp-lsp resolution-accuracy [root]` CLI subcommand.

**Note:** the existing `missing-imports`/`unused-imports` subcommands are themselves not listed in `print_help`'s `SUBCOMMANDS`/`EXAMPLES` sections (an existing gap, not introduced here) — this task matches that same scope exactly: functionally wired and usable, not added to the help text, for consistency with its two siblings.

- [ ] **Step 1: Register the CLI module**

In `src/cli/mod.rs`, insert alphabetically between `mod output;` and `mod run;`:

```rust
mod resolution_accuracy_poc;
```

- [ ] **Step 2: Add the `Subcommand` variant**

In `src/cli/args.rs`, add to the `Subcommand` enum (after the `UnusedImports` variant):

```rust
    /// POC: report resolution-accuracy recall + cache-candidate signals across the workspace.
    ResolutionAccuracy {
        root: Option<PathBuf>,
    },
```

- [ ] **Step 3: Recognize the subcommand name**

In `src/cli/args.rs`'s `is_subcommand` function, add `"resolution-accuracy"` to the `matches!` list (after `"unused-imports"`):

```rust
fn is_subcommand(value: &str) -> bool {
    matches!(
        value,
        "find"
            | "refs"
            | "hover"
            | "complete"
            | "index"
            | "tokens"
            | "tree"
            | "diagnose"
            | "sources"
            | "extract-sources"
            | "check"
            | "missing-imports"
            | "unused-imports"
            | "resolution-accuracy"
    )
}
```

- [ ] **Step 4: Parse the subcommand**

In `src/cli/args.rs`'s `build_subcommand` function, add a match arm (after the `"unused-imports"` arm, before the closing `_ => unreachable!()`):

```rust
        "resolution-accuracy" => Ok(Subcommand::ResolutionAccuracy {
            root: positionals.first().map(PathBuf::from),
        }),
```

- [ ] **Step 5: Dispatch it in `run`**

In `src/cli/run.rs`'s `run` function, add a match arm (after the `Subcommand::UnusedImports` arm, before the closing `}` of the `match args.subcommand` block):

```rust
        Subcommand::ResolutionAccuracy { root } => {
            let root = resolve_root(root.as_deref().or(args.root.as_deref()));
            super::resolution_accuracy_poc::run_resolution_accuracy(&root).await;
        }
```

- [ ] **Step 6: Build and run the full test suite**

Run: `cargo build`
Expected: builds clean, no warnings.

Run: `cargo test --bin kmp-lsp`
Expected: all pass (1743 pre-existing + 7 new from Tasks 1–2 = 1750).

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs src/cli/args.rs src/cli/run.rs
git commit -m "feat(resolution-accuracy): wire the resolution-accuracy CLI subcommand"
```

---

### Task 5: Full verification + smoke run

**Files:** none (verification only).

**Interfaces:** none — this task only runs commands and inspects output.

- [ ] **Step 1: Run the full test suite**

Run: `cargo test --bin kmp-lsp`
Expected: all pass.

Run: `cargo test --tests`
Expected: all pass (integration tests unaffected — this feature has no integration-test surface yet, CLI-only).

- [ ] **Step 2: Run clippy and fmt**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

Run: `cargo fmt --check`
Expected: clean (run `cargo fmt` first if not).

- [ ] **Step 3: Smoke-run the new subcommand**

Run: `cargo run --bin kmp-lsp -- resolution-accuracy tests/fixtures`
Expected: completes without panicking, prints "Indexing ...", a jar-warming line, and the "──────── resolution-accuracy summary ────────" report with non-negative counts. `tests/fixtures` only has 2 `.kt` files, so this is a functional smoke check (the tool runs end-to-end and produces sane-looking output), not a meaningful recall measurement — a real accuracy run needs a full real project (e.g. nowInAndroid, the way `missing-imports`/`unused-imports` were validated), which isn't available in this environment. Leave that as a follow-up for whoever has such a project checked out.

- [ ] **Step 4: Final commit (if fmt made changes)**

```bash
git add -A
git commit -m "chore(resolution-accuracy): fmt"
```

(Skip this step if `cargo fmt --check` was already clean in Step 2 — nothing to commit.)
