//! Extension-function-in-scope lookup and the implicit-receiver callee ladder
//! (member vs. extension, arity-filtered).

use tower_lsp::lsp_types::{Location, Range, Url};

use crate::indexer::{CallShape, Indexer};
use crate::types::{FileData, SymbolEntry};

use super::resolve::resolve_symbol;

// ─── step implementations ────────────────────────────────────────────────────

/// Select the declaring symbol for one extension registry `entry` inside its
/// declaring file's already-parsed symbol table.
///
/// Shared by [`resolve_extension_in_scope`],
/// [`super::qualified::jar_extension_for_type_root`], and
/// `implicit_receiver_extension_match`: all now consider every same-named
/// registry entry rather than just the first, so all three need this exact
/// two-step selection — a file may declare several overloads of the same
/// extension (same name, same receiver, same container), and
/// `extension_declaration_matches` alone can't tell them apart. Prefer the
/// declaring symbol whose `detail` (full signature text) exactly matches this
/// entry's own `detail`; when nothing matches exactly (e.g. the registry
/// entry's detail was computed slightly differently than the symbol table's,
/// or the file has since drifted), fall back to the first declaration with a
/// matching name/receiver/container shape rather than returning nothing at
/// all.
pub(super) fn select_extension_symbol<'file_data>(
    file_data: &'file_data FileData,
    name: &str,
    receiver_base: &str,
    container: Option<&String>,
    detail: &str,
) -> Option<&'file_data SymbolEntry> {
    let declaring_symbols: Vec<_> = file_data
        .symbols
        .iter()
        .filter(|symbol| {
            crate::resolver::infer::extension_declaration_matches(
                symbol,
                name,
                receiver_base,
                container,
            )
        })
        .collect();
    let exact_signature_match = declaring_symbols
        .iter()
        .find(|symbol| symbol.detail == detail);
    exact_signature_match
        .or_else(|| declaring_symbols.first())
        .copied()
}

/// The `Range` half of [`select_extension_symbol`], for callers that only
/// need the location and not the rest of the symbol (params/arity).
pub(super) fn select_extension_symbol_range(
    file_data: &FileData,
    name: &str,
    receiver_base: &str,
    container: Option<&String>,
    detail: &str,
) -> Range {
    select_extension_symbol(file_data, name, receiver_base, container, detail)
        .map(|symbol| symbol.selection_range)
        .unwrap_or_default()
}

/// Look up every in-scope extension function matching `receiver_base`/`name`
/// (same package or explicitly imported in the caller's file).
///
/// Checks `extension_by_receiver` for matching entries, then verifies each
/// candidate is visible from `from_uri` by checking same-package or import
/// coverage. Returns every in-scope match with an accurate `selection_range`,
/// not just the first — `name` may be overloaded (e.g. stdlib's
/// `firstOrNull()` vs. `firstOrNull(predicate)`), and collapsing to one
/// arbitrary overload here, before the caller's own arity-based shape
/// filtering ever runs, silently drops every real call site to a DIFFERENT
/// overload (same reasoning as `member_tiers`'s own doc comment on this same
/// file-local principle, and the PR #304 precedent for qualified member
/// lookups).
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
    let mut matches = Vec::new();
    for entry in entries.iter() {
        if entry.name != name {
            continue;
        }
        if !crate::resolver::infer::extension_entry_is_in_scope(
            entry,
            from_uri,
            caller_file_data_ref,
        ) {
            continue;
        }
        let Ok(uri) = Url::parse(&entry.file_uri) else {
            continue;
        };
        let range = indexer
            .files
            .get(&entry.file_uri)
            .or_else(|| indexer.jar_files.get(&entry.file_uri))
            .map(|fd| {
                select_extension_symbol_range(
                    &fd,
                    name,
                    receiver_base,
                    entry.container.as_ref(),
                    &entry.detail,
                )
            })
            .unwrap_or_default();
        matches.push(Location { uri, range });
    }
    matches
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
        if !crate::resolver::infer::extension_entry_is_in_scope(
            entry,
            from_uri,
            caller_file_data_ref,
        ) {
            continue;
        }
        let Ok(uri) = Url::parse(&entry.file_uri) else {
            continue;
        };
        let symbol = indexer
            .files
            .get(&entry.file_uri)
            .or_else(|| indexer.jar_files.get(&entry.file_uri))
            .and_then(|file_data| {
                select_extension_symbol(
                    &file_data,
                    name,
                    receiver_base,
                    entry.container.as_ref(),
                    &entry.detail,
                )
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
