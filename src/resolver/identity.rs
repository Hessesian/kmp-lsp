//! Symbol identity: classify the identifier under the cursor, then resolve it
//! to its definition sites. Classification is CST work in
//! `indexer::infer::cst_symbol`; deciding that a qualified name is a known
//! type, and finding definitions, is name resolution, so this half lives in
//! the resolver.

use tower_lsp::lsp_types::{Location, Position, Url};

use super::api::Definitions;
use crate::indexer::infer::cst_symbol::{classify_symbol_with, SymbolAtCursor, SymbolRole};
use crate::indexer::infer::deps::InferDeps as _;
use crate::indexer::{CallShape, Indexer, ShapeFiltered};
use crate::types::CursorPos;

/// Classify the identifier under `pos`, treating a qualified name as a type
/// only when the index declares it. See [`classify_symbol_with`].
pub(crate) fn classify_symbol_at(
    indexer: &Indexer,
    uri: &Url,
    pos: CursorPos,
) -> Option<SymbolAtCursor> {
    classify_symbol_with(indexer, uri, pos, names_a_known_type)
}

/// Whether `type_path` (a simple, nested `Outer.Inner`, or package-qualified
/// type name) names a type the index knows. A qualified path is validated as a
/// whole, not by its leaf: `Missing.User` must not pass because some unrelated
/// `User` is indexed.
fn names_a_known_type(indexer: &Indexer, type_path: &str, uri: &Url) -> bool {
    if type_path.contains('.') {
        return !crate::resolver::resolve_type_path_declarations(
            indexer,
            type_path,
            uri,
            crate::resolver::ResolveIo::IndexOnly,
        )
        .is_empty();
    }
    indexer.has_type_definition(type_path)
}

/// `classify_symbol_at`, but taking an LSP `Position` directly — the
/// `Position → CursorPos` conversion every navigation-feature call site
/// otherwise repeats.
pub(crate) fn classify_cursor(
    indexer: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<SymbolAtCursor> {
    classify_symbol_at(
        indexer,
        uri,
        CursorPos {
            line: position.line as usize,
            utf16_col: position.character as usize,
        },
    )
}

/// A definitions lookup result, tagged by how much confidence its identity
/// carries.
#[derive(Debug)]
pub(crate) enum NavigationSource<T> {
    /// Identity established from the CST + index: precise, ranked first.
    CstResolved(T),
    /// Name-based scan: today's behavior, visibly labeled.
    NameScan(T),
}

/// Resolve `symbol`'s identity to its definition site(s).
///
/// `CstResolved` when the CST gave enough information to trust the result
/// (a declaration is trivially its own definition; a receiver-typed member
/// reference is looked up ON that type). `NameScan` for everything the CST
/// couldn't narrow — an untyped receiver, or a bare reference resolved by
/// today's name-based `find_definition_qualified(name, None, uri)` (which
/// can span multiple same-named workspace symbols).
pub(crate) fn resolve_identity(
    symbol: &SymbolAtCursor,
    indexer: &Indexer,
    uri: &Url,
) -> NavigationSource<Definitions> {
    resolve_identity_with_io(symbol, indexer, uri, false)
}

/// Like `resolve_identity` but, when `index_only` is set, never spawns
/// `rg`/`fd` — see `resolve_symbol_index_only`'s own doc comment for why.
/// Used by the resolution-accuracy benchmark, which calls this once per
/// identifier in a whole corpus.
pub(crate) fn resolve_identity_with_io(
    symbol: &SymbolAtCursor,
    indexer: &Indexer,
    uri: &Url,
    index_only: bool,
) -> NavigationSource<Definitions> {
    let find_definition_qualified = |name: &str, qualifier: Option<&str>, uri: &Url| {
        if index_only {
            indexer.find_definition_qualified_index_only(name, qualifier, uri)
        } else {
            indexer.find_definition_qualified(name, qualifier, uri)
        }
    };
    match &symbol.role {
        SymbolRole::Declaration { indexed } => {
            let locations = Definitions(find_definition_qualified(&symbol.name, None, uri));
            // Only declarations `KOTLIN_DEFINITIONS` actually indexes can be
            // trusted CST-resolved; an unindexed one (bare param, val/var-less
            // constructor param, type param) falls through to an unanchored
            // same-file scan or workspace-wide scan — label it NameScan (see
            // `is_indexed_declaration_site`).
            if *indexed {
                NavigationSource::CstResolved(locations)
            } else {
                NavigationSource::NameScan(locations)
            }
        }
        SymbolRole::Reference {
            receiver_type: Some(receiver_type),
            shape,
            ..
        } => {
            let locations = if index_only {
                indexer.find_member_of_type_index_only(&symbol.name, receiver_type, uri)
            } else {
                indexer.find_member_of_type(&symbol.name, receiver_type, uri)
            };
            // A call's own shape rules out a same-named, wrong-arity member/
            // extension on the same receiver type — e.g. `triggers.collect {
            // trigger -> }` (1 arg via trailing lambda) must not resolve to a
            // same-file `Flow.collect(scope, block)` self-declaration just
            // because both are in scope on `Flow`. Filtering to empty here
            // (rather than keeping the wrong-arity candidate) demotes this to
            // `NameScan`, so `find_definition`'s later, arity-blind but
            // receiver-aware fallback (variable-type inference + hierarchy
            // walk, which never consults the extension-in-scope registry that
            // caused the wrong match) gets a chance to find the real target.
            let locations = match shape {
                Some(shape) => shape_filter_locations(indexer, *shape, locations).resolved(),
                None => locations,
            };
            if locations.is_empty() {
                NavigationSource::NameScan(Definitions(locations))
            } else {
                NavigationSource::CstResolved(Definitions(locations))
            }
        }
        SymbolRole::Reference {
            receiver_type: None,
            ..
        }
        | SymbolRole::ImportSegment => NavigationSource::NameScan(Definitions(
            find_definition_qualified(&symbol.name, None, uri),
        )),
    }
}

/// Classify `locations` by whether each one's own declared arity `shape`
/// accepts — each `Location` is looked up by an exact `selection_range` match
/// against the declaring file's own symbol table (how `resolve_qualified`'s
/// candidates are always constructed), so a location that doesn't match any
/// symbol, or whose file isn't indexed, is kept unfiltered — same fail-open
/// reasoning `CallShape::accepts_symbol` applies to a symbol it *can* find.
///
/// `pub(crate)`: also used by `find_definition`'s and `compute_hover`'s
/// `ctx.contextual`-based branches, for the identical self-shadow reason —
/// see `CursorContext::contextual`'s own doc for why that field applies to
/// more than `it`/`this`/named lambda params.
pub(crate) fn shape_filter_locations(
    indexer: &Indexer,
    shape: CallShape,
    locations: Vec<Location>,
) -> ShapeFiltered<Location> {
    ShapeFiltered::classify(locations, |location| {
        let Some(fd) = indexer
            .files
            .get(location.uri.as_str())
            .or_else(|| indexer.jar_files.get(location.uri.as_str()))
        else {
            return true;
        };
        let Some(symbol) = fd
            .symbols
            .iter()
            .find(|s| s.selection_range == location.range)
        else {
            return true;
        };
        shape.accepts_symbol(symbol)
    })
}
