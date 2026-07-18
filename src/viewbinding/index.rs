//! ViewBinding read surface split out of [`crate::indexer::resolution::IndexRead`].

use std::path::Path;
use std::sync::Arc;

use tower_lsp::lsp_types::Range;

use crate::indexer::Indexer;
use crate::viewbinding::LayoutFileData;

/// ViewBinding side-index queries used by navigation and field-type resolution.
pub(crate) trait ViewBindingIndex {
    /// Index layout XML files under `module_root` that are missing from the layout side index.
    fn ensure_module_layouts_indexed(&self, _module_root: &Path) -> usize {
        0
    }

    /// Layout variants for a generated binding class in the given module (default first).
    #[allow(dead_code)]
    fn layouts_for_binding_class(
        &self,
        _class_name: &str,
        _module_root: &Path,
    ) -> Vec<Arc<LayoutFileData>> {
        Vec::new()
    }

    /// True when `uri` is a discovered generated ViewBinding Java file.
    fn is_generated_binding_uri(&self, _uri: &str) -> bool {
        false
    }

    /// Layout side-index entry for a layout XML URI.
    fn layout_data_for_uri(&self, _uri: &str) -> Option<Arc<LayoutFileData>> {
        None
    }

    /// Layout URIs paired with data for a binding class (default variant first).
    fn layout_uris_for_binding_class(
        &self,
        _class_name: &str,
        _module_root: &Path,
    ) -> Vec<(String, Arc<LayoutFileData>)> {
        Vec::new()
    }

    /// Every variant declaring `@+id/{id}` for the given layout name in `module_root`.
    fn layouts_declaring_view_id(
        &self,
        _module_root: &Path,
        _layout_name: &str,
        _id: &str,
    ) -> Vec<(String, Range)> {
        Vec::new()
    }

    /// `<include>` tag ranges whose `android:id` maps to `field_name`.
    fn include_tag_for_field(
        &self,
        _module_root: &Path,
        _layout_name: &str,
        _field_name: &str,
    ) -> Vec<(String, Range)> {
        Vec::new()
    }
}

impl ViewBindingIndex for Indexer {
    fn ensure_module_layouts_indexed(&self, module_root: &Path) -> usize {
        Indexer::ensure_module_layouts_indexed(self, module_root)
    }

    fn layouts_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<Arc<LayoutFileData>> {
        Indexer::layouts_for_binding_class(self, class_name, module_root)
    }

    fn is_generated_binding_uri(&self, uri: &str) -> bool {
        Indexer::is_generated_binding_uri(self, uri)
    }

    fn layout_data_for_uri(&self, uri: &str) -> Option<Arc<LayoutFileData>> {
        Indexer::layout_data_for_uri(self, uri)
    }

    fn layout_uris_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<(String, Arc<LayoutFileData>)> {
        Indexer::layout_uris_for_binding_class(self, class_name, module_root)
    }

    fn layouts_declaring_view_id(
        &self,
        module_root: &Path,
        layout_name: &str,
        id: &str,
    ) -> Vec<(String, Range)> {
        Indexer::layouts_declaring_view_id(self, module_root, layout_name, id)
    }

    fn include_tag_for_field(
        &self,
        module_root: &Path,
        layout_name: &str,
        field_name: &str,
    ) -> Vec<(String, Range)> {
        Indexer::include_tag_for_field(self, module_root, layout_name, field_name)
    }
}

impl ViewBindingIndex for Arc<Indexer> {
    fn ensure_module_layouts_indexed(&self, module_root: &Path) -> usize {
        ViewBindingIndex::ensure_module_layouts_indexed(self.as_ref(), module_root)
    }

    fn layouts_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<Arc<LayoutFileData>> {
        ViewBindingIndex::layouts_for_binding_class(self.as_ref(), class_name, module_root)
    }

    fn is_generated_binding_uri(&self, uri: &str) -> bool {
        ViewBindingIndex::is_generated_binding_uri(self.as_ref(), uri)
    }

    fn layout_data_for_uri(&self, uri: &str) -> Option<Arc<LayoutFileData>> {
        ViewBindingIndex::layout_data_for_uri(self.as_ref(), uri)
    }

    fn layout_uris_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<(String, Arc<LayoutFileData>)> {
        ViewBindingIndex::layout_uris_for_binding_class(self.as_ref(), class_name, module_root)
    }

    fn layouts_declaring_view_id(
        &self,
        module_root: &Path,
        layout_name: &str,
        id: &str,
    ) -> Vec<(String, Range)> {
        ViewBindingIndex::layouts_declaring_view_id(self.as_ref(), module_root, layout_name, id)
    }

    fn include_tag_for_field(
        &self,
        module_root: &Path,
        layout_name: &str,
        field_name: &str,
    ) -> Vec<(String, Range)> {
        ViewBindingIndex::include_tag_for_field(self.as_ref(), module_root, layout_name, field_name)
    }
}

impl<T: ViewBindingIndex + ?Sized> ViewBindingIndex for &T {
    fn ensure_module_layouts_indexed(&self, module_root: &Path) -> usize {
        (**self).ensure_module_layouts_indexed(module_root)
    }

    fn layouts_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<Arc<LayoutFileData>> {
        (**self).layouts_for_binding_class(class_name, module_root)
    }

    fn is_generated_binding_uri(&self, uri: &str) -> bool {
        (**self).is_generated_binding_uri(uri)
    }

    fn layout_data_for_uri(&self, uri: &str) -> Option<Arc<LayoutFileData>> {
        (**self).layout_data_for_uri(uri)
    }

    fn layout_uris_for_binding_class(
        &self,
        class_name: &str,
        module_root: &Path,
    ) -> Vec<(String, Arc<LayoutFileData>)> {
        (**self).layout_uris_for_binding_class(class_name, module_root)
    }

    fn layouts_declaring_view_id(
        &self,
        module_root: &Path,
        layout_name: &str,
        id: &str,
    ) -> Vec<(String, Range)> {
        (**self).layouts_declaring_view_id(module_root, layout_name, id)
    }

    fn include_tag_for_field(
        &self,
        module_root: &Path,
        layout_name: &str,
        field_name: &str,
    ) -> Vec<(String, Range)> {
        (**self).include_tag_for_field(module_root, layout_name, field_name)
    }
}
