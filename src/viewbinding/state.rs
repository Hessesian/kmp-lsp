//! ViewBinding side-index state owned by [`crate::indexer::Indexer`].

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use dashmap::{DashMap, DashSet};

use super::discovery::{
    BindingDiscoveryHandle, DatabindingWatcherHandle, GeneratedBindingClassLocation, ModuleBindings,
};
use super::layout::{LayoutFileData, LayoutIndexingHandle};

/// Layout and generated-binding side indexes plus background worker handles.
pub(crate) struct ViewBindingState {
    /// URI string → parsed Android layout XML metadata.
    pub(crate) layouts: DashMap<String, Arc<LayoutFileData>>,
    /// Module root → discovered generated ViewBinding Java files.
    pub(crate) generated_bindings: DashMap<PathBuf, Arc<ModuleBindings>>,
    /// O(1) membership test for generated binding file URIs.
    pub(crate) generated_binding_uris: DashSet<String>,
    /// `class_name` → module locations for O(1) import/hover pairing.
    pub(crate) generated_binding_by_class: DashMap<String, Vec<GeneratedBindingClassLocation>>,
    /// Secondary index: (module_root, layout_name) → layout file URIs (default variant first).
    pub(crate) layouts_by_module_and_name: DashMap<(PathBuf, String), Vec<String>>,
    /// Modules whose layout XML has been enumerated by `ensure_module_layouts_indexed`.
    pub(crate) layouts_indexed_modules: DashSet<PathBuf>,
    /// Handle for enqueueing background generated-binding discovery.
    pub(crate) binding_discovery: RwLock<BindingDiscoveryHandle>,
    /// Handle for registering module roots with the server-side databinding poll watcher.
    pub(crate) databinding_watcher: RwLock<DatabindingWatcherHandle>,
    /// Handle for enqueueing background layout XML indexing.
    pub(crate) layout_indexing: RwLock<LayoutIndexingHandle>,
}

impl ViewBindingState {
    pub(crate) fn new() -> Self {
        Self {
            layouts: DashMap::new(),
            generated_bindings: DashMap::new(),
            generated_binding_uris: DashSet::new(),
            generated_binding_by_class: DashMap::new(),
            layouts_by_module_and_name: DashMap::new(),
            layouts_indexed_modules: DashSet::new(),
            binding_discovery: RwLock::new(BindingDiscoveryHandle::noop()),
            databinding_watcher: RwLock::new(DatabindingWatcherHandle::noop()),
            layout_indexing: RwLock::new(LayoutIndexingHandle::noop()),
        }
    }

    /// Clear all ViewBinding side-index maps and worker dedup state.
    pub(crate) fn reset(&self) {
        self.layouts.clear();
        self.generated_bindings.clear();
        self.generated_binding_uris.clear();
        self.generated_binding_by_class.clear();
        self.layouts_by_module_and_name.clear();
        self.layouts_indexed_modules.clear();
        if let Ok(handle) = self.binding_discovery.read() {
            handle.clear();
        }
        if let Ok(handle) = self.layout_indexing.read() {
            handle.clear();
        }
    }

    pub(crate) fn insert_layout_secondary_index(&self, uri: &str, data: &LayoutFileData) {
        let key = (data.module_root.clone(), data.layout_name.clone());
        let mut entry = self.layouts_by_module_and_name.entry(key).or_default();
        let uri_string = uri.to_string();
        if !entry.contains(&uri_string) {
            entry.push(uri_string);
        }
    }

    pub(crate) fn remove_layout_secondary_index(&self, data: &LayoutFileData, uri: &str) {
        let key = (data.module_root.clone(), data.layout_name.clone());
        if let Some(mut entry) = self.layouts_by_module_and_name.get_mut(&key) {
            entry.retain(|existing| existing != uri);
            if entry.is_empty() {
                drop(entry);
                self.layouts_by_module_and_name.remove(&key);
            }
        }
    }

    pub(crate) fn layout_data_for_uri(&self, uri: &str) -> Option<Arc<LayoutFileData>> {
        self.layouts.get(uri).map(|entry| Arc::clone(entry.value()))
    }
}
