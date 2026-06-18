//! `find_references` feature — rg-backed reference search with scope narrowing.
//!
//! Entry point: [`find_references`]. The backend adapter calls this after resolving
//! the cursor word; the feature handles scope narrowing, rg search, library filtering,
//! and in-memory current-file hit injection.

use tower_lsp::lsp_types::{Location, Position, Range, SymbolKind, Url};

use super::text_utils::{utf16_column, word_byte_offsets};
use crate::features::traits::{DocumentAccess, ScopeQuery, SearchAccess, SymbolIndex};
use crate::rg::RgSearchRequest;
use crate::StrExt;

// ─── Public entry point ───────────────────────────────────────────────────────

/// Finds all references to `name`, optionally scoped by `qualifier`.
///
/// When `qualifier` is `Some("ReducerA")` (cursor was on `ReducerA.Factory`),
/// the qualifier is used directly as the `parent_class` scope, bypassing the
/// fallible index-lookup that `resolve_scope` uses for unresolved nested types.
/// This prevents false positives when multiple classes define an inner class
/// with the same name (e.g. every class defines a `Factory` or `Builder`).
///
/// When `qualifier` is `None`, scope is inferred from imports and the index.
/// For lowercase methods at their declaration site that are declared inside a
/// doubly-nested class (e.g. `create` inside `Factory` inside `RegularReducer`),
/// the outer class is used for file discovery so callers that reference the outer
/// class via a variable name (`factory.create()`) are found while sibling
/// factories in the same package are excluded.
pub(crate) async fn find_references_with_qualifier(
    name: &str,
    qualifier: Option<&str>,
    uri: &Url,
    line: u32,
    include_decl: bool,
    index: &(impl SymbolIndex + DocumentAccess + ScopeQuery + SearchAccess + Send + Sync),
) -> Vec<Location> {
    let (parent_class, declared_pkg) =
        resolve_scope_with_qualifier(index, uri, line, name, qualifier);

    // A lowercase usage of a JAR/library symbol now also produces a `declared_pkg`
    // (the JAR symbol's package), but the request site is a *usage*, not the symbol's
    // declaration. The `owner_class` / `field_owner` heuristics below assume a
    // declaration site, so they must not fire for JAR-symbol usages.
    let is_jar_symbol_usage = !name.starts_with_uppercase()
        && !index.is_declared_in(uri, name)
        && index.jar_declaration_scope(name).is_some();

    // For lowercase methods at their declaration site, check if they are declared
    // inside a doubly-nested class (e.g. `create` inside `Factory` inside `Reducer`).
    // `declared_pkg.is_some()` is the on_decl proxy for lowercase names: resolve_scope
    // returns (None, Some(pkg)) on_decl and (None, None) off-decl for lowercase names.
    let owner_class = if !name.starts_with_uppercase()
        && declared_pkg.is_some()
        && qualifier.is_none()
        && !is_jar_symbol_usage
    {
        outer_class_for_decl_site(index, uri, line)
    } else {
        None
    };

    // For class members (fields, properties, methods) at their declaration site:
    // scope file discovery to files that mention the declaring class, instead of
    // the whole package.
    let field_owner = if qualifier.is_none() && declared_pkg.is_some() && !is_jar_symbol_usage {
        field_owner_for_decl(index, uri, name, line)
    } else {
        None
    };

    let decl_files = declaration_files_for(
        index,
        name,
        parent_class.as_deref(),
        declared_pkg.as_deref(),
        uri,
    );

    let search = ReferenceSearch {
        uri: uri.clone(),
        name: name.to_string(),
        include_decl,
        parent_class,
        declared_pkg,
        decl_files,
        owner_class,
        field_decl_line: field_owner.is_some().then_some(line),
        field_owner,
    };

    let mut locations = rg_locations(&search, index).await;
    locations.retain(|loc| !index.is_library_uri(&loc.uri));
    add_current_file_locations(
        index,
        uri,
        name,
        search.parent_class.as_deref(),
        search.owner_class.as_deref(),
        include_decl,
        &mut locations,
    );

    locations
}

// ─── Scope resolution ─────────────────────────────────────────────────────────

/// Determine `(parent_class, declared_pkg)` scope for a `findReferences` request.
///
/// Uppercase symbols are narrowed via import analysis or declaration-site lookup
/// so that rg can restrict to the specific class variant.
/// Lowercase symbols at the **declaration site** return `(None, Some(package))` —
/// rg is scoped to same-package files.  Off-declaration-site lowercase names
/// return `(None, None)` — codebase-wide bare-word search via rg.
pub(crate) fn resolve_scope(
    index: &(impl SymbolIndex + ScopeQuery),
    uri: &Url,
    line: u32,
    name: &str,
) -> (Option<String>, Option<String>) {
    resolve_scope_with_qualifier(index, uri, line, name, None)
}

/// Like [`resolve_scope`] but accepts a dot-qualifier (the segment immediately
/// preceding `name` at the cursor, e.g. `"ReducerA"` for `ReducerA.Factory`).
///
/// An uppercase qualifier is used directly as the `parent_class`, which avoids
/// the index-lookup fallback that picks an arbitrary definition when multiple
/// classes define an inner class with the same name.
pub(crate) fn resolve_scope_with_qualifier(
    index: &(impl SymbolIndex + ScopeQuery),
    uri: &Url,
    line: u32,
    name: &str,
    qualifier: Option<&str>,
) -> (Option<String>, Option<String>) {
    // Lowercase names: only scope if we're on the declaration.
    // For methods declared inside a class/interface, include the enclosing class so
    // that `parent_scoped_reference_locations` can discover cross-package callers via
    // `import.*EnclosingClass` (e.g. interactors that call `repo.getTexts()` after
    // importing `IGoldConversionRepository`).  Top-level functions (no enclosing class)
    // fall back to package-scoped discovery as before.
    if !name.starts_with_uppercase() {
        let on_decl = index.is_declared_in(uri, name)
            && index
                .definition_locations(name)
                .iter()
                .any(|l| l.uri == *uri && l.range.start.line == line);
        if on_decl {
            let enclosing_class = index.enclosing_class_at(uri, line);
            return (enclosing_class, index.package_of(uri));
        }
        // JAR-symbol usage (not on its own workspace declaration): scope to the
        // JAR symbol's declaring package/type so file discovery is restricted to
        // workspace files that import it — instead of an unscoped codebase-wide
        // bare-word search that also matches unrelated same-named workspace symbols.
        if let Some((package, container)) = index.jar_declaration_scope(name) {
            return (container, Some(package));
        }
        return (None, None);
    }

    // Fast path: an uppercase dot-qualifier (e.g. "ReducerA" in "ReducerA.Factory")
    // unambiguously identifies the parent class — use it directly rather than
    // guessing from the index (which is non-deterministic when multiple classes
    // share the same inner-class name).
    //
    // `word_and_qualifier_at` returns the full dot-chain (e.g. "Outer.Inner" for
    // "Outer.Inner.Factory").  We preserve the full chain as `parent_class` so
    // `has_wrong_qualifier_at_col` can match it against the full extracted chain on each
    // hit, rather than just the immediate token.
    if let Some(q) = qualifier.filter(|q| q.starts_with_uppercase()) {
        let parent_pkg = index
            .declared_package_of(q, uri)
            .map(|p| format!("{p}.{q}"))
            .or_else(|| index.declared_package_of(name, uri));
        return (Some(q.to_string()), parent_pkg);
    }

    let on_decl = index.is_declared_in(uri, name)
        && index
            .definition_locations(name)
            .iter()
            .any(|l| l.uri == *uri && l.range.start.line == line);
    if on_decl {
        let parent = index.enclosing_class_at(uri, line);
        let pkg = index.package_of(uri);
        return (parent, pkg);
    }
    let (parent, pkg) = index.resolve_symbol_via_import(uri, name);
    if parent.is_some() || pkg.is_some() {
        return (parent, pkg);
    }
    let parent = index.declared_parent_class_of(name, uri);
    let pkg = index.declared_package_of(name, uri);
    (parent, pkg)
}

fn declaration_files_for(
    index: &(impl SymbolIndex + ScopeQuery),
    name: &str,
    parent_class: Option<&str>,
    declared_pkg: Option<&str>,
    source_uri: &Url,
) -> Vec<String> {
    // When `declared_pkg` is available (i.e., we resolved the declaring package
    // from imports or declaration site), use it to filter: only keep definitions
    // that live in that specific package.  This prevents same-named types in
    // different packages (e.g. `com.a.IntroContract.Event` vs
    // `com.b.IntroContract.Event`) from merging their declaration files into the
    // candidate set and producing false positives.
    //
    // Crucially, we use `declared_pkg` (the *declaration* package), NOT the
    // source file's package.  Using the source package would incorrectly drop the
    // declaration file when `findReferences` is invoked from a call site in a
    // *different* package — the common cross-package usage scenario.
    //
    // When `declared_pkg` is None (unscoped lowercase off-decl-site search), fall
    // back to the source package so that same-named top-level symbols from
    // unrelated packages are not merged into candidates.
    // JAR/library-defined symbol: no workspace file lives in its package, so the
    // same-package definition-file scan below would find nothing. Instead, discover
    // the workspace files that *import* the JAR symbol and use those as candidates.
    // This drives the import-scoped reference search (reusing the existing
    // qualifier-precision machinery) and keeps unrelated same-named workspace
    // symbols out of the result set.
    if let Some(jar_pkg) = declared_pkg {
        let no_workspace_decl = index
            .definition_locations(name)
            .iter()
            .all(|loc| index.is_library_uri(&loc.uri));
        if no_workspace_decl && index.jar_declaration_scope(name).is_some() {
            let fqn = format!("{jar_pkg}.{name}");
            return index
                .workspace_importers_of(&fqn)
                .into_iter()
                .filter_map(|url| url.to_file_path().ok())
                .filter_map(|path| path.to_str().map(|s| s.to_owned()))
                .collect();
        }
    }

    let source_pkg = index.package_of(source_uri);
    let pkg_filter = declared_pkg.or(source_pkg.as_deref());
    index
        .definition_locations(name)
        .into_iter()
        .filter(|loc| reference_matches_parent_class(index, loc, parent_class))
        .filter(|loc| {
            let Some(filter) = pkg_filter else {
                return true;
            };
            let Some(file_pkg) = index.package_of(&loc.uri) else {
                return false;
            };
            // Exact match covers the normal case ("com.a" == "com.a").
            // The prefix check handles when `declared_pkg` is a container FQN
            // ("com.a.IntroContract"): the declaration file's package is "com.a"
            // and "com.a.IntroContract".starts_with("com.a.") → accept it.
            file_pkg == filter || filter.starts_with(&format!("{file_pkg}."))
        })
        .filter_map(|loc| loc.uri.to_file_path().ok())
        .filter_map(|path| path.to_str().map(|s| s.to_owned()))
        .collect()
}

/// For a lowercase method at its declaration site, returns the outer-outer class
/// if the method is inside a doubly-nested class
/// (e.g. `create` inside `Factory` inside `RegularReducer` → `"RegularReducer"`).
///
/// Returns `None` if the method has only one level of nesting or none.
/// Uses `enclosing_class_at` (line-specific CST walk) for the direct parent, then
/// `declared_parent_class_of` (preferred-URI-first index lookup) for the outer parent.
fn outer_class_for_decl_site(
    index: &(impl SymbolIndex + ScopeQuery),
    uri: &Url,
    line: u32,
) -> Option<String> {
    let direct_parent = index.enclosing_class_at(uri, line)?;
    index.declared_parent_class_of(&direct_parent, uri)
}

fn reference_matches_parent_class(
    index: &impl SymbolIndex,
    location: &Location,
    parent_class: Option<&str>,
) -> bool {
    let Some(parent_class) = parent_class else {
        return true;
    };
    index
        .enclosing_class_at(&location.uri, location.range.start.line)
        .as_deref()
        == Some(parent_class)
}

// ─── rg search ────────────────────────────────────────────────────────────────

async fn rg_locations(
    search: &ReferenceSearch,
    index: &(impl SymbolIndex + SearchAccess + Send + Sync),
) -> Vec<Location> {
    let file_path = search.uri.to_file_path().ok();
    let (workspace_root, source_roots, matcher) = index.rg_scope_for_path(file_path.as_deref());
    // For uppercase nested types, build two candidate sets from the in-memory
    // import index — avoiding regex edge cases and cross-package FPs.
    // Falls back to workspace-wide rg when the index is not yet populated.
    let (index_candidates, index_qualified_candidates) = if let (Some(parent), Some(pkg)) = (
        search.parent_class.as_deref(),
        search.declared_pkg.as_deref(),
    ) {
        if search.name.starts_with_uppercase() {
            // Build the fully-qualified parent name so that `com.b.IntroContract`
            // is never treated as a candidate for `com.a.IntroContract.Event`.
            // `declared_pkg` is either a plain package ("com.a") or already a
            // container FQN ("com.a.IntroContract") when resolved from a call site.
            let full_parent_fqn = if pkg.ends_with(&format!(".{parent}")) || pkg == parent {
                pkg.to_string()
            } else {
                format!("{pkg}.{parent}")
            };
            // bare-name candidates: files importing `Parent.Name` or `Parent.*`
            let bare = index.files_importing_nested(&full_parent_fqn, &search.name);
            // qualified-pass candidates: also files importing the parent class
            // directly (e.g. `import com.a.ReducerA`) — those can write
            // `ReducerA.Factory` as a qualified reference without importing `Factory`.
            let mut qualified = bare.clone();
            let parent_imports = index.files_importing_class(&full_parent_fqn);
            for f in parent_imports {
                if !qualified.contains(&f) {
                    qualified.push(f);
                }
            }
            (bare, qualified)
        } else {
            (vec![], vec![])
        }
    } else {
        (vec![], vec![])
    };
    let request = search.clone();
    tokio::task::spawn_blocking(move || {
        let rg_req = RgSearchRequest::new(
            &request.name,
            request.parent_class.as_deref(),
            request.declared_pkg.as_deref(),
            workspace_root.as_deref(),
            request.include_decl,
            &request.uri,
            &request.decl_files,
        )
        .with_source_paths(&source_roots)
        .with_index_candidates(index_candidates)
        .with_index_qualified_candidates(index_qualified_candidates);
        let rg_req = match request.owner_class.as_deref() {
            Some(owner) => rg_req.with_owner_class(owner),
            None => rg_req,
        };
        let rg_req = match request.field_owner.as_deref() {
            Some(owner) => {
                let rg_req = rg_req.with_field_owner(owner);
                match request.field_decl_line {
                    Some(dl) => rg_req.with_field_decl_line(dl),
                    None => rg_req,
                }
            }
            None => rg_req,
        };
        crate::rg::rg_find_references(&rg_req, matcher.as_deref())
    })
    .await
    .unwrap_or_default()
}

// ─── In-memory current-file injection ─────────────────────────────────────────

fn add_current_file_locations(
    index: &impl DocumentAccess,
    uri: &Url,
    name: &str,
    parent_class: Option<&str>,
    owner_class: Option<&str>,
    include_decl: bool,
    locations: &mut Vec<Location>,
) {
    let Some(lines) = index.mem_lines_for(uri.as_str()) else {
        return;
    };
    for (line_idx, line) in lines.iter().enumerate() {
        let line_number = line_idx as u32;
        if has_reference_line(locations, uri, line_number) {
            continue;
        }
        // When owner_class is set the caller is using variable-name call syntax
        // (e.g. `reducerFactory.create()`). The only legitimate hit in the declaring
        // file is the declaration itself; all other create() calls here are to OTHER
        // injected factory instances and must be excluded.
        if owner_class.is_some() {
            if include_decl && crate::rg::is_declaration_of(line, name) {
                for loc in line_reference_locations(uri, name, line_number, line) {
                    if !has_reference_start(locations, &loc) {
                        locations.push(loc);
                    }
                }
            }
            continue;
        }
        // Check qualifier per-occurrence so that a line containing both a valid
        // and an invalid qualified reference (e.g. `ReducerA.Factory, ReducerC.Factory`)
        // keeps the valid hit instead of dropping the whole line.
        for loc in line_reference_locations(uri, name, line_number, line) {
            if has_reference_start(locations, &loc) {
                continue;
            }
            // Respect include_decl: if the caller asked to exclude the declaration,
            // skip lines that declare this name (mirrors the rg path behaviour).
            if !include_decl && crate::rg::is_declaration_of(line, name) {
                continue;
            }
            if let Some(parent) = parent_class {
                if crate::rg::has_wrong_qualifier_at_col(
                    line,
                    name,
                    parent,
                    loc.range.start.character,
                ) {
                    continue;
                }
            }
            locations.push(loc);
        }
    }
}

fn has_reference_line(locations: &[Location], uri: &Url, line_number: u32) -> bool {
    locations
        .iter()
        .any(|loc| loc.uri == *uri && loc.range.start.line == line_number)
}

fn line_reference_locations(uri: &Url, name: &str, line_number: u32, line: &str) -> Vec<Location> {
    word_byte_offsets(line, name)
        .map(|offset| reference_location(uri, name, line_number, line, offset))
        .collect()
}

fn reference_location(
    uri: &Url,
    name: &str,
    line_number: u32,
    line: &str,
    offset: usize,
) -> Location {
    let start = utf16_column(&line[..offset]);
    let end = start + utf16_column(name);
    Location {
        uri: uri.clone(),
        range: Range::new(
            Position::new(line_number, start),
            Position::new(line_number, end),
        ),
    }
}

fn has_reference_start(locations: &[Location], candidate: &Location) -> bool {
    locations
        .iter()
        .any(|loc| loc.uri == candidate.uri && loc.range.start == candidate.range.start)
}

// ─── Internal transfer type ────────────────────────────────────────────────────

#[derive(Clone)]
struct ReferenceSearch {
    uri: Url,
    name: String,
    include_decl: bool,
    parent_class: Option<String>,
    declared_pkg: Option<String>,
    decl_files: Vec<String>,
    /// Outer-outer class for owner-scoped file discovery; see [`outer_class_for_decl_site`].
    owner_class: Option<String>,
    /// Declaring class for field-scoped reference search; see [`field_owner_for_decl`].
    field_owner: Option<String>,
    /// 0-based declaration line in `uri`; set when `field_owner` is present so that
    /// the rg path can distinguish the actual declaration from same-named fields in
    /// other classes inside the same file.
    field_decl_line: Option<u32>,
}

// ─── Field/member owner resolution ───────────────────────────────────────────

/// Returns the declaring class of any class member (field or method) at `(uri, line)`.
///
/// Unlike `outer_class_for_decl_site` (which only fires for doubly-nested methods),
/// this fires for **any** lowercase symbol declared directly inside a class or
/// interface — single-level nesting is sufficient.
///
/// Uses `SymbolEntry::container` (set by range-based nesting at parse time),
/// which correctly handles single-line `data class Foo(val field: T)` declarations
/// where `enclosing_class_at`'s row-guard would return `None`.
///
/// Returns `None` for top-level declarations (no enclosing class).
fn field_owner_for_decl(
    index: &impl SymbolIndex,
    uri: &Url,
    name: &str,
    line: u32,
) -> Option<String> {
    let symbols = index.file_symbols(uri);

    // Find the immediate container of the field.
    let immediate_owner = symbols
        .iter()
        .find(|s| {
            s.name == name
                && s.selection_range.start.line == line
                && matches!(
                    s.kind,
                    SymbolKind::PROPERTY | SymbolKind::VARIABLE | SymbolKind::FIELD
                )
        })
        .and_then(|s| s.container.clone())?;

    // Walk up the class hierarchy to find the outermost ancestor.
    // Using the outermost class (e.g. `TextBody`) rather than the immediate
    // nested owner (e.g. `BusyLoader`) avoids false positives: any file that
    // accesses a deeply-nested field MUST reference the top-level class,
    // while unrelated files that happen to mention a same-named nested class
    // (e.g. `ProductScreens.BusyLoader`) are excluded.
    let mut current = immediate_owner;
    loop {
        let parent = symbols.iter().find(|s| {
            s.name == current
                && matches!(
                    s.kind,
                    SymbolKind::CLASS
                        | SymbolKind::INTERFACE
                        | SymbolKind::STRUCT
                        | SymbolKind::ENUM
                        | SymbolKind::OBJECT
                )
        });
        match parent.and_then(|s| s.container.clone()) {
            Some(grandparent) => current = grandparent,
            None => break,
        }
    }

    Some(current)
}

#[cfg(test)]
#[path = "references_tests.rs"]
mod tests;
