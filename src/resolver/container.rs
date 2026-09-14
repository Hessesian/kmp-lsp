//! Container-chain primitives: nested-type paths from imports, from range nesting,
//! and companion-object member lookup.

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;
use crate::StrExt;

/// The enclosing-type chain named by a nested import, outermost-first.
///
/// `com.app.Contract.State.Idle` (symbol `Idle`) → `["Contract", "State"]`.
/// All segments before the imported `symbol`, restricted to type names (uppercase
/// first letter), so leading package segments and the symbol itself are dropped.
/// Returns an empty vec for top-level imports (no enclosing type).
pub(super) fn import_container_chain(full_path: &str, symbol: &str) -> Vec<String> {
    let mut segments: Vec<&str> = full_path.split('.').collect();
    // Drop the trailing symbol segment (the import's leaf), then keep type segments.
    if segments.last() == Some(&symbol) {
        segments.pop();
    }
    segments
        .into_iter()
        .filter(|s| s.starts_with_uppercase())
        .map(|s| s.to_string())
        .collect()
}

/// The chain of enclosing container types (class/interface/object/enum/struct) for
/// the symbol declared at `loc`, outermost-first, looked up across workspace and
/// JAR files. Computed by range nesting so it handles arbitrarily deep nesting.
/// Empty when the file/symbol isn't found or the symbol is top-level.
pub(super) fn enclosing_container_chain(indexer: &Indexer, loc: &Location) -> Vec<String> {
    let Some(file_data) = indexer.file_data_for(loc.uri.as_str()) else {
        return vec![];
    };
    let target = loc.range;
    let mut enclosing: Vec<&crate::types::SymbolEntry> = file_data
        .symbols
        .iter()
        .filter(|s| {
            crate::parser::is_container_kind(s.kind)
                // Exclude the symbol itself (a container can be the imported symbol).
                && s.selection_range != target
                && range_encloses(s.range, target)
        })
        .collect();
    // Outermost first: earliest start, latest end.
    enclosing.sort_by(|a, b| {
        pos_tuple(a.range.start)
            .cmp(&pos_tuple(b.range.start))
            .then_with(|| pos_tuple(b.range.end).cmp(&pos_tuple(a.range.end)))
    });
    enclosing.into_iter().map(|s| s.name.clone()).collect()
}

fn pos_tuple(p: tower_lsp::lsp_types::Position) -> (u32, u32) {
    (p.line, p.character)
}

/// Whether `outer` fully contains `inner` (start ≤ start and end ≥ end).
pub(crate) fn range_encloses(
    outer: tower_lsp::lsp_types::Range,
    inner: tower_lsp::lsp_types::Range,
) -> bool {
    pos_tuple(outer.start) <= pos_tuple(inner.start) && pos_tuple(inner.end) <= pos_tuple(outer.end)
}

/// Find `name` inside the companion object nested in `class_name`.
///
/// `Foo.member` with `Foo` a class name (not a variable) can only ever reach a
/// companion-object member in Kotlin — never an instance member of `Foo`, even
/// when one happens to share the name.
pub(super) fn resolve_companion_member(
    indexer: &Indexer,
    name: &str,
    class_name: &str,
    file_uri: &str,
) -> Vec<Location> {
    let Ok(uri) = Url::parse(file_uri) else {
        return vec![];
    };
    let Some(file_data) = indexer.file_data_for(file_uri) else {
        return vec![];
    };

    if indexer.jar_files.contains_key(file_uri) {
        // A compiled JAR's synthetic FileData gives every symbol its own
        // one-line range keyed by sequential position in the sidecar's flat
        // entry list (see `build_jar_file_data`) — there is no real nesting
        // for range containment to discover, unlike a source-parsed file.
        // Match by container name instead: the sidecar's `entriesFromClass`
        // gives a companion's own class-declaration symbol `container ==
        // class_name`, and gives ITS members that companion's own bare name
        // as their container in turn — mirroring the exact shape
        // `members_for_jar_backed_type` (completion) already matches on.
        let companion_name = file_data
            .symbols
            .iter()
            .find(|symbol| {
                symbol.is_companion_object() && symbol.container.as_deref() == Some(class_name)
            })
            .map(|symbol| symbol.name.as_str());
        let Some(companion_name) = companion_name else {
            return vec![];
        };
        return file_data
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.name == name && symbol.container.as_deref() == Some(companion_name)
            })
            .map(|symbol| Location {
                uri: uri.clone(),
                range: symbol.selection_range,
            })
            .collect();
    }

    // The class's full declaration range (not just its name's selection range) is
    // needed to tell which companion object belongs to it when a file has more
    // than one class.
    let Some(class_range) = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.name == class_name && crate::parser::is_container_kind(symbol.kind))
        .map(|symbol| symbol.range)
    else {
        return vec![];
    };
    let Some(companion) = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.is_companion_object() && range_encloses(class_range, symbol.range))
    else {
        return vec![];
    };
    file_data
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.name == name
                && symbol.range != companion.range
                && range_encloses(companion.range, symbol.range)
        })
        .map(|symbol| Location {
            uri: uri.clone(),
            range: symbol.selection_range,
        })
        .collect()
}
