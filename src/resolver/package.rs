//! Package-of-a-location lookups and package-directory / source-root primitives.

use std::path::Path;
use std::process::Command;

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;
use crate::rg::{build_rg_pattern, parse_rg_line};

use super::fd::import_package_prefix;

/// `location`'s own real package — same three-map fallback chain
/// [`is_denylisted_package_prefix`]'s doc comment explains (per-symbol JAR
/// package first, then a regular source file's single package, then a
/// compiled-only JAR entry's first-symbol-derived package). Shared by every
/// tie-break in [`ambiguity_safe_tail_with_denylist`] that needs to compare
/// a candidate's package against something else.
///
/// A Copilot review pass on this PR flagged the third (coarse, whole-JAR)
/// tier as imprecise for a multi-package JAR — true in the abstract, but
/// dropping it (tried and reverted here) broke three real, already-passing
/// tests that specifically exercise `default_kotlin_import_tie_break` and
/// `import_package_tie_break` against the exact shape this codebase's own
/// JAR tests build: a compiled-JAR-derived candidate with package data
/// available ONLY at the whole-file level (no `jar_symbol_packages` entry).
/// That shape is the common case for a single-package-per-artifact JAR, not
/// the rare one — keeping the fallback is the tested, intentional trade-off.
pub(super) fn location_package(indexer: &Indexer, location: &Location) -> Option<String> {
    jar_symbol_package(indexer, location)
        .or_else(|| {
            indexer
                .files
                .get(location.uri.as_str())
                .and_then(|f| f.package.clone())
        })
        .or_else(|| {
            indexer
                .jar_files
                .get(location.uri.as_str())
                .and_then(|f| f.package.clone())
        })
}

/// Package of the JAR symbol at `loc`, from the `jar_symbol_packages` side table.
/// JAR symbols use a synthetic range whose line number equals the symbol's index
/// within the jar's `FileData.symbols`, so the line indexes the package vector.
/// Returns `None` when unknown (no entry, or pre-per-symbol-package jar cache).
pub(crate) fn jar_symbol_package(indexer: &Indexer, loc: &Location) -> Option<String> {
    let packages = indexer.jar_symbol_packages.get(loc.uri.as_str())?;
    packages
        .get(loc.range.start.line as usize)
        .filter(|p| !p.is_empty())
        .cloned()
}

/// `rg` scoped to the directory that would contain `package` sources.
///
/// Package `com.example.ui` → globs `**/com/example/ui/*.{kt,java,swift}`.
/// This handles the common case where the package structure mirrors the
/// directory tree (standard Kotlin / Maven / Gradle convention).
pub(super) fn rg_in_package_dir(
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
pub(super) fn package_dir_in_source_roots(
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
pub(super) fn import_package_absent_from_source_roots(
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
