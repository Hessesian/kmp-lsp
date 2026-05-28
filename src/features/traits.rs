//! Capability traits — the read-side abstraction boundary.
//!
//! Each trait groups methods by *what the Indexer can do*, not by which feature
//! uses them.  Feature functions compose the traits they need as bounds:
//!
//! ```rust,ignore
//! fn find_definition(cursor: &RawCursor, index: &(impl SymbolIndex + DocumentAccess)) { … }
//! ```
//!
//! Navigation invariant: trait method → go-to-implementation → `impl X for Indexer`.
//! Always two jumps.

use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::lsp_types::{CompletionItem, Location, Position, Range, Url};

use crate::indexer::IgnoreMatcher;
use crate::types::{FileData, SymbolEntry};

// ─── SymbolIndex ─────────────────────────────────────────────────────────────

/// Symbol lookup — find, resolve, and navigate across the indexed codebase.
pub(crate) trait SymbolIndex {
    /// Find definition locations for `name`, using `qualifier` and `from_uri`
    /// to narrow the search to imported/accessible symbols.
    fn find_definition_qualified(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
    ) -> Vec<Location>;

    /// All definition locations for `name` regardless of import context.
    fn definition_locations(&self, name: &str) -> Vec<Location>;

    /// All known direct subtypes (class/interface implementors) of `name`.
    fn subtypes_of(&self, name: &str) -> Vec<Location>;

    /// Return the `FileData` for the indexed file at `uri`, if indexed.
    fn file_data_for(&self, uri: &str) -> Option<Arc<FileData>>;

    /// All top-level symbols indexed for `uri`.
    #[allow(dead_code)]
    fn file_symbols(&self, uri: &Url) -> Vec<SymbolEntry>;

    /// Iterate all indexed files, calling `f(uri_str, file_data)`.
    /// Return `false` from `f` to stop iteration early.
    fn for_each_indexed_file(&self, f: &mut dyn FnMut(&str, &Arc<FileData>) -> bool);

    /// Returns absolute file paths of all indexed files that explicitly import
    /// `parent.name` or `parent.*` (star import of the parent).
    ///
    /// Used to discover candidate files for bare-name reference scanning without
    /// running rg — exact import matching eliminates regex false positives such
    /// as `import Parent.Name.Companion` matching a `\bName\b` rg pattern.
    ///
    /// Returns an empty `Vec` when either the index is not yet populated or when
    /// no indexed file imports the symbol. Callers should always fall back to rg
    /// when the result is empty.
    ///
    /// `full_parent_fqn` must be the **fully-qualified** parent class name, e.g.
    /// `"com.a.IntroContract"` (not just the short name `"IntroContract"`).
    /// Exact equality on `ImportEntry.full_path` ensures that imports of a
    /// same-short-name class in a different package (e.g. `com.b.IntroContract`)
    /// are never treated as candidates for `com.a.IntroContract.Event`.
    ///
    /// Aliased imports (`import Parent.Name as Alias`) are excluded: `Name` is
    /// not available as a bare identifier in those files.
    fn files_importing_nested(&self, full_parent_fqn: &str, name: &str) -> Vec<String> {
        let exact = format!("{full_parent_fqn}.{name}");
        let mut result = Vec::new();
        self.for_each_indexed_file(&mut |uri, fd| {
            for imp in &fd.imports {
                let matched = if imp.is_star {
                    imp.full_path == full_parent_fqn
                } else {
                    // Exclude aliased imports: `import Parent.Name as Alias` makes
                    // `Name` unavailable as a bare identifier in the file.
                    imp.full_path == exact && imp.local_name == name
                };
                if matched {
                    if let Some(path) = tower_lsp::lsp_types::Url::parse(uri)
                        .ok()
                        .and_then(|u| u.to_file_path().ok())
                    {
                        result.push(path.to_string_lossy().into_owned());
                    }
                    return true;
                }
            }
            true
        });
        result
    }

    /// Returns absolute file paths of all indexed files that import `full_fqn`
    /// as a direct (non-nested, non-star) import.
    ///
    /// Used to build qualified-pass candidates: files that import the parent class
    /// directly (e.g. `import com.example.a.ReducerA`) can legally write
    /// `ReducerA.Factory` without importing `ReducerA.Factory` explicitly.
    ///
    /// Aliased imports (`import com.example.a.ReducerA as RA`) are excluded:
    /// the file cannot use the bare name `ReducerA` or `ReducerA.Factory`.
    fn files_importing_class(&self, full_fqn: &str) -> Vec<String> {
        // The local name for a non-aliased import is the last segment of the FQN.
        let expected_local = full_fqn.rsplit('.').next().unwrap_or(full_fqn);
        let mut result = Vec::new();
        self.for_each_indexed_file(&mut |uri, fd| {
            for imp in &fd.imports {
                if !imp.is_star && imp.full_path == full_fqn && imp.local_name == expected_local {
                    if let Some(path) = tower_lsp::lsp_types::Url::parse(uri)
                        .ok()
                        .and_then(|u| u.to_file_path().ok())
                    {
                        result.push(path.to_string_lossy().into_owned());
                    }
                    return true;
                }
            }
            true
        });
        result
    }

    /// Name of the innermost class/object enclosing `row` in `uri`, if any.
    fn enclosing_class_at(&self, uri: &Url, row: u32) -> Option<String>;
}

// ─── DocumentAccess ──────────────────────────────────────────────────────────

/// Document text and cursor-position access.
pub(crate) trait DocumentAccess {
    /// Lines from the in-memory caches only (no disk I/O).
    /// Prefers live (unsaved) buffer; falls back to indexed snapshot.
    fn mem_lines_for(&self, uri: &str) -> Option<Arc<Vec<String>>>;

    /// Lines for `uri`, including disk fallback if not live.
    #[allow(dead_code)]
    fn lines_for(&self, uri: &Url) -> Option<Arc<Vec<String>>>;

    // TODO(per-rule-5): Split into separate functions (e.g. extract_word and extract_qualifier)

    /// Extract the identifier and optional dot-qualifier at `pos`.
    fn word_and_qualifier_at(&self, uri: &Url, pos: Position) -> Option<(String, Option<String>)>;

    /// Extract just the identifier token at `pos`.
    #[allow(dead_code)]
    fn word_at(&self, uri: &Url, pos: Position) -> Option<String>;

    // TODO(per-rule-5): Split into separate functions (e.g. extract_word and get_range)

    /// Extract the identifier token and its source range at `pos`.
    #[allow(dead_code)]
    fn word_and_range_at(&self, uri: &Url, pos: Position) -> Option<(String, Range)>;
}

// ─── ScopeQuery ──────────────────────────────────────────────────────────────

/// Import and package scope resolution, plus library classification.
pub(crate) trait ScopeQuery {
    /// Returns `true` if `uri` is a library/stdlib file (not workspace source).
    fn is_library_uri(&self, uri: &Url) -> bool;

    /// The declared package name for the file at `uri`.
    fn package_of(&self, uri: &Url) -> Option<String>;

    /// Scan imports in `uri` for `name`; returns `(parent_class, declared_pkg)`.
    ///
    /// E.g. `import com.example.DashboardViewModel.Effect`
    /// → `(Some("DashboardViewModel"), Some("com.example.DashboardViewModel"))`
    fn resolve_symbol_via_import(&self, uri: &Url, name: &str) -> (Option<String>, Option<String>);

    /// If `name` is declared as an inner class, return the enclosing class name.
    /// Searches `preferred_uri` first, then any definition site.
    fn declared_parent_class_of(&self, name: &str, preferred_uri: &Url) -> Option<String>;

    /// Package that `name` is declared in.
    /// Searches `preferred_uri` first (same priority rule as `declared_parent_class_of`),
    /// then falls back to the first definition in any file.
    fn declared_package_of(&self, name: &str, preferred_uri: &Url) -> Option<String>;

    /// Returns `true` if `name` is declared in the file at `uri`.
    fn is_declared_in(&self, uri: &Url, name: &str) -> bool;
}

// ─── SearchAccess ────────────────────────────────────────────────────────────

/// Ripgrep-based fallback search context.
pub(crate) trait SearchAccess {
    /// Returns the (workspace_root, ignore_matcher) tuple used to scope `rg` calls.
    fn rg_context(&self) -> (Option<PathBuf>, Option<Arc<IgnoreMatcher>>);

    /// Returns `(effective_root, scoped_source_paths, matcher)` for an rg search
    /// whose context file is `open_file`. Scopes searches to configured source roots
    /// when the open file belongs to the configured workspace.
    ///
    /// Default implementation falls back to `rg_context()` with empty source paths.
    fn rg_scope_for_path(
        &self,
        open_file: Option<&std::path::Path>,
    ) -> (Option<PathBuf>, Vec<String>, Option<Arc<IgnoreMatcher>>) {
        let (root, matcher) = self.rg_context();
        let effective_root = crate::rg::effective_rg_root(root.as_deref(), open_file);
        (effective_root, Vec::new(), matcher)
    }
}

// ─── CompletionIndex ─────────────────────────────────────────────────────────

/// Completion pipeline — already fully orchestrated inside the Indexer.
pub(crate) trait CompletionIndex {
    /// Run the full completion pipeline for `uri` at `position`.
    fn completions(
        &self,
        uri: &Url,
        position: Position,
        snippets: bool,
    ) -> (Vec<CompletionItem>, bool);

    /// `true` while a background scan/index is in progress.
    fn is_indexing_in_progress(&self) -> bool;
}

// ─── SignatureIndex ───────────────────────────────────────────────────────────

/// Function signature lookup with optional receiver type matching.
pub(crate) trait SignatureIndex {
    /// Signature text for `name`, optionally narrowed to `receiver`'s type.
    /// Returns `None` when lookup fails; `Some("")` for a zero-parameter function.
    fn find_fun_signature_with_receiver(
        &self,
        uri: &Url,
        name: &str,
        receiver: Option<&str>,
    ) -> Option<String>;
}

// ─── LiveTreeAccess ──────────────────────────────────────────────────────────

/// Live-syntax access — operations that require the live tree-sitter parse tree.
///
/// Kept separate from the index-based traits because it requires live-tree state
/// that those traits do not; mixing them would force test stubs to provide CST
/// infrastructure unnecessarily.
pub(crate) trait LiveTreeAccess {
    /// Extract the call-site name, qualifier, and active parameter index
    /// at `pos` using the live parse tree for `uri`.
    ///
    /// Returns `None` when the cursor is not inside a call expression or when
    /// no live tree is available.
    fn call_info_at(
        &self,
        pos: tower_lsp::lsp_types::Position,
        uri: &Url,
    ) -> Option<crate::indexer::CallInfo>;

    /// Like `call_info_at` but returns the *enclosing* call expression — the
    /// one that contains the call the cursor is directly inside.
    ///
    /// Useful for signature help: when the cursor is inside a nested call
    /// (e.g. `setOf()`) whose signature cannot be resolved, fall back to the
    /// outer call (`UserData(…)`) so the user still sees helpful parameter info.
    ///
    /// Stops at lambda boundaries so it never crosses scope.
    fn outer_call_info_at(
        &self,
        pos: tower_lsp::lsp_types::Position,
        uri: &Url,
    ) -> Option<crate::indexer::CallInfo>;

    /// Compute folding ranges for `uri` using the live parse tree.
    ///
    /// Returns `None` when no live tree is available for the file.
    fn folding_ranges_for(&self, uri: &Url) -> Option<Vec<tower_lsp::lsp_types::FoldingRange>>;
}
