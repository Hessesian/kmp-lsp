//! The missing-import diagnostic's two reachability predicates: "is `name`
//! reachable from this file's own scope alone?" and "does `receiver` provide
//! `name` as a member or in-scope extension?". Not resolution-chain code —
//! `src/features/missing_import_diagnostics.rs` is their only consumer.

use tower_lsp::lsp_types::Url;

use crate::indexer::Indexer;
use crate::types::CallerContext;
use crate::StrExt;

use super::container::enclosing_container_chain;
use super::find::{find_local_declaration, find_name_in_uri};
use super::hierarchy::walk_hierarchy;
use super::imports::{has_explicit_import, resolvable_via_default_import, resolve_via_imports};
use super::package_scope::{find_in_star_imports, resolve_same_package, star_import_packages};
use super::platform_types::is_stdlib;
use super::qualified::resolve_from_class_hierarchy;
use super::resolve::resolve_local;

/// Strict in-scope reachability check for missing-import detection.
///
/// Answers "is `name` reachable from *this file's own scope alone*?" — i.e. via a
/// local/param declaration, an explicit import, the same package, or a non-stdlib
/// star import. Unlike [`resolve_symbol_no_rg`] it deliberately OMITS the global
/// definitions fallback (and rg): a name that exists *somewhere* in the index but
/// isn't reachable here is exactly a missing-import candidate, so we must not let the
/// global index mask it.
pub(crate) fn resolve_in_scope_strict(indexer: &Indexer, name: &str, from_uri: &Url) -> bool {
    if !resolve_local(indexer, name, from_uri, None).is_empty() {
        return true;
    }
    // An explicit import of `name` (`import a.b.Name` / `… as Alias`) brings the symbol
    // into scope — so it is NOT a missing import, even when the target FQN isn't indexed
    // (e.g. `android.widget.Button`, `java.util.Calendar` whose SDK jars aren't indexed).
    if has_explicit_import(indexer, name, from_uri) {
        return true;
    }
    // Available without an import via Kotlin's default-import packages (kotlin.*, …).
    if resolvable_via_default_import(indexer, name) {
        return true;
    }
    // Function parameters / local vals without an indexed symbol (line scan).
    if !name.starts_with_uppercase() && !find_local_declaration(indexer, name, from_uri).is_empty()
    {
        return true;
    }
    // Index-only import resolution (no fd subprocess) — covers explicit imports.
    if !resolve_via_imports(indexer, name, from_uri, false).is_empty() {
        return true;
    }
    // Star import of a *class's* members (`import Foo.*` brings in `Foo`'s nested
    // types / enum entries / companion members) — distinct from `resolve_via_imports`
    // above, which only handles package-level star imports.
    {
        let (parent, pkg) = indexer.resolve_symbol_via_import(from_uri, name);
        if parent.is_some() || pkg.is_some() {
            return true;
        }
    }
    if !resolve_same_package(indexer, name, from_uri).is_empty() {
        return true;
    }
    // A star import of a stdlib-shaped package (`java.*`/`kotlin.*`/`android.*`/
    // `androidx.*`) has no locally-indexed source for `find_in_star_imports`
    // below to search, so it's excluded from that search entirely — but for
    // THIS check (does some import plausibly cover `name`, at all), that
    // exclusion is wrong: the file compiles, so `import java.util.*` really
    // does bring `Date` into scope even though we can't confirm membership
    // one way or the other. Real, measured false positive on Moneta:
    // `Date`/`Calendar`/`Collections` were flagged as missing imports in
    // files that explicitly had `import java.util.*`.
    if let Some(file_data) = indexer.files.get(from_uri.as_str()) {
        if file_data
            .imports
            .iter()
            .any(|i| i.is_star && is_stdlib(&i.full_path))
        {
            return true;
        }
    }
    let star_pkgs = star_import_packages(indexer, from_uri);
    if find_in_star_imports(indexer, name, &star_pkgs).is_some() {
        return true;
    }
    // Members inherited from a super class/interface (e.g. `Result` from a
    // CoroutineWorker subclass) are in scope without an import.
    !resolve_from_class_hierarchy(indexer, name, from_uri).is_empty()
}

/// Whether the extension-receiver type `receiver` provides `name` as a member or an
/// in-scope extension — so a bare `name` inside `fun Receiver.f() { … }` (or an
/// implicit-receiver lambda body) is resolved by the receiver, not a missing import.
/// Index-only (no rg/fd).
///
/// Covers names declared directly on `receiver` (workspace or JAR), extensions
/// registered for it, and members inherited from its supertype chain (incl. library
/// supertypes), e.g. `fun SomeFragment.ext() { requireActivity() }`, where
/// `requireActivity` is declared on androidx `Fragment`, several levels up
/// `SomeFragment`'s chain.
pub(crate) fn receiver_provides_member(indexer: &Indexer, receiver: &str, name: &str) -> bool {
    // 1. Extension function/property declared on the receiver type.
    if indexer
        .extension_by_receiver
        .get(receiver)
        .is_some_and(|entries| entries.iter().any(|e| e.name == name))
    {
        return true;
    }
    // 2. Member of the receiver type — workspace declaration (container chain match).
    if let Some(sym_locs) = indexer.definitions.get(name) {
        if sym_locs.iter().any(|sym_loc| {
            indexer.file_table.location(*sym_loc).is_some_and(|loc| {
                enclosing_container_chain(indexer, &loc)
                    .iter()
                    .any(|c| c == receiver)
            })
        }) {
            return true;
        }
    }
    // 3. Member of a compiled/sources JAR type (the symbol's recorded container).
    // Promote-before-read (zero budget): diagnostics/keystroke path, no blocking
    // sidecar IPC — without this a lazily-materialized JAR's member reads as
    // absent and a real implicit-receiver call gets flagged as a missing import.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(indexer, name, &mut cache_backed_only);
    if let Some(locs) = indexer.jar_definitions.get(name) {
        for loc in locs.iter() {
            let is_member = indexer
                .jar_files
                .get(loc.uri.as_str())
                .and_then(|fd| {
                    fd.symbols
                        .get(loc.range.start.line as usize)
                        .and_then(|s| s.container.clone())
                })
                .as_deref()
                == Some(receiver);
            if is_member {
                return true;
            }
        }
    }
    // 4. Inherited from the receiver type's supertype chain (incl. library supertypes).
    // Depth 24 (not the shared resolve_from_class_hierarchy's 12): validated on Moneta
    // by the original missing-import POC as real headroom, not just enough — the
    // visited-set bounds total work regardless, so there's no cost to the margin.
    for loc in indexer.lookup_definitions(receiver) {
        // Zero sidecar budget: same diagnostics/keystroke-path, no-blocking-IPC
        // intent as this function's other two promote-before-read calls above.
        let found = walk_hierarchy(
            indexer,
            receiver,
            loc.uri.as_str(),
            CallerContext::default(),
            24,
            0,
            |index, _, class_uri, _| find_name_in_uri(index, name, class_uri),
        );
        if !found.is_empty() {
            return true;
        }
    }
    false
}
