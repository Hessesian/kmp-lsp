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

use std::collections::{BTreeMap, HashMap, HashSet};

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
        let sample_location = format!("{file_label}:{}:{}", outcome.line + 1, outcome.col + 1);
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
                let entry = self
                    .gap
                    .entry(outcome.name.clone())
                    .or_insert_with(|| NamedSample {
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

#[cfg(test)]
#[path = "unresolved_symbol_diagnostics_tests.rs"]
mod tests;
