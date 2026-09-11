//! Symbol resolution for Kotlin (and Java) with a prioritised fallback chain.
//!
//! Resolution order
//! ────────────────
//! 1. **Local file**        — symbols defined in the same file (highest priority).
//! 2. **Explicit imports**  — `import com.example.Foo` or `import com.example.Foo as F`.
//!    Tries the `qualified` index first, then the short-name index.
//! 3. **Same package**      — all symbols in files that share the same `package` declaration
//!    are visible without imports in Kotlin.
//! 4. **Star imports**      — `import com.example.*`  checks indexed files in that package,
//!    then falls back to an `rg` search scoped to the package dir.
//! 5. **Extension functions** — `fun Receiver.name(...)` is stored as a top-level symbol
//!    named `name`; steps 1–4 already pick these up. No special
//!    handling needed beyond noting that receiver type is ignored.
//! 6. **Project-wide `rg`** — pattern `(fun|class|…)\s+NAME\b` across *.kt / *.java.
//!    Last resort; always finds stdlib-shadowing project symbols.
//!
//! Stdlib packages (`kotlin.*`, `java.*`, `android.*`, `androidx.*`) are skipped because
//! their sources aren't present in the project tree.

use std::sync::Arc;

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::{CallShape, Indexer};
use crate::parser::parse_by_extension;
use crate::rg::rg_find_definition;
use crate::types::FileData;
use crate::StrExt;

use super::container::range_encloses;
use super::find::{find_local_declaration, find_name_in_uri, find_name_scoped_to_container};
use super::imports::resolve_via_imports;
use super::package::import_package_absent_from_source_roots;
use super::package_scope::{
    find_in_star_imports, resolve_same_package, resolve_star_imports, star_import_packages,
};
use super::platform_types::resolve_kotlin_builtin_type_platform_equivalent;
use super::qualified::{resolve_from_class_hierarchy, resolve_qualified};
use super::tie_break::ambiguity_safe_tail_with_denylist;

/// Return `FileData` for `uri` — from the live index if indexed, otherwise parse from disk.
/// Returns `None` if the file is not indexed and not readable from disk.
/// Returns an `Arc` so callers can read without copying the full `FileData`.
///
/// Checks `indexer.files` first, then `indexer.jar_files`, then falls back to disk.
/// JAR URIs (`jar:file://...`) cannot be read from disk — when found in `jar_files`,
/// the disk fallback is skipped.
pub(crate) fn ensure_file_data(indexer: &Indexer, uri: &Url) -> Option<Arc<FileData>> {
    if let Some(file_data) = indexer.files.get(uri.as_str()) {
        return Some(file_data.value().clone());
    }

    // URI-keyed `jar_files` read: deliberately NOT gated by a Tier-2 promotion —
    // there is no URI→name reverse index to promote by. A URI for a
    // not-yet-materialized JAR therefore misses here (known limitation);
    // in practice most callers arrive with URIs a name-keyed (promoting)
    // lookup produced, so the miss window is narrow.
    if let Some(file_data) = indexer.jar_files.get(uri.as_str()) {
        return Some(file_data.value().clone());
    }

    let path = uri.to_file_path().ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    Some(Arc::new(parse_by_extension(uri.path(), &content)))
}

// ─── auto-import helpers ──────────────────────────────────────────────────────

/// Return all importable FQNs for a simple symbol name (e.g. "Composable").
pub(crate) fn fqns_for_name(indexer: &Indexer, name: &str) -> Vec<String> {
    indexer
        .importable_fqns
        .read()
        .map(|m| m.get(name).cloned().unwrap_or_default())
        .unwrap_or_default()
}

/// Which IO and fallbacks a resolution pass may use. The plan's "IoPolicy".
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveIo {
    /// Navigation (go-to-def, hover): may spawn `fd`/`rg`, walk the class
    /// hierarchy, and index a cold file on demand. No global-defs tail fallback.
    Full,
    /// Index-only, but imports may still `fd`. No `rg`, no hierarchy, no cold
    /// index. Tail fallback: first global-defs match. (completion/highlight hot path)
    NoRg,
    /// Strictly in-memory: no `fd`, no `rg`, no hierarchy. Tail fallback:
    /// a unique global-defs match wins outright; an ambiguous set gets the
    /// same denylist-first tie-break as `HierarchyAmbiguitySafe` (see
    /// `ambiguity_safe_tail_with_denylist`), still declining if more than
    /// one candidate survives. (diagnostics keystroke path)
    IndexOnly,
    /// Same IO profile as `NoRg`, but NO global-defs tail fallback at all --
    /// only local/imports/same-package/star-imports/hierarchy count as a
    /// match. For callers that already chain their own last-resort fallback
    /// afterward (see `find_fun_return_type_reachable`): letting THIS step's
    /// tail fire first pre-empted that fallback with a match that has no
    /// more claim to correctness than an arbitrary same-named symbol
    /// anywhere in the workspace (a real production bug: a bare-name tail
    /// match here beat a differently-named function's own, more precise
    /// resolution downstream — see the caller's doc comment).
    ScopedOnly,
    /// Same IO profile as `NoRg`, but the global-defs tail fallback is
    /// ambiguity-safe: a unique match wins outright; an ambiguous (N>1)
    /// match gets two narrow tie-breaks in sequence — dropping any
    /// candidate whose declared package starts with a denylisted prefix
    /// (e.g. `com.android.internal.`), then narrowing to candidates whose
    /// own JAR is a real dependency of the calling file's module (when
    /// `workspace.json` module-dependency data is available) — before
    /// still declining if more than one candidate remains. Used only by
    /// the hierarchy walk's own recursion beyond hop 1 (`supertype_targets`),
    /// where `from_uri` becomes a `jar:` synthetic URI with no import list
    /// to disambiguate against — see
    /// `docs/superpowers/specs/2026-08-25-hierarchy-walk-unscoped-name-collision-design.md`
    /// and `docs/superpowers/specs/2026-08-25-real-workspace-json-schema-and-consumption-design.md`.
    HierarchyAmbiguitySafe,
}

/// Resolve `name` as seen from `from_uri`, returning all known definition
/// `Location`s in priority order.  Returns an empty vec only when nothing was
/// found by any strategy including `rg`.
pub(crate) fn resolve_symbol(
    indexer: &Indexer,
    name: &str,
    qualifier: Option<&str>,
    from_uri: &Url,
) -> Vec<Location> {
    resolve_symbol_with_io(indexer, name, qualifier, from_uri, ResolveIo::Full)
}

/// Same dispatch as `resolve_symbol`, but every bare-name fallback step uses
/// `ResolveIo::IndexOnly` instead of always spawning `rg`/`fd`. For a bulk
/// scan that resolves every identifier in a whole corpus (the
/// resolution-accuracy benchmark) and — by design — expects most bare/local
/// references to miss, letting each of those exhaust the full rg/fd fallback
/// chain turned a 13k-file scan into a roughly hour-long run.
pub(crate) fn resolve_symbol_index_only(
    indexer: &Indexer,
    name: &str,
    qualifier: Option<&str>,
    from_uri: &Url,
) -> Vec<Location> {
    resolve_symbol_with_io(indexer, name, qualifier, from_uri, ResolveIo::IndexOnly)
}

fn resolve_symbol_with_io(
    indexer: &Indexer,
    name: &str,
    qualifier: Option<&str>,
    from_uri: &Url,
    io: ResolveIo,
) -> Vec<Location> {
    // 0. Qualified access: `AccountPickerMapper.Content` — cursor on `Content`.
    //    Resolve the qualifier to a file, then search that file for `name`.
    if let Some(qual) = qualifier {
        // For `super` and `this`, never fall through to the unqualified chain:
        // `super.method` must only look in the parent hierarchy, never via rg/index
        // of the current file (which would return the override).
        let is_keyword_qual = qual == "super" || qual == "this";
        let locs = resolve_qualified(indexer, name, qual, from_uri, io);
        if !locs.is_empty() {
            return locs;
        }
        if is_keyword_qual {
            return vec![];
        }
        // Uppercase qualifier is a class/type name — if qualified resolution
        // failed (class not indexed, member not found), don't fall through
        // to unqualified resolution which would incorrectly match lambda params.
        if qual.starts_with_uppercase() {
            return vec![];
        }
        // If qualifier resolution failed (e.g. it's a package name, not a class),
        // fall through to the normal chain.
    }

    // Handle dotted type names like `Outer.Factory`, a package-qualified
    // `demo.Foo`, or a deeply-nested `Bar.Baz.Foo` passed directly as `name`
    // (e.g. from hover/goto-def of a variable's declared type, or the inferred
    // type of a field). Skip any leading lowercase package segments, then walk
    // the type segments by their nesting — each nested type lives in the same
    // file as its enclosing type.
    if name.contains('.') {
        let segments: Vec<&str> = name.split('.').collect();
        // Start at the first type (uppercase) segment, skipping package prefixes.
        if let Some(start) = segments.iter().position(|s| s.starts_with_uppercase()) {
            let outer_locs =
                resolve_chain(indexer, segments[start], from_uri, io, true, None, None);
            if let Some(outer_loc) = outer_locs.first() {
                // A package-qualified plain type (`demo.Foo`) has no nested
                // segments after the type — the resolved type itself is the target.
                if start + 1 == segments.len() {
                    return outer_locs;
                }
                // Walk each remaining nested segment, re-anchoring on its own
                // location so a same-named sibling elsewhere in the file
                // can't shadow it (see `find_name_scoped_to_container`).
                let mut container = outer_loc.clone();
                for seg in &segments[start + 1..] {
                    match find_name_scoped_to_container(indexer, seg, &container) {
                        Some(loc) => container = loc,
                        None => return vec![],
                    }
                }
                return vec![container];
            }
        }
    }

    resolve_chain(indexer, name, from_uri, io, true, None, None)
}

/// Resolve a call's callee name, filtering same-file candidates by `shape`
/// (see [`resolve_local`], built on `CallShape::accepts_symbol`) so an
/// enclosing declaration that shares the callee's name but can't satisfy the
/// call's arity doesn't shadow the real target. Used only by goto-definition's
/// unqualified-callee path (`Indexer::find_definition_for_call`).
pub(crate) fn resolve_callee_definition(
    indexer: &Indexer,
    name: &str,
    uri: &Url,
    shape: CallShape,
) -> Vec<Location> {
    resolve_chain(indexer, name, uri, ResolveIo::Full, true, Some(shape), None)
}

/// The single prioritised resolution chain, parameterised by IO policy.
///
/// `resolve_symbol_with_io` (`Full`/`IndexOnly`), `resolve_symbol_no_rg` (`NoRg`)
/// and `resolve_type_index_only_simple` (`IndexOnly`) are all thin wrappers over
/// this function. The `ResolveIo` policy selects which subprocess fallbacks (`fd`/`rg`),
/// the hierarchy walk, the cold-file on-demand index, and which global-defs tail
/// fallback are permitted — see the `ResolveIo` doc-comment for the per-policy table.
///
/// The chain order is fixed (local → local-decl → imports → swift → same-package →
/// star → hierarchy → rg → tail); each step that is policy-gated simply no-ops when
/// the policy forbids it, so every policy walks the same steps in the same order.
///
/// `shape` is forwarded to step 1 only (see [`resolve_local`]) — every existing
/// caller passes `None`; only [`resolve_callee_definition`] passes a real shape.
///
/// `hierarchy_walk_origin_uri` is forwarded to the tail fallback only, and only
/// matters for [`ResolveIo::HierarchyAmbiguitySafe`] — every other caller passes
/// `None`; only [`resolve_symbol_hierarchy_ambiguity_safe`] passes the hierarchy
/// walk's real starting file (see [`crate::resolver::hierarchy::walk_hierarchy`]),
/// which can differ from `from_uri` past hop 1 of that walk.
fn resolve_chain(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
    io: ResolveIo,
    with_hierarchy: bool,
    shape: Option<CallShape>,
    hierarchy_walk_origin_uri: Option<&Url>,
) -> Vec<Location> {
    // Behavioural knobs derived from the policy (see the `ResolveIo` table):
    //  - `full_io`: cold-index + local-decl + swift + hierarchy + project-wide rg
    //  - `allow_fd`: import resolution may spawn `fd` (everything except IndexOnly)
    //  - `star_rg`: star imports may `rg` the package dir (Full only)
    let full_io = matches!(io, ResolveIo::Full);
    let allow_fd = !matches!(io, ResolveIo::IndexOnly);
    let star_rg = matches!(io, ResolveIo::Full);

    // 0.5 ── on-demand index of the current file if not yet indexed ────────────
    // Ensures resolve_local and find_local_declaration work even at cold start
    // (e.g. the user invokes gd/hover before indexing has reached this file).
    if full_io && !indexer.files.contains_key(from_uri.as_str()) {
        if let Ok(path) = from_uri.to_file_path() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                indexer.index_content(from_uri, &content);
            }
        }
    }

    // 1 ── local (indexed symbols) ────────────────────────────────────────────
    let local = resolve_local(indexer, name, from_uri, shape);
    if !local.is_empty() {
        return local;
    }

    // 1.5 ── local variable / parameter declaration (line scan) ───────────────
    // Catches function parameters without val/var that aren't in the symbol index.
    // Also catches named lambda parameters: `{ item -> ...}` found via the
    // `name ->` pattern in find_declaration_range_in_lines.
    if full_io && !name.starts_with_uppercase() {
        let decl = find_local_declaration(indexer, name, from_uri);
        if !decl.is_empty() {
            return decl;
        }
    }

    // 2 ── explicit imports ───────────────────────────────────────────────────
    let imported = resolve_via_imports(indexer, name, from_uri, allow_fd);
    if !imported.is_empty() {
        return imported;
    }

    // 2.5 ── Swift fast path: definitions index (no package system) ───────────
    // Swift files have no package declarations, so same-package and star-import
    // steps return empty. Use the in-memory definitions index directly to avoid
    // expensive project-wide rg fallback at step 5.
    if full_io
        && crate::Language::from_path(from_uri.path()) == crate::Language::Swift
        && name.starts_with_uppercase()
    {
        if let Some(locs_ref) = indexer.definitions.get(name) {
            // Reconstitute interned `SymbolLoc`s at this boundary before filtering.
            let locs: Vec<Location> = locs_ref
                .iter()
                .filter_map(|sym_loc| indexer.file_table.location(*sym_loc))
                .collect();
            // Prefer definitions from .swift files when available.
            let swift_locs: Vec<Location> = locs
                .iter()
                .filter(|l| crate::Language::from_path(l.uri.path()) == crate::Language::Swift)
                .cloned()
                .collect();
            if !swift_locs.is_empty() {
                return swift_locs;
            }
            if !locs.is_empty() {
                return locs;
            }
        }
    }

    // 3 ── same package ───────────────────────────────────────────────────────
    let same_pkg = resolve_same_package(indexer, name, from_uri);
    if !same_pkg.is_empty() {
        return same_pkg;
    }

    // 4 ── star imports ───────────────────────────────────────────────────────
    if star_rg {
        // Indexed-package scan, then `rg` scoped to the package dir for unindexed files.
        let star = resolve_star_imports(indexer, name, from_uri);
        if !star.is_empty() {
            return star;
        }
    } else {
        // Index-only scan (no rg fallback for unindexed files).
        let star_pkgs = star_import_packages(indexer, from_uri);
        if let Some(loc) = find_in_star_imports(indexer, name, &star_pkgs) {
            return vec![loc];
        }
    }

    // 4.5 ── superclass / interface hierarchy ─────────────────────────────────
    if full_io && with_hierarchy {
        let inherited = resolve_from_class_hierarchy(indexer, name, from_uri);
        if !inherited.is_empty() {
            return inherited;
        }
    }

    // 5 ── project-wide rg ───────────────────────────────────────────────────
    if full_io {
        let (root, source_roots, matcher) = indexer.rg_scope_for_path(None);
        // Skip when an explicit import for this name already went through all
        // source-tree lookups (qualified index + definitions index + fd) and came
        // up empty.  rg searches the same source tree and cannot add anything new.
        // The package-dir check is the authoritative gate: if `android/os/` doesn't
        // exist under any source root, the symbol simply isn't in the project.
        if import_package_absent_from_source_roots(
            indexer,
            name,
            from_uri,
            root.as_deref(),
            &source_roots,
        ) {
            return vec![];
        }
        let rg_locations =
            rg_find_definition(name, root.as_deref(), &source_roots, matcher.as_deref());
        let rg_result = match shape {
            // `rg` is a blind text search with no arity awareness of its own — without this,
            // a same-file, wrong-arity declaration that step 1 already ruled out can come
            // straight back here, since `rg` re-finds it by pattern match alone.
            Some(shape) => rg_locations
                .into_iter()
                .filter(|location| rg_location_satisfies_call_shape(indexer, location, name, shape))
                .collect(),
            None => rg_locations,
        };
        if !rg_result.is_empty() {
            return rg_result;
        }
        // 5.4 ── global definitions index (includes JAR symbols) ───────────────
        // `rg`/`fd` only search the *workspace's own* source tree, so a type
        // used purely through inference and never explicitly imported (Kotlin
        // doesn't require an import for that) — e.g. `scope.async { }.await()`,
        // where `Deferred` is inferred from `async`'s JAR-indexed return type
        // but no file spells out `import kotlinx.coroutines.Deferred` — was
        // unreachable here: real, measured gap. Same ambiguity-safe tie-break
        // `IndexOnly`/`HierarchyAmbiguitySafe` already use below, extended to
        // `Full` too — a unique JAR/workspace candidate wins outright; an
        // ambiguous set still declines rather than guessing. Shape-filtered
        // first, same as `rg_result` just above: an arity-incompatible
        // same-name workspace declaration (e.g. a differently-shaped local
        // overload) must not win here just because it's the sole candidate
        // this index lookup happens to return.
        let jar_tail_candidates = indexer.lookup_definitions(name);
        let jar_tail_candidates = match shape {
            Some(shape) => jar_tail_candidates
                .into_iter()
                .filter(|location| rg_location_satisfies_call_shape(indexer, location, name, shape))
                .collect(),
            None => jar_tail_candidates,
        };
        let jar_tail = ambiguity_safe_tail_with_denylist(indexer, from_uri, jar_tail_candidates);
        if !jar_tail.is_empty() {
            return jar_tail;
        }
        // 5.5 ── Kotlin built-in-type platform equivalent (last resort) ────────
        // `rg`/`fd` search the *workspace's own* source tree and can never find
        // `String`/`CharSequence`: those are compiler intrinsics with no
        // compiled `.class` file in kotlin-stdlib's JAR at all, and no
        // in-workspace source either -- see
        // docs/superpowers/specs/2026-08-27-kotlin-builtin-type-platform-mapping-design.md.
        return resolve_kotlin_builtin_type_platform_equivalent(indexer, name);
    }

    // Tail fallback — global definitions index (includes JAR symbols).
    //  - NoRg: first match.
    //  - ScopedOnly: no tail at all (empty) -- see the variant's doc comment.
    //  - IndexOnly / HierarchyAmbiguitySafe: unique match wins outright, else
    //    the same denylist-first tie-break (`ambiguity_safe_tail_with_denylist`)
    //    -- IndexOnly resolves a bare qualifier root (e.g. `resolve_qualified`'s
    //    uppercase branch resolving `String` before it can even attempt a
    //    member/extension lookup on it) just as often as the hierarchy walk's
    //    own per-hop resolution does, and hit the identical real decoy shape
    //    on the Moneta corpus (13 candidates for bare `String`, including a
    //    `com.android.internal.*`-packaged one) when it used the older,
    //    plain unique-match-only rule.
    //  - Full: never reached — it has its own equivalent tail (5.4 above,
    //    same `ambiguity_safe_tail_with_denylist` call) inside the rg branch,
    //    since Full always returns from within that `if full_io` block.
    //
    // Each non-`ScopedOnly` arm falls through to
    // `resolve_kotlin_builtin_type_platform_equivalent` when its own lookup
    // comes up empty -- see the 5.5 comment above the `Full`/rg branch.
    // `ScopedOnly` is deliberately excluded: "no tail at all" is its own
    // documented contract (its callers already have their own downstream
    // fallback), so it must not gain one here.
    match io {
        ResolveIo::Full => vec![],
        ResolveIo::ScopedOnly => vec![],
        ResolveIo::NoRg => {
            let found = indexer
                .lookup_definitions(name)
                .into_iter()
                .next()
                .map(|loc| vec![loc])
                .unwrap_or_default();
            if !found.is_empty() {
                return found;
            }
            resolve_kotlin_builtin_type_platform_equivalent(indexer, name)
        }
        ResolveIo::IndexOnly => {
            let found = ambiguity_safe_tail_with_denylist(
                indexer,
                from_uri,
                indexer.lookup_definitions(name),
            );
            if !found.is_empty() {
                return found;
            }
            resolve_kotlin_builtin_type_platform_equivalent(indexer, name)
        }
        ResolveIo::HierarchyAmbiguitySafe => {
            let found = ambiguity_safe_tail_with_denylist(
                indexer,
                hierarchy_walk_origin_uri.unwrap_or(from_uri),
                indexer.lookup_definitions(name),
            );
            if !found.is_empty() {
                return found;
            }
            resolve_kotlin_builtin_type_platform_equivalent(indexer, name)
        }
    }
}

/// Index-only resolver for use in completion paths (`ResolveIo::NoRg` — see
/// its own doc comment for the exact IO policy, since restating it here is
/// how this comment drifted out of sync with it the first time).
pub(crate) fn resolve_symbol_no_rg(indexer: &Indexer, name: &str, from_uri: &Url) -> Vec<Location> {
    resolve_chain(indexer, name, from_uri, ResolveIo::NoRg, false, None, None)
}

/// Ambiguity-safe sibling of [`resolve_symbol_no_rg`], scoped to the
/// hierarchy walk's own recursion (`supertype_targets` in
/// `resolver/hierarchy.rs`) — see [`ResolveIo::HierarchyAmbiguitySafe`].
/// Does not alter `resolve_symbol_no_rg` itself or any of its other
/// callers' behavior.
///
/// `hierarchy_walk_origin_uri` is the hierarchy walk's real starting file
/// (see [`crate::resolver::hierarchy::walk_hierarchy`]'s doc comment),
/// forwarded here from `supertype_targets` so the module-scoped tie-break
/// can still find real Gradle dependency data past hop 1, where `from_uri`
/// itself has become the previous hop's own (often `jar:`) resolved URI.
pub(crate) fn resolve_symbol_hierarchy_ambiguity_safe(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
    hierarchy_walk_origin_uri: Option<&Url>,
) -> Vec<Location> {
    resolve_chain(
        indexer,
        name,
        from_uri,
        ResolveIo::HierarchyAmbiguitySafe,
        false,
        None,
        hierarchy_walk_origin_uri,
    )
}

/// Like [`resolve_symbol_no_rg`] but without its global-defs tail fallback --
/// for callers that already chain their own last-resort fallback afterward
/// (see [`ResolveIo::ScopedOnly`]).
pub(crate) fn resolve_symbol_scoped_only(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
) -> Vec<Location> {
    resolve_chain(
        indexer,
        name,
        from_uri,
        ResolveIo::ScopedOnly,
        false,
        None,
        None,
    )
}

/// Index-only type resolver for the diagnostics hot path.
///
/// Same resolution chain as `resolve_symbol_no_rg` but:
/// - Skips the `fd_find_and_parse` fallback in import resolution (no subprocess spawns)
/// - Makes the global definitions fallback ambiguity-safe (returns only if exactly 1 candidate)
///
/// This keeps behavior consistent with navigation (imports + package context) without
/// the IO cost that causes timeouts when called per-`when`-expression during diagnostics.
pub(crate) fn resolve_type_index_only(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
) -> Vec<Location> {
    // Handle dotted type names like `DashboardInvestedContract.Effect` — mirrors
    // the same pattern in `resolve_symbol` (see dotted-name block above).
    if let Some(dot) = name.find('.') {
        let outer = &name[..dot];
        let inner = &name[dot + 1..];
        // Use the full simple chain for the outer (no recursion into dotted split).
        let outer_locs = resolve_type_index_only_simple(indexer, outer, from_uri);
        if let Some(outer_loc) = outer_locs.first() {
            let locs = find_name_in_uri(indexer, inner, outer_loc.uri.as_str());
            if !locs.is_empty() {
                return locs;
            }
        }
    }

    resolve_type_index_only_simple(indexer, name, from_uri)
}

/// Inner helper: resolves a simple (non-dotted) type name using the index-only chain.
fn resolve_type_index_only_simple(indexer: &Indexer, name: &str, from_uri: &Url) -> Vec<Location> {
    resolve_chain(
        indexer,
        name,
        from_uri,
        ResolveIo::IndexOnly,
        false,
        None,
        None,
    )
}

// ─── missing-import diagnostic helpers ────────────────────────────────────────

/// Step 1 — symbols defined in the same source file.
///
/// `shape` is `Some` only when the caller knows it's resolving a call's callee
/// (see [`resolve_callee_definition`]) — a same-file match whose arity provably
/// can't satisfy the call is dropped, so a same-named-but-wrong-arity enclosing
/// declaration doesn't shadow the real (often library) target. `None` preserves
/// today's pure name-match behaviour exactly, for every other caller.
pub(super) fn resolve_local(
    indexer: &Indexer,
    name: &str,
    uri: &Url,
    shape: Option<CallShape>,
) -> Vec<Location> {
    indexer
        .files
        .get(uri.as_str())
        .map(|f| {
            f.symbols
                .iter()
                .filter(|symbol| {
                    symbol.name == name && shape.is_none_or(|shape| shape.accepts_symbol(symbol))
                })
                .map(|symbol| Location {
                    uri: uri.clone(),
                    range: symbol.selection_range,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `rg`-step counterpart to the arity check above: `rg` finds `location`
/// by blind text match, with no parsed symbol of its own, so this first has
/// to find the `SymbolEntry` `location` actually landed on — its file must
/// already be indexed (an `rg` hit in a file the index has never seen has no
/// `param_counts` to check against) and must contain a `name` symbol whose
/// range encloses the point `rg` reported. Fails open (keeps `location`)
/// whenever either lookup comes up empty, matching this module's existing
/// fail-open convention (see [`is_import_reachable`]) — arity-gating only
/// fires when it can be answered with confidence, never as a guess.
fn rg_location_satisfies_call_shape(
    indexer: &Indexer,
    location: &Location,
    name: &str,
    shape: CallShape,
) -> bool {
    let Some(file_data) = indexer.files.get(location.uri.as_str()) else {
        return true;
    };
    let Some(symbol) = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.name == name && range_encloses(symbol.range, location.range))
    else {
        return true;
    };
    shape.accepts_symbol(symbol)
}

// ─── impl Indexer wrappers ────────────────────────────────────────────────────

impl crate::indexer::Indexer {
    pub(crate) fn resolve_symbol(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
    ) -> Vec<Location> {
        resolve_symbol(self, name, qualifier, from_uri)
    }
    pub(crate) fn resolve_symbol_index_only(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
    ) -> Vec<Location> {
        resolve_symbol_index_only(self, name, qualifier, from_uri)
    }
    pub(crate) fn resolve_symbol_no_rg(&self, name: &str, from_uri: &Url) -> Vec<Location> {
        resolve_symbol_no_rg(self, name, from_uri)
    }

    /// Find `name` accessed through `qualifier`, restricted to the qualifier's
    /// own type: a real member (declared in the class body or inherited) and —
    /// when the qualifier root is a type name — an extension on that type, but
    /// never the unqualified bare-word fallback chain that the outer
    /// [`Indexer::resolve_symbol`] falls through to when the qualifier doesn't
    /// resolve. (It delegates to [`resolve_qualified`], which can surface an
    /// extension for an uppercase root.) Used by diagnostics that need a
    /// scoped, qualifier-anchored lookup rather than the global fallback — the
    /// caller is responsible for confirming membership when it must exclude
    /// extensions (see `is_member_of` in the nullable-dot-call diagnostic).
    pub(crate) fn resolve_member_only(
        &self,
        name: &str,
        qualifier: &str,
        from_uri: &Url,
    ) -> Vec<Location> {
        resolve_qualified(self, name, qualifier, from_uri, ResolveIo::Full)
    }
}
