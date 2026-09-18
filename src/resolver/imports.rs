//! Explicit-import resolution and Kotlin's own default-import package/type sets.

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;
use crate::StrExt;

use super::container::{enclosing_container_chain, import_container_chain};
use super::fd::{fd_find_and_parse, import_package_prefix};
use super::package::{jar_symbol_package, package_dir_in_source_roots};

/// Whether `from_uri` has an explicit (non-star) import whose local name is `name`
/// — `import a.b.Name` or `import a.b.Whatever as Name`. Presence alone proves the
/// name is in scope; the target FQN need not be indexed.
pub(super) fn has_explicit_import(indexer: &Indexer, name: &str, from_uri: &Url) -> bool {
    indexer
        .files
        .get(from_uri.as_str())
        .map(|f| f.imports.iter().any(|i| !i.is_star && i.local_name == name))
        .unwrap_or(false)
}

/// Kotlin's own default-import package set: every one of these is available
/// on every Kotlin/JVM file with no explicit `import` ever needed, by
/// definition of the language (Kotlin reference, "Default imports"). A
/// hardcoded LANGUAGE fact, not project data that could be stale or
/// incomplete — safe to trust unconditionally, the same spirit as
/// [`DENYLISTED_PACKAGE_PREFIXES`], just a preference instead of an
/// exclusion.
pub(super) const KOTLIN_DEFAULT_IMPORT_PACKAGES: &[&str] = &[
    "kotlin",
    "kotlin.annotation",
    "kotlin.collections",
    "kotlin.comparisons",
    "kotlin.io",
    "kotlin.ranges",
    "kotlin.sequences",
    "kotlin.text",
    "kotlin.jvm",
    "java.lang",
];

/// Kotlin's implicit default-import packages (JVM target): names declared directly
/// in these are in scope in every file without an `import`. Narrower than
/// [`is_stdlib`] — `android`/`androidx`/most `java.*` are *not* auto-imported.
pub(super) fn is_default_import_package(pkg: &str) -> bool {
    KOTLIN_DEFAULT_IMPORT_PACKAGES.contains(&pkg)
}

/// Core `kotlin.*` types from the language's default imports (`kotlin`,
/// `kotlin.collections`, …). The companion of [`is_default_import_package`] at the
/// type level: both encode the spec-defined default-import set so a bare `Number` /
/// `List` / `Result` isn't treated as a missing import when the (rarely-indexed)
/// kotlin-stdlib jar provides no concrete symbol to confirm it.
fn is_default_import_type(name: &str) -> bool {
    matches!(
        name,
        // kotlin.* primitives & core types
        "Number" | "Byte" | "Short" | "Int" | "Long" | "Float" | "Double" | "Char"
        | "Boolean" | "String" | "CharSequence" | "Any" | "Unit" | "Nothing"
        | "Comparable" | "Enum" | "Annotation" | "Function" | "Lazy" | "Result"
        | "Pair" | "Triple" | "Throwable" | "Exception" | "Error" | "RuntimeException"
        // kotlin.collections.* (default-imported)
        | "Array" | "Iterable" | "Iterator" | "Collection" | "List" | "Set" | "Map"
        | "MutableIterable" | "MutableCollection" | "MutableList" | "MutableSet"
        | "MutableMap" | "ArrayList" | "HashMap" | "HashSet" | "LinkedHashMap"
        | "LinkedHashSet"
        // kotlin.* specialized primitive array types (default-imported, same as
        // `Array` above) -- real, measured false-positive source: `ByteArray`
        // alone was 45% of the missing-import POC's total flags on the Moneta
        // corpus before this fix.
        | "ByteArray" | "CharArray" | "ShortArray" | "IntArray" | "LongArray"
        | "FloatArray" | "DoubleArray" | "BooleanArray"
        // kotlin.sequences.*
        | "Sequence"
    )
}

/// Whether `name` is available without an import because a symbol of that name is
/// declared in a Kotlin default-import package (e.g. `kotlin.Result`, `kotlin.apply`),
/// or `name` is itself one of the core default-import types.
///
/// Checks `jar_definitions`/`definitions` directly (by package), not the narrower
/// `importable_fqns` cache — that cache only holds container-less symbols recorded
/// for auto-import completion, and top-level `kotlin.*` functions (`error`, `run`,
/// `with`, `repeat`, …) aren't reliably captured there, so a `fqns_for_name`-only
/// check would silently miss them and flag real stdlib calls as missing imports.
pub(super) fn resolvable_via_default_import(indexer: &Indexer, name: &str) -> bool {
    if is_default_import_type(name) {
        return true;
    }
    // Promote-before-read (zero budget): this runs on the diagnostics/keystroke
    // path — no blocking sidecar IPC here. Without this, a default-import
    // top-level function (kotlin.error, kotlin.with, …) whose JAR hasn't been
    // materialized yet reads as absent and gets flagged as a missing import.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(indexer, name, &mut cache_backed_only);
    if let Some(locs) = indexer.jar_definitions.get(name) {
        for loc in locs.iter() {
            if jar_symbol_package(indexer, loc)
                .as_deref()
                .is_some_and(is_default_import_package)
            {
                return true;
            }
            if indexer
                .jar_files
                .get(loc.uri.as_str())
                .and_then(|fd| fd.package.clone())
                .as_deref()
                .is_some_and(is_default_import_package)
            {
                return true;
            }
        }
    }
    if let Some(sym_locs) = indexer.definitions.get(name) {
        for sym_loc in sym_locs.iter() {
            let Some(loc) = indexer.file_table.location(*sym_loc) else {
                continue;
            };
            if indexer
                .files
                .get(loc.uri.as_str())
                .and_then(|fd| fd.package.clone())
                .as_deref()
                .is_some_and(is_default_import_package)
            {
                return true;
            }
        }
    }
    false
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
pub(super) fn resolve_via_imports(
    indexer: &Indexer,
    name: &str,
    uri: &Url,
    allow_fd: bool,
) -> Vec<Location> {
    let imports: Vec<crate::types::ImportEntry> = match indexer.files.get(uri.as_str()) {
        Some(f) => f.imports.iter().filter(|i| !i.is_star).cloned().collect(),
        None => return vec![],
    };

    for imp in imports.iter().filter(|i| i.local_name == name) {
        // i) qualified index — exact FQN (works for top-level classes).
        //    `qualified` stores an interned `SymbolLoc`; reconstitute the
        //    `Location` here, at the return boundary.
        if let Some(sym_loc) = indexer.qualified.get(&imp.full_path) {
            match indexer.file_table.location(*sym_loc) {
                Some(loc) => return vec![loc],
                // Unreachable by construction: every SymbolLoc in `qualified`
                // was interned before insert and FileIds are never reused.
                // Loud in dev builds; release degrades to the fallback ladder
                // below rather than mis-resolving.
                None => debug_assert!(
                    false,
                    "qualified SymbolLoc has no file_table entry for {}",
                    imp.full_path
                ),
            }
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
            // Reconstitute interned `SymbolLoc`s at this boundary.
            all_locations.extend(
                locs.iter()
                    .filter_map(|sym_loc| indexer.file_table.location(*sym_loc)),
            );
        }
        // Promote-before-read (zero budget): an imported name whose JAR is
        // Tier-1-only must become visible here — this exact read shipped the
        // first promote-AFTER-read ordering bug. Blocking IPC for imports
        // already happens at file open (per-import promotion), and this
        // helper also runs on keystroke/diagnostics paths, so only free
        // cache-backed promotions are allowed.
        let mut cache_backed_only = 0usize;
        crate::indexer::jar::ensure_jar_definitions_for(indexer, short, &mut cache_backed_only);
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
