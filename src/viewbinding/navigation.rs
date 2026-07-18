//! ViewBinding navigation — post-resolution remap, hover, references (PR 4–5).
//!
//! Definition remap miss policy: keep the resolved generated-Java location when
//! XML targets are unavailable (no layouts, missing `@+id`, etc.). Field misses
//! must not fall back to the binding class layout header. Unresolved symbols
//! stay silently empty; diagnostics explain build/staleness issues.

use std::path::Path;

use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position, Range, SymbolKind, Url};
use tree_sitter::Tree;

use super::receiver::{
    binding_class_for_bare_field_access, binding_class_for_receiver_chain,
    binding_class_from_receiver_type, receiver_matches_binding_class,
    receiver_type_for_binding_field_reference,
};
use crate::backend::cursor::CursorContext;
use crate::backend::format::format_contextual_hover;
use crate::features::definition::locs_to_opt_response;
use crate::features::references::{
    find_references_scoped_to_files, find_references_with_qualifier,
};
use crate::features::traits::{DocumentAccess, SymbolIndex};
use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte, RequestParseCache};
use crate::indexer::{IndexRead, Indexer};
use crate::inlay_hints::ts_byte_col_to_utf16;
use crate::queries::KIND_SIMPLE_IDENT;
use crate::resolver::{
    infer::infer_field_chain_type, infer_receiver_type, infer_receiver_type_at, ReceiverKind,
};
use crate::types::{FileData, SymbolEntry};
use crate::viewbinding::ViewBindingIndex;
use crate::viewbinding::{
    binding_class_name_for_layout, binding_field_name_to_id, binding_id_to_field_name,
    element_tag_at_layout_position, id_attribute_position_for_view_id, is_layout_xml_path,
    java_field_type_from_detail, layout_name_for_binding_class, layout_path_components,
    module_root_for_generated_file, module_root_for_source_file, short_type_name,
    view_id_at_layout_position,
};
use crate::StrExt;

const ANDROID_TAG_PREFIXES: &[&str] = &["android.widget.", "android.view.", "android.webkit."];

// ─── Kotlin-side post-resolution remap (definition only) ───────────────────────

/// Remap generated `*Binding.java` definition targets to layout XML.
pub(crate) fn remap_generated_binding_definitions<I: IndexRead + ViewBindingIndex>(
    index: &I,
    locations: Vec<Location>,
) -> Vec<Location> {
    log::debug!(
        "viewbinding: remap_generated_binding_definitions pre_remap_count={}",
        locations.len()
    );
    let mut remapped = Vec::new();
    for location in locations {
        if !index.is_generated_binding_uri(location.uri.as_str()) {
            remapped.push(location);
            continue;
        }
        let Some(targets) = remap_single_binding_location(index, &location) else {
            log::debug!(
                "viewbinding: remap miss, keeping generated Java fallback uri={}",
                location.uri
            );
            remapped.push(location);
            continue;
        };
        log::debug!(
            "viewbinding: remap hit uri={} target_count={}",
            location.uri,
            targets.len()
        );
        remapped.extend(targets);
    }
    remapped
}

fn remap_single_binding_location<I: IndexRead + ViewBindingIndex>(
    index: &I,
    location: &Location,
) -> Option<Vec<Location>> {
    let path = location.uri.to_file_path().ok()?;
    let module_root = module_root_for_generated_file(&path)?;
    let file_data = index.get_file_data(location.uri.as_str())?;

    if let Some(symbol) = symbol_at_location(&file_data, location) {
        if symbol.kind == SymbolKind::CLASS {
            return remap_binding_class(index, symbol, &module_root);
        }
        if symbol.kind == SymbolKind::CONSTRUCTOR && symbol.name.ends_with("Binding") {
            let class_symbol = file_data
                .symbols
                .iter()
                .find(|entry| entry.kind == SymbolKind::CLASS && entry.name == symbol.name)?;
            return remap_binding_class(index, class_symbol, &module_root);
        }
        return match symbol.kind {
            SymbolKind::FIELD | SymbolKind::PROPERTY | SymbolKind::VARIABLE => {
                remap_binding_field(index, symbol, &module_root, &file_data)
            }
            SymbolKind::METHOD | SymbolKind::FUNCTION if symbol.name == "getRoot" => {
                remap_root_view(index, &module_root, &file_data)
            }
            _ => None,
        };
    }

    if location_targets_binding_field(&file_data, location) {
        return None;
    }

    let class_symbol = binding_class_symbol_for_location(&file_data, location)?;
    remap_binding_class(index, class_symbol, &module_root)
}

fn location_targets_binding_field(file_data: &FileData, location: &Location) -> bool {
    file_data.symbols.iter().any(|symbol| {
        matches!(
            symbol.kind,
            SymbolKind::FIELD | SymbolKind::PROPERTY | SymbolKind::VARIABLE
        ) && (position_in_range(location.range.start, symbol.selection_range)
            || position_in_range(location.range.start, symbol.range))
    })
}

fn binding_class_symbol_for_location<'a>(
    file_data: &'a FileData,
    location: &Location,
) -> Option<&'a SymbolEntry> {
    file_data
        .symbols
        .iter()
        .find(|symbol| {
            symbol.kind == SymbolKind::CLASS
                && symbol.name.ends_with("Binding")
                && position_in_class_header(location.range.start, symbol)
        })
        .or_else(|| {
            file_data.symbols.iter().find(|symbol| {
                symbol.kind == SymbolKind::CLASS
                    && symbol.name.ends_with("Binding")
                    && position_in_range(location.range.start, symbol.range)
            })
        })
}

fn position_in_class_header(position: Position, class_symbol: &SymbolEntry) -> bool {
    position_in_range(position, class_symbol.selection_range)
}

fn remap_binding_class<I: IndexRead + ViewBindingIndex>(
    index: &I,
    symbol: &SymbolEntry,
    module_root: &Path,
) -> Option<Vec<Location>> {
    let mut entries = index.layout_uris_for_binding_class(&symbol.name, module_root);
    if entries.is_empty() {
        let on_demand = index.ensure_module_layouts_indexed(module_root);
        log::debug!(
            "viewbinding: remap_binding_class class={} module={} on_demand_indexed={}",
            symbol.name,
            module_root.display(),
            on_demand
        );
        entries = index.layout_uris_for_binding_class(&symbol.name, module_root);
    }
    if entries.is_empty() {
        log::debug!(
            "viewbinding: remap_binding_class no layouts for class={} module={}",
            symbol.name,
            module_root.display()
        );
        return None;
    }
    Some(
        entries
            .into_iter()
            .filter_map(|(uri_string, _data)| {
                location_from_layout_uri(&uri_string).map(layout_file_start_location)
            })
            .collect::<Vec<_>>(),
    )
    .filter(|locations| !locations.is_empty())
}

fn location_from_layout_uri(uri_string: &str) -> Option<Url> {
    Url::parse(uri_string).ok()
}

fn remap_binding_field<I: IndexRead + ViewBindingIndex>(
    index: &I,
    symbol: &SymbolEntry,
    module_root: &Path,
    file_data: &FileData,
) -> Option<Vec<Location>> {
    if symbol.name == "rootView" {
        return remap_root_view(index, module_root, file_data);
    }

    let class_name = symbol.container.as_deref()?;
    let layout_name = layout_name_for_binding_class(class_name)?;

    index.ensure_module_layouts_indexed(module_root);

    let targets = layout_targets_for_binding_field(index, module_root, &layout_name, &symbol.name);
    if targets.is_empty() {
        log::debug!(
            "viewbinding: remap_binding_field no @+id for field={} layout={} module={}",
            symbol.name,
            layout_name,
            module_root.display()
        );
        return None;
    }
    Some(locations_from_uri_ranges(targets))
}

/// Shared layout side-index lookup for a binding field (`@+id` or `<include>`).
fn layout_targets_for_binding_field<I: IndexRead + ViewBindingIndex>(
    index: &I,
    module_root: &Path,
    layout_name: &str,
    field_name: &str,
) -> Vec<(String, Range)> {
    let include_targets = index.include_tag_for_field(module_root, layout_name, field_name);
    if !include_targets.is_empty() {
        return include_targets;
    }
    let view_id = binding_field_name_to_id(field_name);
    index.layouts_declaring_view_id(module_root, layout_name, &view_id)
}

fn remap_root_view<I: IndexRead + ViewBindingIndex>(
    index: &I,
    module_root: &Path,
    file_data: &FileData,
) -> Option<Vec<Location>> {
    let class_name = binding_class_from_file_data(file_data)?;
    let layout_name = layout_name_for_binding_class(&class_name)?;
    index.ensure_module_layouts_indexed(module_root);
    let entries = index.layout_uris_for_binding_class(&class_name, module_root);
    let targets: Vec<Location> = entries
        .into_iter()
        .filter(|(_uri, data)| data.layout_name == layout_name)
        .filter_map(|(uri_string, data)| {
            Url::parse(&uri_string).ok().map(|uri| Location {
                uri,
                range: data.root_tag.range,
            })
        })
        .collect();
    if targets.is_empty() {
        None
    } else {
        Some(targets)
    }
}

fn binding_class_from_file_data(file_data: &FileData) -> Option<String> {
    file_data
        .symbols
        .iter()
        .find(|symbol| symbol.kind == SymbolKind::CLASS)
        .map(|symbol| symbol.name.clone())
}

fn layout_file_start_location(uri: Url) -> Location {
    Location {
        uri,
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        },
    }
}

fn locations_from_uri_ranges(entries: Vec<(String, Range)>) -> Vec<Location> {
    entries
        .into_iter()
        .filter_map(|(uri_string, range)| {
            Url::parse(&uri_string)
                .ok()
                .map(|uri| Location { uri, range })
        })
        .collect()
}

fn symbol_at_location<'a>(file_data: &'a FileData, location: &Location) -> Option<&'a SymbolEntry> {
    if let Some(symbol) = file_data
        .symbols
        .iter()
        .find(|symbol| symbol.selection_range == location.range)
    {
        return Some(symbol);
    }
    if let Some(symbol) = file_data
        .symbols
        .iter()
        .find(|symbol| position_in_range(location.range.start, symbol.selection_range))
    {
        return Some(symbol);
    }
    file_data.symbols.iter().find(|symbol| {
        symbol.kind == SymbolKind::CLASS && position_in_range(location.range.start, symbol.range)
    })
}

fn position_in_range(position: Position, range: Range) -> bool {
    (position.line > range.start.line
        || (position.line == range.start.line && position.character >= range.start.character))
        && (position.line < range.end.line
            || (position.line == range.end.line && position.character <= range.end.character))
}

// ─── Binding-type implementation (Kotlin) ────────────────────────────────────

/// True when `field_name` is declared in the generated Java binding class.
pub(crate) fn binding_field_in_generated_java(
    index: &Indexer,
    expected_binding_class: &str,
    field_name: &str,
    source_uri: &Url,
) -> bool {
    let Some(path) = source_uri.to_file_path().ok() else {
        return false;
    };
    let Some(module_root) = module_root_for_source_file(&path) else {
        return false;
    };
    let Some(module) = index.viewbinding.generated_bindings.get(&module_root) else {
        return false;
    };
    let Some(entry) = module.entries.get(expected_binding_class) else {
        return false;
    };
    let Some(file_data) = index.file_data_for(&entry.file_uri) else {
        return false;
    };
    file_data.symbols.iter().any(|symbol| {
        symbol.name == field_name
            && matches!(
                symbol.kind,
                SymbolKind::FIELD | SymbolKind::PROPERTY | SymbolKind::VARIABLE
            )
    })
}

/// True when `field_name` maps to a live `@+id` or `<include>` in `layout_name`.
pub(crate) fn binding_field_in_live_layout_by_name(
    index: &Indexer,
    module_root: &Path,
    layout_name: &str,
    field_name: &str,
) -> bool {
    index.ensure_module_layouts_indexed(module_root);
    !layout_targets_for_binding_field(index, module_root, layout_name, field_name).is_empty()
}

/// True when `field_name` maps to a live `@+id` or `<include>` in the paired layout.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn binding_field_in_live_layout(
    index: &Indexer,
    expected_binding_class: &str,
    field_name: &str,
    source_uri: &Url,
) -> bool {
    let Some(path) = source_uri.to_file_path().ok() else {
        return false;
    };
    let Some(module_root) = module_root_for_source_file(&path) else {
        return false;
    };
    let Some(layout_name) = layout_name_for_binding_class(expected_binding_class) else {
        return false;
    };
    binding_field_in_live_layout_by_name(index, &module_root, &layout_name, field_name)
}

/// Return the raw generated Java class for a `*Binding` type usage — no remap.
pub(crate) fn find_binding_implementation(
    index: &(impl SymbolIndex + IndexRead + ViewBindingIndex),
    ctx: &CursorContext,
    uri: &Url,
    _position: Position,
) -> Option<GotoDefinitionResponse> {
    if !ctx.word.ends_with("Binding") {
        return None;
    }
    let locations = index.find_definition_qualified(&ctx.word, ctx.qualifier.as_deref(), uri);
    let binding_locations: Vec<Location> = locations
        .iter()
        .filter(|location| index.is_generated_binding_uri(location.uri.as_str()))
        .cloned()
        .collect();
    if !binding_locations.is_empty() {
        return locs_to_opt_response(binding_locations);
    }
    let has_competing_workspace_class = locations.iter().any(|location| {
        !index.is_generated_binding_uri(location.uri.as_str())
            && index
                .get_file_data(location.uri.as_str())
                .is_some_and(|file_data| {
                    file_data
                        .symbols
                        .iter()
                        .any(|symbol| symbol.name == ctx.word)
                })
    });
    if has_competing_workspace_class {
        return None;
    }
    locs_to_opt_response(binding_locations)
}

/// Definition on `binding.field` — resolve straight to `@+id/…` via the layout side index.
pub(crate) fn find_binding_field_definition(
    index: &Indexer,
    uri: &Url,
    position: Position,
    ctx: &CursorContext,
) -> Option<GotoDefinitionResponse> {
    if ctx.word.starts_with_uppercase() {
        return None;
    }
    let expected_class = resolve_expected_binding_class(index, uri, position, ctx, None)?;
    let path = uri.to_file_path().ok()?;
    let module_root = module_root_for_source_file(&path)?;
    let layout_name = layout_name_for_binding_class(&expected_class)?;
    log::debug!(
        "viewbinding: find_binding_field_definition field={} qualifier={:?} class={} module={}",
        ctx.word,
        ctx.qualifier,
        expected_class,
        module_root.display()
    );
    index.ensure_module_layouts_indexed(&module_root);
    let targets = layout_targets_for_binding_field(index, &module_root, &layout_name, &ctx.word);
    locs_to_opt_response(locations_from_uri_ranges(targets))
}

// ─── XML-side navigation ─────────────────────────────────────────────────────

/// Definition on `@+id/...` or `@id/...` inside a layout XML file.
pub(crate) fn find_layout_xml_definition(
    index: &(impl IndexRead + ViewBindingIndex + DocumentAccess),
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let path = uri.to_file_path().ok()?;
    if !is_layout_xml_path(&path) {
        return None;
    }
    let current = index.layout_data_for_uri(uri.as_str())?;
    let view_id = view_id_at_layout_position(&current, position)?;
    let declarations = index.layouts_declaring_view_id(
        current.module_root.as_path(),
        &current.layout_name,
        &view_id,
    );
    locs_to_opt_response(locations_from_uri_ranges(declarations))
}

/// Implementation on a layout XML element tag name.
pub(crate) fn find_layout_xml_implementation(
    index: &(impl SymbolIndex + IndexRead + ViewBindingIndex + DocumentAccess),
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let path = uri.to_file_path().ok()?;
    if !is_layout_xml_path(&path) {
        return None;
    }
    let layout_data = index.layout_data_for_uri(uri.as_str())?;
    let tag_name = element_tag_at_layout_position(&layout_data, position)?;
    if tag_name.contains('.') {
        let mut locations = index.qualified_definition_locations(&tag_name);
        if locations.is_empty() {
            locations = index.find_definition_qualified(&tag_name, None, uri);
        }
        if locations.is_empty() {
            if let Some((qualifier, simple)) = tag_name.rsplit_once('.') {
                locations = index.find_definition_qualified(simple, Some(qualifier), uri);
            }
        }
        return locs_to_opt_response(locations);
    }
    for prefix in ANDROID_TAG_PREFIXES {
        let qualified = format!("{prefix}{tag_name}");
        let locations = index.find_definition_qualified(&qualified, None, uri);
        if !locations.is_empty() {
            return locs_to_opt_response(locations);
        }
    }
    None
}

/// Kotlin-style hover for a generated binding field: `val title: TextView` / `val title: TextView?`.
pub(crate) fn format_binding_field_hover(
    field_name: &str,
    type_name: &str,
    nullable: bool,
) -> String {
    let short = short_type_name(type_name);
    let rendered_type = if nullable { format!("{short}?") } else { short };
    let signature = format!("val {field_name}: {rendered_type}");
    format_contextual_hover(&signature, ".kt", None)
}

/// Kotlin-style hover for a field on a known generated binding class, resolved
/// as seen from the source file at `uri` (import package first, then the file's
/// own module, then a workspace-unique match).
pub(crate) fn binding_field_hover_for_class(
    index: &Indexer,
    uri: &Url,
    class_name: &str,
    field_name: &str,
) -> Option<String> {
    let binding_file_uri = binding_file_uri_for_source(index, uri, class_name)?;
    let file_data = index.file_data_for(&binding_file_uri)?;
    let symbol = file_data.symbols.iter().find(|symbol| {
        symbol.name == field_name
            && matches!(
                symbol.kind,
                SymbolKind::FIELD | SymbolKind::PROPERTY | SymbolKind::VARIABLE
            )
    })?;
    let type_name = java_field_type_from_detail(&symbol.detail, field_name)?;
    Some(format_binding_field_hover(
        field_name,
        &type_name,
        symbol.nullable,
    ))
}

/// Resolve the generated binding file for `class_name` as seen from the source
/// file at `uri`, so multi-module workspaces with same-named binding classes
/// pick the module the file actually refers to.
fn binding_file_uri_for_source(index: &Indexer, uri: &Url, class_name: &str) -> Option<String> {
    index.generated_binding_file_uri_for_source(uri, class_name)
}

/// When `location` is a generated binding field, return Kotlin-style hover markdown.
pub(crate) fn binding_field_hover_at_location<I: IndexRead + ViewBindingIndex>(
    index: &I,
    location: &Location,
    field_name: &str,
) -> Option<String> {
    if !index.is_generated_binding_uri(location.uri.as_str()) {
        return None;
    }
    let file_data = index.get_file_data(location.uri.as_str())?;
    let symbol = file_data
        .symbols
        .iter()
        .find(|symbol| {
            symbol.name == field_name
                && matches!(
                    symbol.kind,
                    SymbolKind::FIELD | SymbolKind::PROPERTY | SymbolKind::VARIABLE
                )
        })
        .or_else(|| symbol_at_location(&file_data, location))?;
    if symbol.name != field_name {
        return None;
    }
    if !matches!(
        symbol.kind,
        SymbolKind::FIELD | SymbolKind::PROPERTY | SymbolKind::VARIABLE
    ) {
        return None;
    }
    let type_name = java_field_type_from_detail(&symbol.detail, field_name)?;
    Some(format_binding_field_hover(
        field_name,
        &type_name,
        symbol.nullable,
    ))
}

// ─── Receiver-verified references (PR 5) ─────────────────────────────────────

/// Resolve the expected `*Binding` class for a references request at `position`.
pub(crate) fn resolve_expected_binding_class(
    index: &Indexer,
    uri: &Url,
    position: Position,
    ctx: &CursorContext,
    parse_cache: Option<&mut RequestParseCache>,
) -> Option<String> {
    if index.is_generated_binding_uri(uri.as_str()) {
        if !ctx.word.starts_with_uppercase() {
            return binding_class_from_file_uri(index, uri);
        }
        return None;
    }

    if let Some(receiver_type) = ctx.contextual.as_ref() {
        if let Some(class_name) = binding_class_from_receiver_type(receiver_type) {
            return Some(class_name);
        }
    }

    if let Some(qualifier) = ctx.qualifier.as_deref() {
        if qualifier.contains('.') {
            let segments: Vec<&str> = qualifier.split('.').collect();
            if let Some(class_name) =
                binding_class_for_receiver_chain(index, uri, position, &segments)
            {
                return Some(class_name);
            }
            let chain: Vec<String> = segments
                .iter()
                .map(|segment| (*segment).to_string())
                .collect();
            if let Some(receiver_type) = infer_field_chain_type(index, &chain, uri) {
                return binding_class_from_receiver_type(&receiver_type);
            }
        }
        let receiver_type =
            infer_receiver_type_at(index, qualifier, uri, position).or_else(|| {
                infer_receiver_type(
                    index,
                    ReceiverKind::Contextual {
                        name: qualifier,
                        position,
                    },
                    uri,
                )
            })?;
        return binding_class_from_receiver_type(&receiver_type);
    }

    binding_class_for_bare_field_at(index, uri, position, &ctx.word, parse_cache)
}

fn binding_class_for_bare_field_at(
    index: &Indexer,
    uri: &Url,
    position: Position,
    field_name: &str,
    mut parse_cache: Option<&mut RequestParseCache>,
) -> Option<String> {
    let (tree, bytes) = live_or_disk_tree(index, parse_cache.as_deref_mut(), uri)?;
    let line_text = index
        .mem_lines_for(uri.as_str())?
        .get(position.line as usize)?
        .clone();
    let byte_column =
        crate::indexer::live_tree::utf16_col_to_byte(&line_text, position.character as usize);
    let target_point = tree_sitter::Point {
        row: position.line as usize,
        column: byte_column,
    };
    let identifier_node = tree
        .root_node()
        .descendant_for_point_range(target_point, target_point)?;
    if identifier_node.kind() != KIND_SIMPLE_IDENT {
        return None;
    }
    binding_class_for_bare_field_access(
        index,
        &identifier_node,
        field_name,
        &bytes,
        uri,
        parse_cache,
    )
}

fn binding_class_from_file_uri(index: &Indexer, uri: &Url) -> Option<String> {
    let file_data = index.get_file_data(uri.as_str())?;
    binding_class_from_file_data(&file_data)
}

/// Find Kotlin usages of a binding field, verified by receiver type.
pub(crate) async fn find_binding_field_references(
    index: &Indexer,
    parse_cache: &mut RequestParseCache,
    expected_binding_class: &str,
    field_name: &str,
    uri: &Url,
    line: u32,
    include_decl: bool,
) -> Vec<Location> {
    let scope_files = index.workspace_files_importing_binding_class(expected_binding_class, uri);
    let candidates = if scope_files.is_empty() {
        find_references_with_qualifier(field_name, None, uri, line, include_decl, index).await
    } else {
        log::debug!(
            "viewbinding: binding field refs class={} field={} scoped_to_importers={}",
            expected_binding_class,
            field_name,
            scope_files.len()
        );
        find_references_scoped_to_files(field_name, uri, line, include_decl, scope_files, index)
            .await
    };
    let candidate_count = candidates.len();

    let verified: Vec<Location> = candidates
        .into_iter()
        .map(|location| normalize_reference_location_to_utf16(index, &location, field_name))
        .filter(|location| !index.is_generated_binding_uri(location.uri.as_str()))
        .filter(|location| {
            verify_binding_field_reference(
                index,
                parse_cache,
                location,
                field_name,
                expected_binding_class,
            )
        })
        .collect();
    log::debug!(
        "viewbinding: binding field refs class={} field={} candidates={} verified={}",
        expected_binding_class,
        field_name,
        candidate_count,
        verified.len()
    );
    verified
}

fn verify_binding_field_reference(
    index: &Indexer,
    parse_cache: &mut RequestParseCache,
    location: &Location,
    field_name: &str,
    expected_binding_class: &str,
) -> bool {
    let Some((tree, bytes)) = live_or_disk_tree(index, Some(parse_cache), &location.uri) else {
        return false;
    };
    let Some(receiver_type) = receiver_type_for_binding_field_reference(
        index,
        parse_cache,
        &tree,
        &bytes,
        location,
        field_name,
    ) else {
        return false;
    };
    receiver_matches_binding_class(&receiver_type, expected_binding_class)
}

fn live_or_disk_tree(
    index: &Indexer,
    parse_cache: Option<&mut RequestParseCache>,
    uri: &Url,
) -> Option<(Tree, Vec<u8>)> {
    if let Some(document) = index.live_doc(uri) {
        return Some((document.tree.clone(), document.bytes.clone()));
    }
    if let Some(cache) = parse_cache.as_ref() {
        if let Some(document) = cache.get(uri.as_str()) {
            return Some((document.tree.clone(), document.bytes.clone()));
        }
    }
    let path = uri.to_file_path().ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let language = lang_for_path(uri.path())?;
    let document = parse_live(&content, language)?;
    let document = std::sync::Arc::new(document);
    if let Some(cache) = parse_cache {
        cache.insert(uri.to_string(), std::sync::Arc::clone(&document));
    }
    Some((document.tree.clone(), document.bytes.clone()))
}

#[cfg(test)]
pub(crate) fn normalize_reference_location_to_utf16_for_test(
    index: &Indexer,
    location: &Location,
    field_name: &str,
) -> Location {
    normalize_reference_location_to_utf16(index, location, field_name)
}

/// Ripgrep emits byte columns; the index scan emits UTF-16. Normalize to UTF-16 once
/// at ingestion so downstream CST probes use a single coordinate system.
fn normalize_reference_location_to_utf16(
    index: &Indexer,
    location: &Location,
    field_name: &str,
) -> Location {
    let line_text = index
        .mem_lines_for(location.uri.as_str())
        .or_else(|| {
            index
                .files
                .get(location.uri.as_str())
                .map(|file_data| file_data.lines.clone())
        })
        .and_then(|lines| lines.get(location.range.start.line as usize).cloned())
        .unwrap_or_default();
    let character = location.range.start.character as usize;
    let utf16_as_byte = utf16_col_to_byte(&line_text, character);
    if reference_identifier_at_byte_column(&line_text, utf16_as_byte) == Some(field_name) {
        return location.clone();
    }
    if reference_identifier_at_byte_column(&line_text, character) == Some(field_name) {
        let utf16_column = ts_byte_col_to_utf16(line_text.as_bytes(), &[0], 0, character) as u32;
        return Location {
            uri: location.uri.clone(),
            range: Range {
                start: Position {
                    line: location.range.start.line,
                    character: utf16_column,
                },
                end: Position {
                    line: location.range.end.line,
                    character: utf16_column.saturating_add(field_name.len() as u32),
                },
            },
        };
    }
    location.clone()
}

fn reference_identifier_at_byte_column(line_text: &str, byte_column: usize) -> Option<&str> {
    if byte_column > line_text.len() || !line_text.is_char_boundary(byte_column) {
        return None;
    }
    let suffix = &line_text[byte_column..];
    let end = suffix
        .char_indices()
        .find(|(_, character)| !character.is_alphanumeric() && *character != '_')
        .map(|(index, _)| index)
        .unwrap_or(suffix.len());
    let identifier = &suffix[..end];
    if identifier.is_empty() {
        None
    } else {
        Some(identifier)
    }
}

pub(crate) async fn find_layout_xml_references(
    index: &Indexer,
    parse_cache: &mut RequestParseCache,
    uri: &Url,
    position: Position,
    include_decl: bool,
) -> Option<Vec<Location>> {
    let path = uri.to_file_path().ok()?;
    if !is_layout_xml_path(&path) {
        return None;
    }
    ensure_layout_side_index_for_uri(index, uri, &path);
    let layout_data = index.layout_data_for_uri(uri.as_str())?;
    let view_id = view_id_at_layout_position(&layout_data, position)?;
    let field_name = binding_id_to_field_name(&view_id);
    let expected_class = binding_class_name_for_layout(&layout_data.layout_name);
    let decl_position = id_attribute_position_for_view_id(&layout_data, &view_id)?;
    log::debug!(
        "viewbinding: layout xml refs view_id={view_id} field={field_name} class={expected_class} layout={}",
        layout_data.layout_name
    );
    let locations = find_binding_field_references(
        index,
        parse_cache,
        &expected_class,
        &field_name,
        uri,
        decl_position.line,
        include_decl,
    )
    .await;
    if locations.is_empty() {
        log::debug!(
            "viewbinding: layout xml refs resolved nothing for view_id={view_id} class={expected_class}"
        );
    }
    Some(locations)
}

fn ensure_layout_side_index_for_uri(index: &Indexer, _uri: &Url, path: &Path) {
    if let Some(components) = layout_path_components(path) {
        index.request_module_layout_indexing(components.module_root);
    }
}

/// Whether a binding field's id still exists in any live layout variant.
pub(crate) fn view_id_live_for_binding_field(
    index: &Indexer,
    module_root: &Path,
    layout_name: &str,
    field_name: &str,
) -> bool {
    binding_field_in_live_layout_by_name(index, module_root, layout_name, field_name)
}

#[cfg(test)]
#[path = "navigation_tests.rs"]
mod tests;
