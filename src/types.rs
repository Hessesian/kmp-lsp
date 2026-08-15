use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_lsp::lsp_types::{Range, SymbolKind};

/// Classification of a file's source set.
/// Determined at scan time based on file path and workspace configuration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SourceSet {
    /// Production source code
    #[default]
    Main,
    /// Test source code (src/test/, src/androidTest/, etc.)
    Test,
    /// Library/SDK source from sourcePaths — excluded from references and rename
    Library,
}

/// File language, derived from path extension.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Language {
    Kotlin,
    Java,
    Swift,
}

impl Language {
    /// All languages, in priority order for extension matching.
    const ALL: [Language; 3] = [Language::Java, Language::Swift, Language::Kotlin];

    pub(crate) fn from_path(path: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|lang| {
                lang.parser()
                    .file_extensions()
                    .iter()
                    .any(|ext| path.ends_with(&format!(".{ext}")))
            })
            .unwrap_or(Language::Kotlin)
    }

    /// LSP language identifier; delegates to the language's provider.
    pub(crate) fn language_id(self) -> &'static str {
        self.parser().language_id()
    }

    pub(crate) fn code_fence(self) -> &'static str {
        self.language_id()
    }

    pub(crate) fn needs_semicolons(self) -> bool {
        matches!(self, Language::Java)
    }

    /// Returns true if `line` looks like an override declaration of `method_name`
    /// in this language.
    ///
    /// - **Kotlin**: requires `override` and `fun` on the same line as the declaration.
    /// - **Java**: accepts any declaration of `method_name` — `@Override` is placed
    ///   on the *preceding* line and is not visible in a single-line rg result.
    /// - **Swift**: always false (not supported).
    pub(crate) fn is_override_declaration(self, line: &str, method_name: &str) -> bool {
        use crate::rg::is_declaration_of;
        match self {
            Language::Kotlin => {
                line.contains("override")
                    && line.contains("fun ")
                    && is_declaration_of(line, method_name)
            }
            Language::Java => is_declaration_of(line, method_name),
            Language::Swift => false,
        }
    }

    /// Returns true if `detail` (the indexed declaration signature) indicates
    /// this symbol overrides a supertype member.
    pub(crate) fn detail_is_override(self, detail: &str) -> bool {
        match self {
            Language::Kotlin => detail.contains("override"),
            // Java @Override is an annotation on the preceding line; the indexed
            // detail is just the method signature — accept any same-named method
            // found via BFS subtypes (already scoped to confirmed implementors).
            Language::Java => true,
            Language::Swift => false,
        }
    }

    pub(crate) fn val_keyword(self) -> &'static str {
        match self {
            Language::Swift => "let",
            _ => "val",
        }
    }

    /// Return the stateless [`LanguageParser`] singleton for this language.
    ///
    /// This is the single authoritative dispatch point: use it instead of
    /// matching on the enum or calling `parse_by_extension` directly.
    pub(crate) fn parser(self) -> &'static dyn crate::language::LanguageParser {
        match self {
            Language::Kotlin => &crate::language::kotlin::KotlinParser,
            Language::Java => &crate::language::java::JavaParser,
            Language::Swift => &crate::language::swift::SwiftParser,
        }
    }
}

/// A position within a document used by infer functions.
///
/// `utf16_col` is a UTF-16 code unit offset, matching the LSP `Position.character` field.
/// Using a named struct (rather than a bare `(usize, usize)` pair) prevents silent
/// transposition of line and column arguments at call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorPos {
    pub line: usize,
    pub utf16_col: usize,
}

impl From<tower_lsp::lsp_types::Position> for CursorPos {
    fn from(position: tower_lsp::lsp_types::Position) -> Self {
        CursorPos {
            line: position.line as usize,
            utf16_col: position.character as usize,
        }
    }
}

/// The caller's position context, used for visibility filtering and type-param substitution.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CallerContext<'a> {
    pub uri: Option<&'a str>,
    pub cursor_line: Option<u32>,
}

/// Kotlin/Java visibility of a declared symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum Visibility {
    #[default]
    Public,
    Internal,
    Protected,
    Private,
}

/// The rarely-populated ("cold") fields of a [`SymbolEntry`], split out behind a
/// single `Option<Box<..>>` so the common symbol (which has none of them) pays
/// only one pointer-sized field instead of four inline `String`/`Vec` headers.
///
/// Measurement (see `indexer::memory_probe_tests`): across a 740k-symbol corpus,
/// 99.1% of symbols have all four of these empty/default simultaneously, so the
/// boxed allocation is skipped for the vast majority of entries.
///
/// Never construct or read these directly on a `SymbolEntry`; go through
/// [`pack_cold_fields`] on the construction side and the accessor methods
/// (`type_params()`, `extension_receiver()`, `extension_receiver_type()`,
/// `doc()`) on the read side, which preserve the prior "empty when absent"
/// semantics without allocating.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SymbolColdFields {
    /// Generic type parameter names extracted from the CST at parse time.
    /// e.g. `class Foo<T, U>` → `["T", "U"]`.
    pub type_params: Vec<String>,
    /// For extension functions: the receiver type name (without generics).
    /// e.g. `fun MyType.foo()` → `"MyType"`, `fun <T> List<T>.bar()` → `"List"`.
    pub extension_receiver: String,
    /// For extension functions: the full receiver type including generics.
    /// e.g. `fun <T> List<T>.bar()` → `"List<T>"`,
    ///      `fun <E, S> Flow<ReducedResult<E, S>>.collectState(…)` → `"Flow<ReducedResult<E, S>>"`.
    /// Empty when the receiver has no generics (in which case `extension_receiver`
    /// already carries the full type).
    pub extension_receiver_type: String,
    /// KDoc / Javadoc text for this symbol.
    /// Empty for source-indexed symbols (doc is extracted live from `FileData.lines`).
    /// Populated for JAR-indexed symbols where we have no real source lines.
    pub doc: String,
}

/// Pack the four rarely-populated fields into an optional boxed [`SymbolColdFields`].
///
/// Returns `None` — allocating no `Box` — when all four are empty/default, which
/// is the ~99% common case and the entire point of the split. Only symbols that
/// actually carry generics, an extension receiver, or doc text pay the heap
/// allocation.
pub(crate) fn pack_cold_fields(
    type_params: Vec<String>,
    extension_receiver: String,
    extension_receiver_type: String,
    doc: String,
) -> Option<Box<SymbolColdFields>> {
    if type_params.is_empty()
        && extension_receiver.is_empty()
        && extension_receiver_type.is_empty()
        && doc.is_empty()
    {
        None
    } else {
        Some(Box::new(SymbolColdFields {
            type_params,
            extension_receiver,
            extension_receiver_type,
            doc,
        }))
    }
}

/// Single symbol definition entry stored in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SymbolEntry {
    pub name: String,
    pub kind: SymbolKind,
    pub visibility: Visibility,
    /// Span of the entire declaration node.
    pub range: Range,
    /// Span of only the identifier — used for `selectionRange` in DocumentSymbol.
    pub selection_range: Range,
    /// Short signature shown in hover/symbol lists.
    /// e.g. `"fun addBiometryToPowerAuth(isAllowedForActiveOp: Boolean)"`,
    ///      `"class CreatePinViewModel"`, `"val isChecked: Boolean"`.
    /// Empty string when not computed.
    #[serde(default)]
    pub detail: String,
    /// Raw parameter text extracted from the CST at index time.
    /// Content between `(` and `)` of `function_value_parameters` / `formal_parameters`.
    /// e.g. `"x: Int, y: String = \"\""`. Empty for zero-param functions or non-callable symbols.
    #[serde(default)]
    pub params: String,
    /// `(required, total)` parameter counts derived from tree nodes at index time.
    /// A param is "required" when it has no `=` default value sibling in the CST.
    /// `(0, 0)` for non-callable symbols or zero-param functions.
    #[serde(default)]
    pub param_counts: (u8, u8),
    /// Enclosing class/object/interface name (immediate parent only).
    /// `None` for top-level declarations; `Some("ClassName")` for members.
    /// Assigned by `assign_containers()` after extraction.
    #[serde(default)]
    pub container: Option<String>,
    /// The four rarely-populated fields (`type_params`, `extension_receiver`,
    /// `extension_receiver_type`, `doc`), boxed together so the common symbol —
    /// which has none of them — pays one pointer instead of four inline headers.
    ///
    /// Production code accesses this exclusively through the accessor methods
    /// (`type_params()`, `extension_receiver()`, `extension_receiver_type()`,
    /// `doc()`) and constructs via [`pack_cold_fields`] — never reads `.cold`
    /// directly. Memory-profiling code (`indexer::memory_probe_tests`) is a
    /// sanctioned exception: it inspects `.cold` directly to account for the
    /// boxed allocation's own size, which is exactly the kind of layout
    /// measurement the accessors intentionally hide.
    ///
    /// NOTE: no `skip_serializing_if` — the on-disk cache uses bincode, a
    /// non-self-describing format that deserializes fields positionally, so
    /// conditionally omitting a field would desync the reader. `None` already
    /// encodes as a single tag byte, so the common case stays compact.
    #[serde(default)]
    pub cold: Option<Box<SymbolColdFields>>,
    /// True when the last value parameter is a function type (lambda), meaning the caller
    /// may use trailing-lambda syntax: `foo { }` instead of `foo({ })`.
    #[serde(default)]
    pub trailing_lambda: bool,
    /// True when the declaration carries an `@Deprecated` annotation.
    /// Used by completion to hide (library) or deprioritize + tag (workspace) the symbol.
    #[serde(default)]
    pub deprecated: bool,
}

impl SymbolEntry {
    /// Return the line number where the symbol's identifier starts.
    ///
    /// This is a convenience accessor for `.selection_range.start.line` (the identifier line),
    /// distinguishing it from `.range.start.line` (the full declaration start, which may differ on
    /// multiline declarations). Reduces coupling and avoids repeated deep field access.
    pub(crate) fn selection_start(&self) -> u32 {
        self.selection_range.start.line
    }

    /// This symbol's `(required, total)` parameter-count range, for checking
    /// whether a call's `CallShape` could target it. `None` when arity
    /// filtering doesn't apply — a non-callable `kind` (arity is meaningless)
    /// or a vararg declaration (`param_counts` can't represent its true
    /// unbounded upper end) — meaning always treat this symbol as compatible.
    pub(crate) fn arity_for_call_shape_check(&self) -> Option<(u8, u8)> {
        if !matches!(
            self.kind,
            SymbolKind::FUNCTION
                | SymbolKind::METHOD
                | SymbolKind::CONSTRUCTOR
                | SymbolKind::OPERATOR
        ) {
            return None;
        }
        if self.params.contains("vararg ") || self.params.contains("vararg\t") {
            return None;
        }
        Some(self.param_counts)
    }

    /// Whether this is a `companion object` declaration, named
    /// (`companion object Factory`) or anonymous (synthesized under the
    /// implicit name `Companion` — see `parser::extract_anonymous_companion_objects`).
    ///
    /// `kind` alone can't tell one apart from a plain `object`; the
    /// `companion` soft-keyword survives only in `detail`, the raw
    /// declaration text — which may carry modifiers, annotations, and
    /// comments alongside the keywords. Matching the two keywords as
    /// *adjacent* tokens accepts every real form (`private companion
    /// object`, `companion object Factory`) while rejecting a comment that
    /// merely mentions the word (`private /* companion */ object Registry`).
    pub(crate) fn is_companion_object(&self) -> bool {
        if self.kind != SymbolKind::OBJECT {
            return false;
        }
        let tokens: Vec<&str> = self.detail.split_whitespace().collect();
        tokens
            .windows(2)
            .any(|pair| pair[0] == "companion" && pair[1] == "object")
    }

    /// Generic type parameter names extracted from the CST at parse time.
    /// e.g. `class Foo<T, U>` → `["T", "U"]`. Empty slice for non-generic symbols.
    pub(crate) fn type_params(&self) -> &[String] {
        self.cold
            .as_ref()
            .map_or(&[], |cold_fields| &cold_fields.type_params)
    }

    /// For extension functions: the receiver type name (without generics).
    /// e.g. `fun MyType.foo()` → `"MyType"`. Empty string for non-extension symbols.
    pub(crate) fn extension_receiver(&self) -> &str {
        self.cold
            .as_ref()
            .map_or("", |cold_fields| &cold_fields.extension_receiver)
    }

    /// For extension functions: the full receiver type including generics.
    /// e.g. `fun <T> List<T>.bar()` → `"List<T>"`. Empty string for non-extension
    /// symbols or when the receiver has no generics.
    pub(crate) fn extension_receiver_type(&self) -> &str {
        self.cold
            .as_ref()
            .map_or("", |cold_fields| &cold_fields.extension_receiver_type)
    }

    /// KDoc / Javadoc text for this symbol. Empty for source-indexed symbols
    /// (doc is extracted live from `FileData.lines`); populated for JAR-indexed
    /// symbols where we have no real source lines.
    pub(crate) fn doc(&self) -> &str {
        self.cold
            .as_ref()
            .map_or("", |cold_fields| &cold_fields.doc)
    }

    /// Mutable access to the boxed cold fields, allocating the box on first use.
    /// Only needed by test setup that mutates a symbol after construction; the
    /// production path builds the box up-front via [`pack_cold_fields`].
    #[cfg(test)]
    fn cold_mut(&mut self) -> &mut SymbolColdFields {
        self.cold
            .get_or_insert_with(|| Box::new(SymbolColdFields::default()))
    }

    /// Replace the generic type parameter names. Allocates the cold box if absent.
    #[cfg(test)]
    pub(crate) fn set_type_params(&mut self, type_params: Vec<String>) {
        self.cold_mut().type_params = type_params;
    }

    /// Replace the KDoc / Javadoc text. Allocates the cold box if absent.
    #[cfg(test)]
    pub(crate) fn set_doc(&mut self, doc: String) {
        self.cold_mut().doc = doc;
    }
}

/// A lightweight record of an extension symbol, stored in the `extension_by_receiver`
/// reverse index for O(1) lookup by receiver type name.
#[derive(Debug, Clone)]
pub(crate) struct ExtensionEntry {
    /// URI of the file that declares this extension.
    pub file_uri: String,
    pub name: String,
    pub kind: SymbolKind,
    pub detail: String,
    pub visibility: Visibility,
    pub package: Option<String>,
    /// True when the last value parameter is a function type — trailing-lambda call is valid.
    pub trailing_lambda: bool,
    /// True when the declaring symbol carries an `@Deprecated` annotation.
    pub deprecated: bool,
}

/// One import statement parsed from a Kotlin file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ImportEntry {
    /// Fully-qualified path without the trailing `.*`.
    /// e.g. `"com.example.Foo"` or `"com.example"` for star imports.
    pub full_path: String,
    /// The name usable locally: last segment, alias, or `"*"` for star.
    pub local_name: String,
    /// True for `import com.example.*`.
    pub is_star: bool,
}

impl ImportEntry {
    /// Does this import make `symbol_name` accessible when defined in `def_pkg`?
    ///
    /// Handles:
    /// - Star import: `import com.example.*` covers any symbol in package `com.example`
    /// - Direct import: `import com.example.Foo` covers `Foo` from `com.example`
    /// - Nested class import: `import com.example.Outer.Config` covers `Config` from `com.example`
    ///   (the nested container `Outer` is an intermediate segment)
    pub(crate) fn covers(&self, def_pkg: &str, symbol_name: &str) -> bool {
        if self.is_star {
            return self.full_path == def_pkg;
        }
        if self.local_name != symbol_name {
            return false;
        }
        if self.full_path == format!("{def_pkg}.{symbol_name}") {
            return true;
        }
        if let Some(rest) = self.full_path.strip_prefix(def_pkg) {
            if let Some(rest) = rest.strip_prefix('.') {
                return rest == symbol_name || rest.ends_with(&format!(".{symbol_name}"));
            }
        }
        false
    }
}

/// A structural syntax error detected by tree-sitter.
///
/// These are zero-false-positive issues: missing brackets, unclosed strings,
/// garbled syntax from a bad edit.  They are NOT serialized to the disk cache
/// (cheap to recompute on every parse).
#[derive(Debug, Clone)]
pub(crate) struct SyntaxError {
    pub range: Range,
    pub message: String,
}

/// All data we keep in memory for one source file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileData {
    pub symbols: Vec<SymbolEntry>,
    pub imports: Vec<ImportEntry>,
    /// Package declaration, e.g. `"com.example.app"`.
    pub package: Option<String>,
    /// Raw source lines — kept for `word_at()` lookups without hitting disk.
    /// Wrapped in Arc so that `clone()` is a cheap atomic refcount bump,
    /// not a full Vec<String> copy (which allocates one heap block per line).
    pub lines: Arc<Vec<String>>,
    /// Source set classification for this file.
    #[serde(default)]
    pub source_set: SourceSet,
    /// Lower-cased identifiers found before `:` on non-comment lines.
    /// Populated once at parse time; used by completion without re-scanning.
    pub declared_names: Vec<String>,
    /// Supertype relationships extracted from the CST at parse time.
    /// Each entry is `(class_name_line, supertype_name, type_args)` where:
    /// - `class_name_line` matches `SymbolEntry::selection_range.start.line` for the declaring class
    /// - `supertype_name` is the base name without type arguments (e.g. `"FlowReducer"`)
    /// - `type_args` are the concrete type arguments (e.g. `["Event", "Effect", "State"]`)
    #[serde(default)]
    pub supers: Vec<(u32, String, Vec<String>)>,
    /// RHS-inferred types for unannotated properties, extracted from the CST at parse time.
    /// Each entry is `(declaration_line, var_name, inferred_type)`.
    /// Used as the primary type inference path for indexed files, avoiding fragile string
    /// scanning for patterns like `inject<T>()`, `by lazy { T() }`, and `T(args)`.
    #[serde(default)]
    pub rhs_types: Vec<(u32, String, String)>,
    /// Method-call RHS patterns for unannotated properties: `val x = receiver.method(args)`.
    /// Each entry is `(declaration_line, var_name, receiver_name, method_name)`.
    /// Used by method-return-type inference for indexed files.
    #[serde(default)]
    pub method_call_rhs: Vec<(u32, String, String, String)>,
    /// Field-access RHS patterns for unannotated properties: `val x = receiver.field`.
    /// Each entry is `(declaration_line, var_name, receiver_name, field_name)`.
    /// Used by field-type inference for indexed files (e.g. constructor params
    /// that expose a field as a class property).
    #[serde(default)]
    pub field_access_rhs: Vec<(u32, String, String, String)>,
    /// Explicit type annotations for properties, extracted from the CST at parse time.
    /// Each entry is `(declaration_line, var_name, declared_type)` where `declared_type`
    /// preserves generics and nullability: `val x: List<Foo>?` → `"List<Foo>?"`.
    /// Covers both `user_type` and `nullable_type` annotation nodes.
    /// Takes priority over line-scan inference for indexed files.
    #[serde(default)]
    pub type_annotations: Vec<(u32, String, String)>,
    /// Structural syntax errors from tree-sitter (ERROR / MISSING nodes).
    /// Transient — not serialized to disk cache.
    #[serde(skip)]
    pub syntax_errors: Vec<SyntaxError>,
}

impl FileData {
    /// Find the name of the innermost class/interface/object/enum that contains
    /// `line` in this file's symbol list. Returns `None` if the symbol is
    /// top-level (not inside any class).
    pub(crate) fn containing_class_at(&self, line: u32) -> Option<String> {
        const CLASS_KINDS: &[SymbolKind] = &[
            SymbolKind::CLASS,
            SymbolKind::INTERFACE,
            SymbolKind::STRUCT,
            SymbolKind::ENUM,
            SymbolKind::OBJECT,
        ];
        self.symbols
            .iter()
            .filter(|s| CLASS_KINDS.contains(&s.kind))
            .filter(|s| s.range.start.line <= line && line <= s.range.end.line)
            .min_by_key(|s| s.range.end.line.saturating_sub(s.range.start.line))
            .map(|s| s.name.clone())
    }
}

/// Result of parsing a single file. Pure data, no side effects.
/// This is what index_content will return instead of mutating DashMaps.
#[derive(Debug, Clone)]
pub(crate) struct FileIndexResult {
    /// File URI that was parsed.
    pub uri: tower_lsp::lsp_types::Url,
    /// Parsed file data (symbols, imports, package, lines).
    pub data: FileData,
    /// Supertype relationships discovered in this file.
    /// Format: (supertype_name, implementing_class_location)
    pub supertypes: Vec<(String, tower_lsp::lsp_types::Location)>,
    /// Content hash for cache invalidation.
    pub content_hash: u64,
    /// Parse error if tree-sitter failed.
    pub error: Option<String>,
}

/// Statistics about an indexing run.
#[derive(Debug, Clone, Default)]
pub(crate) struct IndexStats {
    /// Files loaded from cache (mtime unchanged).
    pub cache_hits: usize,
    /// Files actually parsed by tree-sitter.
    pub files_parsed: usize,
    /// Total symbols extracted.
    pub symbols_extracted: usize,
}

/// Result of indexing an entire workspace. Pure data, no side effects.
/// This is what index_workspace will return instead of mutating state.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceIndexResult {
    /// All successfully parsed files.
    pub files: Vec<FileIndexResult>,
    /// Statistics about the indexing run.
    pub stats: IndexStats,
    /// Workspace root that was indexed.
    pub workspace_root: std::path::PathBuf,
    /// True if the run was aborted mid-way (e.g. root generation changed).
    /// Callers must NOT call apply_workspace_result when this is true — doing
    /// so would reset_index_state() and apply only the partial result set.
    pub aborted: bool,
    /// True when the workspace was fully scanned (not truncated by MAX_INDEX_FILES).
    /// Written into the on-disk cache so warm-manifest mode is only used when the
    /// cache is a complete snapshot of the workspace.
    pub complete_scan: bool,
}

// ─── File interning ───────────────────────────────────────────────────────────

/// Index of a file's URI inside a [`FileTable`]. A 4-byte handle that replaces a
/// per-entry `tower_lsp::Location`'s heap-allocated `Url` in the index maps: the
/// same file's URI is stored once (in the table) instead of once per symbol.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct FileId(u32);

/// A symbol's location expressed as an interned [`FileId`] plus its range —
/// 20 bytes, no owned heap — instead of a full `Location` (104 B inline + the
/// `Url` string on the heap). Convert to a `Location` **only at the LSP
/// boundary** via [`FileTable::location`]; never store a `Location` back into an
/// index map.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SymbolLoc {
    pub(crate) file: FileId,
    pub(crate) range: Range,
}

impl SymbolLoc {
    pub(crate) fn new(file: FileId, range: Range) -> Self {
        Self { file, range }
    }
}

/// Append-only interning table mapping file URIs to [`FileId`]s and back.
///
/// `by_id[FileId] == Arc<Url>` for that file; `by_uri[uri] == FileId`. The table
/// is append-only for the lifetime of an [`crate::indexer::Indexer`]: re-indexing
/// the same URI returns its existing `FileId` (idempotent), so already-interned
/// `SymbolLoc`s stay valid across `reset_index_state` (which *retains* library
/// entries rather than clearing). Growth is bounded by the number of distinct
/// files seen in a session, so no rebuild/clear is needed — the append-only
/// invariant is what keeps retained library `SymbolLoc`s from dangling.
pub(crate) struct FileTable {
    by_id: std::sync::RwLock<Vec<Arc<tower_lsp::lsp_types::Url>>>,
    by_uri: dashmap::DashMap<String, FileId>,
}

/// Throttle counter for [`FileTable::url`]'s invariant-violation warning —
/// see [`crate::util::throttled_warn`].
static FILE_ID_LOOKUP_MISSES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl Default for FileTable {
    fn default() -> Self {
        Self::new()
    }
}

impl FileTable {
    pub(crate) fn new() -> Self {
        Self {
            by_id: std::sync::RwLock::new(Vec::new()),
            by_uri: dashmap::DashMap::new(),
        }
    }

    /// Intern `uri`, returning its stable [`FileId`]. Idempotent: the same URI
    /// always maps to the same id for the table's lifetime.
    pub(crate) fn intern(&self, uri: &tower_lsp::lsp_types::Url) -> FileId {
        if let Some(existing) = self.by_uri.get(uri.as_str()) {
            return *existing;
        }
        // Serialize appends under the id-vec write lock; re-check inside the
        // critical section so a racing interner cannot allocate a second id for
        // the same URI.
        let mut ids = self.by_id.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = self.by_uri.get(uri.as_str()) {
            return *existing;
        }
        // Fail fast rather than wrap: a truncated id would alias an existing
        // file and silently corrupt every lookup keyed on it.
        assert!(
            u32::try_from(ids.len()).is_ok(),
            "FileTable overflow: more than u32::MAX distinct files interned"
        );
        let id = FileId(ids.len() as u32);
        ids.push(Arc::new(uri.clone()));
        self.by_uri.insert(uri.as_str().to_string(), id);
        id
    }

    /// The interned `Url` for `id`, or `None` if `id` is not from this table.
    ///
    /// `id` normally comes from a [`SymbolLoc`] this same table produced via
    /// [`Self::intern`], and the table is append-only (see the struct doc) —
    /// so a miss here isn't a legitimate "not found," it means a `SymbolLoc`
    /// outlived or crossed into a different `FileTable` than the one that
    /// interned it. Every caller (`location`, and the many direct `url()`
    /// call sites across resolution) treats a miss as "drop this entry from
    /// the result," which looks identical to an ordinary absence — this is
    /// the one place that can tell the two apart.
    pub(crate) fn url(&self, id: FileId) -> Option<Arc<tower_lsp::lsp_types::Url>> {
        let table = self.by_id.read().unwrap_or_else(|e| e.into_inner());
        let found = table.get(id.0 as usize).cloned();
        if found.is_none() {
            crate::util::throttled_warn(&FILE_ID_LOOKUP_MISSES, 5, || {
                format!(
                    "FileTable::url: FileId({}) has no entry (table holds {} interned files) — \
                     a SymbolLoc referencing it predates or crosses into a different FileTable; \
                     the caller silently drops whatever result depended on it",
                    id.0,
                    table.len(),
                )
            });
        }
        found
    }

    /// Build a `tower_lsp::Location` from a [`SymbolLoc`]. This is the ONLY place
    /// a `Location` is reconstituted from interned index data — the LSP boundary.
    /// Returns `None` if the [`FileId`] is unknown (should not happen for ids the
    /// table itself produced).
    pub(crate) fn location(&self, loc: SymbolLoc) -> Option<tower_lsp::lsp_types::Location> {
        self.url(loc.file)
            .map(|url| tower_lsp::lsp_types::Location {
                uri: (*url).clone(),
                range: loc.range,
            })
    }

    /// Snapshot of all interned `Url`s (cheap `Arc` clones). Used by the memory
    /// probe to attribute the table's bytes; not on any production path.
    #[cfg(test)]
    pub(crate) fn urls_snapshot(&self) -> Vec<Arc<tower_lsp::lsp_types::Url>> {
        self.by_id.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Interned identifier for a JAR path, into a [`JarTable`]. Mirrors [`FileId`]/
/// [`FileTable`] — same double-checked-locking intern, same append-only
/// growth (JAR identity doesn't change mid-session; reindex rebuilds the
/// whole table, see Task 11).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct JarId(u32);

/// Append-only interning table mapping JAR paths to [`JarId`]s and back.
/// Mirrors [`FileTable`] precisely, substituting plain path strings for
/// `Url`s since JAR paths don't need URL parsing.
pub(crate) struct JarTable {
    by_id: std::sync::RwLock<Vec<String>>,
    by_path: dashmap::DashMap<String, JarId>,
}

impl Default for JarTable {
    fn default() -> Self {
        Self::new()
    }
}

impl JarTable {
    pub(crate) fn new() -> Self {
        Self {
            by_id: std::sync::RwLock::new(Vec::new()),
            by_path: dashmap::DashMap::new(),
        }
    }

    /// Intern `path`, returning its stable [`JarId`]. Idempotent and race-safe:
    /// a fast-path read first, then a double-checked write under the same
    /// lock `FileTable::intern` uses (see PR #208's review for why this is
    /// race-free — the re-check happens inside the critical section, so a
    /// losing concurrent caller observes the winner's id, never mints a
    /// second one).
    pub(crate) fn intern(&self, path: &str) -> JarId {
        if let Some(existing) = self.by_path.get(path) {
            return *existing;
        }
        // Serialize appends under the id-vec write lock; re-check inside the
        // critical section so a racing interner cannot allocate a second id for
        // the same path.
        let mut ids = self.by_id.write().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = self.by_path.get(path) {
            return *existing;
        }
        // Fail fast rather than wrap: a truncated id would alias an existing
        // jar and silently corrupt every lookup keyed on it.
        assert!(
            u32::try_from(ids.len()).is_ok(),
            "JarTable overflow: more than u32::MAX distinct jars interned"
        );
        let id = JarId(ids.len() as u32);
        ids.push(path.to_owned());
        self.by_path.insert(path.to_owned(), id);
        id
    }

    /// The interned path for `id`, or `None` if `id` is not from this table.
    pub(crate) fn path(&self, id: JarId) -> Option<String> {
        self.by_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id.0 as usize)
            .cloned()
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
