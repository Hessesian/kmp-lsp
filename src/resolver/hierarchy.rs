//! Class hierarchy traversal — walk supertypes for member resolution.

use std::collections::HashSet;

use crate::indexer::{Indexer, InferDeps};
use crate::types::{CallerContext, FileData};

/// Per-WALK cap on blocking sidecar-IPC promotion attempts for ancestor
/// classes living in not-yet-materialized JARs. The walk runs on paths that
/// carry no promotion budget of their own — per-name inference (inlay hints
/// fan `resolve_from_class_hierarchy` out across every visible name) and
/// bare completion's inherited-members collector — so an unbudgeted
/// promotion here bypassed every existing request cap: with a cold cache,
/// each distinct un-cached ancestor JAR paid a ~200ms blocking round trip,
/// unbounded across a walk. Cache-backed promotions bypass this (free, pure
/// in-memory); genuinely cold ancestors beyond the cap stay Tier-1 for this
/// walk and are covered by file-open import promotion or a later walk —
/// `materialized`/`materialization_failed` memoize outcomes, so per session
/// each JAR pays at most one attempt. Per-REQUEST budget threading (one
/// budget shared across all the walks a request triggers) is deferred to
/// the accessor-function refactor.
///
/// This is the *default* every interactive, keystroke-latency-sensitive
/// caller should pass. `walk_hierarchy` takes the budget as a parameter,
/// not a hardcoded internal — a caller with a different latency tolerance
/// (e.g. a user-initiated rename, not a per-keystroke completion) may pass
/// a larger budget so its walk can run to actual completion instead of
/// guessing under a cap sized for a different use case.
pub(crate) const MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK: usize = 3;

/// Walk the class hierarchy starting from `start_class`, collecting items at each level.
/// `T` is what the visitor produces per symbol. `max_depth` prevents infinite loops.
/// `sidecar_budget` bounds blocking JAR-promotion round trips for this walk — pass
/// [`MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`] for interactive callers, or a
/// caller-specific budget for operations that can tolerate more latency.
pub(crate) fn walk_hierarchy<'a, T, F>(
    idx: &'a Indexer,
    start_class: &str,
    start_uri: &str,
    caller: CallerContext<'a>,
    max_depth: usize,
    sidecar_budget: usize,
    collect: F,
) -> Vec<T>
where
    F: Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>,
{
    let mut walker = HierarchyWalker {
        idx,
        caller,
        max_depth,
        collect,
        visited: HashSet::from([(start_uri.to_owned(), start_class.to_owned())]),
        items: Vec::new(),
        sidecar_budget,
    };
    walker.recurse(start_class, start_uri, 0);
    walker.items
}

struct HierarchyWalker<'a, T, F>
where
    F: Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>,
{
    idx: &'a Indexer,
    caller: CallerContext<'a>,
    max_depth: usize,
    collect: F,
    visited: HashSet<(String, String)>,
    items: Vec<T>,
    /// Remaining blocking-IPC promotion attempts for THIS walk (see
    /// [`MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK`]).
    sidecar_budget: usize,
}

impl<'a, T, F> HierarchyWalker<'a, T, F>
where
    F: Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>,
{
    fn recurse(&mut self, class_name: &str, class_uri: &str, depth: usize) {
        if depth >= self.max_depth {
            return;
        }

        for (super_name, super_uri) in
            supertype_targets(self.idx, class_name, class_uri, &mut self.sidecar_budget)
        {
            if !self.visited.insert((super_uri.clone(), super_name.clone())) {
                continue;
            }
            self.items.extend((self.collect)(
                self.idx,
                &super_name,
                &super_uri,
                self.caller,
            ));
            self.recurse(&super_name, &super_uri, depth + 1);
        }
    }
}

fn supertype_targets(
    idx: &Indexer,
    class_name: &str,
    class_uri: &str,
    sidecar_budget: &mut usize,
) -> Vec<(String, String)> {
    use tower_lsp::lsp_types::Url;
    let Ok(uri) = Url::parse(class_uri) else {
        return vec![];
    };
    let Some(file_data) = super::ensure_file_data(idx, &uri) else {
        return vec![];
    };

    super_names_for_class(&file_data, class_name)
        .into_iter()
        .flat_map(|super_name| {
            // A super class living in a not-yet-materialized JAR is invisible
            // to `resolve_symbol_no_rg` (it reads Tier-2 `jar_definitions`)
            // — promote it first, or the hierarchy walk silently dead-ends
            // here and every inherited member (e.g. `setState` on a library
            // `MviViewModel` base class) disappears from completion/hover.
            // BUDGETED per walk (see the constant above): unbudgeted, this
            // was the one promotion site reachable around every request cap.
            // `super_name` can be a dotted qualified spelling
            // (`class X : com.lib.Base()`) — the accessor handles the
            // bare-leaf fallback.
            crate::indexer::jar::ensure_jar_definitions_for(idx, &super_name, sidecar_budget);
            super::resolve_symbol_no_rg(idx, &super_name, &uri)
                .into_iter()
                .map(move |loc| (super_name.clone(), loc.uri.to_string()))
        })
        .collect()
}

fn super_names_for_class(file_data: &FileData, class_name: &str) -> Vec<String> {
    if class_name.is_empty() {
        return file_data
            .supers
            .iter()
            .map(|(_, name, _)| name.clone())
            .collect();
    }

    let class_line = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.name == class_name)
        .map(|symbol| symbol.selection_start());
    match class_line {
        Some(line) => file_data
            .supers
            .iter()
            .filter(|(super_line, _, _)| *super_line == line)
            .map(|(_, name, _)| name.clone())
            .collect(),
        None => file_data
            .supers
            .iter()
            .map(|(_, name, _)| name.clone())
            .collect(),
    }
}

/// How a candidate receiver's type relates to a target (query) declaring
/// type. `Exact`/`Inherited` are the two ways a candidate is *proven* to
/// belong; `Unrelated` is a proven exclusion; `Unresolvable` means the
/// index doesn't have enough data to prove anything either way (the
/// candidate type itself isn't indexed) — never treat this as `Unrelated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReceiverTypeAgreement {
    Exact,
    Inherited,
    Unrelated,
    Unresolvable,
}

/// Ascending walk from `candidate_type`: does `target_type` appear among
/// its supertypes? Same mechanism `resolve_from_class_hierarchy` already
/// uses for the string engine's inherited-member lookups, applied in
/// reverse — not "find the member," but "does this ancestor chain contain
/// that type." `sidecar_budget` bounds blocking JAR-promotion round trips —
/// see [`walk_hierarchy`].
pub(crate) fn supertype_chain_contains(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
    sidecar_budget: usize,
) -> bool {
    walk_hierarchy(
        indexer,
        candidate_type,
        candidate_uri,
        CallerContext::default(),
        12,
        sidecar_budget,
        |_, super_name, _, _| {
            if super_name == target_type {
                vec![()]
            } else {
                vec![]
            }
        },
    )
    .into_iter()
    .next()
    .is_some()
}

/// The full receiver-type-agreement decision: exact match (cheap, no walk),
/// else — only if `candidate_type` is genuinely indexed, so a negative
/// result is trustworthy — an ascending supertype walk. `sidecar_budget`
/// bounds blocking JAR-promotion round trips the walk may spend — see
/// [`walk_hierarchy`].
pub(crate) fn receiver_type_agreement(
    indexer: &Indexer,
    candidate_type: &str,
    candidate_uri: &str,
    target_type: &str,
    sidecar_budget: usize,
) -> ReceiverTypeAgreement {
    if candidate_type == target_type {
        return ReceiverTypeAgreement::Exact;
    }
    if !indexer.has_type_definition(candidate_type) {
        return ReceiverTypeAgreement::Unresolvable;
    }
    if supertype_chain_contains(
        indexer,
        candidate_type,
        candidate_uri,
        target_type,
        sidecar_budget,
    ) {
        ReceiverTypeAgreement::Inherited
    } else {
        ReceiverTypeAgreement::Unrelated
    }
}

#[cfg(test)]
#[path = "hierarchy_tests.rs"]
mod tests;
