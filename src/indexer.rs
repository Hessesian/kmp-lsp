use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use dashmap::{DashMap, DashSet};
use tower_lsp::lsp_types::*;

use crate::types::FileData;

// Re-export rg-module items that existing callers reach via `crate::indexer::`.
pub(crate) use self::scan::{NoopReporter, ProgressReporter};
pub(crate) use crate::rg::IgnoreMatcher;

mod doc;
mod html_md;

mod cst_folding;
pub(crate) use self::cst_folding::cst_folding_ranges;

mod infer;
pub(crate) mod resolution;
// Re-export pure helpers from submodules so existing callers within this file
// and the inline test module (`use super::*`) continue to resolve them by name.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::infer::deps::TestDeps;
#[allow(unused_imports)]
pub(crate) use self::infer::{
    args::{
        extract_first_arg, extract_named_arg_name, find_as_call_arg_type,
        find_named_param_type_in_sig, has_named_params_not_it,
    },
    cst_cursor::{cst_call_info, cst_cursor_is_local_var, cst_outer_call_info, CallInfo},
    deps::{CallableInfo, InferDeps},
    expr_type::infer_expr_type,
    it_this::{
        find_it_element_type, find_it_element_type_in_lines, find_named_lambda_param_type,
        find_named_lambda_param_type_in_lines, find_this_context_in_lines,
        find_this_element_type_in_lines, is_lambda_param, lambda_brace_pos_for_param,
        lambda_param_position_on_line, line_has_lambda_param, ThisContext,
    },
    lambda::{
        lambda_type_first_input, lambda_type_nth_input, lambda_type_receiver, RECEIVER_THIS_FNS,
        SCOPE_FUNCTIONS,
    },
    receiver::lambda_receiver_type_from_context,
    sig::{
        collect_all_fun_params_texts, collect_params_from_line, collect_signature,
        find_fun_params_text_fast, find_fun_signature_full, find_fun_signature_with_receiver,
        is_import_reachable, last_fun_param_type_str, nth_fun_param_type_str,
        resolve_call_signature, split_params_at_depth_zero, strip_trailing_call_args, CallSite,
        ResolutionScope, SignatureResult,
    },
    type_subst::find_last_dot_at_depth_zero,
};

mod cache;
pub(crate) use self::cache::workspace_cache_path;
pub(crate) use self::cache::xdg_cache_base;

pub(crate) mod enrich;
pub(crate) use self::enrich::EnrichmentHandle;

mod discover;

mod scan;
pub(crate) const MAX_FILES_UNLIMITED: usize = usize::MAX;

mod workspace_root;
pub(crate) use self::workspace_root::WorkspaceRoot;

mod apply;
pub(crate) mod jar;
pub(crate) mod jar_cache;
pub(crate) mod jar_phase;
pub(crate) mod sources_jar_cache;

#[cfg(test)]
#[path = "indexer/jar_tests.rs"]
mod jar_tests;
#[cfg(test)]
#[path = "indexer/sources_jar_cache_tests.rs"]
mod sources_jar_cache_tests;
#[cfg(test)]
pub(crate) use self::apply::build_bare_names;
#[allow(unused_imports)]
pub(crate) use self::apply::file_contributions;
#[cfg(test)]
pub(crate) use self::apply::stale_keys_for;

pub(crate) mod lookup;
pub(crate) use lookup::apply_type_subst;

mod node_ext;
pub(crate) use node_ext::NodeExt;

mod scope;
pub(crate) use scope::find_enclosing_call_name;
pub(crate) use scope::is_id_char;
pub(crate) use scope::last_ident_in;

pub(crate) mod live_tree;
pub(crate) use live_tree::LiveDoc;
mod live_tree_impl;

// Re-export cache/scan items needed by the inline test module below.
#[cfg(test)]
use self::cache::{cache_entry_to_file_result, FileCacheEntry};
use crate::resolver::infer_variable_type_raw;
#[cfg(test)]
use crate::rg::regex_escape;
#[cfg(test)]
#[allow(unused_imports)]
use crate::types::{FileIndexResult, IndexStats};
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
pub(crate) mod test_helpers;

// ─── Pure helper types ────────────────────────────────────────────────────────

/// Everything a single file *adds* to the index. Pure value — no DashMaps.
pub(crate) struct FileContributions {
    pub definitions: HashMap<String, Vec<Location>>,
    /// Both `pkg.Sym` and `pkg.FileStem.Sym` keys.
    pub qualified: HashMap<String, Location>,
    pub packages: HashMap<String, Vec<String>>,
    pub subtypes: HashMap<String, Vec<Location>>,
    /// receiver_base_name → extension entries from this file.
    pub extensions: HashMap<String, Vec<crate::types::ExtensionEntry>>,
    pub file_data: (String, Arc<crate::types::FileData>),
    pub content_hash: (String, u64),
}

/// Keys to remove from the index when a file is replaced.
pub(crate) struct StaleKeys {
    pub definition_names: Vec<String>,
    /// Both aliases: `pkg.Sym` AND `pkg.FileStem.Sym`.
    pub qualified_keys: Vec<String>,
    pub package: Option<String>,
}

pub(crate) type CompletionCacheEntry = (String, String, Vec<CompletionItem>, bool);

pub(crate) struct Indexer {
    /// URI string → parsed file data.
    pub(crate) files: DashMap<String, Arc<FileData>>,
    /// Short name → definition locations  (fast first-pass lookup).
    pub(crate) definitions: DashMap<String, Vec<Location>>,
    /// Fully-qualified name → location   (e.g. "com.example.Foo" → …).
    pub(crate) qualified: DashMap<String, Location>,
    /// Package name → vec of URI strings (for same-package resolution).
    pub(crate) packages: DashMap<String, Vec<String>>,
    /// Workspace root path + monotonic staleness generation.
    /// The only write path is [`WorkspaceRoot::set`], which always bumps the
    /// generation — coupling enforced by the type, not by convention.
    /// Written only by [`crate::workspace::Actor`]; read-paths elsewhere observe it.
    pub(crate) workspace_root: WorkspaceRoot,
    /// URI string → xxHash of last indexed content (skip identical re-parses).
    content_hashes: DashMap<String, u64>,
    /// Semaphore capping concurrent parse workers.
    parse_sem: Arc<tokio::sync::Semaphore>,
    /// Times tree-sitter actually ran (used in tests).
    pub parse_count: AtomicU64,
    /// URI string → pre-built completion items for that file.
    /// Populated lazily on first dot-completion hit; cleared on re-index.
    pub(crate) completion_cache: DashMap<String, Arc<Vec<CompletionItem>>>,
    /// URI string → lines of the CURRENT document content.
    /// Updated synchronously on every did_change, bypassing the 120ms debounce.
    /// Used by `completions()` so dot-detection always sees the latest text.
    /// Arc-wrapped so `.clone()` is a cheap refcount bump, not a full Vec copy.
    pub(crate) live_lines: DashMap<String, Arc<Vec<String>>>,
    /// Reverse supertype index: supertype name → locations of implementing/extending classes.
    /// Populated during `index_content()` for fast `goToImplementation` lookups.
    pub(crate) subtypes: DashMap<String, Vec<Location>>,
    /// Cached sorted list of all project class/symbol names for bare-word completion.
    /// Rebuilt after each file index; avoids iterating `definitions` on every keystroke.
    pub(crate) bare_name_cache: std::sync::RwLock<Vec<String>>,
    /// Dirty flag: set to true when definitions change and bare_name_cache needs rebuild.
    /// Rebuild is deferred until next read; avoids full rebuild on every keystroke.
    pub(crate) bare_names_dirty: AtomicBool,
    /// Last completion result: (uri, context_key, items).
    /// `context_key` = line text up to (but not including) the current word.
    /// When the key matches, the cached items are returned without recomputation —
    /// covers the common "typing more characters in the same word/after same dot" case.
    pub(crate) last_completion: std::sync::Mutex<Option<CompletionCacheEntry>>,
    /// Cache for the ancestor type-name sets computed by `collect_this_extensions`.
    /// Key: `"ClassName@file_uri"`. Value: the resolved ancestor set.
    /// Avoids re-running `walk_hierarchy` + `resolve_symbol_no_rg` on every line change.
    /// Cleared together with the rest of the completion caches on reindex / JAR finish.
    pub(crate) this_ext_ancestor_cache:
        DashMap<String, std::sync::Arc<std::collections::HashSet<String>>>,
    /// Monotonically incremented whenever index state changes in a way that could
    /// invalidate an in-flight completion result (JAR indexing completes, workspace
    /// reindex). Completion requests capture this epoch before computing and refuse
    /// to store if the epoch has advanced, preventing stale results from racing
    /// past an invalidation.
    pub(crate) completion_epoch: AtomicU64,
    /// Guard to prevent concurrent background indexing runs on same Indexer.
    pub(crate) indexing_in_progress: std::sync::atomic::AtomicBool,
    /// Set when a reindex request arrives while a scan is already running.
    /// The pending scan is started by the active scan's caller once its full
    /// workflow (impl + apply + source_paths + save_cache) completes.
    pub(crate) pending_reindex: std::sync::atomic::AtomicBool,
    /// Root to use for the pending reindex. `None` means use the current workspace root.
    /// Written under a mutex so the *last* concurrent caller wins (RA OpQueue semantics).
    pub(crate) pending_reindex_root: RwLock<Option<PathBuf>>,
    /// Max files cap for the pending reindex. Preserves the intent of the last caller:
    /// a full (unbounded) reindex queued during a bounded scan keeps its unlimited cap.
    pub(crate) pending_reindex_max: std::sync::atomic::AtomicUsize,
    /// Number of parse tasks completed in current indexing run (for progress tracking).
    pub(crate) parse_tasks_completed: std::sync::atomic::AtomicUsize,
    /// Total number of parse tasks spawned in current indexing run.
    pub(crate) parse_tasks_total: std::sync::atomic::AtomicUsize,
    /// Paths currently scheduled or in-flight: canonical path -> generation when scheduled.
    /// Prevents duplicate scheduling of identical parse work for same generation.
    scheduled_paths: DashMap<String, u64>,
    /// Set when workspace was explicitly configured (env var, config file, or changeRoot command).
    /// When true, `did_open` auto-detection will NOT override the workspace.
    /// Written only by [`crate::workspace::Actor`].
    pub(crate) workspace_pinned: std::sync::atomic::AtomicBool,
    /// Set to true after a non-truncated workspace scan; false after a truncated one.
    /// Drives `complete_scan` on the on-disk cache so warm-manifest mode is only
    /// used when the cache is known to be a full workspace snapshot.
    pub(crate) last_scan_complete: std::sync::atomic::AtomicBool,
    /// User-configured ignore patterns from LSP `initializationOptions`.
    /// Applied during file discovery to exclude matching paths.
    /// Written only by [`crate::workspace::Actor`]; tests configure it through actor events too.
    pub(crate) ignore_matcher: RwLock<Option<Arc<IgnoreMatcher>>>,
    /// Resolved source paths written by the workspace actor for `index_source_paths`.
    /// Populated from `Config::resolve_sources()`, which merges `initializationOptions.indexingOptions.sourcePaths`,
    /// auto-discovered `workspace.json` / build-layout paths, and the default extract-sources dir.
    /// Written only by [`crate::workspace::Actor`]; visibility stays `pub(crate)` for read-path consumers.
    pub(crate) source_paths_raw: RwLock<Vec<String>>,
    /// Workspace source roots for scoping rg searches to project source directories only.
    /// Populated exclusively from workspace.json JetBrains module sourceRoots
    /// (`workspace_json::load_source_paths`). LSP init `sourcePaths` is intentionally
    /// excluded — it is an additive indexing override for stubs/generated code, not a
    /// search-scope restriction. Auto-detected build-layout paths, Android SDK sources,
    /// and ~/.kmp-lsp/sources are also excluded.
    pub(crate) workspace_source_roots: RwLock<Vec<String>>,
    /// URIs of files indexed from `sourcePaths` that lie outside the workspace root.
    /// These are treated as library sources: available for hover/definition/autocomplete
    /// but excluded from findReferences and rename.
    pub(crate) library_uris: DashSet<String>,
    /// Extracted-on-disk JAR source `file://` URI string → its original
    /// `jar:…!/Foo.kt` sources URI. Populated when go-to-definition extracts a
    /// `*-sources.jar` entry to disk. Lets features map an opened extracted file
    /// back to the indexed `jar:` entry (which carries the real package/symbols).
    pub(crate) extracted_jar_sources: DashMap<String, String>,
    /// Simple name → sorted vec of importable FQNs.
    /// e.g. "Composable" → ["androidx.compose.runtime.Composable"]
    /// Built from top-level symbols only (no synthetic file-stem keys).
    /// Rebuilt in rebuild_bare_name_cache(); used by complete_bare for auto-import edits.
    pub(crate) importable_fqns: std::sync::RwLock<std::collections::HashMap<String, Vec<String>>>,
    /// URI string → live parse tree for currently-open editor files.
    /// Updated synchronously on every `did_open` / `did_change`; removed on `did_close`.
    /// Not cleared on `reset_index_state` — open-file trees survive workspace reindex.
    pub(crate) live_trees: DashMap<String, Arc<LiveDoc>>,
    /// Per-session cache for function signature lookups.
    /// Key: (fn_name, uri_string) → cached params text.
    /// Cleared on reindex to avoid stale results.
    pub(crate) sig_cache: DashMap<(String, String), Option<String>>,
    /// Like `sig_cache` but for the index-only fast lookup used by lambda/receiver
    /// inference (no rg, no all-files re-scan per call). Separate so it never holds an
    /// index-only `None` that would mask `sig_cache`'s rg-backed result.
    pub(crate) sig_fast_cache: DashMap<(String, String), Option<String>>,
    /// Handle for submitting unresolved symbols to background rg enrichment.
    /// Noop in CLI mode and tests; set via `set_enrichment_handle`.
    pub(crate) enrichment: std::sync::RwLock<EnrichmentHandle>,
    /// Observable phase of the JAR symbol indexing pipeline.
    /// Readable by features (hover, completion) without touching workspace actor state.
    /// Transitions: `Pending` → `InProgress` → `Ready`/`Failed`.
    pub(crate) jar_phase: Arc<std::sync::Mutex<crate::indexer::jar_phase::JarPhase>>,
    /// Long-lived sidecar process for JAR/AAR symbol indexing.
    /// `None` when `kmp-jar-indexer` binary/jar is not present, or after a crash.
    pub(crate) jar_sidecar: std::sync::Mutex<Option<crate::sidecar::SidecarHandle>>,
    /// Reverse index: receiver type base-name → extension symbols declared for that receiver.
    /// e.g. `"ViewModel"` → [ExtensionEntry { name: "viewModelScope", … }]
    /// Used by bare-word completion and type inference to avoid full file scans.
    /// Cleared by `reset_index_state`; populated by `apply_contributions`.
    pub(crate) extension_by_receiver: DashMap<String, Vec<crate::types::ExtensionEntry>>,
    /// Symbols extracted from Gradle-cache JARs/AARs via the sidecar process.
    /// Keyed by a synthetic `jar:file://...` URI string.
    /// Intentionally NOT cleared by `reset_index_state()` — JAR symbols survive workspace reindex.
    pub(crate) jar_files: DashMap<String, Arc<FileData>>,
    /// Name → locations for JAR-sourced symbols.
    /// NOT cleared by `reset_index_state()`.
    pub(crate) jar_definitions: DashMap<String, Vec<tower_lsp::lsp_types::Location>>,
    /// Reverse index: JAR URI → symbol names in that JAR.
    /// Enables O(symbols_in_jar) removal instead of O(total_jar_symbols).
    /// NOT cleared by `reset_index_state()`.
    pub(crate) jar_uri_to_defs: DashMap<String, Vec<String>>,
    /// JAR URI → per-symbol package, index-aligned with the jar's `FileData.symbols`
    /// (and the synthetic line number, which equals the symbol's index). Lets import
    /// resolution filter a JAR symbol by its *real* package instead of the unreliable
    /// one-package-per-jar inference. Empty string where the sidecar gave no package.
    /// NOT cleared by `reset_index_state()`.
    pub(crate) jar_symbol_packages: DashMap<String, Vec<String>>,
}

/// Cap on how many same-named definitions a receiver-less by-name inference lookup
/// scans (see [`Indexer::find_in_workspace_defs`]). Common method names resolve to
/// thousands of library definitions once source-JARs are indexed; without a receiver
/// type the exact overload can't be picked anyway, so bound the work.
pub(crate) const MAX_BY_NAME_DEFS: usize = 64;

impl InferDeps for Indexer {
    fn find_fun_params_text(&self, fn_name: &str, uri: &Url) -> Option<String> {
        // Index-only: lambda/receiver inference is a hot path (called per lambda in
        // inlay hints/hover); the old rg + all-files fallback spawned subprocesses for
        // un-indexed stdlib callees (`let`/`forEachIndexed`) and made inlay time out.
        find_fun_params_text_fast(fn_name, self, uri)
    }
    fn find_var_type(&self, var_name: &str, uri: &Url) -> Option<String> {
        infer_variable_type_raw(self, var_name, uri)
    }
    fn find_field_type(&self, class_name: &str, field_name: &str) -> Option<String> {
        if let Some(type_name) = synthetic_enum_field(self, class_name, field_name) {
            return Some(type_name);
        }
        crate::resolver::infer::find_field_type_in_class(self, class_name, field_name)
    }
    fn find_fun_return_type(&self, fn_name: &str) -> Option<String> {
        crate::resolver::infer::find_fun_return_type_by_name(self, fn_name)
    }

    fn find_fun_return_type_reachable(&self, fn_name: &str, uri: &Url) -> Option<String> {
        crate::resolver::infer::find_fun_return_type_reachable(self, fn_name, uri)
    }
    fn find_class_type_params(&self, class_name: &str) -> Vec<String> {
        let Some(locations) = self.definitions.get(class_name) else {
            return Vec::new();
        };
        for loc in locations.iter() {
            if let Some(file_data) = self.files.get(loc.uri.as_str()) {
                if let Some(sym) = file_data
                    .symbols
                    .iter()
                    .find(|s| s.name == class_name && !s.type_params.is_empty())
                {
                    return sym.type_params.clone();
                }
            }
        }
        Vec::new()
    }
    fn find_method_return_type_for_type(
        &self,
        class_name: &str,
        method_name: &str,
    ) -> Option<String> {
        if let Some(type_name) = synthetic_enum_method(self, class_name, method_name) {
            return Some(type_name);
        }
        if let Some(type_name) =
            crate::resolver::infer::find_method_return_type(self, class_name, method_name, None)
        {
            return Some(type_name);
        }
        if let Some(type_name) = crate::resolver::infer::find_extension_fn_return_type(
            self,
            class_name,
            method_name,
            None,
        ) {
            return Some(type_name);
        }
        crate::resolver::infer::find_method_return_type_via_supertypes(
            self,
            class_name,
            method_name,
            None,
        )
    }
    fn find_method_params_text(&self, class_name: &str, method_name: &str) -> Option<String> {
        crate::indexer::infer::sig::find_method_params_in_class(self, class_name, method_name)
    }
    fn find_fun_callable_info(&self, fn_name: &str, _uri: &Url) -> Option<CallableInfo> {
        // Workspace definitions first (scoped + capped — see find_in_workspace_defs).
        let from_workspace = self.find_in_workspace_defs(fn_name, |loc| {
            let file_data = self.files.get(loc.uri.as_str())?;
            let sym = file_data
                .symbols
                .iter()
                .find(|s| s.name == fn_name && !s.type_params.is_empty())?;
            Some(CallableInfo {
                type_params: sym.type_params.clone(),
                extension_receiver_type: sym.extension_receiver_type.clone(),
            })
        });
        if from_workspace.is_some() {
            return from_workspace;
        }
        // Fallback: JAR-indexed files (sidecar symbols carry type_params). JAR symbols
        // use a synthetic line == index into `symbols`, so address each entry directly
        // (O(1)) rather than a per-loc linear `find` over the whole jar's symbol list.
        let jar_locs = self.jar_definitions.get(fn_name)?;
        for loc in jar_locs.iter().take(MAX_BY_NAME_DEFS) {
            if let Some(file_data) = self.jar_files.get(loc.uri.as_str()) {
                if let Some(sym) = file_data
                    .symbols
                    .get(loc.range.start.line as usize)
                    .filter(|s| s.name == fn_name && !s.type_params.is_empty())
                {
                    return Some(CallableInfo {
                        type_params: sym.type_params.clone(),
                        extension_receiver_type: sym.extension_receiver_type.clone(),
                    });
                }
            }
        }
        None
    }
    fn find_contextual_type(
        &self,
        name: &str,
        uri: &Url,
        line: usize,
        col: usize,
    ) -> Option<String> {
        use tower_lsp::lsp_types::Position;
        self.infer_lambda_param_type_at(name, uri, Position::new(line as u32, col as u32))
    }
}

// ─── Synthetic enum members ──────────────────────────────────────────────────
//
// Kotlin generates these on every enum class:
//   .entries  → EnumEntries<T>  (effectively List<T>)
//   .values() → Array<T>
//   .valueOf(String) → T
//   .name     → String  (instance)
//   .ordinal  → Int     (instance)

fn is_enum_class(indexer: &Indexer, class_name: &str) -> bool {
    let Some(locs) = indexer.definitions.get(class_name) else {
        return false;
    };
    for loc in locs.iter() {
        if let Some(fd) = indexer.files.get(loc.uri.as_str()) {
            if fd
                .symbols
                .iter()
                .any(|s| s.name == class_name && s.kind == SymbolKind::ENUM)
            {
                return true;
            }
        }
    }
    false
}

fn synthetic_enum_field(indexer: &Indexer, class_name: &str, field_name: &str) -> Option<String> {
    // Check name first to avoid expensive is_enum_class lookup for non-synthetic fields
    match field_name {
        "entries" | "name" | "ordinal" => {}
        _ => return None,
    }
    if !is_enum_class(indexer, class_name) {
        return None;
    }
    match field_name {
        "entries" => Some(format!("List<{class_name}>")),
        "name" => Some("String".to_string()),
        "ordinal" => Some("Int".to_string()),
        _ => None,
    }
}

fn synthetic_enum_method(indexer: &Indexer, class_name: &str, method_name: &str) -> Option<String> {
    match method_name {
        "values" | "valueOf" => {}
        _ => return None,
    }
    if !is_enum_class(indexer, class_name) {
        return None;
    }
    match method_name {
        "values" => Some(format!("Array<{class_name}>")),
        "valueOf" => Some(class_name.to_string()),
        _ => None,
    }
}

impl Indexer {
    pub(crate) fn parse_sem(&self) -> Arc<tokio::sync::Semaphore> {
        Arc::clone(&self.parse_sem)
    }

    pub(crate) fn new() -> Self {
        use crate::indexer::jar_phase::JarPhase;

        #[cfg(not(test))]
        let jar_sidecar = crate::sidecar::SidecarHandle::try_launch();
        #[cfg(test)]
        let jar_sidecar: Option<crate::sidecar::SidecarHandle> = None;

        let initial_jar_phase = if jar_sidecar.is_some() {
            JarPhase::Pending
        } else {
            JarPhase::Unavailable
        };

        Self {
            files: DashMap::new(),
            definitions: DashMap::new(),
            qualified: DashMap::new(),
            packages: DashMap::new(),
            workspace_root: WorkspaceRoot::new(),
            content_hashes: DashMap::new(),
            // Allow configurable concurrent parse workers. Default to number of CPU cores.
            // Use env KMP_LSP_PARSE_WORKERS to override.
            parse_sem: {
                // Default to half of available CPUs to avoid saturating system.
                let cpus = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);
                let default = (cpus / 2).max(1);
                let configured = std::env::var("KMP_LSP_PARSE_WORKERS")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(default);
                Arc::new(tokio::sync::Semaphore::new(configured))
            },
            parse_count: AtomicU64::new(0),
            completion_cache: DashMap::new(),
            live_lines: DashMap::new(),
            subtypes: DashMap::new(),
            bare_name_cache: std::sync::RwLock::new(Vec::new()),
            bare_names_dirty: AtomicBool::new(true),
            last_completion: std::sync::Mutex::new(None),
            this_ext_ancestor_cache: DashMap::new(),
            completion_epoch: AtomicU64::new(0),
            indexing_in_progress: std::sync::atomic::AtomicBool::new(false),
            pending_reindex: std::sync::atomic::AtomicBool::new(false),
            pending_reindex_root: RwLock::new(None),
            pending_reindex_max: std::sync::atomic::AtomicUsize::new(0),
            parse_tasks_completed: std::sync::atomic::AtomicUsize::new(0),
            parse_tasks_total: std::sync::atomic::AtomicUsize::new(0),
            scheduled_paths: DashMap::new(),
            workspace_pinned: std::sync::atomic::AtomicBool::new(false),
            last_scan_complete: std::sync::atomic::AtomicBool::new(false),
            ignore_matcher: RwLock::new(None),
            source_paths_raw: RwLock::new(Vec::new()),
            workspace_source_roots: RwLock::new(Vec::new()),
            library_uris: DashSet::new(),
            extracted_jar_sources: DashMap::new(),
            importable_fqns: std::sync::RwLock::new(std::collections::HashMap::new()),
            live_trees: DashMap::new(),
            sig_cache: DashMap::new(),
            sig_fast_cache: DashMap::new(),
            enrichment: std::sync::RwLock::new(EnrichmentHandle::noop()),
            jar_sidecar: std::sync::Mutex::new(jar_sidecar),
            jar_phase: Arc::new(std::sync::Mutex::new(initial_jar_phase)),
            jar_files: DashMap::new(),
            jar_definitions: DashMap::new(),
            jar_uri_to_defs: DashMap::new(),
            jar_symbol_packages: DashMap::new(),
            extension_by_receiver: DashMap::new(),
        }
    }

    /// Test-only constructor that marks any URI whose path starts with
    /// `library_prefix` as a library source (excluded from rename/references).
    ///
    /// ```rust
    /// let indexer = Indexer::for_test_with_library("/sdk/");
    /// indexer.index_content(&uri("/sdk/Foo.kt"), "class Foo");  // Library
    /// indexer.index_content(&uri("/src/Bar.kt"), "class Bar");  // Main
    /// ```
    #[cfg(test)]
    #[allow(dead_code)] // test helper; not yet used but kept for future subtype tests
    pub(crate) fn for_test_with_library(library_prefix: &str) -> Self {
        let indexer = Self::new();
        if let Ok(mut raw) = indexer.source_paths_raw.write() {
            *raw = vec![library_prefix.to_string()];
        }
        indexer
    }

    /// Clear all index maps. Called before a full workspace re-index and on root switch.
    ///
    /// Clears workspace-source data (files, definitions, packages, subtypes, …) and caches.
    /// JAR entries and library source entries in `extension_by_receiver` are **retained** so
    /// that extension completions (e.g. `viewModelScope.launch`) remain available during the
    /// ~5 s it takes for `index_source_paths` to restore the library cache.  Workspace source
    /// entries are removed and will be re-added by `apply_workspace_result`.
    ///
    /// Does NOT touch orchestration fields (workspace_root, parse_sem, generation counters,
    /// live_lines).
    pub(crate) fn reset_index_state(&self) {
        // Workspace vs library discriminator.
        //
        // Library data must survive `reset_index_state` (per the documented design
        // intent for `jar_files` / `jar_definitions` in this struct): dot-completion,
        // hover, and go-to-definition for library symbols should remain available
        // during the few seconds it takes `index_source_paths` /
        // `index_sources_jars` to repopulate the workspace scan.
        //
        // We capture `library_uris` first, then selectively retain only library
        // entries in the per-file maps and `extension_by_receiver`.  `library_uris`
        // itself is cleared at the end so callers (e.g. `index_sources_jars`,
        // `index_source_paths`) repopulate it with the current set.
        let library_uris: std::collections::HashSet<String> =
            self.library_uris.iter().map(|u| u.clone()).collect();
        let is_library = |uri: &str| library_uris.contains(uri);

        // Per-file maps: keep only library entries.
        self.files.retain(|uri, _| is_library(uri));
        self.definitions.retain(|_name, locs| {
            locs.retain(|l| is_library(l.uri.as_str()));
            !locs.is_empty()
        });
        self.qualified
            .retain(|_fqn, loc| is_library(loc.uri.as_str()));
        self.packages.retain(|_pkg, uris| {
            uris.retain(|u| is_library(u));
            !uris.is_empty()
        });
        self.subtypes.retain(|_super, locs| {
            locs.retain(|l| is_library(l.uri.as_str()));
            !locs.is_empty()
        });
        self.content_hashes.retain(|uri, _| is_library(uri));
        self.completion_cache.clear();
        // `extension_by_receiver` uses an explicit iter + collect loop instead of
        // `DashMap::retain`.  In DashMap 5.x, `retain` with a closure that both
        // mutates the inner Vec (`entries.retain`) and reads back its length
        // (`!entries.is_empty()`) hangs on this specific map (other maps with
        // the same closure shape — definitions, subtypes — don't).
        //
        // CRITICAL: all mutations must be deferred until AFTER the iterator is
        // fully dropped.  An `iter()` holds a read guard on the current shard;
        // calling `insert`/`remove` for a key on that same shard while the guard
        // is alive requests a write lock the thread can never get (parking_lot
        // RwLocks are non-reentrant) → self-deadlock.  This fires only when a
        // receiver has mixed library+workspace entries, so it is data-dependent
        // and was previously intermittent.  Collect first, then mutate.
        let mut empty_keys: Vec<String> = Vec::new();
        let mut updated: Vec<(String, Vec<crate::types::ExtensionEntry>)> = Vec::new();
        for entry in self.extension_by_receiver.iter() {
            let mut entries = entry.value().clone();
            let before = entries.len();
            entries.retain(|e| is_library(&e.file_uri));
            if entries.is_empty() {
                empty_keys.push(entry.key().clone());
            } else if entries.len() != before {
                updated.push((entry.key().clone(), entries));
            }
        }
        for (key, entries) in updated {
            self.extension_by_receiver.insert(key, entries);
        }
        for key in empty_keys {
            self.extension_by_receiver.remove(&key);
        }
        // Keep library_uris in sync with the retained file entries — don't
        // clear entirely, as that would make is_library_uri() return false for
        // URIs whose FileData is still present, leaving the index inconsistent.
        // Callers (index_sources_jars, index_source_paths) will add any new
        // URIs when they next run.
        self.library_uris
            .retain(|uri| self.files.contains_key(uri.as_str()));
        if let Ok(mut cache) = self.bare_name_cache.write() {
            cache.clear();
        }
        if let Ok(mut map) = self.importable_fqns.write() {
            map.clear();
        }
        if let Ok(mut last) = self.last_completion.lock() {
            *last = None;
        }
        self.this_ext_ancestor_cache.clear();
        self.completion_epoch.fetch_add(1, Ordering::Release);
        self.sig_cache.clear();
        self.sig_fast_cache.clear();
        // Clear enrichment dedup so symbols are re-attempted after reindex.
        if let Ok(handle) = self.enrichment.read() {
            handle.clear();
        }
        self.bare_names_dirty.store(true, Ordering::Release);
    }

    /// Install an enrichment handle (called once during LSP backend init).
    pub(crate) fn set_enrichment_handle(&self, handle: EnrichmentHandle) {
        if let Ok(mut guard) = self.enrichment.write() {
            *guard = handle;
        }
    }

    /// Submit an unresolved symbol for background rg enrichment.
    /// No-op in CLI mode or when the handle isn't set.
    pub(crate) fn submit_enrichment(&self, symbol: &str) {
        let generation = self.workspace_root.generation();
        if let Ok(handle) = self.enrichment.read() {
            handle.submit(symbol, generation);
        }
    }

    /// Update the live-lines cache for `uri` without any debounce.
    /// Called from `did_change` before the debounced re-index so that
    /// `completions()` always sees the current document text.
    pub(crate) fn set_live_lines(&self, uri: &Url, content: &str) {
        let lines: Arc<Vec<String>> = Arc::new(content.lines().map(String::from).collect());
        self.live_lines.insert(uri.to_string(), lines);
    }

    /// Returns lines for `uri` from the in-memory caches only (no disk I/O).
    /// Prefers live (unsaved) lines; falls back to the last indexed snapshot.
    /// Use this on hot paths (completion, hover, signature help).
    /// For cold-start / rg-based paths that may need disk, use `scope::lines_for`.
    pub(crate) fn mem_lines_for(&self, uri: &str) -> Option<Arc<Vec<String>>> {
        if let Some(live) = self.live_lines.get(uri) {
            return Some(live.clone());
        }
        self.files.get(uri).map(|f| f.lines.clone())
    }

    pub(crate) fn definition_locations(&self, name: &str) -> Vec<Location> {
        self.lookup_definitions(name)
    }

    /// Returns parsed file data for `uri`, or `None` if not yet indexed.
    pub(crate) fn file_data_for(&self, uri: &str) -> Option<Arc<FileData>> {
        self.files
            .get(uri)
            .map(|r| Arc::clone(&*r))
            .or_else(|| self.jar_files.get(uri).map(|r| Arc::clone(&*r)))
    }

    /// Returns all known direct subtypes of `name` (empty if none).
    pub(crate) fn subtypes_of(&self, name: &str) -> Vec<Location> {
        self.subtypes
            .get(name)
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }

    /// Calls `f(uri, file_data)` for every indexed file.
    /// Return `false` from the callback to stop iteration early.
    pub(crate) fn for_each_indexed_file(&self, mut f: impl FnMut(&str, &Arc<FileData>) -> bool) {
        for entry in self.files.iter() {
            if !f(entry.key(), entry.value()) {
                return;
            }
        }
        for entry in self.jar_files.iter() {
            if !f(entry.key(), entry.value()) {
                return;
            }
        }
    }

    /// Clear JAR-sourced symbol maps (called on workspace root change).
    /// Also removes JAR URIs from `library_uris` so `is_library_uri` stays consistent.
    /// Resets `jar_phase` to `Pending` if the sidecar is available, so the next
    /// `spawn_jar_indexing` call will re-index the new workspace's JARs.
    pub(crate) fn clear_jar_index(&self) {
        use crate::indexer::jar_phase::JarPhase;

        // Remove stale JAR URIs from the library set before clearing the maps.
        for entry in self.jar_files.iter() {
            self.library_uris.remove(entry.key());
        }
        self.jar_files.clear();
        self.jar_definitions.clear();
        self.bare_names_dirty.store(true, Ordering::Release);
        self.jar_uri_to_defs.clear();

        // Reset phase: if sidecar is alive, mark as Pending so the next call
        // to spawn_jar_indexing will re-index for the new workspace root.
        if let Ok(mut phase) = self.jar_phase.lock() {
            let sidecar_alive = self
                .jar_sidecar
                .try_lock()
                .map(|g| g.is_some())
                .unwrap_or(true); // locked = running = alive
            *phase = if sidecar_alive {
                JarPhase::Pending
            } else {
                JarPhase::Unavailable
            };
        }
    }

    /// Look up all definition locations for `name`, merging workspace and JAR results.
    ///
    /// Prefer this over `self.definitions.get(name)` anywhere JAR symbols should be visible.
    pub(crate) fn lookup_definitions(&self, name: &str) -> Vec<tower_lsp::lsp_types::Location> {
        let mut locs: Vec<tower_lsp::lsp_types::Location> = self
            .definitions
            .get(name)
            .map(|r| r.clone())
            .unwrap_or_default();
        if let Some(jar_locs) = self.jar_definitions.get(name) {
            locs.extend(jar_locs.iter().cloned());
        }
        locs
    }

    /// Apply `f` to each *workspace* (non-library) definition location of `name`,
    /// returning the first `Some`. The single chokepoint for **receiver-less,
    /// best-effort by-name inference lookups** (return/field/callable/signature
    /// resolution that has no import or receiver scope).
    ///
    /// Ubiquitous names like `create`/`build` resolve to thousands of source-JAR
    /// definitions once those are indexed; scanning them all per lookup stalled
    /// inlay/hover for seconds. Library definitions are skipped (their metadata comes
    /// from indexed symbol detail / the JAR index) and the scan is capped — without a
    /// receiver type the exact overload can't be picked anyway.
    ///
    /// The (≤ [`MAX_BY_NAME_DEFS`]) candidate locations are snapshotted and the
    /// `definitions` read guard is dropped before `f` runs, so `f` may freely resolve
    /// other names (re-enter `definitions`) without risking a reader/writer deadlock
    /// on the shard.
    pub(crate) fn find_in_workspace_defs<T>(
        &self,
        name: &str,
        f: impl FnMut(&Location) -> Option<T>,
    ) -> Option<T> {
        let candidates: Vec<Location> = self
            .definitions
            .get(name)?
            .iter()
            .filter(|loc| !self.library_uris.contains(loc.uri.as_str()))
            .take(MAX_BY_NAME_DEFS)
            .cloned()
            .collect();
        candidates.iter().find_map(f)
    }

    pub(crate) fn is_library_uri(&self, uri: &Url) -> bool {
        self.library_uris.contains(uri.as_str())
    }

    /// Record that `extracted_uri` (a `file://` copy on disk) was extracted from
    /// `jar_source_uri` (a `jar:…!/Foo.kt` sources entry).
    pub(crate) fn record_extracted_jar_source(&self, extracted_uri: &Url, jar_source_uri: &Url) {
        self.extracted_jar_sources.insert(
            extracted_uri.as_str().to_owned(),
            jar_source_uri.to_string(),
        );
    }

    /// If `uri` is an extracted JAR-source `file://`, return its original
    /// `jar:…!/Foo.kt` sources URI (which is the one indexed with the real package
    /// and symbols). Returns `None` for any other URI.
    pub(crate) fn original_jar_source_uri(&self, uri: &Url) -> Option<Url> {
        let jar = self.extracted_jar_sources.get(uri.as_str())?;
        Url::parse(jar.value()).ok()
    }

    /// Return `(effective_root, scoped_source_paths, matcher)` for an rg search
    /// whose context file is `open_file`.
    ///
    /// `effective_root` is derived via `effective_rg_root`: when `open_file` lives
    /// outside the configured workspace root, it walks up to the nearest `.git` root
    /// so rg searches the *actual* project of that file.
    ///
    /// `scoped_source_paths` is non-empty only when `effective_root` matches the
    /// configured workspace root — when the file belongs to a different project,
    /// workspace source roots don't apply and we fall back to a full-root search.
    ///
    /// Pass `None` for `open_file` to get workspace-level scope (no file context).
    pub(crate) fn rg_scope_for_path(
        &self,
        open_file: Option<&std::path::Path>,
    ) -> (
        Option<std::path::PathBuf>,
        Vec<String>,
        Option<Arc<crate::rg::IgnoreMatcher>>,
    ) {
        let workspace_root = self.workspace_root.get();
        let source_roots = self
            .workspace_source_roots
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let matcher = self
            .ignore_matcher
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let effective_root = crate::rg::effective_rg_root(workspace_root.as_deref(), open_file);

        // source_roots belong to the configured workspace — when rg switches to
        // an external project (effective_root != workspace_root), they must not
        // leak into the search.
        let scoped_source_roots = match (&effective_root, &workspace_root) {
            (Some(effective_root), Some(workspace_root)) if effective_root == workspace_root => {
                source_roots
            }
            _ => vec![],
        };

        (effective_root, scoped_source_roots, matcher)
    }

    pub(crate) fn remove_live_lines(&self, uri: &Url) {
        self.live_lines.remove(uri.as_str());
    }

    pub(crate) fn remove_indexed_file(&self, uri: &Url) {
        self.files.remove(uri.as_str());
    }

    /// Bust the completion cache so the next request recomputes with the latest
    /// index state. Called when JAR indexing finishes to surface new symbols
    /// (e.g. `launch {}`) without requiring the user to retype.
    pub(crate) fn invalidate_completion_cache(&self) {
        if let Ok(mut last) = self.last_completion.lock() {
            *last = None;
        }
        self.this_ext_ancestor_cache.clear();
        self.completion_epoch.fetch_add(1, Ordering::Release);
    }

    // ─── completion helpers (methods on Indexer) ─────────────────────────────

    /// Ensures the file at `uri` is indexed, loading from disk if needed.
    /// Called on the completion hot-path before the debounced re-index finishes.
    pub(crate) fn ensure_indexed(&self, uri: &Url) {
        // Skip on-demand indexing during a full workspace scan to avoid
        // contending with the scan's index maps. The full scan will index
        // this file shortly; completions proceed with whatever is already
        // in the index plus live_lines.
        if self
            .indexing_in_progress
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        if !self.files.contains_key(uri.as_str()) {
            if let Ok(path) = uri.to_file_path() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    self.index_content(uri, &content);
                }
            }
        }
    }

    /// Uses `live_lines` (updated synchronously on every keystroke) for the
    /// current file's line text, falling back to indexed lines or disk.
    pub(crate) fn completions(
        &self,
        uri: &Url,
        position: Position,
        snippets: bool,
    ) -> (Vec<CompletionItem>, bool) {
        crate::features::completion::run_completions(self, uri, position, snippets)
    }
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "indexer_tests.rs"]
mod tests;
