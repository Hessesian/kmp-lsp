//! ViewBinding side index, discovery, field types, and navigation helpers.

pub(crate) mod diagnostics;
pub(crate) mod discovery;
pub(crate) mod field_type;
pub(crate) mod hover;
pub(crate) mod index;
pub(crate) mod inference;
pub(crate) mod layout;
pub(crate) mod navigation;
pub(crate) mod receiver;
pub(crate) mod state;
pub(crate) mod watcher;

pub(crate) use diagnostics::{stale_binding_field_diagnostics, viewbinding_import_diagnostics};
pub(crate) use discovery::{
    binding_class_name_for_layout, binding_field_name_to_id, binding_id_to_field_name,
    discover_databinding_dirs, import_triggers_binding_discovery,
    is_generated_binding_watcher_path, is_view_binding_class_name, layout_name_for_binding_class,
    module_root_for_generated_file, module_root_for_source_file, spawn_binding_discovery_worker,
    DatabindingWatcherHandle, DatabindingWatcherState, ModuleBindingsCacheEntry,
};
#[cfg(test)]
pub(crate) use field_type::binding_layout_completion_fields;
pub(crate) use field_type::{
    binding_field_type, binding_layout_dot_completion_items, infer_bare_binding_field_type,
    java_field_type_from_detail, short_type_name,
};
pub(crate) use index::ViewBindingIndex;
pub(crate) use inference::{
    binding_field_type_in_class, binding_type_from_initializer_node,
    infer_view_binding_delegate_type, view_binding_delegate_type_from_property,
};
pub(crate) use layout::{
    element_tag_at_layout_position, id_attribute_position_for_view_id, is_layout_xml_path,
    layout_path_components, spawn_layout_indexing_worker, view_id_at_layout_position,
    LayoutCacheEntry, LayoutFileData,
};
pub(crate) use navigation::{
    binding_field_hover_at_location, binding_field_hover_for_class,
    binding_field_in_generated_java, find_binding_field_references, find_layout_xml_references,
    resolve_expected_binding_class,
};
pub(crate) use state::ViewBindingState;
pub(crate) use watcher::spawn_databinding_watcher;

#[cfg(test)]
pub(crate) use navigation::{
    binding_field_in_live_layout, find_binding_field_definition, find_binding_implementation,
    find_layout_xml_definition, find_layout_xml_implementation, format_binding_field_hover,
    normalize_reference_location_to_utf16_for_test, remap_generated_binding_definitions,
};
