//! Ambiguity tie-breaks: narrowing a same-named candidate set down to one,
//! in decreasing order of certainty (denylist → module-scoped Gradle deps →
//! Kotlin default imports → the calling file's own import list).

use std::collections::HashSet;

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;

use super::fd::import_package_prefix;
use super::imports::KOTLIN_DEFAULT_IMPORT_PACKAGES;
use super::package::location_package;

/// Package prefixes that can never be a legitimate app-facing reference of
/// ANY kind — not just a supertype. `com.android.internal.*` is Android's
/// hidden, non-public platform-internal API surface: it never ships in the
/// public SDK a real app compiles against, so no ordinary app source can
/// reference it as a supertype, an import, a variable's type, or anything
/// else. That's a property of the package itself, not of which resolution
/// step happens to be looking a name up — the same invariant is what makes
/// this denylist safe to share across every caller of
/// [`ambiguity_safe_tail_with_denylist`] (originally only
/// [`ResolveIo::HierarchyAmbiguitySafe`]'s supertype walk, now also
/// [`ResolveIo::IndexOnly`]'s general bare-name tail), not just the one it
/// was first evidenced against.
///
/// Checked only as a last-resort tie-break once a lookup has already found
/// more than one same-named candidate. Deliberately narrow (currently a
/// single entry, the one directly evidenced by the real repro): a denylist
/// can only ever fail to help, never introduce a new wrong preference the
/// way a broader "prefer this package family" heuristic could — see the
/// design doc's self-critique. Do not grow this into a general
/// preference-ranking system.
const DENYLISTED_PACKAGE_PREFIXES: &[&str] = &["com.android.internal."];

/// Tail fallback shared by [`ResolveIo::HierarchyAmbiguitySafe`] (the
/// hierarchy walk's own per-hop resolution), [`ResolveIo::IndexOnly`]
/// (general bare-name resolution — diagnostics, `resolve_qualified`'s
/// qualifier-root lookup, etc.), and `Full`'s own equivalent tail (see
/// `resolve_chain`'s step 5.4): a unique candidate wins outright; an
/// ambiguous set gets four narrow tie-breaks in sequence — first
/// [`DENYLISTED_PACKAGE_PREFIXES`] (unconditional, project-wide), then
/// [`module_scoped_tie_break`] (real per-module Gradle dependency data, when
/// `workspace.json` provides it), then [`default_kotlin_import_tie_break`]
/// (Kotlin's own hardcoded default-import package set — a language fact, not
/// project data), then [`import_package_tie_break`] (the calling file's own
/// already-parsed import list, always available — no external data needed) —
/// before still declining unless one of them leaves exactly one candidate.
/// See the real-workspace-json-schema design doc's §5 for why denylist-first
/// is correct: it's unconditional and needs no loaded data. The remaining
/// three run in *decreasing* certainty: module-scoped narrowing is the real
/// Gradle dependency graph (most precise, but only as available as
/// `workspace.json`'s own data); Kotlin's default imports are a fixed
/// language-level fact, always available, but only relevant when a candidate
/// actually lives in one of those packages; import-package narrowing is the
/// weakest — a file merely importing a *sibling* from the right package, not
/// the ambiguous name itself, so it runs last.
///
/// Crucially, each tie-break's authority carries forward even when it
/// doesn't land on a unique winner by itself: when module-scoped narrowing
/// (or default-import narrowing) proves some candidates more plausible than
/// others without reaching uniqueness, the NEXT tie-break is only ever
/// handed that narrowed subset, never the original, unnarrowed set — a
/// candidate an earlier, stronger tie-break has already disproven (or simply
/// left out) must never be resurrected by a later, weaker signal. The
/// original, unnarrowed set only ever reaches a later tie-break when the
/// earlier one had nothing to say at all (no data, for module-scoping; no
/// candidate in a default-import package, for Kotlin's default imports —
/// see [`ModuleScopedOutcome`] and [`default_kotlin_import_tie_break`]'s own
/// doc). `origin_uri` is the real file to resolve an owning module/import
/// list from — the hierarchy walk's own starting file for
/// `HierarchyAmbiguitySafe`, or simply the caller's own `from_uri` for
/// `IndexOnly`/`Full`.
pub(super) fn ambiguity_safe_tail_with_denylist(
    indexer: &Indexer,
    origin_uri: &Url,
    locations: Vec<Location>,
) -> Vec<Location> {
    if locations.len() == 1 {
        return locations;
    }
    if locations.is_empty() {
        return vec![];
    }
    let filtered: Vec<Location> = locations
        .into_iter()
        .filter(|location| !is_denylisted_package_prefix(indexer, location))
        .collect();
    if filtered.len() == 1 {
        return filtered;
    }
    if filtered.len() < 2 {
        return vec![];
    }
    let after_module_scope = match module_scoped_tie_break(indexer, origin_uri, filtered.clone()) {
        ModuleScopedOutcome::Narrowed(narrowed) if narrowed.len() == 1 => return narrowed,
        ModuleScopedOutcome::Narrowed(narrowed) => narrowed,
        ModuleScopedOutcome::NoData | ModuleScopedOutcome::NoDependenciesSurvived => filtered,
    };
    let after_default_import = default_kotlin_import_tie_break(indexer, after_module_scope);
    if after_default_import.len() == 1 {
        return after_default_import;
    }
    import_package_tie_break(indexer, origin_uri, after_default_import)
}

/// Tie-break run between [`module_scoped_tie_break`] and
/// [`import_package_tie_break`]: when one or more candidates live in a
/// package Kotlin implicitly imports for every file (see
/// [`KOTLIN_DEFAULT_IMPORT_PACKAGES`]), prefer those over any candidate that
/// would need a real import — or a real receiver-typed member match, which
/// the caller already tried and failed, or this tail would never run — to be
/// reachable at all. Real, measured case: `apply`/`run` (Kotlin's own scope
/// functions, `kotlin.apply`/`kotlin.run`) collide with hundreds of
/// unrelated same-named JVM/Android members (`java.util.function.
/// Function.apply`, `Runnable.run`, countless builder `.apply()` methods)
/// across a real dependency graph — none of THOSE are ever reachable
/// without an explicit import, so a default-imported candidate is always at
/// least as plausible, and in this fallback's context (a bare-name retry
/// after receiver-scoped lookup already failed) is virtually always the
/// actually-intended target.
///
/// Only narrows, never fully declines: when no candidate is in a
/// default-import package, this is a no-op and the original set passes
/// through unchanged to the next tie-break.
fn default_kotlin_import_tie_break(indexer: &Indexer, locations: Vec<Location>) -> Vec<Location> {
    let narrowed: Vec<Location> = locations
        .iter()
        .filter(|location| {
            location_package(indexer, location)
                .is_some_and(|pkg| KOTLIN_DEFAULT_IMPORT_PACKAGES.contains(&pkg.as_str()))
        })
        .cloned()
        .collect();
    if narrowed.is_empty() {
        locations
    } else {
        narrowed
    }
}

/// Outcome of [`module_scoped_tie_break`] consulting real per-module Gradle
/// dependency data for an ambiguous candidate set — three real, distinct
/// cases the caller must not collapse into a single "declined" bucket (see
/// [`ambiguity_safe_tail_with_denylist`]'s doc comment for why the
/// distinction matters).
enum ModuleScopedOutcome {
    /// No `workspace.json` module-dependency data is available at all for
    /// `origin_uri`'s owning module (`owning_module_dependencies` returned
    /// `None`) — module-scoping has nothing to say, so the next tie-break
    /// must fall back to the original, unnarrowed candidate set.
    NoData,
    /// Dependency data WAS available, but none of the candidates are real
    /// dependencies of the calling module. Treated the same as `NoData`
    /// rather than as a proof that every candidate is wrong: a workspace's
    /// dependency data can be incomplete (see `owning_module_dependencies`'s
    /// own doc comment), so eliminating every candidate is more likely a
    /// data gap than a genuine "none of these are possible" result — the
    /// next tie-break falls back to the original, unnarrowed set too.
    NoDependenciesSurvived,
    /// Dependency data narrowed the candidates to a real, positive,
    /// non-empty subset — every remaining candidate is a proven dependency
    /// of the calling module. Length 1 is a unique winner the caller returns
    /// immediately; length > 1 is still ambiguous, but the next tie-break
    /// must only ever choose among THESE candidates, never a candidate this
    /// narrowing already ruled out.
    Narrowed(Vec<Location>),
}

/// Second tie-break for [`ambiguity_safe_tail_with_denylist`]: when the
/// denylist alone still leaves more than one candidate, narrow using the
/// hierarchy walk's real starting file's own module's real Gradle dependency
/// set (see `workspace_json::load_module_dependencies`) — a candidate
/// survives only if its own JAR's `(group, artifact, version)` is a
/// dependency of the module `hierarchy_walk_origin_uri` belongs to. See
/// [`ModuleScopedOutcome`] for the three distinct outcomes this can produce
/// and how the caller must treat each one.
fn module_scoped_tie_break(
    indexer: &Indexer,
    hierarchy_walk_origin_uri: &Url,
    locations: Vec<Location>,
) -> ModuleScopedOutcome {
    let Some(dependencies) = owning_module_dependencies(indexer, hierarchy_walk_origin_uri) else {
        return ModuleScopedOutcome::NoData;
    };
    let narrowed: Vec<Location> = locations
        .into_iter()
        .filter(|location| {
            candidate_gradle_meta(location).is_some_and(|meta| dependencies.contains(&meta))
        })
        .collect();
    if narrowed.is_empty() {
        ModuleScopedOutcome::NoDependenciesSurvived
    } else {
        ModuleScopedOutcome::Narrowed(narrowed)
    }
}

/// Third tie-break for [`ambiguity_safe_tail_with_denylist`]: when the
/// denylist and module-scoped narrowing still leave more than one candidate,
/// narrow using `origin_uri`'s own imports — not of the ambiguous name
/// itself (`resolve_via_imports` already tried that, earlier in the chain,
/// and failed, or this tail would never have been reached) but of any OTHER
/// symbol from the same package. A file that writes `import
/// kotlinx.coroutines.async` but never spells out `Deferred` (used only
/// through inference, e.g. `scope.async { }.await()`) still tells us
/// `kotlinx.coroutines` is a real, in-use package for this file — evidence
/// an unrelated same-named decoy from a package this file never otherwise
/// references (real, measured case: `com.google.firebase.components.Deferred`)
/// doesn't have. Weaker than [`module_scoped_tie_break`] (an inference from
/// unrelated imports, not the real dependency graph), so tried after it, but
/// needs no `workspace.json` data at all — narrows real cases that
/// module-scoping can't when a workspace has no module-dependency data
/// loaded (e.g. a `workspace.json` with only `sourcePaths`, no `libraries`).
///
/// A star import (`import com.foo.bar.*`) counts too, as evidence for its
/// own exact package only: `ImportEntry::full_path` for a star import is
/// already the bare package (there's no trailing symbol segment to strip,
/// unlike a non-star import's `full_path`), so it's used as-is rather than
/// through [`import_package_prefix`] — passing a star import's `full_path`
/// through that helper would wrongly strip its own last segment as if it
/// were an imported symbol, over-widening `com.foo.bar` to `com.foo`.
pub(super) fn import_package_tie_break(
    indexer: &Indexer,
    origin_uri: &Url,
    locations: Vec<Location>,
) -> Vec<Location> {
    let Some(file_data) = indexer.files.get(origin_uri.as_str()) else {
        return vec![];
    };
    let imported_packages: std::collections::HashSet<String> = file_data
        .imports
        .iter()
        .map(|i| {
            if i.is_star {
                i.full_path.clone()
            } else {
                import_package_prefix(&i.full_path)
            }
        })
        .collect();
    if imported_packages.is_empty() {
        return vec![];
    }
    let narrowed: Vec<Location> = locations
        .into_iter()
        .filter(|location| {
            location_package(indexer, location).is_some_and(|pkg| imported_packages.contains(&pkg))
        })
        .collect();
    if narrowed.len() == 1 {
        narrowed
    } else {
        vec![]
    }
}

/// Looks up `from_uri`'s owning module's real Gradle dependency set: the
/// content-root directory that is the longest-prefix match of `from_uri`'s
/// file path (see `workspace_json::load_module_dependencies`'s own doc
/// comment for why this is the correct module-identity lookup, not a
/// `build.gradle.kts`-nearest-ancestor heuristic). A cheap lookup against
/// already-loaded, pre-parsed state — no file I/O or parsing on this path.
/// Returns `None` when `from_uri` isn't a real file path (e.g. a `jar:`
/// synthetic URI — [`module_scoped_tie_break`]'s caller passes the hierarchy
/// walk's real starting file for this reason, not the current hop's own URI)
/// or no `workspace.json` module data was loaded for this workspace.
fn owning_module_dependencies(
    indexer: &Indexer,
    from_uri: &Url,
) -> Option<HashSet<crate::cli::extract_sources::GradleMeta>> {
    let file_path = crate::path_util::path_from_uri(from_uri)?;
    let dependencies_by_content_root = indexer
        .module_dependencies
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    dependencies_by_content_root
        .iter()
        .filter(|(content_root, _)| file_path.starts_with(content_root.as_path()))
        .max_by_key(|(content_root, _)| content_root.as_os_str().len())
        .map(|(_, dependencies)| dependencies.clone())
}

/// Resolves a candidate hierarchy-walk `Location`'s own JAR path to its
/// Gradle coordinates, via the existing [`crate::cli::extract_sources::parse_jar_meta`]
/// (design doc §2 point 3's cross-check opportunity). Handles both a
/// compiled-only JAR URI (`jar:file://<jar>`, no entry — how `jar_definitions`
/// locations are shaped) and a sources-JAR entry URI (`jar:file://<jar>!/<entry>`);
/// [`crate::jar_extract::parse_jar_entry_uri`] only handles the latter shape,
/// so it is not reused here. Returns `None` for a non-`jar:` URI or an
/// unparseable jar path, never a wrong guess.
fn candidate_gradle_meta(location: &Location) -> Option<crate::cli::extract_sources::GradleMeta> {
    let rest = location.uri.as_str().strip_prefix("jar:")?;
    let jar_part = rest.split_once("!/").map_or(rest, |(jar, _)| jar);
    let jar_path = crate::path_util::path_from_uri(&Url::parse(jar_part).ok()?)?;
    crate::cli::extract_sources::parse_jar_meta(&jar_path)
}

/// Whether `location` has a package matching one of
/// [`DENYLISTED_PACKAGE_PREFIXES`]. Tries [`jar_symbol_package`] first — a
/// real compiled JAR spans many packages across its symbols, so `location`'s
/// own accurate per-symbol package (the `jar_symbol_packages` side table) is
/// checked before `indexer.jar_files`' single `FileData.package`, which
/// `build_jar_file_data` derives from only the FIRST class-like symbol it
/// happens to find and is therefore not necessarily `location`'s own real
/// package. Falls back to `indexer.files` (regular source files, always
/// exactly one package each, so no per-symbol ambiguity exists) and then
/// `indexer.jar_files` (compiled-only entries pre-dating the per-symbol
/// cache, or files this side table has no entry for at all) — same two-map
/// lookup order as [`crate::indexer::infer::sig::collect_params_from_file`].
/// Locations with no known package anywhere are never treated as
/// denylisted — the tie-break must only ever remove a candidate it can
/// positively prove is denylisted.
fn is_denylisted_package_prefix(indexer: &Indexer, location: &Location) -> bool {
    let Some(package) = location_package(indexer, location) else {
        return false;
    };
    DENYLISTED_PACKAGE_PREFIXES
        .iter()
        .any(|prefix| package.starts_with(prefix))
}
