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

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use tower_lsp::lsp_types::{Location, SymbolKind, Url};

use crate::indexer::Indexer;
use crate::parser::parse_by_extension;
use crate::rg::{build_rg_pattern, parse_rg_line, rg_find_definition};
use crate::types::{CallerContext, FileData};
use crate::StrExt;

use super::fd::{fd_find_and_parse, import_package_prefix};
use super::find::{find_local_declaration, find_name_in_uri, find_name_in_uri_after_line};
use super::hierarchy::walk_hierarchy;
use super::infer::{infer_field_type, infer_variable_type};

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
    /// unique global-defs match only (ambiguity-safe). (diagnostics keystroke path)
    IndexOnly,
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
    // 0. Qualified access: `AccountPickerMapper.Content` — cursor on `Content`.
    //    Resolve the qualifier to a file, then search that file for `name`.
    if let Some(qual) = qualifier {
        // For `super` and `this`, never fall through to the unqualified chain:
        // `super.method` must only look in the parent hierarchy, never via rg/index
        // of the current file (which would return the override).
        let is_keyword_qual = qual == "super" || qual == "this";
        let locs = resolve_qualified(indexer, name, qual, from_uri);
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
            let outer_locs = resolve_symbol_inner(indexer, segments[start], from_uri, true);
            if let Some(outer_loc) = outer_locs.first() {
                // A package-qualified plain type (`demo.Foo`) has no nested
                // segments after the type — the resolved type itself is the target.
                if start + 1 == segments.len() {
                    return outer_locs;
                }
                // Walk each remaining nested segment within the current file.
                let mut current_file = outer_loc.uri.to_string();
                let mut resolved: Option<Vec<Location>> = None;
                for seg in &segments[start + 1..] {
                    let locs = find_name_in_uri(indexer, seg, &current_file);
                    match locs.first() {
                        Some(loc) => {
                            current_file = loc.uri.to_string();
                            resolved = Some(locs);
                        }
                        None => {
                            resolved = None;
                            break;
                        }
                    }
                }
                if let Some(locs) = resolved {
                    return locs;
                }
            }
        }
    }

    resolve_symbol_inner(indexer, name, from_uri, true)
}

/// Internal resolver.  When `with_hierarchy` is false step 4.5 is skipped to
/// avoid infinite recursion inside `resolve_from_class_hierarchy` (which calls
/// this function to locate each superclass, and those files would in turn call
/// the hierarchy walk again with a fresh visited-set, looping forever).
pub(crate) fn resolve_symbol_inner(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
    with_hierarchy: bool,
) -> Vec<Location> {
    resolve_chain(indexer, name, from_uri, ResolveIo::Full, with_hierarchy)
}

/// The single prioritised resolution chain, parameterised by IO policy.
///
/// `resolve_symbol_inner` (`Full`), `resolve_symbol_no_rg` (`NoRg`) and
/// `resolve_type_index_only_simple` (`IndexOnly`) are all thin wrappers over this
/// function. The `ResolveIo` policy selects which subprocess fallbacks (`fd`/`rg`),
/// the hierarchy walk, the cold-file on-demand index, and which global-defs tail
/// fallback are permitted — see the `ResolveIo` doc-comment for the per-policy table.
///
/// The chain order is fixed (local → local-decl → imports → swift → same-package →
/// star → hierarchy → rg → tail); each step that is policy-gated simply no-ops when
/// the policy forbids it, so every policy walks the same steps in the same order.
fn resolve_chain(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
    io: ResolveIo,
    with_hierarchy: bool,
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
    let local = resolve_local(indexer, name, from_uri);
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
            let locs: Vec<Location> = locs_ref.clone();
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
        let star_pkgs: Vec<String> = match indexer.files.get(from_uri.as_str()) {
            Some(f) => f
                .imports
                .iter()
                .filter(|i| i.is_star && !is_stdlib(&i.full_path))
                .map(|i| i.full_path.clone())
                .collect(),
            None => vec![],
        };
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
        return rg_find_definition(name, root.as_deref(), &source_roots, matcher.as_deref());
    }

    // Tail fallback — global definitions index (includes JAR symbols).
    //  - NoRg: first match.   - IndexOnly: unique match only (ambiguity-safe).
    //  - Full: never reached (returns inside the rg branch above).
    match io {
        ResolveIo::Full => vec![],
        ResolveIo::NoRg => indexer
            .lookup_definitions(name)
            .into_iter()
            .next()
            .map(|loc| vec![loc])
            .unwrap_or_default(),
        ResolveIo::IndexOnly => {
            let locs = indexer.lookup_definitions(name);
            if locs.len() == 1 {
                locs
            } else {
                vec![]
            }
        }
    }
}

/// Returns the first Location found by scanning star-import packages.
fn find_in_star_imports(indexer: &Indexer, name: &str, star_pkgs: &[String]) -> Option<Location> {
    for pkg in star_pkgs {
        if let Some(loc) = find_symbol_in_package(indexer, name, pkg) {
            return Some(loc);
        }
    }
    None
}

/// Index-only resolver for use in completion paths.
///
/// Identical to `resolve_symbol_inner` but omits:
/// - Step 4's `rg_in_package_dir` fallback (inside `resolve_star_imports`)
/// - Step 4.5 hierarchy walk
/// - Step 5 `rg_find_definition`
///
/// Completion is triggered on every keystroke; spawning external `rg`/`fd`
/// processes on each request would block the LSP thread and spike CPU.
pub(crate) fn resolve_symbol_no_rg(indexer: &Indexer, name: &str, from_uri: &Url) -> Vec<Location> {
    resolve_chain(indexer, name, from_uri, ResolveIo::NoRg, false)
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
    resolve_chain(indexer, name, from_uri, ResolveIo::IndexOnly, false)
}

// ─── step implementations ────────────────────────────────────────────────────

/// Look up an extension function by receiver base name, filtering by scope
/// (same package or explicitly imported in the caller's file).
///
/// Checks `extension_by_receiver` for matching entries, then verifies each
/// candidate is visible from `from_uri` by checking same-package or import
/// coverage. Returns the first matching extension's `Location` with an accurate
/// `selection_range`, or an empty `Vec` if none is in scope.
fn resolve_extension_in_scope(
    indexer: &Indexer,
    receiver_base: &str,
    name: &str,
    from_uri: &Url,
) -> Vec<Location> {
    let Some(entries) = indexer.extension_by_receiver.get(receiver_base) else {
        return vec![];
    };
    let caller_file_data = indexer.files.get(from_uri.as_str());
    let caller_file_data_ref: Option<&FileData> = caller_file_data.as_deref().map(|v| v.as_ref());
    let caller_package: Option<&String> =
        caller_file_data.as_ref().and_then(|fd| fd.package.as_ref());
    for entry in entries.iter() {
        if entry.name != name {
            continue;
        }
        let in_scope = crate::resolver::infer::extension_is_in_scope(
            entry.package.as_ref(),
            &entry.name,
            caller_package,
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
                                s.name == name
                                    && s.extension_receiver == receiver_base
                                    && s.container.is_none()
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

/// Step 0 — dot-qualified access.
///
/// Handles two families of chains:
///
/// **Uppercase root** (`Outer.Inner`, `A.B.C.D`): all segments are class/object
/// names; the root identifies the file and all nested types live in the same
/// file, so we resolve root → file and search that file for `name`.
///
/// **Lowercase root** (`variable.field`, `account.account.interestPlanCode`):
/// the first segment is a variable/parameter — we infer its declared type, then
/// traverse every subsequent lowercase segment as a field access (inferring each
/// field's type in turn) until we have a file to search `name` in.
/// Uppercase segments inside a lowercase chain are treated as nested class names
/// within the current file.
fn resolve_qualified(
    indexer: &Indexer,
    name: &str,
    qualifier: &str,
    from_uri: &Url,
) -> Vec<Location> {
    let segments: Vec<&str> = qualifier.split('.').collect();
    let root = segments[0];

    // ── `this.member` — search current file and its superclass hierarchy ──────
    if root == "this" {
        let locs = find_name_in_uri(indexer, name, from_uri.as_str());
        if !locs.is_empty() {
            return locs;
        }
        return resolve_from_class_hierarchy(indexer, name, from_uri);
    }

    // ── `super.member` — search superclass hierarchy only ────────────────────
    if root == "super" {
        return resolve_from_class_hierarchy(indexer, name, from_uri);
    }

    if root.starts_with_uppercase() {
        let root_base = root.last_segment();

        // Extension functions take precedence over member functions,
        // but only when they are in scope (same package or imported).
        let ext_locs = resolve_extension_in_scope(indexer, root_base, name, from_uri);
        if !ext_locs.is_empty() {
            return ext_locs;
        }

        // Then check member functions (same-file).
        let qual_locs = resolve_symbol(indexer, root, None, from_uri);
        for qual_loc in &qual_locs {
            // `Foo.member` with `Foo` a class name (not a variable) can only reach a
            // companion-object member in Kotlin — never an instance member of `Foo`,
            // even if one shares the name. Try the companion first so a same-named
            // instance member declared earlier in the file can't shadow it.
            //
            // Only the single-segment `Foo.member` form names `root` as the
            // qualifying class. For a multi-segment qualifier like
            // `Outer.Inner.member`, `root` is `Outer` — not the class the member
            // is accessed on — so probing `Outer`'s companion would mis-resolve;
            // fall through to the nested-segment handling instead.
            if segments.len() == 1 {
                let companion_locs =
                    resolve_companion_member(indexer, name, root, qual_loc.uri.as_str());
                if !companion_locs.is_empty() {
                    return companion_locs;
                }
            }

            let after_line = qual_loc.range.start.line;
            let locs =
                find_name_in_uri_after_line(indexer, name, qual_loc.uri.as_str(), after_line);
            if !locs.is_empty() {
                return locs;
            }
        }
        // Extension functions may live in a different file than the receiver class.
        let root_base = root.last_segment();
        if let Some(entries) = indexer.extension_by_receiver.get(root_base) {
            for entry in entries.iter() {
                if entry.name == name {
                    if let Ok(uri) = Url::parse(&entry.file_uri) {
                        // Look up the symbol in the declaring file for accurate range.
                        let range = indexer
                            .files
                            .get(&entry.file_uri)
                            .or_else(|| indexer.jar_files.get(&entry.file_uri))
                            .and_then(|fd| {
                                fd.symbols
                                    .iter()
                                    .find(|s| s.name == name)
                                    .map(|s| s.selection_range)
                            })
                            .unwrap_or_default();
                        return vec![Location { uri, range }];
                    }
                }
            }
        }
        return vec![];
    }

    // ── Lowercase root: variable / parameter type inference ──────────────────
    let Some(start_type) = infer_variable_type(indexer, root, from_uri) else {
        return vec![];
    };
    // A nullable receiver resolves members from its underlying (non-null) class,
    // so drop any trailing `?` before resolving the type to a file — otherwise
    // `resolve_symbol("Confirmation?")` would find nothing.
    let start_type = start_type.strip_nullable();

    // `start_type` may be a dotted nested type like `Outer.Inner`.
    // Split into outer (for file resolution) and optional inner (nested class).
    let (outer_type, inner_type) = match start_type.find('.') {
        Some(dot) => (&start_type[..dot], Some(&start_type[dot + 1..])),
        None => (start_type, None),
    };

    // Resolve the variable's type to its source file.
    let type_locs = resolve_symbol(indexer, outer_type, None, from_uri);
    let mut current_file: Option<String> = type_locs.first().map(|l| l.uri.to_string());

    // If there's a nested type component (e.g. `Factory` in `Outer.Factory`),
    // the members we want to search are inside that nested type.
    // We don't need to change `current_file` because nested types live in the
    // same file; instead we record each nested level as a trailing qualifier
    // segment to process. A deeply-nested type like `Scenes.Confirmation` must
    // be split per-level — searching for a literal `"Scenes.Confirmation"`
    // symbol finds nothing, since each nested class is indexed on its own name.
    let extra_segments: Vec<&str> = inner_type
        .map(|t| t.split('.').collect())
        .unwrap_or_default();

    // Traverse remaining qualifier segments (plus any from the nested type).
    for &seg in extra_segments.iter().chain(segments[1..].iter()) {
        let Some(ref uri) = current_file else {
            return vec![];
        };
        if seg.starts_with_uppercase() {
            // Nested class / companion object — likely in the same file.
            // Search current file first; fall back to a global resolve.
            let locs = find_name_in_uri(indexer, seg, uri);
            current_file = if !locs.is_empty() {
                locs.first().map(|l| l.uri.to_string())
            } else {
                resolve_symbol(indexer, seg, None, from_uri)
                    .first()
                    .map(|l| l.uri.to_string())
            };
        } else {
            // Field access: infer the declared type of this field.
            let Some(field_type) = infer_field_type(indexer, uri, seg) else {
                return vec![];
            };
            let locs = resolve_symbol(indexer, &field_type, None, from_uri);
            current_file = locs.first().map(|l| l.uri.to_string());
        }
    }

    // Search the resolved type's file for the target member.
    let Some(ref resolved_uri) = current_file else {
        return vec![];
    };
    let locs = find_name_in_uri(indexer, name, resolved_uri);
    if !locs.is_empty() {
        return locs;
    }

    // Member not found directly — walk the superclass/interface hierarchy.
    let Ok(parsed_uri) = Url::parse(resolved_uri) else {
        return vec![];
    };
    resolve_from_class_hierarchy(indexer, name, &parsed_uri)
}

/// Step 1 — symbols defined in the same source file.
fn resolve_local(indexer: &Indexer, name: &str, uri: &Url) -> Vec<Location> {
    indexer
        .files
        .get(uri.as_str())
        .map(|f| {
            f.symbols
                .iter()
                .filter(|s| s.name == name)
                .map(|s| Location {
                    uri: uri.clone(),
                    range: s.selection_range,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Package of the JAR symbol at `loc`, from the `jar_symbol_packages` side table.
/// JAR symbols use a synthetic range whose line number equals the symbol's index
/// within the jar's `FileData.symbols`, so the line indexes the package vector.
/// Returns `None` when unknown (no entry, or pre-per-symbol-package jar cache).
fn jar_symbol_package(indexer: &Indexer, loc: &Location) -> Option<String> {
    let packages = indexer.jar_symbol_packages.get(loc.uri.as_str())?;
    packages
        .get(loc.range.start.line as usize)
        .filter(|p| !p.is_empty())
        .cloned()
}

/// The enclosing-type chain named by a nested import, outermost-first.
///
/// `com.app.Contract.State.Idle` (symbol `Idle`) → `["Contract", "State"]`.
/// All segments before the imported `symbol`, restricted to type names (uppercase
/// first letter), so leading package segments and the symbol itself are dropped.
/// Returns an empty vec for top-level imports (no enclosing type).
fn import_container_chain(full_path: &str, symbol: &str) -> Vec<String> {
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
fn enclosing_container_chain(indexer: &Indexer, loc: &Location) -> Vec<String> {
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
fn range_encloses(outer: tower_lsp::lsp_types::Range, inner: tower_lsp::lsp_types::Range) -> bool {
    pos_tuple(outer.start) <= pos_tuple(inner.start) && pos_tuple(inner.end) <= pos_tuple(outer.end)
}

/// Find `name` inside the companion object nested in `class_name`.
///
/// `Foo.member` with `Foo` a class name (not a variable) can only ever reach a
/// companion-object member in Kotlin — never an instance member of `Foo`, even
/// when one happens to share the name. The companion is identified by its
/// `detail`, which always starts with the literal `"companion object"` keywords
/// (set by [`crate::parser::extract_detail`] for both the named and the
/// synthesized-anonymous form — see [`crate::parser::extract_anonymous_companion_objects`]).
fn resolve_companion_member(
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
    let Some(companion) = file_data.symbols.iter().find(|symbol| {
        // `kind == OBJECT` already restricts this to object declarations; among
        // those, the companion is the one carrying the `companion` soft-keyword.
        // Match it as a token rather than a prefix so leading modifiers or an
        // annotation line (`private companion object`, `@JvmStatic\ncompanion
        // object`) don't hide it.
        symbol.kind == SymbolKind::OBJECT
            && symbol
                .detail
                .split_whitespace()
                .any(|token| token == "companion")
            && range_encloses(class_range, symbol.range)
    }) else {
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

/// Step 2 — explicit single-symbol imports.
///
/// Handles three cases:
///   a. Top-level class:   `import com.example.Foo`
///   b. Nested class:      `import com.example.OuterClass.InnerClass`
///   c. Alias:             `import com.example.Foo as F`
///
/// Resolution sub-steps (each tried in order):
///   i.   qualified index  — exact match, O(1), works once file is indexed
///   ii.  definitions index — short-name, filtered to expected package
///   iii. fd + on-demand parse — works at cold start; tries parent class file
///        first for nested symbols (AccountPickerContract.kt before Event.kt).
///        Gated by `allow_fd`: the index-only policy passes `false` to stay
///        strictly in-memory (no subprocess spawns) while keeping sub-steps i–ii.
fn resolve_via_imports(indexer: &Indexer, name: &str, uri: &Url, allow_fd: bool) -> Vec<Location> {
    let imports: Vec<crate::types::ImportEntry> = match indexer.files.get(uri.as_str()) {
        Some(f) => f.imports.iter().filter(|i| !i.is_star).cloned().collect(),
        None => return vec![],
    };

    for imp in imports.iter().filter(|i| i.local_name == name) {
        // i) qualified index — exact FQN (works for top-level classes)
        if let Some(loc) = indexer.qualified.get(&imp.full_path) {
            return vec![loc.clone()];
        }

        // ii) short-name index filtered to the expected package.
        //     For `…AccountPickerContract.Event` the expected package is
        //     `…accountpicker` (all-lowercase prefix segments).
        //     This avoids returning an unrelated `Event` from another package.
        let short = imp.full_path.last_segment();
        let expected_pkg = import_package_prefix(&imp.full_path);
        // The enclosing-type chain named by a nested import, outermost-first:
        // `com.app.Contract.State.Idle` → ["Contract", "State"]. Classes can nest
        // arbitrarily deep, so we compare the *whole* chain rather than just the
        // immediate parent — `Contract.State.Sub.Idle` and `Contract.Event.Sub.Idle`
        // share the immediate container `Sub` but differ higher up.
        let expected_chain = import_container_chain(&imp.full_path, short);
        let mut all_locations: Vec<tower_lsp::lsp_types::Location> = Vec::new();
        if let Some(locs) = indexer.definitions.get(short) {
            all_locations.extend(locs.iter().cloned());
        }
        if let Some(locs) = indexer.jar_definitions.get(short) {
            all_locations.extend(locs.iter().cloned());
        }
        if !all_locations.is_empty() {
            let mut filtered: Vec<_> = all_locations
                .iter()
                .filter(|loc| {
                    // Compiled-JAR (sidecar) symbols: filter by the sidecar's real
                    // per-symbol package (the `jar_symbol_packages` side table is
                    // populated only for compiled JARs). This keeps an
                    // `import a.b.c.remember` from also matching an unrelated
                    // `remember` in the Kotlin compiler / gradle plugin / KSP jars.
                    if let Some(pkg) = jar_symbol_package(indexer, loc) {
                        return pkg == expected_pkg || pkg.starts_with(&format!("{expected_pkg}."));
                    }
                    // Everything else — workspace, `sourcePaths` libraries, AND
                    // sources-JARs (which are `jar:…!/….kt` URIs but live in `files`
                    // with a real package) — filters by the file's package. Fail open
                    // when the package is unknown (e.g. compiled JAR on an older cache
                    // with no per-symbol package) so we never regress.
                    indexer
                        .files
                        .get(loc.uri.as_str())
                        .and_then(|f| f.package.clone())
                        .map(|p| p == expected_pkg || p.starts_with(&format!("{expected_pkg}.")))
                        .unwrap_or(true)
                })
                .cloned()
                .collect();

            // Nested-class disambiguation: when the import names an enclosing-type
            // chain (e.g. `Contract.State.Idle`), prefer candidates whose enclosing
            // container chain matches it. Two sealed classes in the same
            // package/interface can expose identically-named members (`State.Idle` vs
            // `Event.Idle`); the package filter alone keeps both, so go-to-definition
            // would jump to both. Only narrows when at least one candidate matches, so
            // a set that can't be container-resolved (e.g. JAR symbols whose synthetic
            // ranges don't line up with the symbol entry) is never emptied.
            if !expected_chain.is_empty() {
                let chain_matches: Vec<_> = filtered
                    .iter()
                    .filter(|loc| enclosing_container_chain(indexer, loc) == expected_chain)
                    .cloned()
                    .collect();
                if !chain_matches.is_empty() {
                    filtered = chain_matches;
                }
            }

            if !filtered.is_empty() {
                return filtered;
            }
        }

        // iii) on-demand fd + parse (indexing race or file never opened).
        //
        // Guard: skip when the import's package directory doesn't exist under
        // any source root.  A single stat() per import prevents spawning fd
        // processes for SDK/stdlib packages (android.os, androidx.*…) whose
        // sources are never present in the project tree.
        if allow_fd {
            let (root, source_roots, matcher) = indexer.rg_scope_for_path(None);
            if package_dir_in_source_roots(&imp.full_path, root.as_deref(), &source_roots) {
                let locs =
                    fd_find_and_parse(name, &imp.full_path, root.as_deref(), matcher.as_deref());
                if !locs.is_empty() {
                    return locs;
                }
            }
        }
    }
    vec![]
}

/// Step 3 — same-package visibility (no import needed in Kotlin).
///
/// Finds all indexed files sharing the same `package` declaration as `from_uri`
/// and searches their symbols.
fn resolve_same_package(indexer: &Indexer, name: &str, uri: &Url) -> Vec<Location> {
    // Get package name, release the dashmap ref immediately.
    let pkg: String = match indexer
        .files
        .get(uri.as_str())
        .and_then(|f| f.package.clone())
    {
        Some(p) => p,
        None => return vec![],
    };

    let peer_uris: Vec<String> = match indexer.packages.get(&pkg) {
        Some(u) => u.clone(),
        None => return vec![],
    };

    let self_str = uri.as_str();
    for peer_uri_str in &peer_uris {
        if peer_uri_str == self_str {
            continue;
        }
        if let Some(f) = indexer.files.get(peer_uri_str) {
            for sym in f.symbols.iter().filter(|s| s.name == name) {
                if let Ok(u) = Url::parse(peer_uri_str) {
                    return vec![Location {
                        uri: u,
                        range: sym.selection_range,
                    }];
                }
            }
        }
    }

    // Also check compiled JAR definitions for same-package symbols.
    if let Some(locs) = indexer.jar_definitions.get(name) {
        for loc in locs.iter() {
            if let Some(f) = indexer.jar_files.get(loc.uri.as_str()) {
                if f.package.as_ref() == Some(&pkg) {
                    return vec![loc.clone()];
                }
            }
        }
    }

    vec![]
}

/// Returns the first symbol named `name` found in the exact package `pkg`,
/// or an empty Vec if none is found.
fn symbols_in_package(indexer: &Indexer, name: &str, pkg: &str) -> Vec<Location> {
    find_symbol_in_package(indexer, name, pkg).map_or(vec![], |l| vec![l])
}

/// Scan all indexed files in `pkg` for the first symbol named `name`.
fn find_symbol_in_package(indexer: &Indexer, name: &str, pkg: &str) -> Option<Location> {
    let peer_uris: Vec<String> = indexer
        .packages
        .get(pkg)
        .map(|u| u.clone())
        .unwrap_or_default();
    for peer_uri_str in peer_uris {
        if let Some(f) = indexer.files.get(&peer_uri_str) {
            for sym in f.symbols.iter().filter(|s| s.name == name) {
                if let Ok(u) = Url::parse(&peer_uri_str) {
                    return Some(Location {
                        uri: u,
                        range: sym.selection_range,
                    });
                }
            }
        }
    }

    // Also check compiled JAR definitions.
    if let Some(locs) = indexer.jar_definitions.get(name) {
        for loc in locs.iter() {
            if let Some(f) = indexer.jar_files.get(loc.uri.as_str()) {
                if f.package.as_ref().is_some_and(|p| p == pkg) {
                    return Some(loc.clone());
                }
            }
        }
    }

    None
}

/// Step 4 — star imports: `import com.example.*`.
///
/// For each star import:
///   a. Check indexed files in that package (fast, O(files_in_package)).
///   b. If nothing found, run `rg` scoped to the package directory path
///      (handles files that were never opened / indexed).
///
/// Stdlib packages are skipped entirely.
fn resolve_star_imports(indexer: &Indexer, name: &str, uri: &Url) -> Vec<Location> {
    let star_pkgs: Vec<String> = match indexer.files.get(uri.as_str()) {
        Some(f) => f
            .imports
            .iter()
            .filter(|i| i.is_star && !is_stdlib(&i.full_path))
            .map(|i| i.full_path.clone())
            .collect(),
        None => return vec![],
    };

    for pkg in star_pkgs {
        // a) indexed files in this package
        let locs = symbols_in_package(indexer, name, &pkg);
        if !locs.is_empty() {
            return locs;
        }

        // b) rg scoped to the package directory for unindexed files
        let (root, _, matcher) = indexer.rg_scope_for_path(None);
        let locs = rg_in_package_dir(name, &pkg, root.as_deref(), matcher.as_deref());
        if !locs.is_empty() {
            return locs;
        }
    }
    vec![]
}

// ─── step 4.5: superclass / interface hierarchy ───────────────────────────────

/// Walk the superclass / interface hierarchy of the class(es) declared in
/// `from_uri` looking for a symbol named `name`.
///
/// Algorithm
/// ---------
/// 1. Extract direct supertype names from `from_uri`'s lines.
/// 2. Resolve each supertype through the normal chain (imports, same-package…).
/// 3. Search the resolved file's symbol table for `name`.
/// 4. Recurse into that file's own supertypes (depth-limited, cycle-safe).
fn resolve_from_class_hierarchy(indexer: &Indexer, name: &str, from_uri: &Url) -> Vec<Location> {
    // Deep enough for real Android/Kotlin hierarchies: app base classes often stack
    // several levels (`…Fragment → BaseFragment → … → androidx Fragment`) before the
    // library super that declares an inherited member like `requireActivity`. The
    // visited-set bounds total work regardless of depth.
    let results = walk_hierarchy(
        indexer,
        "",
        from_uri.as_str(),
        CallerContext::default(),
        12,
        |index, _, class_uri, _| find_name_in_uri(index, name, class_uri),
    );
    // Stable dedup via HashSet — diamond inheritance can produce the same location
    // via multiple paths; dedup_by only removes consecutive duplicates.
    let mut seen = HashSet::new();
    results
        .into_iter()
        .filter(|loc| {
            seen.insert((
                loc.uri.clone(),
                loc.range.start.line,
                loc.range.start.character,
            ))
        })
        .collect()
}

/// `rg` scoped to the directory that would contain `package` sources.
///
/// Package `com.example.ui` → globs `**/com/example/ui/*.{kt,java,swift}`.
/// This handles the common case where the package structure mirrors the
/// directory tree (standard Kotlin / Maven / Gradle convention).
fn rg_in_package_dir(
    name: &str,
    package: &str,
    root: Option<&Path>,
    matcher: Option<&crate::rg::IgnoreMatcher>,
) -> Vec<Location> {
    let Some(_guard) = crate::rg::try_acquire_rg_slot() else {
        log::debug!("rg_in_package_dir: at capacity, skipping {name}");
        return vec![];
    };
    let pkg_path = package.replace('.', "/");
    let pattern = build_rg_pattern(name);

    let search_root: std::borrow::Cow<Path> = match root {
        Some(r) => std::borrow::Cow::Borrowed(r),
        None => std::borrow::Cow::Owned(std::env::current_dir().unwrap_or_default()),
    };

    let mut cmd = Command::new("rg");
    cmd.args([
        "--no-heading",
        "--with-filename",
        "--line-number",
        "--column",
    ]);
    for ext in crate::rg::SOURCE_EXTENSIONS {
        // Positive globs first — negative globs must come after to avoid being
        // overridden by later positive globs (rg: last matching glob wins).
        cmd.args(["--glob", &format!("**/{pkg_path}/*.{ext}")]);
    }
    cmd.args(["-e", &pattern]);
    cmd.arg(search_root.as_ref());

    let out = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return vec![],
    };

    let locs: Vec<Location> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_rg_line)
        .collect();
    match matcher {
        Some(m) => m.filter_locs(locs),
        None => locs,
    }
}

// ─── shared helpers ───────────────────────────────────────────────────────────

/// Returns `true` if the package directory derived from `import_path` exists as a
/// subdirectory of at least one search root.
///
/// `android.os.Bundle` → pkg_dir `android/os` → checks `{root}/android/os/`.
///
/// A single `stat()` call per root replaces the need for a hardcoded stdlib
/// blocklist: if the directory doesn't exist in the project tree, no fd/rg
/// subprocess can find anything there either.
///
/// Returns `true` (allow search) when the package prefix is empty or no roots
/// are available — the conservative fallback.
fn package_dir_in_source_roots(
    import_path: &str,
    root: Option<&std::path::Path>,
    source_roots: &[String],
) -> bool {
    let pkg = import_package_prefix(import_path);
    if pkg.is_empty() {
        return true;
    }
    let pkg_dir = pkg.replace('.', "/");
    let search_roots: Vec<&std::path::Path> = if !source_roots.is_empty() {
        source_roots
            .iter()
            .map(|s| std::path::Path::new(s.as_str()))
            .collect()
    } else if let Some(r) = root {
        vec![r]
    } else {
        return true;
    };
    search_roots.iter().any(|r| r.join(&pkg_dir).is_dir())
}

/// Returns `true` when `name` has an explicit non-star import in `uri` AND
/// that import's package directory is absent from every source root.
///
/// When both conditions hold, `resolve_via_imports` already exhausted all
/// source-tree lookups (qualified index + definitions index + fd) and came up
/// empty.  A project-wide `rg` scan of the same source tree cannot add anything.
fn import_package_absent_from_source_roots(
    indexer: &Indexer,
    name: &str,
    uri: &Url,
    root: Option<&std::path::Path>,
    source_roots: &[String],
) -> bool {
    let Some(file_data) = indexer.files.get(uri.as_str()) else {
        return false;
    };
    let Some(imp) = file_data
        .imports
        .iter()
        .find(|i| !i.is_star && i.local_name == name)
    else {
        return false;
    };
    !package_dir_in_source_roots(&imp.full_path, root, source_roots)
}

/// Returns true for packages whose sources aren't present in a typical project.
///
/// Kotlin automatically imports `kotlin.*` and `kotlin.collections.*` etc.
/// Android projects don't ship `android.*` / `androidx.*` sources by default.
/// Swift: framework imports like Foundation, UIKit, etc. have no local sources.
pub(crate) fn is_stdlib(pkg: &str) -> bool {
    // Check dotted prefixes before splitting.
    if pkg.starts_with("com.sun") {
        return true;
    }
    let first = pkg.split('.').next().unwrap_or("");
    matches!(
        first,
        "kotlin" | "java" | "javax" | "android" | "androidx" | "sun"
        // Swift standard frameworks
        | "Foundation" | "UIKit" | "SwiftUI" | "Combine" | "CoreData"
        | "CoreGraphics" | "CoreLocation" | "MapKit" | "AVFoundation"
        | "WebKit" | "StoreKit" | "GameKit" | "ARKit" | "RealityKit"
        | "Swift" | "ObjectiveC" | "Darwin" | "Dispatch" | "os"
    )
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
        resolve_qualified(self, name, qualifier, from_uri)
    }
}
