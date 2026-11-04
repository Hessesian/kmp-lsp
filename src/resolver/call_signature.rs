//! Which declaration a call binds to, and its parameter signature: resolves
//! the callee through name resolution, then reads the chosen declaration.

use tower_lsp::lsp_types::{SymbolKind, Url};

use super::{infer_receiver_type, ReceiverKind};
use crate::indexer::infer::sig::*;
use crate::indexer::infer::Resolution;
use crate::indexer::Indexer;
use crate::types::SourceSet;

/// Everything needed to resolve a call expression's signature.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallSite<'a> {
    /// The bare function / constructor name (without qualifier).
    pub name: &'a str,
    /// The receiver variable name, if any (`foo.bar()` → `Some("foo")`).
    pub qualifier: Option<&'a str>,
    /// URI of the file containing the call.
    pub caller_uri: &'a Url,
    /// The call's actual argument shape, when the caller has one — used by
    /// [`resolve_unqualified`] to notice when its same-file-first shortcut
    /// picked a candidate the call's own arity can't satisfy (a same-named,
    /// wrong-arity same-file declaration is a name collision, e.g. a
    /// same-named enclosing extension function calling a differently-shaped
    /// library function of the same name — not necessarily the call's real
    /// target). `None` preserves the unfiltered same-file-first behavior
    /// exactly, for callers with no shape to offer.
    pub shape: Option<crate::indexer::infer::deps::CallShape>,
}

/// A resolved call-site signature — exactly one arity envelope, safe to emit
/// a diagnostic against. The `Resolution::Resolved` payload for
/// `resolve_call_signature`.
#[derive(Debug, Clone)]
pub(crate) struct Signature {
    pub(crate) params_text: String,
    pub(crate) param_counts: (usize, usize),
}

// ─── Multiline signature collector ───────────────────────────────────────────

/// Signature lookup with optional dot-receiver context.
///
/// When `receiver` is given (e.g. `"oneYearOlderInteractor"`), resolves its
/// type, finds that type's file, and looks up `name` there specifically.
/// Returns `None` if the receiver type cannot be resolved — no fallback to
/// plain name-based search, to avoid returning a completely unrelated overload.
///
/// This is the free-function form of the former `Indexer::find_fun_signature_with_receiver`
/// method.  `backend.rs` calls this directly.
pub(crate) fn find_fun_signature_with_receiver(
    idx: &Indexer,
    uri: &Url,
    name: &str,
    receiver: Option<&str>,
) -> Option<String> {
    if let Some(receiver) = receiver {
        let Some(receiver_type) = infer_receiver_type(idx, ReceiverKind::Variable(receiver), uri)
        else {
            // Receiver present but type could not be resolved — avoid a global
            // name-only scan that may return a completely unrelated function.
            return None;
        };
        let locs = idx.resolve_symbol(&receiver_type.outer, None, uri);
        for loc in &locs {
            if let Some(data) = idx.files.get(loc.uri.as_str()) {
                if let Some(sig) = collect_fun_params_text(name, loc.uri.as_str(), idx) {
                    return Some(sig);
                }
                // Also search by line range within the type's body.
                let type_end = data
                    .symbols
                    .iter()
                    .find(|s| s.name == receiver_type.outer)
                    .map(|s| s.range.end.line)
                    .unwrap_or(u32::MAX);
                for symbol_entry in data
                    .symbols
                    .iter()
                    .filter(|s| s.name == name && s.range.start.line <= type_end)
                {
                    if !symbol_entry.params.is_empty() {
                        return Some(symbol_entry.params.clone());
                    }
                    if let Some(params) = extract_params_from_detail(&symbol_entry.detail) {
                        return Some(params);
                    }
                }
            }
        }
        return None;
    }
    find_fun_signature_full(name, idx, uri)
}

/// A function declared directly on a workspace class.
struct MemberDeclaration {
    params_text: String,
    counts: (u8, u8),
}

/// The functions `method_name` declared directly on the workspace class
/// `class_name` (no extensions, no supertypes; JAR classes are not scanned).
fn class_members(indexer: &Indexer, class_name: &str, method_name: &str) -> Vec<MemberDeclaration> {
    let (container, type_base) = match class_name.rsplit_once('.') {
        Some((container, base)) => (Some(container), base),
        None => (None, class_name),
    };
    let mut members: Vec<MemberDeclaration> = Vec::new();

    if let Some(locations) = indexer.definitions.get(type_base) {
        for location in locations.iter() {
            let Some(url) = indexer.file_table.url(location.file) else {
                continue;
            };
            let Some(file_data) = indexer.files.get(url.as_str()) else {
                continue;
            };
            // Verify the class exists (and if qualified, check its own container).
            let has_class = file_data.symbols.iter().any(|s| {
                s.name == type_base
                    && is_class_like(s.kind)
                    && container.is_none_or(|c| s.container.as_deref() == Some(c))
            });
            if !has_class {
                continue;
            }

            // The method as a direct member of type_base.
            for symbol_entry in &file_data.symbols {
                if symbol_entry.name != method_name {
                    continue;
                }
                if !matches!(
                    symbol_entry.kind,
                    SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::OPERATOR
                ) {
                    continue;
                }
                if symbol_entry.container.as_deref() != Some(type_base) {
                    continue;
                }
                if let Some(params_text) =
                    extract_params_or_fallback(symbol_entry, &file_data.lines)
                {
                    members.push(MemberDeclaration {
                        params_text,
                        counts: symbol_entry.param_counts,
                    });
                }
            }
        }
    }
    members
}

/// The members `method_name` that `file_data` declares directly on its class
/// `class_name` and that can take a call of `shape`, each with the type
/// parameters a return type of theirs may mention.
pub(crate) fn members_taking_call_in_file(
    file_data: &crate::types::FileData,
    class_name: &str,
    method_name: &str,
    shape: crate::indexer::infer::deps::CallShape,
) -> Vec<crate::indexer::infer::deps::MemberTypeParameters> {
    let owner_class = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.name == class_name && is_class_like(symbol.kind))
        .map(|owner| owner.type_params().to_vec())
        .unwrap_or_default();
    file_data
        .symbols
        .iter()
        .filter(|symbol| symbol.name == method_name)
        .filter(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::OPERATOR
            )
        })
        .filter(|symbol| symbol.container.as_deref() == Some(class_name))
        .filter(|symbol| {
            extract_params_or_fallback(symbol, &file_data.lines).is_some_and(|params_text| {
                candidate_applies_to_call(&params_text, symbol.param_counts, shape)
            })
        })
        .map(|symbol| crate::indexer::infer::deps::MemberTypeParameters {
            function: symbol.type_params().to_vec(),
            owner_class: owner_class.clone(),
        })
        .collect()
}

/// Receiver-aware params lookup: find `method_name`'s parameter text inside
/// the class `class_name`.  Uses range containment to avoid picking a method
/// from an unrelated class in the same file.
///
/// Supports dotted names like `"DepositAccountReducer.Factory"` — splits into
/// container `"DepositAccountReducer"` and type_base `"Factory"`, then filters
/// definitions to only those whose container matches.
///
/// Searches both members (`container == type_base`) and extension functions on
/// the receiver (`extension_by_receiver`, member-less by definition) — a
/// receiver type routinely has both, e.g. `Flow.collect(collector:
/// FlowCollector<T>)` (member) alongside `Flow<T>.collect(action: suspend (T)
/// -> Unit)` (extension), and only the caller's actual `shape` (arg count,
/// trailing lambda) tells them apart; see [`CallShape`].
///
/// Deliberately gathers members by scanning files that declare `type_base`
/// (bounded by how many classes share that name) rather than by scanning every
/// file that declares a method named `method_name` — the latter is what makes
/// `resolve_qualified` bail on ubiquitous names like `collect` (hundreds of
/// unrelated declarations); this function has no such bail, so it must not
/// take that path. The extension half is similarly cheap: `extension_by_receiver`
/// is keyed by receiver type, not by method name.
pub(crate) fn find_method_params_in_class(
    indexer: &Indexer,
    class_name: &str,
    method_name: &str,
    caller_uri: &Url,
    shape: crate::indexer::infer::deps::CallShape,
) -> Option<String> {
    let type_base = class_name
        .rsplit_once('.')
        .map_or(class_name, |(_, base)| base);
    let mut candidates: Vec<(String, (u8, u8))> = class_members(indexer, class_name, method_name)
        .into_iter()
        .map(|member| (member.params_text, member.counts))
        .collect();

    let mut cache_backed_only = 0usize;
    if let Some(entries) =
        crate::indexer::jar::extension_entries_for(indexer, type_base, &mut cache_backed_only)
    {
        let mut seen_files = std::collections::HashSet::new();
        for entry in entries.iter().filter(|e| e.name == method_name) {
            if seen_files.insert(entry.file_uri.clone()) {
                candidates.extend(collect_params_from_file(
                    method_name,
                    &entry.file_uri,
                    indexer,
                    caller_uri.as_str(),
                    ResolutionScope::CrossFile,
                    Some(type_base),
                ));
            }
        }
    }
    let any_direct_candidate_applies = candidates
        .iter()
        .any(|(text, counts)| candidate_applies_to_call(text, *counts, shape));
    if !any_direct_candidate_applies {
        candidates.extend(inherited_extension_params(
            indexer,
            type_base,
            method_name,
            caller_uri,
            shape,
        ));
    }

    pick_candidate_matching_shape(candidates, shape)
}

/// Parameter texts of the extension `method_name` declared on `type_base`'s
/// nearest supertype — `List<X>.joinToString` binds `Iterable<T>.joinToString`.
fn inherited_extension_params(
    indexer: &Indexer,
    type_base: &str,
    method_name: &str,
    caller_uri: &Url,
    shape: crate::indexer::infer::deps::CallShape,
) -> Vec<(String, (u8, u8))> {
    crate::resolver::extension_declarations_on(indexer, type_base, method_name, caller_uri, shape)
        .iter()
        .filter_map(declaration_signature)
        .collect()
}

/// An extension declaration's parameter text and `(required, total)` counts.
pub(crate) fn declaration_signature(
    declaration: &crate::resolver::ExtensionDeclaration,
) -> Option<(String, (u8, u8))> {
    let symbol = declaration.symbol()?;
    let params = extract_params_or_fallback(symbol, declaration.lines()?)?;
    Some((
        params,
        trusted_param_counts(symbol, declaration.is_compiled_jar()),
    ))
}

/// Number of indexed definitions (workspace + JAR) for a call name.
///
/// A ubiquitous name (`create`, `loadData`, …) resolves to hundreds of source-JAR
/// overloads once source-JARs are indexed; the call-argument diagnostics path uses
/// this to bail before a multi-second cross-file scan. Cheap — just two map lookups
/// and the slice lengths, no cloning.
fn total_definition_count(call_name: &str, idx: &Indexer) -> usize {
    let workspace = idx
        .definitions
        .get(call_name)
        .map(|locations| locations.len())
        .unwrap_or(0);
    let jar = idx
        .jar_definitions
        .get(call_name)
        .map(|locations| locations.len())
        .unwrap_or(0);
    workspace + jar
}

/// Resolve the signature for a qualified call `qualifier.name(...)`.
///
/// Uses the receiver type to find the correct function definition:
/// - Infers the declared type of the qualifier variable
/// - Searches `definitions` and `extension_by_receiver` for matching
///   member (container == type) or extension (extension_receiver == type)
///   symbols, filtered by import reachability
/// - Deduplicates by arity envelope and returns `Resolution::Ambiguous` when ambiguous
fn resolve_qualified(call: &CallSite<'_>, qualifier: &str, idx: &Indexer) -> Resolution<Signature> {
    let receiver =
        match infer_receiver_type(idx, ReceiverKind::Variable(qualifier), call.caller_uri) {
            Some(receiver) => receiver,
            None => return Resolution::Unresolved,
        };
    // container and extension_receiver store simple class names (no package),
    // and extension_by_receiver is keyed by simple name — so leaf is correct.
    let receiver_base = &receiver.leaf;
    let caller_uri = call.caller_uri.as_str();

    // Bail for a ubiquitous name (hundreds of source-JAR overloads of `create`,
    // `loadData`, …) instead of scanning every definition's file — a multi-second
    // stall. This must bail the *whole* resolution, not just Phase 1: members win
    // over extensions in Kotlin, so if Phase 1 were skipped while Phase 2's
    // receiver-scoped extension lookup still ran, it could return a `Unique`
    // signature for an extension while ignoring a same-named member of a different
    // arity that Phase 1 would have found — a false param-count diagnostic.
    // Promote-before-read (zero budget): param-count diagnostics run per call
    // site — no blocking sidecar IPC here. Promoting before the ubiquity count
    // also keeps that count accurate for Tier-1-only cache-backed JARs.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(idx, call.name, &mut cache_backed_only);

    if total_definition_count(call.name, idx) > crate::indexer::MAX_BY_NAME_DEFS {
        // known-ambiguous (ubiquitous name) — see `total_definition_count`.
        return Resolution::Ambiguous;
    }

    let mut found: Vec<(String, (u8, u8))> = Vec::new();

    // Phase 1: definitions + jar_definitions — source and JAR symbols.
    {
        let mut locations: Vec<tower_lsp::lsp_types::Location> = Vec::new();
        if let Some(locs) = idx.definitions.get(call.name) {
            // Reconstitute interned `SymbolLoc`s at this boundary.
            locations.extend(
                locs.iter()
                    .filter_map(|sym_loc| idx.file_table.location(*sym_loc)),
            );
        }
        if let Some(locs) = idx.jar_definitions.get(call.name) {
            locations.extend(locs.iter().cloned());
        }
        for location in &locations {
            found.extend(collect_params_from_file(
                call.name,
                location.uri.as_str(),
                idx,
                caller_uri,
                ResolutionScope::CrossFile,
                Some(receiver_base),
            ));
        }
    }

    // Phase 2: extension_by_receiver for JAR-only extensions.
    // Always runs — a JAR extension with different arity is an overload,
    // not a replacement. Skipping it would pick the wrong arity from Phase 1.
    {
        // Atomic promote+read (zero budget) — same policy as Phase 1 above.
        let mut cache_backed_only = 0usize;
        if let Some(entries) =
            crate::indexer::jar::extension_entries_for(idx, receiver_base, &mut cache_backed_only)
        {
            for entry in entries.iter() {
                if entry.name != call.name {
                    continue;
                }
                found.extend(collect_params_from_file(
                    call.name,
                    &entry.file_uri,
                    idx,
                    caller_uri,
                    ResolutionScope::CrossFile,
                    Some(receiver_base),
                ));
            }
        }
    }

    build_result(found)
}

/// What the same-file scan for `call.name` settles about resolution, before
/// any cross-file/JAR search runs.
enum SameFileVerdict {
    /// A member of the class the call is textually inside. Kotlin binds an
    /// unqualified call to the enclosing class's own member unconditionally
    /// — decisive regardless of the call's shape.
    MemberWins(Vec<(String, (u8, u8))>),
    /// A same-file top-level/extension declaration whose shape the call
    /// actually satisfies — also decisive.
    ShapeMatches(Vec<(String, (u8, u8))>),
    /// A same-file top-level/extension declaration exists, but the call's
    /// shape doesn't satisfy it (e.g. a same-named enclosing extension
    /// function calling a differently-shaped library function of the same
    /// name — a name collision, not this call's target). Not decisive on its
    /// own; carried through as `resolve_unqualified`'s last resort.
    NameCollision(Vec<(String, (u8, u8))>),
    /// No same-file declaration of this name at all.
    None,
}

fn classify_same_file(call: &CallSite<'_>, idx: &Indexer) -> SameFileVerdict {
    let same_file = collect_params_from_file(
        call.name,
        call.caller_uri.as_str(),
        idx,
        call.caller_uri.as_str(),
        ResolutionScope::SameFile,
        None,
    );
    if same_file.is_empty() {
        return SameFileVerdict::None;
    }
    let is_member = idx
        .files
        .get(call.caller_uri.as_str())
        .is_some_and(|file_data| {
            file_data
                .symbols
                .iter()
                .any(|symbol| symbol.name == call.name && symbol.container.is_some())
        });
    if is_member {
        return SameFileVerdict::MemberWins(same_file);
    }
    let matches_shape = call.shape.is_none_or(|shape| {
        same_file
            .iter()
            .any(|(_, (required, total))| shape.accepts(*required, *total))
    });
    if matches_shape {
        SameFileVerdict::ShapeMatches(same_file)
    } else {
        SameFileVerdict::NameCollision(same_file)
    }
}

/// Resolve the signature for an unqualified call `name(…)`.
///
/// Priority:
/// 1. Same-file (see [`SameFileVerdict`] for exactly when this is decisive).
/// 2. Cross-file/JAR, import-aware — see [`resolve_cross_file`].
/// 3. A same-file candidate the call's shape ruled out is still preferred
///    over nothing: if step 2 finds no candidate anywhere else either, the
///    ruled-out same-file one is, after all, the only real candidate in
///    scope, so a genuine arity typo against it is still worth flagging.
fn resolve_unqualified(call: &CallSite<'_>, idx: &Indexer) -> Resolution<Signature> {
    match classify_same_file(call, idx) {
        SameFileVerdict::MemberWins(entries) | SameFileVerdict::ShapeMatches(entries) => {
            build_result(entries)
        }
        SameFileVerdict::NameCollision(rejected) => match resolve_cross_file(call, idx) {
            Resolution::Unresolved => build_result(rejected),
            resolved_elsewhere => resolved_elsewhere,
        },
        SameFileVerdict::None => resolve_cross_file(call, idx),
    }
}

/// Search every *other* file/JAR declaring `call.name`, import/reachability
/// filtered — the caller's own file was already fully considered by
/// [`classify_same_file`], so its locations are skipped here rather than
/// re-scanned.
///
/// Bails to `Ambiguous` without scanning for names ubiquitous enough that the
/// scan itself would be a multi-second diagnostics-hot-path stall *and* the
/// result would resolve to `Ambiguous` regardless (e.g. `collect`, `create`).
fn resolve_cross_file(call: &CallSite<'_>, idx: &Indexer) -> Resolution<Signature> {
    // Promote-before-read (zero budget): param-count diagnostics run per call
    // site — no blocking sidecar IPC here. Promoting before the ubiquity count
    // also keeps that count accurate for Tier-1-only cache-backed JARs.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(idx, call.name, &mut cache_backed_only);
    if total_definition_count(call.name, idx) > crate::indexer::MAX_BY_NAME_DEFS {
        // known-ambiguous (ubiquitous name) — see `total_definition_count`.
        return Resolution::Ambiguous;
    }

    let caller_source_set = idx
        .files
        .get(call.caller_uri.as_str())
        .map(|file| file.source_set)
        .unwrap_or_default();

    let mut all: Vec<(String, (u8, u8))> = Vec::new();
    let mut locations: Vec<tower_lsp::lsp_types::Location> = Vec::new();
    if let Some(locs) = idx.definitions.get(call.name) {
        // Reconstitute interned `SymbolLoc`s at this boundary.
        locations.extend(
            locs.iter()
                .filter_map(|sym_loc| idx.file_table.location(*sym_loc)),
        );
    }
    if let Some(locs) = idx.jar_definitions.get(call.name) {
        locations.extend(locs.iter().cloned());
    }
    for loc in &locations {
        if loc.uri.as_str() == call.caller_uri.as_str() {
            continue;
        }
        let definition_source_set = idx
            .files
            .get(loc.uri.as_str())
            .or_else(|| idx.jar_files.get(loc.uri.as_str()))
            .map(|file| file.source_set)
            .unwrap_or_default();
        if definition_source_set == SourceSet::Test && caller_source_set != SourceSet::Test {
            continue;
        }
        let sigs = collect_params_from_file(
            call.name,
            loc.uri.as_str(),
            idx,
            call.caller_uri.as_str(),
            ResolutionScope::CrossFile,
            None,
        );
        all.extend(sigs);
    }
    build_result(all)
}

/// Build a `Resolution<Signature>` from a list of `(params_text, param_counts)` pairs.
///
/// Deduplicates by arity envelope. If there are multiple distinct envelopes,
/// the function is considered overloaded (`Ambiguous`) and the caller should
/// skip the diagnostic.
fn build_result(entries: Vec<(String, (u8, u8))>) -> Resolution<Signature> {
    if entries.is_empty() {
        return Resolution::Unresolved;
    }
    // Deduplicate by arity envelope.
    let mut seen: std::collections::HashSet<(u8, u8)> = std::collections::HashSet::new();
    let mut deduped: Vec<(String, (u8, u8))> = Vec::new();
    for (text, counts) in entries {
        if seen.insert(counts) {
            deduped.push((text, counts));
        }
    }
    if deduped.len() > 1 {
        return Resolution::Ambiguous;
    }
    let Some((params_text, (required, total))) = deduped.into_iter().next() else {
        return Resolution::Unresolved;
    };
    Resolution::Resolved(Signature {
        params_text,
        param_counts: (required as usize, total as usize),
    })
}

/// Resolve the call site's signature using a unified, single-entry-point pipeline.
///
/// Resolution strategy:
/// - **Qualified** (`qualifier.name(…)`): resolve receiver type, find method within it.
///   Returns `Resolution::Unresolved` when the receiver type is unknown to prevent
///   a fallback global-name scan from matching unrelated functions.
/// - **Unqualified** (`name(…)`): check the current file first, then cross-file
///   definitions filtered by import reachability and nested-class exclusion.
///
/// This function does NOT trigger on-demand indexing (`rg` / disk reads).
/// Use `find_fun_signature_full` for hover/completion where latency is acceptable.
pub(crate) fn resolve_call_signature(call: &CallSite<'_>, idx: &Indexer) -> Resolution<Signature> {
    if let Some(qualifier) = call.qualifier {
        resolve_qualified(call, qualifier, idx)
    } else {
        resolve_unqualified(call, idx)
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────
