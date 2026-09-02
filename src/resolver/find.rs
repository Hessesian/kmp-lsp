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

/// Like `find_name_in_uri` but prefers declarations at or after `after_line`.
///
/// Used when we already know the qualifier class lives at `after_line` — we
/// want the parameter/field of THAT class, not a same-named field in a
/// different class that happens to appear earlier in the same file.
///
/// Strategy:
///   1. Symbol table — pick the symbol at or after `after_line` with the
///      smallest line number (closest match).  Fall back to any match if none
///      found after the hint line.
///   2. Line scan — search only lines >= `after_line`.
///
/// Loads `FileData` via `ensure_file_data`, which checks the in-memory
/// index (files + jar_files) and falls back to on-demand disk parse.
pub(crate) fn find_name_in_uri_after_line(
    idx: &Indexer,
    name: &str,
    file_uri: &str,
    after_line: u32,
) -> Vec<Location> {
    let Ok(uri) = Url::parse(file_uri) else {
        return vec![];
    };

    let Some(file_data) = ensure_file_data(idx, &uri) else {
        return vec![];
    };

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

    let contained = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.selection_range == container.range)
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
/// Only widens the primary range-containment path — the degenerate-range
/// (JAR stub) fallback still returns at most one candidate, same as before;
/// broadening overload support there needs sidecar-side changes (the JAR
/// indexer would need to preserve each overload as a distinct symbol
/// candidate), a separate, larger effort not needed for the real-world
/// pure-Java-source case this was measured against.
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

    find_name_in_uri_after_line(
        idx,
        name,
        container.uri.as_str(),
        container.range.start.line,
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
