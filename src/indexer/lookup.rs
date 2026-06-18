//! Lookup phase: query the index for symbol information.
//!
//! This module owns the "read path" of the indexer for symbol resolution:
//!
//! - [`Indexer::is_declared_in`]             — test if a name is declared in a file
//! - [`Indexer::find_definition`]            — resolve definition locations by name
//! - [`Indexer::find_definition_qualified`]  — resolve with optional dot-qualifier
//! - [`Indexer::file_symbols`]               — all symbols declared in a file
//! - [`Indexer::package_of`]                 — package declared in a file
//! - [`Indexer::declared_package_of`]        — package in which a name is declared
//! - [`Indexer::declared_parent_class_of`]   — enclosing class at declaration site
//! - [`Indexer::resolve_symbol_via_import`]  — resolve parent class / package via imports

use tower_lsp::lsp_types::*;

use super::Indexer;
use crate::types::SymbolEntry;
use crate::StrExt;

impl Indexer {
    /// Returns true if `name` has at least one definition location inside `uri`.
    pub(crate) fn is_declared_in(&self, uri: &Url, name: &str) -> bool {
        self.definitions
            .get(name)
            .map(|locs| locs.iter().any(|l| l.uri == *uri))
            .unwrap_or(false)
    }

    /// Resolve definition locations for `name` (with optional dot-qualifier).
    #[allow(dead_code)]
    pub(crate) fn find_definition(&self, name: &str, from_uri: &Url) -> Vec<Location> {
        self.resolve_symbol(name, None, from_uri)
    }

    pub(crate) fn find_definition_qualified(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
    ) -> Vec<Location> {
        self.resolve_symbol(name, qualifier, from_uri)
    }

    /// All symbols declared in the given file (for `documentSymbol`).
    pub(crate) fn file_symbols(&self, uri: &Url) -> Vec<SymbolEntry> {
        self.files
            .get(uri.as_str())
            .map(|d| d.symbols.clone())
            .unwrap_or_default()
    }

    /// Return the package declared in the given file, if any.
    pub(crate) fn package_of(&self, uri: &Url) -> Option<String> {
        self.files.get(uri.as_str())?.package.clone()
    }

    /// If `name` resolves to a compiled/sources JAR definition, return its
    /// `(package, container)` — e.g. `("androidx.compose.runtime", None)` for the
    /// top-level `remember`, or `("androidx.compose.ui", Some("Modifier"))` for a
    /// member declared inside `Modifier`. Returns `None` for workspace-only names.
    ///
    /// The package is read from the per-symbol `jar_symbol_packages` side table
    /// (index-aligned with the symbol's synthetic line number), falling back to the
    /// per-jar inferred package. The container is read from the JAR symbol entry.
    pub(crate) fn jar_declaration_scope(&self, name: &str) -> Option<(String, Option<String>)> {
        let locs = self.jar_definitions.get(name)?;
        for loc in locs.iter() {
            let uri_str = loc.uri.as_str();
            let symbol_index = loc.range.start.line as usize;
            let package = self
                .jar_symbol_packages
                .get(uri_str)
                .and_then(|packages| {
                    packages
                        .get(symbol_index)
                        .filter(|p| !p.is_empty())
                        .cloned()
                })
                .or_else(|| {
                    self.jar_files
                        .get(uri_str)
                        .and_then(|fd| fd.package.clone())
                });
            let Some(package) = package else {
                continue;
            };
            // A top-level Kotlin declaration is registered in `qualified` under
            // `pkg.name`, while a member is registered under `pkg.Container.name`.
            // The symbol's stored `container` for a top-level fun/val is its JVM
            // facade class (e.g. `ComposablesKt`), which is not a usable Kotlin
            // type qualifier — so report `None` for top-level symbols.
            let is_top_level = self.qualified.contains_key(&format!("{package}.{name}"));
            let container = if is_top_level {
                None
            } else {
                self.jar_files.get(uri_str).and_then(|fd| {
                    fd.symbols
                        .get(symbol_index)
                        .and_then(|s| s.container.clone())
                })
            };
            return Some((package, container));
        }
        None
    }

    /// Workspace files (`file://`, non-library) that import `fqn` — either via an
    /// explicit import of the symbol or a star import of its declaring package.
    ///
    /// `fqn` is the fully-qualified name, e.g. `"androidx.compose.runtime.remember"`.
    /// Reuses [`crate::types::ImportEntry::covers`] for the import-matching rules.
    /// Library/JAR files are excluded so results are workspace usages only.
    pub(crate) fn workspace_importers_of(&self, fqn: &str) -> Vec<Url> {
        let Some((def_pkg, symbol_name)) = fqn.rsplit_once('.') else {
            return Vec::new();
        };
        let mut result = Vec::new();
        for entry in self.files.iter() {
            let uri_str = entry.key();
            if self.library_uris.contains(uri_str) {
                continue;
            }
            if !uri_str.starts_with("file://") {
                continue;
            }
            let imports_symbol = entry
                .value()
                .imports
                .iter()
                .any(|imp| imp.covers(def_pkg, symbol_name));
            if imports_symbol {
                if let Ok(url) = Url::parse(uri_str) {
                    result.push(url);
                }
            }
        }
        result
    }

    /// Return the package in which `name` is declared, by looking up its
    /// definition locations and reading the `package` field of those files.
    pub(crate) fn declared_package_of(&self, name: &str, preferred_uri: &Url) -> Option<String> {
        let locs = self.definitions.get(name)?;
        // Prefer the declaration in the current file (mirrors declared_parent_class_of).
        for loc in locs.iter() {
            if loc.uri == *preferred_uri {
                if let Some(f) = self.files.get(loc.uri.as_str()) {
                    if let Some(pkg) = &f.package {
                        return Some(pkg.clone());
                    }
                }
            }
        }
        // Fall back to first definition in any file.
        for loc in locs.iter() {
            if let Some(f) = self.files.get(loc.uri.as_str()) {
                if let Some(pkg) = &f.package {
                    return Some(pkg.clone());
                }
            }
        }
        None
    }

    /// If `name` is declared as an inner/nested class, return the name of its
    /// enclosing class at the declaration site in `preferred_uri` (if found there),
    /// otherwise the first definition site.
    pub(crate) fn declared_parent_class_of(
        &self,
        name: &str,
        preferred_uri: &Url,
    ) -> Option<String> {
        let locs = self.definitions.get(name)?;
        // Try declaration in the preferred (current) file first.
        for loc in locs.iter() {
            if loc.uri == *preferred_uri {
                return self.enclosing_class_at(&loc.uri, loc.range.start.line);
            }
        }
        // Fall back to first definition in any file.
        for loc in locs.iter() {
            if let Some(parent) = self.enclosing_class_at(&loc.uri, loc.range.start.line) {
                return Some(parent);
            }
        }
        None
    }

    /// Scan imports in `uri` for `name` and return (parent_class, declared_pkg)
    /// as resolved from the import statement.  E.g.:
    ///   `import com.example.DashboardViewModel.Effect`
    ///   → parent_class = Some("DashboardViewModel"), pkg = Some("com.example.DashboardViewModel")
    ///
    ///   `import com.example.DashboardViewModel.*` (star import)
    ///   → if Effect is a known nested class of DashboardViewModel in the index,
    ///     parent_class = Some("DashboardViewModel"), pkg = Some("com.example.DashboardViewModel")
    pub(crate) fn resolve_symbol_via_import(
        &self,
        uri: &Url,
        name: &str,
    ) -> (Option<String>, Option<String>) {
        let file = match self.files.get(uri.as_str()) {
            Some(f) => f,
            None => return (None, None),
        };
        for line in file.lines.iter() {
            let t = line.trim();
            if !t.starts_with("import ") {
                continue;
            }
            // Handle `import a.b.c.Name` and `import a.b.c.Name as Alias`
            let import_path = t["import ".len()..].split_whitespace().next().unwrap_or("");
            let segments: Vec<&str> = import_path.split('.').collect();
            // Last segment should match `name` (or be `*`).
            let last = *segments.last().unwrap_or(&"");
            if last != name && last != "*" {
                continue;
            }

            // Star import: `import a.b.ClassName.*`
            // If the segment before `*` is an uppercase class name and `name` is a known
            // nested class of that class, resolve to it.
            if last == "*" && segments.len() >= 2 {
                let maybe_parent = segments[segments.len() - 2];
                if maybe_parent.starts_with_uppercase() {
                    // Check if `name` is actually declared inside `maybe_parent`.
                    let decl_uri = self.definitions.get(name).and_then(|locs| {
                        locs.iter()
                            .find(|loc| {
                                self.enclosing_class_at(&loc.uri, loc.range.start.line)
                                    .as_deref()
                                    == Some(maybe_parent)
                            })
                            .map(|loc| loc.uri.clone())
                    });
                    if let Some(decl_uri) = decl_uri {
                        let pkg = self
                            .files
                            .get(decl_uri.as_str())
                            .and_then(|f| f.package.clone())
                            .map(|p| format!("{p}.{maybe_parent}"));
                        return (Some(maybe_parent.to_string()), pkg);
                    }
                }
                continue;
            }

            // Found a matching import. The declared package is everything up to (not incl.) `name`.
            // The parent class is the segment immediately before `name` if it starts uppercase.
            if last == name && segments.len() >= 2 {
                let pkg = segments[..segments.len() - 1].join(".");
                let parent = segments
                    .get(segments.len() - 2)
                    .filter(|s| s.starts_with_uppercase())
                    .map(|s| s.to_string());
                return (parent, Some(pkg));
            }
        }
        (None, None)
    }
}

pub(crate) fn symbol_kw(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::CLASS => "class",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "fun",
        SymbolKind::METHOD => "fun",
        SymbolKind::VARIABLE => "var",
        SymbolKind::CONSTANT => "val",
        SymbolKind::OBJECT => "object",
        SymbolKind::TYPE_PARAMETER => "typealias",
        SymbolKind::ENUM => "enum class",
        SymbolKind::FIELD => "field",
        _ => "symbol",
    }
}

pub(crate) fn symbol_kw_for_lang(kind: SymbolKind, lang: &str) -> &'static str {
    // Resolve to a Language and delegate to its provider so keyword logic stays
    // in one place. Unknown lang strings fall back to Kotlin (default path).
    let language = match lang {
        "java" => crate::Language::Java,
        "swift" => crate::Language::Swift,
        _ => crate::Language::Kotlin,
    };
    language.parser().symbol_keyword(kind)
}

pub(crate) fn lang_str(path: &str) -> &'static str {
    crate::Language::from_path(path).code_fence()
}

// ─── Generic type parameter substitution ─────────────────────────────────────

/// Apply a type-parameter substitution map to a type string.
///
/// Only replaces whole-word occurrences (character boundaries), so `EventType`
/// is not partially replaced when substituting `Event`.
///
/// Re-exported as `crate::indexer::apply_type_subst` for use by inlay_hints,
/// backend handlers, and the resolution module.
pub(crate) fn apply_type_subst(
    sig: &str,
    subst: &std::collections::HashMap<String, String>,
) -> String {
    if subst.is_empty() {
        return sig.to_owned();
    }
    let mut result = String::with_capacity(sig.len() + 16);
    let chars: Vec<char> = sig.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_alphabetic() || ch == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            if let Some(replacement) = subst.get(&ident) {
                result.push_str(replacement);
            } else {
                result.push_str(&ident);
            }
        } else {
            result.push(ch);
            i += 1;
        }
    }
    result
}

#[cfg(test)]
#[path = "lookup_tests.rs"]
mod tests;
