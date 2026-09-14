//! Extension-function-in-scope lookup and the implicit-receiver callee ladder
//! (member vs. extension, arity-filtered).

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::{CallShape, Indexer};
use crate::types::FileData;

use super::resolve::resolve_symbol;

// ─── step implementations ────────────────────────────────────────────────────

/// Look up an extension function by receiver base name, filtering by scope
/// (same package or explicitly imported in the caller's file).
///
/// Checks `extension_by_receiver` for matching entries, then verifies each
/// candidate is visible from `from_uri` by checking same-package or import
/// coverage. Returns the first matching extension's `Location` with an accurate
/// `selection_range`, or an empty `Vec` if none is in scope.
pub(super) fn resolve_extension_in_scope(
    indexer: &Indexer,
    receiver_base: &str,
    name: &str,
    from_uri: &Url,
) -> Vec<Location> {
    // Atomic promote+read (zero budget): this helper serves goto-definition
    // AND the per-call-site diagnostics path (`resolve_member`), so blocking
    // sidecar IPC is forbidden here — cache-backed promotions are still free.
    let mut cache_backed_only = 0usize;
    let Some(entries) =
        crate::indexer::jar::extension_entries_for(indexer, receiver_base, &mut cache_backed_only)
    else {
        return vec![];
    };
    let caller_file_data = indexer.files.get(from_uri.as_str());
    let caller_file_data_ref: Option<&FileData> = caller_file_data.as_deref().map(|v| v.as_ref());
    for entry in entries.iter() {
        if entry.name != name {
            continue;
        }
        let in_scope = crate::resolver::infer::extension_is_in_scope(
            entry.package.as_ref(),
            &entry.name,
            entry.container.as_ref(),
            entry.visibility,
            entry.file_uri == from_uri.as_str(),
            caller_file_data_ref,
        );
        if in_scope {
            if let Ok(uri) = Url::parse(&entry.file_uri) {
                let range = indexer
                    .files
                    .get(&entry.file_uri)
                    .or_else(|| indexer.jar_files.get(&entry.file_uri))
                    .and_then(|fd| {
                        fd.symbols
                            .iter()
                            .find(|s| {
                                crate::resolver::infer::extension_declaration_matches(
                                    s,
                                    name,
                                    receiver_base,
                                    entry.container.as_ref(),
                                )
                            })
                            .map(|s| s.selection_range)
                    })
                    .unwrap_or_default();
                return vec![Location { uri, range }];
            }
        }
    }
    vec![]
}

/// Resolve `name(...)` as an implicit `this.name(...)` against `receiver_base`
/// — the enclosing extension function's own declared receiver type (see
/// `parser::enclosing_extension_receiver_at`) — tried only when nothing else
/// resolves `name` by plain bare-name search (imports/same-package/star/
/// hierarchy/rg have no receiver-type awareness at all, so a bare call inside
/// an extension function's own body that targets a same-named member/
/// extension of that receiver is invisible to every one of them).
///
/// Mirrors `resolve_qualified`'s member-vs-extension precedence for
/// `TypeName.member`, but shape-filters both halves: `resolve_extension_in_scope`
/// has no arity awareness of its own, and the enclosing declaration itself is
/// one of its own registered "extensions in scope" (same file) — without
/// filtering, this would just resurrect the self-shadow bug through a new path.
pub(crate) fn resolve_implicit_receiver_callee(
    indexer: &Indexer,
    receiver_base: &str,
    name: &str,
    from_uri: &Url,
    shape: CallShape,
) -> Vec<Location> {
    if let Some(loc) =
        implicit_receiver_extension_match(indexer, receiver_base, name, from_uri, shape)
    {
        return vec![loc];
    }
    implicit_receiver_member_match(indexer, receiver_base, name, from_uri, shape)
        .map(|loc| vec![loc])
        .unwrap_or_default()
}

/// The extension-in-scope half of [`resolve_implicit_receiver_callee`] — same
/// registry and in-scope check as `resolve_extension_in_scope`, plus a
/// `shape.accepts(...)` gate on each candidate's own declared arity (vararg
/// declarations are exempt: `param_counts` can't represent a vararg's true
/// unbounded upper end). Deliberately not `CallShape::accepts_symbol` — that
/// also exempts non-callable *kinds*, which would let a same-named property
/// wrongly satisfy any shape here; this is a selection loop picking the one
/// real candidate, not a rejection filter over an already-narrowed list.
fn implicit_receiver_extension_match(
    indexer: &Indexer,
    receiver_base: &str,
    name: &str,
    from_uri: &Url,
    shape: CallShape,
) -> Option<Location> {
    let mut cache_backed_only = 0usize;
    let entries =
        crate::indexer::jar::extension_entries_for(indexer, receiver_base, &mut cache_backed_only)?;
    let caller_file_data = indexer.files.get(from_uri.as_str());
    let caller_file_data_ref: Option<&FileData> = caller_file_data.as_deref().map(|v| v.as_ref());
    for entry in entries.iter() {
        if entry.name != name {
            continue;
        }
        let in_scope = crate::resolver::infer::extension_is_in_scope(
            entry.package.as_ref(),
            &entry.name,
            entry.container.as_ref(),
            entry.visibility,
            entry.file_uri == from_uri.as_str(),
            caller_file_data_ref,
        );
        if !in_scope {
            continue;
        }
        let Ok(uri) = Url::parse(&entry.file_uri) else {
            continue;
        };
        let symbol = indexer
            .files
            .get(&entry.file_uri)
            .or_else(|| indexer.jar_files.get(&entry.file_uri))
            .and_then(|fd| {
                fd.symbols
                    .iter()
                    .find(|s| {
                        crate::resolver::infer::extension_declaration_matches(
                            s,
                            name,
                            receiver_base,
                            entry.container.as_ref(),
                        )
                    })
                    .cloned()
            });
        let Some(symbol) = symbol else { continue };
        let is_vararg = symbol.params.contains("vararg ") || symbol.params.contains("vararg\t");
        if is_vararg || shape.accepts(symbol.param_counts.0, symbol.param_counts.1) {
            return Some(Location {
                uri,
                range: symbol.selection_range,
            });
        }
    }
    None
}

/// The member half of [`resolve_implicit_receiver_callee`] — resolves
/// `receiver_base` to its declaring file (import-aware, via the same
/// `resolve_symbol` the explicit-qualifier path already uses — this is what
/// makes a compiled-JAR-only receiver type work), then scans *every*
/// same-named symbol declared there (not just the first, unlike
/// `find_name_in_uri_after_line`) for one whose arity `shape` accepts.
fn implicit_receiver_member_match(
    indexer: &Indexer,
    receiver_base: &str,
    name: &str,
    from_uri: &Url,
    shape: CallShape,
) -> Option<Location> {
    for type_loc in resolve_symbol(indexer, receiver_base, None, from_uri) {
        let Some(symbol) = indexer
            .files
            .get(type_loc.uri.as_str())
            .or_else(|| indexer.jar_files.get(type_loc.uri.as_str()))
            .and_then(|fd| {
                fd.symbols
                    .iter()
                    .find(|s| {
                        s.name == name
                            && (s.params.contains("vararg ")
                                || s.params.contains("vararg\t")
                                || shape.accepts(s.param_counts.0, s.param_counts.1))
                    })
                    .cloned()
            })
        else {
            continue;
        };
        return Some(Location {
            uri: type_loc.uri,
            range: symbol.selection_range,
        });
    }
    None
}
