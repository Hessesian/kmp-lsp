//! Same-package and star-import resolution: everything visible without an
//! explicit `import` beyond the calling file itself.

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;

use super::package::{jar_symbol_package, location_package, rg_in_package_dir};
use super::platform_types::is_stdlib;

/// Returns the first Location found by scanning star-import packages.
pub(super) fn find_in_star_imports(
    indexer: &Indexer,
    name: &str,
    star_pkgs: &[String],
) -> Option<Location> {
    for pkg in star_pkgs {
        if let Some(loc) = find_symbol_in_package(indexer, name, pkg) {
            return Some(loc);
        }
    }
    None
}

/// Step 3 — same-package visibility (no import needed in Kotlin).
///
/// Finds all indexed files sharing the same `package` declaration as `from_uri`
/// and searches their symbols.
pub(super) fn resolve_same_package(indexer: &Indexer, name: &str, uri: &Url) -> Vec<Location> {
    // Get package name, release the dashmap ref immediately.
    let pkg: String = match indexer
        .files
        .get(uri.as_str())
        .and_then(|f| f.package.clone())
    {
        Some(p) => p,
        None => return vec![],
    };

    let peer_ids: Vec<crate::types::FileId> = match indexer.packages.get(&pkg) {
        Some(ids) => ids.clone(),
        None => return vec![],
    };

    let self_str = uri.as_str();
    for peer_id in &peer_ids {
        let Some(peer_url) = indexer.file_table.url(*peer_id) else {
            continue;
        };
        let peer_uri_str = peer_url.as_str();
        if peer_uri_str == self_str {
            continue;
        }
        if let Some(f) = indexer.files.get(peer_uri_str) {
            if let Some(sym) = f.symbols.iter().find(|s| s.name == name) {
                return vec![Location {
                    uri: (*peer_url).clone(),
                    range: sym.selection_range,
                }];
            }
        }
    }

    // Also check compiled JAR definitions for same-package symbols.
    // Promote-before-read (zero budget): serves all three resolution policies,
    // including keystroke/diagnostics paths — no blocking sidecar IPC here.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(indexer, name, &mut cache_backed_only);
    if let Some(locs) = indexer.jar_definitions.get(name) {
        for loc in locs.iter() {
            // `jar_symbol_package` (the sidecar's real per-symbol package)
            // first, NOT `indexer.jar_files.get(...).package` — that whole-JAR
            // fallback is `build_jar_file_data`'s guess from the FIRST
            // class-like symbol's `detail` text, which the sidecar's
            // pure-Java fallback (`JavaClassVisitor`, e.g. Android's
            // AAPT-generated `R.jar`) never includes a package in (`"class
            // R"`, not `"class pkg.R"`) — so for a jar like that, this same-
            // package check could never fire at all without the per-symbol
            // table, regardless of how many symbols really live in `pkg`.
            if location_package(indexer, loc).as_ref() == Some(&pkg) {
                return vec![loc.clone()];
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
pub(crate) fn find_symbol_in_package(indexer: &Indexer, name: &str, pkg: &str) -> Option<Location> {
    let peer_ids: Vec<crate::types::FileId> = indexer
        .packages
        .get(pkg)
        .map(|ids| ids.clone())
        .unwrap_or_default();
    for peer_id in peer_ids {
        let Some(peer_url) = indexer.file_table.url(peer_id) else {
            continue;
        };
        if let Some(f) = indexer.files.get(peer_url.as_str()) {
            if let Some(sym) = f.symbols.iter().find(|s| s.name == name) {
                return Some(Location {
                    uri: (*peer_url).clone(),
                    range: sym.selection_range,
                });
            }
        }
    }

    // Also check compiled JAR definitions.
    // Promote-before-read (zero budget): star-import scans run per-name on
    // keystroke/diagnostics paths — no blocking sidecar IPC here.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(indexer, name, &mut cache_backed_only);
    if let Some(locs) = indexer.jar_definitions.get(name) {
        for loc in locs.iter() {
            // `jar_symbol_package` (this location's own real per-symbol
            // package) first — a real JAR spans many packages, so
            // `FileData.package` (the whole synthetic file's first-symbol
            // guess, checked only as a fallback) is not necessarily this
            // specific symbol's own package. Same reasoning as
            // `is_denylisted_package_prefix`.
            let candidate_pkg = jar_symbol_package(indexer, loc).or_else(|| {
                indexer
                    .jar_files
                    .get(loc.uri.as_str())
                    .and_then(|f| f.package.clone())
            });
            if candidate_pkg.as_deref() == Some(pkg) {
                return Some(loc.clone());
            }
        }
    }

    None
}

/// Non-stdlib star-import packages (`import com.example.*`) visible from `uri`,
/// i.e. `ImportEntry::full_path` for every `is_star` import whose package is
/// not `java.*`/`kotlin.*`/`android.*`/`androidx.*`. Stdlib star imports are
/// excluded here because there is no locally-indexed source to search for
/// them (see [`resolve_in_scope_strict`]'s separate, deliberate stdlib check).
/// Returns an empty `Vec` when `uri` is not indexed.
pub(super) fn star_import_packages(indexer: &Indexer, uri: &Url) -> Vec<String> {
    match indexer.files.get(uri.as_str()) {
        Some(f) => f
            .imports
            .iter()
            .filter(|i| i.is_star && !is_stdlib(&i.full_path))
            .map(|i| i.full_path.clone())
            .collect(),
        None => vec![],
    }
}

/// Step 4 — star imports: `import com.example.*`.
///
/// For each star import:
///   a. Check indexed files in that package (fast, O(files_in_package)).
///   b. If nothing found, run `rg` scoped to the package directory path
///      (handles files that were never opened / indexed).
///
/// Stdlib packages are skipped entirely.
pub(super) fn resolve_star_imports(indexer: &Indexer, name: &str, uri: &Url) -> Vec<Location> {
    let star_pkgs = star_import_packages(indexer, uri);

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
