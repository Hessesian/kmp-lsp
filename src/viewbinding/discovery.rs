//! AGP-generated ViewBinding class discovery and layout pairing helpers.
//!
//! Scans `<module>/build/` for `*Binding.java` files whose `package` ends in
//! `.databinding`, indexes them through the normal Java pipeline, and maintains
//! a per-module side index for query-time layout↔class pairing (PR 4+).

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashSet;
use tokio::sync::mpsc;
use tower_lsp::lsp_types::{Range, Url};
use walkdir::WalkDir;

use super::layout::LayoutFileData;
use crate::types::ImportEntry;

// ─── Data types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct GeneratedBindingEntry {
    pub class_name: String,
    pub file_uri: String,
    /// File mtime (seconds since Unix epoch) when the entry was discovered.
    pub modified_at_secs: u64,
    /// Sub-second mtime component for finer change detection.
    #[serde(default)]
    pub modified_at_nanos: u32,
    /// File size in bytes when the entry was discovered.
    #[serde(default)]
    pub file_size: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ModuleBindings {
    pub entries: HashMap<String, GeneratedBindingEntry>,
}

/// Reverse index entry: where a generated `*Binding` class lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedBindingClassLocation {
    pub module_root: PathBuf,
    pub file_uri: String,
    pub package: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ModuleBindingsCacheEntry {
    pub entries: HashMap<String, GeneratedBindingEntry>,
}

// ─── Pure path helpers ────────────────────────────────────────────────────────

/// Derive the Android module root from a generated binding path:
/// `<X>/build/.../FooBarBinding.java` → `<X>`.
pub(crate) fn module_root_for_generated_file(path: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = path.components().collect();
    let build_index = components.iter().rposition(
        |component| matches!(component, Component::Normal(name) if name.to_str() == Some("build")),
    )?;
    if build_index == 0 {
        return None;
    }
    Some(components[..build_index].iter().collect())
}

/// Derive the Android module root from a Kotlin/Java source path:
/// `<X>/src/<sourceset>/...` → `<X>`.
pub(crate) fn module_root_for_source_file(path: &Path) -> Option<PathBuf> {
    let components: Vec<Component<'_>> = path.components().collect();
    let source_index = components.iter().rposition(
        |component| matches!(component, Component::Normal(name) if name.to_str() == Some("src")),
    )?;
    if source_index == 0 {
        return None;
    }
    Some(components[..source_index].iter().collect())
}

/// True when `path` matches the best-effort watcher pattern for generated bindings:
/// under a `build/` segment, filename `*Binding.java`, and a `databinding` path segment.
pub(crate) fn is_generated_binding_watcher_path(path: &Path) -> bool {
    if !is_binding_java_filename(path) {
        return false;
    }
    let mut has_build = false;
    let mut has_databinding = false;
    for component in path.components() {
        if let Component::Normal(name) = component {
            match name.to_str() {
                Some("build") => has_build = true,
                Some("databinding") => has_databinding = true,
                _ => {}
            }
        }
    }
    has_build && has_databinding
}

fn is_binding_java_filename(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    file_name.ends_with("Binding.java")
}

// ─── Name mapping (AGP ViewBinding conventions) ───────────────────────────────

/// Layout file name → generated binding class name (`foo_bar` → `FooBarBinding`).
pub(crate) fn binding_class_name_for_layout(layout_name: &str) -> String {
    format!("{}Binding", snake_case_to_pascal_case(layout_name))
}

/// Generated binding class name → layout file name (`FooBarBinding` → `foo_bar`).
pub(crate) fn layout_name_for_binding_class(class_name: &str) -> Option<String> {
    let base = class_name.strip_suffix("Binding")?;
    if base.is_empty() {
        return None;
    }
    Some(pascal_case_to_snake_case(base))
}

/// True when `class_name` follows AGP ViewBinding naming (`FooBarBinding` → `foo_bar`).
pub(crate) fn is_view_binding_class_name(class_name: &str) -> bool {
    layout_name_for_binding_class(class_name).is_some()
}

fn snake_case_to_pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut characters = segment.chars();
            match characters.next() {
                None => String::new(),
                Some(first) => first
                    .to_uppercase()
                    .chain(characters.flat_map(char::to_lowercase))
                    .collect(),
            }
        })
        .collect()
}

fn pascal_case_to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_uppercase() && index > 0 {
            result.push('_');
        }
        result.extend(character.to_lowercase());
    }
    result
}

/// ViewBinding field name → layout view id (`fooBar` → `foo_bar`).
pub(crate) fn binding_field_name_to_id(field_name: &str) -> String {
    pascal_case_to_snake_case(field_name)
}

/// Layout view id → ViewBinding field name (`foo_bar` → `fooBar`).
pub(crate) fn binding_id_to_field_name(id: &str) -> String {
    snake_case_to_camel_case(id)
}

/// True when a layout `@+id/…` value exactly matches a field lookup id.
pub(crate) fn view_id_exact_match(view_id: &str, lookup_id: &str) -> bool {
    view_id == lookup_id
}

/// True when a layout `@+id/…` value matches a field lookup id after camelCase normalization.
pub(crate) fn view_id_normalized_match(view_id: &str, lookup_id: &str) -> bool {
    binding_id_to_field_name(view_id) == binding_id_to_field_name(lookup_id)
}

/// Exact id match first; normalized camelCase/snake_case only when it yields a single candidate.
pub(crate) fn view_id_matches_lookup(view_id: &str, lookup_id: &str) -> bool {
    view_id_exact_match(view_id, lookup_id) || view_id_normalized_match(view_id, lookup_id)
}

fn snake_case_to_camel_case(name: &str) -> String {
    let mut segments = name.split('_').filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return String::new();
    };
    let mut result = first.to_string();
    for segment in segments {
        let mut characters = segment.chars();
        if let Some(first_char) = characters.next() {
            result.extend(first_char.to_uppercase());
            result.extend(characters.flat_map(char::to_lowercase));
        }
    }
    result
}

// ─── Package verification ─────────────────────────────────────────────────────

fn package_from_java_source(content: &str) -> Option<String> {
    for line in content.lines().take(30) {
        let trimmed = line.trim();
        if let Some(package_name) = trimmed.strip_prefix("package ") {
            let package_name = package_name.trim_end_matches(';').trim();
            if !package_name.is_empty() {
                return Some(package_name.to_string());
            }
        }
    }
    None
}

fn is_databinding_package(package_name: &str) -> bool {
    package_name.ends_with(".databinding")
}

fn read_binding_java_metadata(path: &Path) -> Option<(String, String, u64, u32, u64)> {
    let content = std::fs::read_to_string(path).ok()?;
    let package_name = package_from_java_source(&content)?;
    if !is_databinding_package(&package_name) {
        return None;
    }
    let class_name = path.file_stem()?.to_str()?.to_string();
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok();
    let modified_at_secs = modified
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let modified_at_nanos = modified
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    let file_size = metadata.len();
    let file_uri = Url::from_file_path(path).ok()?.to_string();
    Some((
        class_name,
        file_uri,
        modified_at_secs,
        modified_at_nanos,
        file_size,
    ))
}

fn binding_entry_is_newer_or_equal(
    existing: &GeneratedBindingEntry,
    candidate: &GeneratedBindingEntry,
) -> bool {
    if existing.modified_at_secs != candidate.modified_at_secs {
        return existing.modified_at_secs > candidate.modified_at_secs;
    }
    if existing.modified_at_nanos != candidate.modified_at_nanos {
        return existing.modified_at_nanos > candidate.modified_at_nanos;
    }
    existing.file_size >= candidate.file_size
}

// ─── Discovery ────────────────────────────────────────────────────────────────

/// Discover `databinding` directories under `<module_root>/build/`.
pub(crate) fn discover_databinding_dirs(module_root: &Path) -> Vec<PathBuf> {
    let build_dir = module_root.join("build");
    if !build_dir.is_dir() {
        return Vec::new();
    }

    let mut dirs = Vec::new();
    for entry in WalkDir::new(&build_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|walk_entry| {
            if !walk_entry.file_type().is_dir() {
                return true;
            }
            let Some(name) = walk_entry.file_name().to_str() else {
                return true;
            };
            !matches!(name, "tmp" | "kotlin")
        })
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_dir() {
            continue;
        }
        if entry.file_name() == "databinding" {
            dirs.push(entry.path().to_path_buf());
        }
    }
    dirs
}

fn discover_generated_bindings_in_dirs(databinding_dirs: &[PathBuf]) -> Vec<GeneratedBindingEntry> {
    let mut by_class_name: HashMap<String, GeneratedBindingEntry> = HashMap::new();
    for databinding_dir in databinding_dirs {
        for entry in WalkDir::new(databinding_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if !path.is_file() || !is_binding_java_filename(path) {
                continue;
            }
            let Some((class_name, file_uri, modified_at_secs, modified_at_nanos, file_size)) =
                read_binding_java_metadata(path)
            else {
                continue;
            };
            let candidate = GeneratedBindingEntry {
                class_name: class_name.clone(),
                file_uri,
                modified_at_secs,
                modified_at_nanos,
                file_size,
            };
            match by_class_name.get(&class_name) {
                Some(existing) if binding_entry_is_newer_or_equal(existing, &candidate) => {}
                _ => {
                    by_class_name.insert(class_name, candidate);
                }
            }
        }
    }
    by_class_name.into_values().collect()
}

fn discover_generated_bindings_in_build_tree(module_root: &Path) -> Vec<GeneratedBindingEntry> {
    let build_dir = module_root.join("build");
    if !build_dir.is_dir() {
        return Vec::new();
    }

    let mut by_class_name: HashMap<String, GeneratedBindingEntry> = HashMap::new();
    for entry in WalkDir::new(&build_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() || !is_binding_java_filename(path) {
            continue;
        }
        let Some((class_name, file_uri, modified_at_secs, modified_at_nanos, file_size)) =
            read_binding_java_metadata(path)
        else {
            continue;
        };
        let candidate = GeneratedBindingEntry {
            class_name: class_name.clone(),
            file_uri,
            modified_at_secs,
            modified_at_nanos,
            file_size,
        };
        match by_class_name.get(&class_name) {
            Some(existing) if binding_entry_is_newer_or_equal(existing, &candidate) => {}
            _ => {
                by_class_name.insert(class_name, candidate);
            }
        }
    }

    by_class_name.into_values().collect()
}

/// Walk databinding dirs (or all of `build/` when none are known) for generated
/// `*Binding.java` files.
///
/// Keeps the newest mtime per class name across build variants. Ignores files
/// with the wrong package or that cannot be read.
pub(crate) fn discover_generated_bindings(
    module_root: &Path,
    databinding_dirs: Option<&[PathBuf]>,
) -> Vec<GeneratedBindingEntry> {
    if let Some(dirs) = databinding_dirs.filter(|dirs| !dirs.is_empty()) {
        return discover_generated_bindings_in_dirs(dirs);
    }

    let discovered_dirs = discover_databinding_dirs(module_root);
    if !discovered_dirs.is_empty() {
        return discover_generated_bindings_in_dirs(&discovered_dirs);
    }

    discover_generated_bindings_in_build_tree(module_root)
}

// ─── Import trigger ───────────────────────────────────────────────────────────

/// True when an import path matches `*.databinding.*Binding` (non-star).
pub(crate) fn import_triggers_binding_discovery(import_path: &str) -> bool {
    if !import_path.ends_with("Binding") {
        return false;
    }
    let segments: Vec<&str> = import_path.split('.').collect();
    if segments.len() < 2 {
        return false;
    }
    segments.get(segments.len() - 2) == Some(&"databinding")
}

pub(crate) fn file_imports_trigger_binding_discovery(imports: &[ImportEntry]) -> bool {
    imports
        .iter()
        .any(|entry| !entry.is_star && import_triggers_binding_discovery(&entry.full_path))
}

// ─── Databinding poll watcher handle (PR 3) ─────────────────────────────────

/// Shared registration state for the server-side databinding poll watcher.
pub(crate) struct DatabindingWatcherState {
    pub(crate) watched_module_roots: Mutex<HashSet<PathBuf>>,
    pub(crate) cancelled: AtomicBool,
}

impl DatabindingWatcherState {
    pub(crate) fn new() -> Self {
        Self {
            watched_module_roots: Mutex::new(HashSet::new()),
            cancelled: AtomicBool::new(false),
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn registered_module_roots(&self) -> Vec<PathBuf> {
        self.watched_module_roots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

/// Cheap handle for registering module roots with the databinding poll watcher.
#[derive(Clone)]
pub(crate) struct DatabindingWatcherHandle {
    state: Option<Arc<DatabindingWatcherState>>,
}

impl DatabindingWatcherHandle {
    pub(crate) fn noop() -> Self {
        Self { state: None }
    }

    pub(crate) fn new(state: Arc<DatabindingWatcherState>) -> Self {
        Self { state: Some(state) }
    }

    /// Stop the poll loop (called on LSP shutdown).
    pub(crate) fn cancel(&self) {
        if let Some(state) = &self.state {
            state.cancel();
        }
    }

    /// Register `module_root` for polling. Idempotent.
    pub(crate) fn watch_module(&self, module_root: &Path) {
        let Some(state) = &self.state else {
            return;
        };
        state
            .watched_module_roots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(module_root.to_path_buf());
    }
}

// ─── Background worker ────────────────────────────────────────────────────────

struct BindingDiscoveryRequest {
    module_root: PathBuf,
    databinding_dirs: Option<Vec<PathBuf>>,
}

/// Cheap handle for enqueueing per-module generated binding discovery.
#[derive(Clone)]
pub(crate) struct BindingDiscoveryHandle {
    sender: Option<mpsc::UnboundedSender<BindingDiscoveryRequest>>,
    in_progress: Arc<DashSet<PathBuf>>,
    rerun_requested: Arc<DashSet<PathBuf>>,
}

impl BindingDiscoveryHandle {
    pub(crate) fn noop() -> Self {
        Self {
            sender: None,
            in_progress: Arc::new(DashSet::new()),
            rerun_requested: Arc::new(DashSet::new()),
        }
    }

    pub(crate) fn is_noop(&self) -> bool {
        self.sender.is_none()
    }

    /// Enqueue discovery for `module_root`. Duplicate in-flight requests set a rerun flag.
    #[allow(dead_code)]
    pub(crate) fn request(&self, module_root: PathBuf) {
        self.request_with_dirs(module_root, None);
    }

    /// Enqueue discovery with pre-resolved databinding dirs (watcher hot path).
    pub(crate) fn request_with_dirs(
        &self,
        module_root: PathBuf,
        databinding_dirs: Option<Vec<PathBuf>>,
    ) {
        let Some(ref sender) = self.sender else {
            return;
        };
        if self.in_progress.contains(&module_root) {
            self.rerun_requested.insert(module_root);
            return;
        }
        if !self.in_progress.insert(module_root.clone()) {
            return;
        }
        let _ = sender.send(BindingDiscoveryRequest {
            module_root,
            databinding_dirs,
        });
    }

    pub(crate) fn clear(&self) {
        self.in_progress.clear();
        self.rerun_requested.clear();
    }
}

/// Spawn the background binding-discovery worker. Returns a handle for hot-path callers.
pub(crate) fn spawn_binding_discovery_worker(
    indexer: Arc<crate::indexer::Indexer>,
) -> BindingDiscoveryHandle {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let in_progress = Arc::new(DashSet::new());
    let rerun_requested = Arc::new(DashSet::new());
    let handle = BindingDiscoveryHandle {
        sender: Some(sender.clone()),
        in_progress: Arc::clone(&in_progress),
        rerun_requested: Arc::clone(&rerun_requested),
    };
    tokio::spawn(async move {
        while let Some(request) = receiver.recv().await {
            let module_root = request.module_root;
            let databinding_dirs = request.databinding_dirs;
            let module_for_blocking = module_root.clone();
            let indexer = Arc::clone(&indexer);
            let in_progress = Arc::clone(&in_progress);
            let rerun_requested = Arc::clone(&rerun_requested);
            let discovery_succeeded = tokio::task::spawn_blocking(move || {
                indexer.index_generated_bindings(&module_for_blocking, databinding_dirs.as_deref());
            })
            .await
            .is_ok();
            in_progress.remove(&module_root);
            if discovery_succeeded
                && rerun_requested.remove(&module_root).is_some()
                && in_progress.insert(module_root.clone())
            {
                let _ = sender.send(BindingDiscoveryRequest {
                    module_root,
                    databinding_dirs: None,
                });
            }
        }
    });
    handle
}

// ─── Indexer integration ─────────────────────────────────────────────────────

impl crate::indexer::Indexer {
    pub(crate) fn set_binding_discovery_handle(&self, handle: BindingDiscoveryHandle) {
        if let Ok(mut guard) = self.viewbinding.binding_discovery.write() {
            *guard = handle;
        }
    }

    pub(crate) fn request_generated_binding_discovery(&self, module_root: PathBuf) {
        self.request_generated_binding_discovery_with_dirs(module_root, None);
    }

    pub(crate) fn request_generated_binding_discovery_with_dirs(
        &self,
        module_root: PathBuf,
        databinding_dirs: Option<Vec<PathBuf>>,
    ) {
        if let Ok(handle) = self.viewbinding.binding_discovery.read() {
            if handle.is_noop() {
                self.index_generated_bindings(&module_root, databinding_dirs.as_deref());
                return;
            }
            handle.request_with_dirs(module_root, databinding_dirs);
        }
    }

    /// Discover generated bindings for `module_root`, update the side index, and
    /// feed each file through the normal Java indexer.
    ///
    /// Idempotent and additive — safe to call repeatedly.
    ///
    pub(crate) fn set_databinding_watcher_handle(&self, handle: DatabindingWatcherHandle) {
        if let Ok(mut guard) = self.viewbinding.databinding_watcher.write() {
            *guard = handle;
        }
        // Re-register modules discovered before the real watcher was installed.
        if let Ok(watcher) = self.viewbinding.databinding_watcher.read() {
            for module in self.viewbinding.generated_bindings.iter() {
                watcher.watch_module(module.key());
            }
        }
    }

    pub(crate) fn index_generated_bindings(
        &self,
        module_root: &Path,
        databinding_dirs: Option<&[PathBuf]>,
    ) {
        if let Ok(handle) = self.viewbinding.databinding_watcher.read() {
            handle.watch_module(module_root);
        }

        let previous_bindings = self
            .viewbinding
            .generated_bindings
            .get(module_root)
            .map(|module| Arc::clone(module.value()));

        if let Some(previous_bindings) = &previous_bindings {
            for previous_entry in previous_bindings.entries.values() {
                self.viewbinding
                    .generated_binding_uris
                    .remove(&previous_entry.file_uri);
            }
            self.remove_generated_binding_class_entries_for_module(module_root);
        }

        let discovered = discover_generated_bindings(module_root, databinding_dirs);
        let entries: HashMap<String, GeneratedBindingEntry> = discovered
            .into_iter()
            .map(|entry| (entry.class_name.clone(), entry))
            .collect();
        self.viewbinding.generated_bindings.insert(
            module_root.to_path_buf(),
            Arc::new(ModuleBindings {
                entries: entries.clone(),
            }),
        );
        for entry in entries.values() {
            self.viewbinding
                .generated_binding_uris
                .insert(entry.file_uri.clone());
        }

        if let Some(previous_bindings) = previous_bindings {
            self.remove_undiscovered_binding_files_from_index(&previous_bindings, &entries);
        }

        for entry in entries.values() {
            let Ok(uri) = Url::parse(&entry.file_uri) else {
                continue;
            };
            if let Ok(path) = uri.to_file_path() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    self.index_content(&uri, &content);
                }
            }
        }

        for entry in entries.values() {
            self.insert_generated_binding_class_index(entry, module_root);
        }
    }

    /// Drop index entries for binding files that a re-discovery no longer sees
    /// (deleted by a clean build, or superseded by a newer build variant).
    /// Without this, stale `qualified`/`definitions` entries keep resolving to
    /// old generated paths.
    fn remove_undiscovered_binding_files_from_index(
        &self,
        previous_bindings: &ModuleBindings,
        current_entries: &HashMap<String, GeneratedBindingEntry>,
    ) {
        for previous_entry in previous_bindings.entries.values() {
            let still_discovered = current_entries
                .values()
                .any(|entry| entry.file_uri == previous_entry.file_uri);
            if still_discovered {
                continue;
            }
            self.viewbinding
                .generated_binding_uris
                .remove(&previous_entry.file_uri);
            self.remove_stale_for_uri(&previous_entry.file_uri);
            self.files.remove(&previous_entry.file_uri);
        }
    }

    pub(crate) fn maybe_enqueue_binding_discovery_for_file(
        &self,
        uri: &Url,
        imports: &[ImportEntry],
    ) {
        if !file_imports_trigger_binding_discovery(imports) {
            return;
        }
        let Ok(path) = uri.to_file_path() else {
            return;
        };
        let Some(module_root) = module_root_for_source_file(&path) else {
            return;
        };
        self.request_generated_binding_discovery(module_root);
    }

    /// Layout variants for a binding class in the given module, default variant first.
    pub(crate) fn layouts_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<Arc<LayoutFileData>> {
        let Some(layout_name) = layout_name_for_binding_class(class_name) else {
            return Vec::new();
        };
        self.matching_layout_entries(module_root, &layout_name)
            .into_iter()
            .map(|(_uri, data)| data)
            .collect()
    }

    /// Direct read from the layout side index.
    pub(crate) fn layout_data_for_uri(&self, uri: &str) -> Option<Arc<LayoutFileData>> {
        self.viewbinding
            .layouts
            .get(uri)
            .map(|entry| Arc::clone(entry.value()))
    }

    fn matching_layout_entries(
        &self,
        module_root: &Path,
        layout_name: &str,
    ) -> Vec<(String, Arc<LayoutFileData>)> {
        let key = (module_root.to_path_buf(), layout_name.to_string());
        if let Some(uris) = self.viewbinding.layouts_by_module_and_name.get(&key) {
            let mut entries: Vec<(String, Arc<LayoutFileData>)> = uris
                .iter()
                .filter_map(|uri| {
                    self.viewbinding
                        .layouts
                        .get(uri)
                        .map(|entry| (uri.clone(), Arc::clone(entry.value())))
                })
                .collect();
            entries.sort_by(|left, right| {
                match (
                    left.1.variant_qualifier.is_empty(),
                    right.1.variant_qualifier.is_empty(),
                ) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => left.1.variant_qualifier.cmp(&right.1.variant_qualifier),
                }
            });
            return entries;
        }

        let mut entries: Vec<(String, Arc<LayoutFileData>)> = self
            .viewbinding
            .layouts
            .iter()
            .filter_map(|entry| {
                let data = entry.value();
                if data.module_root.as_path() == module_root && data.layout_name == layout_name {
                    Some((entry.key().clone(), Arc::clone(data)))
                } else {
                    None
                }
            })
            .collect();
        entries.sort_by(|left, right| {
            match (
                left.1.variant_qualifier.is_empty(),
                right.1.variant_qualifier.is_empty(),
            ) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => left.1.variant_qualifier.cmp(&right.1.variant_qualifier),
            }
        });
        entries
    }

    /// Layout variants for a binding class, with side-index URIs (default first).
    pub(crate) fn layout_uris_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<(String, Arc<LayoutFileData>)> {
        let Some(layout_name) = layout_name_for_binding_class(class_name) else {
            return Vec::new();
        };
        self.matching_layout_entries(module_root, &layout_name)
    }

    /// Every variant declaring `@+id/{id}` for the given layout name in `module_root`.
    pub(crate) fn layouts_declaring_view_id(
        &self,
        module_root: &Path,
        layout_name: &str,
        id: &str,
    ) -> Vec<(String, Range)> {
        let entries = self.matching_layout_entries(module_root, layout_name);
        let exact_matches: Vec<(String, Range)> = entries
            .into_iter()
            .filter_map(|(uri, data)| {
                data.view_ids
                    .iter()
                    .find(|view_id| view_id_exact_match(&view_id.id, id))
                    .map(|view_id| (uri, view_id.id_attribute_range))
            })
            .collect();
        if !exact_matches.is_empty() {
            return exact_matches;
        }

        let normalized_matches: Vec<(String, Range)> = self
            .matching_layout_entries(module_root, layout_name)
            .into_iter()
            .filter_map(|(uri, data)| {
                data.view_ids
                    .iter()
                    .find(|view_id| {
                        !view_id_exact_match(&view_id.id, id)
                            && view_id_normalized_match(&view_id.id, id)
                    })
                    .map(|view_id| (uri, view_id.id_attribute_range))
            })
            .collect();
        if normalized_matches.len() == 1 {
            return normalized_matches;
        }
        Vec::new()
    }

    /// `<include>` tag ranges whose `android:id` maps to `field_name`.
    pub(crate) fn include_tag_for_field(
        &self,
        module_root: &Path,
        layout_name: &str,
        field_name: &str,
    ) -> Vec<(String, Range)> {
        let lookup_id = binding_field_name_to_id(field_name);
        let entries = self.matching_layout_entries(module_root, layout_name);
        let exact_matches: Vec<(String, Range)> = entries
            .iter()
            .filter_map(|(uri, data)| {
                data.includes
                    .iter()
                    .find(|include| {
                        include
                            .id
                            .as_deref()
                            .is_some_and(|include_id| view_id_exact_match(include_id, &lookup_id))
                    })
                    .map(|include| (uri.clone(), include.tag_range))
            })
            .collect();
        if !exact_matches.is_empty() {
            return exact_matches;
        }

        let normalized_matches: Vec<(String, Range)> = entries
            .into_iter()
            .filter_map(|(uri, data)| {
                data.includes
                    .iter()
                    .find(|include| {
                        include.id.as_deref().is_some_and(|include_id| {
                            !view_id_exact_match(include_id, &lookup_id)
                                && view_id_normalized_match(include_id, &lookup_id)
                        })
                    })
                    .map(|include| (uri, include.tag_range))
            })
            .collect();
        if normalized_matches.len() == 1 {
            return normalized_matches;
        }
        Vec::new()
    }

    /// True when a generated binding class has been discovered for `class_name` in `module_root`.
    pub(crate) fn generated_binding_discovered(
        &self,
        module_root: &Path,
        class_name: &str,
    ) -> bool {
        self.viewbinding
            .generated_bindings
            .get(module_root)
            .is_some_and(|module| module.entries.contains_key(class_name))
    }

    /// True when at least one layout variant exists for `layout_name` in `module_root`.
    pub(crate) fn layout_exists_for_binding(&self, module_root: &Path, layout_name: &str) -> bool {
        !self
            .matching_layout_entries(module_root, layout_name)
            .is_empty()
    }

    /// True when every layout variant for `layout_name` opts out via `tools:viewBindingIgnore`.
    pub(crate) fn all_layout_variants_ignore_view_binding(
        &self,
        module_root: &Path,
        layout_name: &str,
    ) -> bool {
        let entries = self.matching_layout_entries(module_root, layout_name);
        !entries.is_empty() && entries.iter().all(|(_uri, data)| data.view_binding_ignore)
    }

    /// All discovered locations for a generated binding class name.
    pub(crate) fn generated_binding_locations_for_class(
        &self,
        class_name: &str,
    ) -> Vec<GeneratedBindingClassLocation> {
        self.viewbinding
            .generated_binding_by_class
            .get(class_name)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Fully-qualified name for a generated binding class as seen from `source_uri`.
    pub(crate) fn binding_class_fqn_for_source(
        &self,
        class_name: &str,
        source_uri: &Url,
    ) -> Option<String> {
        if let Some(binding_file_uri) =
            self.generated_binding_file_uri_for_source(source_uri, class_name)
        {
            let file_data = self.file_data_for(&binding_file_uri)?;
            let package = file_data.package.as_deref()?;
            return Some(format!("{package}.{class_name}"));
        }
        let file_data = self.file_data_for(source_uri.as_str())?;
        let class_suffix = format!(".{class_name}");
        let import = file_data.imports.iter().find(|import| {
            !import.is_star
                && import.full_path.ends_with(&class_suffix)
                && (import.local_name == class_name
                    || import
                        .full_path
                        .rsplit_once('.')
                        .is_some_and(|(_, simple)| simple == class_name))
        })?;
        Some(import.full_path.clone())
    }

    /// Workspace source files that import the binding class, for narrowing rg search.
    pub(crate) fn workspace_files_importing_binding_class(
        &self,
        class_name: &str,
        source_uri: &Url,
    ) -> Vec<String> {
        let Some(fqn) = self.binding_class_fqn_for_source(class_name, source_uri) else {
            return Vec::new();
        };
        self.workspace_importers_of(&fqn)
            .into_iter()
            .filter_map(|url| url.to_file_path().ok())
            .filter_map(|path| path.to_str().map(|path_string| path_string.to_owned()))
            .collect()
    }

    /// Resolve the generated binding file for `class_name` as seen from `source_uri`.
    pub(crate) fn generated_binding_file_uri_for_source(
        &self,
        source_uri: &Url,
        class_name: &str,
    ) -> Option<String> {
        if let Some(imported) = self.generated_binding_file_uri_from_import(source_uri, class_name)
        {
            return Some(imported);
        }
        if let Some(own_module) =
            self.generated_binding_file_uri_in_own_module(source_uri, class_name)
        {
            return Some(own_module);
        }
        let locations = self.generated_binding_locations_for_class(class_name);
        if locations.len() == 1 {
            Some(locations[0].file_uri.clone())
        } else {
            None
        }
    }

    fn generated_binding_file_uri_from_import(
        &self,
        source_uri: &Url,
        class_name: &str,
    ) -> Option<String> {
        let file_data = self.file_data_for(source_uri.as_str())?;
        let class_suffix = format!(".{class_name}");
        let import = file_data.imports.iter().find(|import| {
            !import.is_star
                && import.full_path.ends_with(&class_suffix)
                && (import.local_name == class_name
                    || import
                        .full_path
                        .rsplit_once('.')
                        .is_some_and(|(_, simple)| simple == class_name))
        })?;
        let (import_package, _class) = import.full_path.rsplit_once('.')?;
        for location in self.generated_binding_locations_for_class(class_name) {
            if location.package.as_deref() == Some(import_package) {
                return Some(location.file_uri);
            }
        }
        None
    }

    fn generated_binding_file_uri_in_own_module(
        &self,
        source_uri: &Url,
        class_name: &str,
    ) -> Option<String> {
        let path = source_uri.to_file_path().ok()?;
        let module_root = module_root_for_source_file(&path)?;
        let module = self.viewbinding.generated_bindings.get(&module_root)?;
        let entry = module.entries.get(class_name)?;
        Some(entry.file_uri.clone())
    }

    fn insert_generated_binding_class_index(
        &self,
        entry: &GeneratedBindingEntry,
        module_root: &Path,
    ) {
        let package = self
            .file_data_for(&entry.file_uri)
            .and_then(|file_data| file_data.package.clone());
        let location = GeneratedBindingClassLocation {
            module_root: module_root.to_path_buf(),
            file_uri: entry.file_uri.clone(),
            package,
        };
        self.viewbinding
            .generated_binding_by_class
            .entry(entry.class_name.clone())
            .or_default()
            .push(location);
    }

    fn remove_generated_binding_class_entries_for_module(&self, module_root: &Path) {
        let class_names: Vec<String> = self
            .viewbinding
            .generated_binding_by_class
            .iter()
            .filter_map(|entry| {
                let references_module = entry
                    .value()
                    .iter()
                    .any(|location| location.module_root == *module_root);
                references_module.then(|| entry.key().clone())
            })
            .collect();
        for class_name in class_names {
            if let Some(mut locations) = self
                .viewbinding
                .generated_binding_by_class
                .get_mut(&class_name)
            {
                locations.retain(|location| location.module_root != *module_root);
                if locations.is_empty() {
                    drop(locations);
                    self.viewbinding
                        .generated_binding_by_class
                        .remove(&class_name);
                }
            }
        }
    }

    /// True when `uri` is a discovered generated binding file (side-index membership).
    pub(crate) fn is_generated_binding_uri(&self, uri: &str) -> bool {
        self.viewbinding.generated_binding_uris.contains(uri)
    }

    pub(crate) fn restore_generated_bindings_from_cache(
        &self,
        cached: &HashMap<String, ModuleBindingsCacheEntry>,
    ) {
        for (module_root_string, cache_entry) in cached {
            let module_root = PathBuf::from(module_root_string);
            let mut fresh_entries = HashMap::new();
            let mut needs_rediscovery = false;

            for (class_name, entry) in &cache_entry.entries {
                let Ok(uri) = Url::parse(&entry.file_uri) else {
                    needs_rediscovery = true;
                    continue;
                };
                let Ok(path) = uri.to_file_path() else {
                    needs_rediscovery = true;
                    continue;
                };
                if !path.exists() {
                    needs_rediscovery = true;
                    continue;
                }
                let current_mtime = std::fs::metadata(&path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0);
                let current_nanos = std::fs::metadata(&path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.subsec_nanos())
                    .unwrap_or(0);
                let current_size = std::fs::metadata(&path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
                if current_mtime != entry.modified_at_secs
                    || current_nanos != entry.modified_at_nanos
                    || current_size != entry.file_size
                {
                    needs_rediscovery = true;
                }
                fresh_entries.insert(class_name.clone(), entry.clone());
                if let Ok(content) = std::fs::read_to_string(&path) {
                    self.index_content(&uri, &content);
                }
            }

            let entries_empty = fresh_entries.is_empty();
            if !entries_empty {
                for entry in fresh_entries.values() {
                    self.viewbinding
                        .generated_binding_uris
                        .insert(entry.file_uri.clone());
                    self.insert_generated_binding_class_index(entry, &module_root);
                }
                self.viewbinding.generated_bindings.insert(
                    module_root.clone(),
                    Arc::new(ModuleBindings {
                        entries: fresh_entries,
                    }),
                );
                if let Ok(watcher) = self.viewbinding.databinding_watcher.read() {
                    watcher.watch_module(&module_root);
                }
            }

            if needs_rediscovery || entries_empty {
                self.request_generated_binding_discovery(module_root);
            }
        }
    }
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
