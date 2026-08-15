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

// Temporary: this module's `pub(crate)` items have no consumer outside
// `#[cfg(test)]` until Task 4 registers `cli::resolution_accuracy_poc` in
// `cli/mod.rs`. The repo's pre-commit hook runs bare `cargo clippy -D
// warnings` (no `--tests`), which doesn't compile `#[cfg(test)]` code, so
// without this every commit before Task 4 fails on dead-code errors. Remove
// this line in Task 4 once the CLI wiring makes the whole chain reachable
// from a non-test entry point — `cargo clippy -- -D warnings` passing clean
// afterward confirms every item here is genuinely used.
#![cfg_attr(not(test), allow(dead_code))]

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

#[cfg(test)]
#[path = "unresolved_symbol_diagnostics_tests.rs"]
mod tests;
