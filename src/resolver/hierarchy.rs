//! Class hierarchy traversal — walk supertypes for member resolution.

use std::collections::HashSet;

use crate::indexer::{Indexer, InferDeps};
use crate::types::{CallerContext, FileData};
use crate::StrExt;

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
    start_uri: &'a str,
    caller: CallerContext<'a>,
    max_depth: usize,
    sidecar_budget: usize,
    collect: F,
) -> Vec<T>
where
    F: Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>,
{
    // The walk's own unchanging origin, for module-scoped ambiguity narrowing
    // (`module_scoped_tie_break`) — prefer `caller.uri`, which most callers
    // already populate with the real editing file for an unrelated purpose
    // (see e.g. `find_field_type_via_class_hierarchy`), and fall back to
    // `start_uri`, which equals the real editing file at every remaining
    // call site that leaves `caller` at its default. Threaded unchanged
    // through every recursive hop below — never updated to a hop's own
    // resolved URI — because past hop 1 that per-hop URI is frequently a
    // `jar:` synthetic URI with no owning module of its own.
    let origin_uri = caller.uri.unwrap_or(start_uri);
    let mut walker = HierarchyWalker {
        idx,
        caller,
        max_depth,
        collect,
        visited: HashSet::from([(start_uri.to_owned(), start_class.to_owned())]),
        items: Vec::new(),
        sidecar_budget,
        origin_uri,
    };
    walker.recurse(start_class, start_uri, 0);
    walker.items
}

/// Breadth-first counterpart to [`walk_hierarchy`], for callers that need
/// Kotlin's own "nearest, most specific applicable ancestor wins"
/// precedence rather than every match across the whole chain.
/// `walk_hierarchy`'s depth-first traversal fully explores one direct
/// supertype's entire ancestor chain before ever touching the NEXT direct
/// supertype — so when a class has multiple direct supertypes (an entirely
/// ordinary Kotlin shape, e.g. implementing several interfaces), a farther
/// ancestor found down the first branch can appear before a nearer,
/// directly-implemented one down a sibling branch. This instead visits
/// supertypes strictly level-by-level and returns as soon as ANY level
/// produces a match — never checking, let alone returning, anything from a
/// farther level once a nearer one has something.
pub(crate) fn walk_hierarchy_breadth_first<T, F>(
    idx: &Indexer,
    start_class: &str,
    start_uri: &str,
    caller: CallerContext<'_>,
    max_depth: usize,
    mut sidecar_budget: usize,
    collect: F,
) -> Vec<T>
where
    F: Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>,
{
    let origin_uri = caller.uri.unwrap_or(start_uri);
    let mut visited: HashSet<(String, String)> =
        HashSet::from([(start_uri.to_owned(), start_class.to_owned())]);
    let mut current_level: Vec<(String, String)> =
        vec![(start_class.to_owned(), start_uri.to_owned())];
    for _ in 0..max_depth {
        let mut next_level: Vec<(String, String)> = Vec::new();
        let mut found: Vec<T> = Vec::new();
        for (class_name, class_uri) in &current_level {
            for (super_name, super_uri) in
                supertype_targets(idx, class_name, class_uri, &mut sidecar_budget, origin_uri)
            {
                if !visited.insert((super_uri.clone(), super_name.clone())) {
                    continue;
                }
                found.extend(collect(idx, &super_name, &super_uri, caller));
                next_level.push((super_name, super_uri));
            }
        }
        if !found.is_empty() {
            return found;
        }
        if next_level.is_empty() {
            break;
        }
        current_level = next_level;
    }
    Vec::new()
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
    /// The real file this walk started from — unchanging across every hop,
    /// unlike the per-hop `class_uri` that `recurse` walks with. See
    /// [`walk_hierarchy`]'s doc comment for how it's derived.
    origin_uri: &'a str,
}

impl<'a, T, F> HierarchyWalker<'a, T, F>
where
    F: Fn(&Indexer, &str, &str, CallerContext<'_>) -> Vec<T>,
{
    fn recurse(&mut self, class_name: &str, class_uri: &str, depth: usize) {
        if depth >= self.max_depth {
            return;
        }

        for (super_name, super_uri) in supertype_targets(
            self.idx,
            class_name,
            class_uri,
            &mut self.sidecar_budget,
            self.origin_uri,
        ) {
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
    origin_uri: &str,
) -> Vec<(String, String)> {
    use tower_lsp::lsp_types::Url;
    let Ok(uri) = Url::parse(class_uri) else {
        return vec![];
    };
    let Some(file_data) = super::ensure_file_data(idx, &uri) else {
        return vec![];
    };
    // Parsed once per hop, not required to succeed: a bad `origin_uri` only
    // costs the module-scoped tie-break below, never the rest of this walk.
    let origin_url = Url::parse(origin_uri).ok();

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
            // (`class X : com.lib.Base()`) — `ensure_jar_definitions_for`
            // handles that itself (tries the full name, falls back to the
            // bare leaf), so it gets the original spelling.
            crate::indexer::jar::ensure_jar_definitions_for(idx, &super_name, sidecar_budget);
            let super_leaf = super_name.last_segment().to_owned();
            // A dotted spelling names either a package-qualified type
            // (`com.other.Seq`), a nested-type chain (`Outer.Inner`), or
            // both at once (`com.other.Outer.Inner`). Resolving it
            // precisely — instead of falling straight to the leaf-only
            // ambiguity-safe chain below, which is keyed by simple name
            // only and could resolve to an unrelated same-leaf class
            // reachable via same-package/import from this hop's own file —
            // matters for the same reason in every one of these shapes:
            // silently picking the WRONG supertype instead of the one the
            // source specifically qualified to avoid exactly that.
            //
            // Package vs. type segments are told apart the same way
            // `resolve_symbol_with_io`'s own dotted-name handling already
            // does: skip leading lowercase (package) segments, the first
            // uppercase segment is the outermost TYPE. A real package
            // segment is never uppercase-first; an enclosing type's name
            // always is.
            if super_name.contains('.') {
                let segments: Vec<&str> = super_name.split('.').collect();
                if let Some(start) = segments.iter().position(|s| s.starts_with_uppercase()) {
                    let outer = segments[start];
                    let mut container = if start > 0 {
                        // Leading lowercase segments are a real package --
                        // resolve the outermost type there exactly
                        // (`find_symbol_in_package`, no ambiguity risk).
                        let pkg = segments[..start].join(".");
                        super::find_symbol_in_package(idx, outer, &pkg)
                    } else {
                        // No package prefix at all -- a pure nested-type
                        // chain. Resolve the outermost type ambiguity-safely.
                        super::resolve_symbol_hierarchy_ambiguity_safe(
                            idx,
                            outer,
                            &uri,
                            origin_url.as_ref(),
                        )
                        .into_iter()
                        .next()
                    };
                    // Walk any remaining nested-type segments
                    // (`find_name_scoped_to_container`, the same helper
                    // `resolve_qualified` already uses for this) into the
                    // specific outer type's own scope.
                    for &seg in &segments[start + 1..] {
                        container = container.as_ref().and_then(|c| {
                            crate::resolver::find::find_name_scoped_to_container(idx, seg, c)
                        });
                    }
                    if let Some(loc) = container {
                        return vec![(super_leaf, loc.uri.to_string())];
                    }
                }
            }
            // Ambiguity-safe, not `resolve_symbol_no_rg`'s raw first-match tail: at
            // hop 2+ `uri` is frequently a `jar:` synthetic URI with no import list
            // to disambiguate a same-named collision against (compiled JARs carry
            // no import statements) -- see the hierarchy-walk-unscoped-name-
            // collision design doc. Scoped to this one call site; the other seven
            // `resolve_symbol_no_rg` callers are unaffected. `origin_url` (the
            // walk's real starting file, not this hop's `uri`) is passed
            // separately so the module-scoped tie-break can still find real
            // Gradle dependency data past hop 1. Reached for an unqualified
            // `super_name`, or a qualified one whose exact package lookup
            // above found nothing (falls back to the same leaf-only
            // resolution every unqualified supertype already went through).
            super::resolve_symbol_hierarchy_ambiguity_safe(
                idx,
                &super_leaf,
                &uri,
                origin_url.as_ref(),
            )
            .into_iter()
            .map(move |loc| (super_leaf.clone(), loc.uri.to_string()))
            .collect::<Vec<_>>()
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
