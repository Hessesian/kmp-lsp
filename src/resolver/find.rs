use std::sync::Arc;
use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;
use crate::LinesExt;

use super::ensure_file_data;
use super::resolve::range_encloses;

/// Search for `name` in a specific file identified by its URI string.
///
/// Checks the in-memory symbol index first; falls back to raw line scanning
/// (for constructor parameters) and finally on-demand tree-sitter parsing.
pub(crate) fn find_name_in_uri(idx: &Indexer, name: &str, file_uri: &str) -> Vec<Location> {
    let Ok(uri) = Url::parse(file_uri) else {
        return vec![];
    };

    let Some(file_data) = ensure_file_data(idx, &uri) else {
        return vec![];
    };
    if let Some(sym) = file_data.symbols.iter().find(|s| s.name == name) {
        return vec![Location {
            uri,
            range: sym.selection_range,
        }];
    }
    if let Some(range) = file_data.lines.find_declaration_range(name) {
        return vec![Location { uri, range }];
    }
    vec![]
}

/// Every same-named symbol tagged as declared directly inside `class_name`,
/// within `file_uri` — for a caller that already knows the ancestor's own
/// class NAME (a hierarchy walk's `super_name`) but not its declaration
/// Location, so `find_all_names_scoped_to_container`'s range-containment
/// path doesn't apply. Skips straight to the same container-tag match that
/// function's own JAR-stub fallback uses, since JAR method/field symbols
/// always carry a real `container` tag in place of a real enclosing range
/// (see `find_name_in_uri_after_line`'s doc for the bug this avoids: a
/// same-named sibling class's members leaking in purely by file position).
///
/// Real, measured bug this fixes: `resolve_from_class_hierarchy_scoped`'s
/// walk used to call `find_name_in_uri` per ancestor — arity-blind (first
/// same-named symbol in the WHOLE file, not scoped to the ancestor at all)
/// — so a wrong-arity or unrelated same-named member could win over the
/// real inherited overload(s) `name` actually has.
pub(crate) fn find_all_names_with_container_in_uri(
    idx: &Indexer,
    name: &str,
    class_name: &str,
    file_uri: &str,
) -> Vec<Location> {
    let Ok(uri) = Url::parse(file_uri) else {
        return vec![];
    };
    let Some(file_data) = ensure_file_data(idx, &uri) else {
        return vec![];
    };
    file_data
        .symbols
        .iter()
        .filter(|symbol| symbol.name == name && symbol.container.as_deref() == Some(class_name))
        .map(|symbol| Location {
            uri: uri.clone(),
            range: symbol.selection_range,
        })
        .collect()
}

/// Like `find_name_in_uri` but prefers declarations at or after `after_line`.
///
/// Used when we already know the qualifier class lives at `after_line` — we
/// want the parameter/field of THAT class, not a same-named field in a
/// different class that happens to appear earlier in the same file.
///
/// Strategy:
///   1. Symbol table, container-scoped when `container_name` is known — a
///      same-named symbol tagged with a DIFFERENT container is never
///      returned, no matter its position (see below for why "closest by
///      line" alone is unsound). Falls to position only when the caller
///      doesn't know the container (e.g. resolving a top-level name).
///   2. Line scan — search only lines >= `after_line`.
///
/// Loads `FileData` via `ensure_file_data`, which checks the in-memory
/// index (files + jar_files) and falls back to on-demand disk parse.
///
/// Real bug this fixes: a compiled JAR can hold several classes in one
/// synthetic per-JAR `FileData` (e.g. `NavHostController extends
/// NavController`, both from the same `navigation-runtime` JAR). Position-
/// only matching mis-attributed `NavController`'s own `navigate(...)`
/// overloads to `NavHostController` purely because they land later in file
/// order — there's no real body range to contain them (JAR symbol ranges
/// are single-line stubs) — which fed `resolve_qualified` a wrong-arity
/// "own member" instead of the correct arity-complete overload set reached
/// via the class hierarchy walk. Every JAR-derived symbol carries a real
/// `container` tag (its actual enclosing class), so trusting it over
/// position is strictly more correct whenever it's available.
pub(crate) fn find_name_in_uri_after_line(
    idx: &Indexer,
    name: &str,
    file_uri: &str,
    after_line: u32,
    container_name: Option<&str>,
) -> Vec<Location> {
    let Ok(uri) = Url::parse(file_uri) else {
        return vec![];
    };

    let Some(file_data) = ensure_file_data(idx, &uri) else {
        return vec![];
    };

    if let Some(container_name) = container_name {
        let same_container: Vec<Location> = file_data
            .symbols
            .iter()
            .filter(|s| s.name == name && s.container.as_deref() == Some(container_name))
            .map(|s| Location {
                uri: uri.clone(),
                range: s.selection_range,
            })
            .collect();
        if !same_container.is_empty() {
            return same_container;
        }
        // Known container, no member of it anywhere in the file — that's an
        // authoritative "not a member here", not a hint to guess by
        // position. Still falls through to the line scan below for names
        // the symbol table never captures at all (e.g. constructor params).
    } else {
        // a) Symbol table: find the closest symbol at or after `after_line`.
        let best = file_data
            .symbols
            .iter()
            .filter(|s| s.name == name && s.selection_start() >= after_line)
            .min_by_key(|s| s.selection_start());

        if let Some(sym) = best {
            return vec![Location {
                uri,
                range: sym.selection_range,
            }];
        }

        // Fallback: any symbol with this name (different class, same file)
        if let Some(sym) = file_data.symbols.iter().find(|s| s.name == name) {
            return vec![Location {
                uri,
                range: sym.selection_range,
            }];
        }
    }

    // b) Line scan scoped to after_line first, then the whole file.
    if let Some(range) = file_data
        .lines
        .find_declaration_range_after(name, after_line)
    {
        return vec![Location { uri, range }];
    }
    if let Some(range) = file_data.lines.find_declaration_range(name) {
        return vec![Location { uri, range }];
    }
    vec![]
}

/// Find `name` declared within `container`'s own body via exact
/// range-containment, falling back to `find_name_in_uri_after_line` when
/// `container`'s own symbol entry can't be located, or when it's located but
/// its recorded range doesn't enclose any matching member (e.g. degenerate
/// JAR stub ranges — see the fallback call site below).
pub(crate) fn find_name_scoped_to_container(
    idx: &Indexer,
    name: &str,
    container: &Location,
) -> Option<Location> {
    let file_data = ensure_file_data(idx, &container.uri)?;

    let container_symbol = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.selection_range == container.range);

    let contained = container_symbol
        .and_then(|container_symbol| {
            file_data.symbols.iter().find(|symbol| {
                symbol.name == name
                    && symbol.range != container_symbol.range
                    && range_encloses(container_symbol.range, symbol.range)
            })
        })
        .map(|found| Location {
            uri: container.uri.clone(),
            range: found.selection_range,
        });
    if contained.is_some() {
        return contained;
    }

    // Range-containment misses degenerate containers whose declaration range
    // doesn't actually span their members — e.g. JAR-derived stub symbols,
    // which record only a name's line, not a real body range.
    find_name_in_uri_after_line(
        idx,
        name,
        container.uri.as_str(),
        container.range.start.line,
        container_symbol.map(|symbol| symbol.name.as_str()),
    )
    .into_iter()
    .next()
}

/// Like [`find_name_scoped_to_container`], but returns EVERY same-named
/// symbol declared directly inside `container`'s own body, not just the
/// first match — for a caller that needs to hand an overloaded name's full
/// candidate set to arity-based shape filtering, instead of collapsing to
/// one arbitrary overload before that filtering ever runs.
///
/// Real, measured bug this fixes: a Java class's overloaded method (e.g.
/// `FormatUtil.formatAmount`, 6 overloads) always resolved to the SAME one
/// candidate via `find_name_scoped_to_container`'s `.find()` (Java method
/// symbols land in reverse source order in `file_data.symbols`, so `.find()`
/// always picked the highest-arity, last-declared overload) — which then
/// failed arity-based shape filtering for nearly every real call site, since
/// callers overwhelmingly use the OTHER overloads.
///
/// Widens both the primary range-containment path AND the degenerate-range
/// (JAR stub) fallback — a JAR-derived class's synthetic `FileData` is flat
/// (every symbol gets its own single-line `range == selection_range`), so a
/// class's own range never truly *encloses* its members and the
/// range-containment path always misses for it, falling through to the
/// `container`-field match below. Real, measured bug this fixes: JUnit's
/// `org.junit.Assert.fail()`/`.assertEquals(a, b)` (0-arg and 2-arg calls)
/// resolved to nothing — the old fallback (`find_name_in_uri_after_line`)
/// returned only the ONE closest same-named entry after the class's own
/// synthetic line, silently dropping every other real overload.
pub(crate) fn find_all_names_scoped_to_container(
    idx: &Indexer,
    name: &str,
    container: &Location,
) -> Vec<Location> {
    let Some(file_data) = ensure_file_data(idx, &container.uri) else {
        return vec![];
    };

    let Some(container_symbol) = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.selection_range == container.range)
    else {
        return find_name_in_uri_after_line(
            idx,
            name,
            container.uri.as_str(),
            container.range.start.line,
            None,
        );
    };

    let contained: Vec<Location> = file_data
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.name == name
                && symbol.range != container_symbol.range
                && range_encloses(container_symbol.range, symbol.range)
        })
        .map(|found| Location {
            uri: container.uri.clone(),
            range: found.selection_range,
        })
        .collect();
    if !contained.is_empty() {
        return contained;
    }

    // Degenerate-range (JAR stub) container: match every same-named symbol
    // whose own `container` field (the sidecar's recorded immediate enclosing
    // class, e.g. "Assert") names this container's bare symbol name — the
    // one linkage JAR-derived symbols DO carry, in place of a real enclosing
    // range.
    let by_container: Vec<Location> = file_data
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.name == name
                && symbol.container.as_deref() == Some(container_symbol.name.as_str())
        })
        .map(|found| Location {
            uri: container.uri.clone(),
            range: found.selection_range,
        })
        .collect();
    if !by_container.is_empty() {
        return by_container;
    }

    find_name_in_uri_after_line(
        idx,
        name,
        container.uri.as_str(),
        container.range.start.line,
        Some(container_symbol.name.as_str()),
    )
}

/// Like `find_declaration_range_in_lines` but only searches from `start_line`.
pub(crate) fn find_declaration_range_after_line(
    lines: &[String],
    name: &str,
    start_line: u32,
) -> Option<tower_lsp::lsp_types::Range> {
    use tower_lsp::lsp_types::{Position, Range};
    let start = start_line as usize;
    if start >= lines.len() {
        return None;
    }
    lines[start..].find_declaration_range(name).map(|r| Range {
        start: Position {
            line: r.start.line + start_line,
            character: r.start.character,
        },
        end: Position {
            line: r.end.line + start_line,
            character: r.end.character,
        },
    })
}

///
/// Returns the location of `name:` in the current file.  This catches function
/// parameters that lack `val`/`var` and are therefore absent from the symbol index.
pub(crate) fn find_local_declaration(idx: &Indexer, name: &str, uri: &Url) -> Vec<Location> {
    // Prefer live_lines (unsaved buffer) so newly-typed params are found immediately.
    let lines: Arc<Vec<String>> = if let Some(ll) = idx.live_lines.get(uri.as_str()) {
        ll.clone()
    } else if let Some(data) = idx.files.get(uri.as_str()) {
        data.lines.clone()
    } else {
        return vec![];
    };
    if let Some(range) = lines.find_declaration_range(name) {
        return vec![Location {
            uri: uri.clone(),
            range,
        }];
    }
    vec![]
}

// ─── impl Indexer wrappers ────────────────────────────────────────────────────

impl crate::indexer::Indexer {
    pub(crate) fn find_name_in_uri(&self, name: &str, file_uri: &str) -> Vec<Location> {
        find_name_in_uri(self, name, file_uri)
    }
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
