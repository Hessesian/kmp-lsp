//! Extension-function-in-scope lookup and the implicit-receiver callee ladder
//! (member vs. extension, arity-filtered).

use std::sync::Arc;

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
    select_extension_symbol_index(file_data, name, receiver_base, container, detail)
        .map(|index| &file_data.symbols[index])
}

/// [`select_extension_symbol`]'s answer as an index into `file_data.symbols`,
/// for callers that keep the file alive instead of cloning the symbol.
fn select_extension_symbol_index(
    file_data: &FileData,
    name: &str,
    receiver_base: &str,
    container: Option<&String>,
    detail: &str,
) -> Option<usize> {
    let is_declaring_symbol = |symbol: &&SymbolEntry| {
        crate::resolver::infer::extension_declaration_matches(
            symbol,
            name,
            receiver_base,
            container,
        )
    };
    // Two non-allocating passes instead of collecting every shape-matching
    // declaration into a `Vec` first: this is called once per registry entry
    // by `implicit_receiver_extension_match`'s own per-entry loop, so an
    // allocation and a full re-scan of every overload on every entry (the
    // `Vec` version's cost even when the very first declaration is an exact
    // match) is real, measurable overhead the old single-entry-point
    // `.find(extension_declaration_matches)` never paid.
    let is_declaring_index = |index: &usize| is_declaring_symbol(&&file_data.symbols[*index]);
    (0..file_data.symbols.len())
        .find(|index| is_declaring_index(index) && file_data.symbols[*index].detail == detail)
        .or_else(|| (0..file_data.symbols.len()).find(is_declaring_index))
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
    extension_declarations_in_scope(indexer, receiver_base, name, from_uri)
        .into_iter()
        .map(|declaration| {
            let range = declaration
                .symbol()
                .map(|symbol| symbol.selection_range)
                .unwrap_or_default();
            Location {
                uri: declaration.uri,
                range,
            }
        })
        .collect()
}

/// One in-scope extension registry entry for `receiver_base.name`, with its
/// declaring symbol when the declaring file is loaded.
pub(crate) struct ExtensionDeclaration {
    pub(crate) uri: Url,
    declaring: Option<DeclaringSymbol>,
}

impl ExtensionDeclaration {
    pub(crate) fn symbol(&self) -> Option<&SymbolEntry> {
        let declaring = self.declaring.as_ref()?;
        Some(&declaring.file.symbols[declaring.index])
    }

    /// Whether the declaring file is a compiled-JAR (sidecar) index, whose
    /// signatures carry no default-value markers.
    pub(crate) fn is_compiled_jar(&self) -> bool {
        self.declaring
            .as_ref()
            .is_some_and(|declaring| declaring.is_compiled_jar)
    }

    /// The declaring file's source lines, when it is loaded.
    pub(crate) fn lines(&self) -> Option<&[String]> {
        Some(&self.declaring.as_ref()?.file.lines)
    }
}

/// A symbol held as its loaded file plus an index, so a match across the
/// supertype walk costs an `Arc` bump rather than a `SymbolEntry` clone.
struct DeclaringSymbol {
    file: Arc<FileData>,
    index: usize,
    is_compiled_jar: bool,
}

/// The registry walk behind [`resolve_extension_in_scope`]: every in-scope
/// extension `name` declared on exactly `receiver_base`.
fn extension_declarations_in_scope(
    indexer: &Indexer,
    receiver_base: &str,
    name: &str,
    from_uri: &Url,
) -> Vec<ExtensionDeclaration> {
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
        let declaring_file = match indexer.files.get(&entry.file_uri) {
            Some(file_data) => Some((file_data, false)),
            None => indexer
                .jar_files
                .get(&entry.file_uri)
                .map(|file_data| (file_data, true)),
        };
        let declaring = declaring_file.and_then(|(file_data, is_compiled_jar)| {
            let index = select_extension_symbol_index(
                &file_data,
                name,
                receiver_base,
                entry.container.as_ref(),
                &entry.detail,
            )?;
            Some(DeclaringSymbol {
                file: Arc::clone(&file_data),
                index,
                is_compiled_jar,
            })
        });
        matches.push(ExtensionDeclaration { uri, declaring });
    }
    matches
}

/// The in-scope extension declarations `receiver.name(...)` can bind to, by
/// Kotlin's most-specific-receiver rule: those declared on `receiver_type`'s
/// own type, else on its nearest supertype level (breadth-first, so
/// `List<X>.mapIndexed` reaches `Iterable<T>.mapIndexed` and never a
/// same-named `Array`/`CharSequence` overload). Empty when no extension
/// applies — a member, or an unindexed receiver hierarchy.
pub(crate) fn extension_declarations_on(
    indexer: &Indexer,
    receiver_type: &str,
    name: &str,
    from_uri: &Url,
    shape: CallShape,
) -> Vec<ExtensionDeclaration> {
    use crate::StrExt;

    let receiver_base = receiver_type
        .dotted_ident_prefix()
        .last_segment()
        .to_owned();
    let own = extension_declarations_in_scope(indexer, &receiver_base, name, from_uri);
    let own_declaration_takes_the_call = own.iter().any(|declaration| {
        crate::indexer::declaration_signature(declaration).is_none_or(|(params_text, counts)| {
            crate::indexer::candidate_applies_to_call(&params_text, counts, shape)
        })
    });
    if own_declaration_takes_the_call {
        return own;
    }
    let caller = crate::types::CallerContext {
        uri: Some(from_uri.as_str()),
        cursor_line: None,
    };
    super::infer::reachable_class_candidates(indexer, &receiver_base, Some(from_uri))
        .into_iter()
        .take(crate::indexer::MAX_BY_NAME_DEFS)
        .map(|class_location| {
            super::hierarchy::walk_hierarchy_breadth_first(
                indexer,
                &receiver_base,
                class_location.uri.as_str(),
                caller,
                12,
                super::MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
                |indexer, super_name, _, _| {
                    extension_declarations_in_scope(indexer, super_name, name, from_uri)
                },
            )
        })
        .find(|declarations| !declarations.is_empty())
        .unwrap_or(own)
}

/// The type parameters of the member `name` that a call of `shape` on
/// `receiver_type` binds: declared on the receiver's class as `from_uri`
/// reaches it, else on the nearest supertype that declares one. A member
/// then wins over any same-named extension.
pub(crate) fn member_type_parameters_taking_call(
    indexer: &Indexer,
    receiver_type: &str,
    name: &str,
    from_uri: &Url,
    shape: CallShape,
) -> Option<crate::indexer::MemberTypeParameters> {
    use crate::StrExt;

    let receiver_base = receiver_type
        .dotted_ident_prefix()
        .last_segment()
        .to_owned();
    let caller = crate::types::CallerContext {
        uri: Some(from_uri.as_str()),
        cursor_line: None,
    };
    super::infer::reachable_class_candidates(indexer, &receiver_base, Some(from_uri))
        .into_iter()
        .take(crate::indexer::MAX_BY_NAME_DEFS)
        .find_map(|class_location| {
            let class_uri = class_location.uri.as_str();
            let declared_on_class = indexer.files.get(class_uri).and_then(|file_data| {
                crate::indexer::members_taking_call_in_file(&file_data, &receiver_base, name, shape)
                    .into_iter()
                    .next()
            });
            declared_on_class.or_else(|| {
                super::hierarchy::walk_hierarchy_breadth_first(
                    indexer,
                    &receiver_base,
                    class_uri,
                    caller,
                    12,
                    super::MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
                    |indexer, super_name, super_uri, _| {
                        indexer
                            .files
                            .get(super_uri)
                            .map_or_else(Vec::new, |file_data| {
                                crate::indexer::members_taking_call_in_file(
                                    &file_data, super_name, name, shape,
                                )
                            })
                    },
                )
                .into_iter()
                .next()
            })
        })
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
    resolve_receiver_callee_in_type_scope(indexer, receiver_base, name, from_uri, from_uri, shape)
}

/// [`resolve_implicit_receiver_callee`] for a receiver whose TYPE NAME was
/// written in `type_scope` rather than in the caller (`from_uri`) — an inferred
/// receiver. `type_scope` resolves `receiver_base` to its declaring file;
/// which extensions are in scope stays a property of the caller.
pub(crate) fn resolve_receiver_callee_in_type_scope(
    indexer: &Indexer,
    receiver_base: &str,
    name: &str,
    type_scope: &Url,
    from_uri: &Url,
    shape: CallShape,
) -> Vec<Location> {
    if let Some(loc) =
        implicit_receiver_extension_match(indexer, receiver_base, name, from_uri, shape)
    {
        return vec![loc];
    }
    implicit_receiver_member_match(indexer, receiver_base, name, type_scope, shape)
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
    type_scope: &Url,
    shape: CallShape,
) -> Option<Location> {
    for type_loc in resolve_symbol(indexer, receiver_base, None, type_scope) {
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
