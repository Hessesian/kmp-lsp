//! ViewBinding diagnostics: build-required import warning, viewBindingIgnore, staleness.

use std::path::Path;

use tower_lsp::lsp_types::*;

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::{Indexer, NodeExt};
use crate::inlay_hints::{line_starts, ts_byte_col_to_utf16};
use crate::queries::{
    KIND_CALL_EXPR, KIND_NAV_EXPR, KIND_NAV_SUFFIX, KIND_PARAMETER, KIND_SIMPLE_IDENT,
    KIND_VAR_DECL,
};
use crate::viewbinding::{
    import_triggers_binding_discovery, layout_name_for_binding_class, module_root_for_source_file,
};
use crate::Language;
use crate::StrExt;

use super::navigation::view_id_live_for_binding_field;
use super::receiver::{binding_class_for_bare_field_access, binding_class_for_field_access};

const DIAGNOSTIC_SOURCE: &str = "kmp-lsp";

/// Warn on databinding imports when the paired layout exists but binding generation is missing or opted out.
pub(crate) fn viewbinding_import_diagnostics(index: &Indexer, uri: &Url) -> Vec<Diagnostic> {
    if !matches!(
        Language::from_path(uri.path()),
        Language::Kotlin | Language::Java
    ) {
        return Vec::new();
    }

    let Some(file_data) = index.file_data_for(uri.as_str()) else {
        return Vec::new();
    };
    let Some(module_root) = uri
        .to_file_path()
        .ok()
        .and_then(|path| module_root_for_source_file(&path))
    else {
        return Vec::new();
    };

    let mut diagnostics = Vec::new();
    for import in &file_data.imports {
        if import.is_star || !import_triggers_binding_discovery(&import.full_path) {
            continue;
        }
        let Some(class_name) = import.full_path.rsplit('.').next() else {
            continue;
        };
        let Some(layout_name) = layout_name_for_binding_class(class_name) else {
            continue;
        };
        if !index.layout_exists_for_binding(&module_root, &layout_name) {
            continue;
        }

        let message = if index.all_layout_variants_ignore_view_binding(&module_root, &layout_name) {
            "Layout opts out of ViewBinding (`tools:viewBindingIgnore`)".to_string()
        } else if !index.generated_binding_discovered(&module_root, class_name) {
            "ViewBinding class not generated — build the project".to_string()
        } else {
            continue;
        };

        if let Some(range) = import_line_range(&file_data.lines, import) {
            diagnostics.push(Diagnostic {
                range,
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some(DIAGNOSTIC_SOURCE.into()),
                message,
                ..Default::default()
            });
        }
    }
    diagnostics
}

fn import_line_range(lines: &[String], import: &crate::types::ImportEntry) -> Option<Range> {
    let full_path_needle = format!("import {}", import.full_path);
    let alias_needle = format!("import {} as {}", import.full_path, import.local_name);
    for (line_index, line) in lines.iter().enumerate() {
        if line.contains(&full_path_needle) || line.contains(&alias_needle) {
            let end_col = line
                .chars()
                .map(|character| character.len_utf16() as u32)
                .sum();
            return Some(Range {
                start: Position {
                    line: line_index as u32,
                    character: 0,
                },
                end: Position {
                    line: line_index as u32,
                    character: end_col,
                },
            });
        }
    }
    None
}

/// Information diagnostics on stale binding field usages (id gone from all layout variants).
pub(crate) fn stale_binding_field_diagnostics(
    index: &Indexer,
    uri: &Url,
    document: &LiveDoc,
) -> Vec<Diagnostic> {
    if !matches!(Language::from_path(uri.path()), Language::Kotlin) {
        return Vec::new();
    }
    if !module_has_binding_staleness_context(index, uri) {
        return Vec::new();
    }

    let bytes = &document.bytes;
    let mut diagnostics = Vec::new();
    collect_stale_binding_fields(
        document.tree.root_node(),
        bytes,
        index,
        uri,
        &mut diagnostics,
    );
    diagnostics
}

fn collect_stale_binding_fields(
    node: tree_sitter::Node,
    bytes: &[u8],
    index: &Indexer,
    uri: &Url,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match node.kind() {
        KIND_NAV_EXPR => {
            if let Some(diagnostic) = check_stale_binding_field(&node, bytes, index, uri) {
                diagnostics.push(diagnostic);
            }
        }
        KIND_SIMPLE_IDENT => {
            if let Some(diagnostic) = check_stale_bare_binding_field(&node, bytes, index, uri) {
                diagnostics.push(diagnostic);
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_stale_binding_fields(cursor.node(), bytes, index, uri, diagnostics);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn check_stale_binding_field(
    navigation_node: &tree_sitter::Node,
    bytes: &[u8],
    index: &Indexer,
    uri: &Url,
) -> Option<Diagnostic> {
    let named_count = navigation_node.named_child_count();
    if named_count < 2 {
        return None;
    }
    let receiver_node = navigation_node.named_child(0)?;
    let suffix_node = navigation_node.named_child(named_count - 1)?;
    if suffix_node.child(0)?.kind() != "." {
        return None;
    }
    let field_node = suffix_node.first_child_of_kind(KIND_SIMPLE_IDENT)?;
    let field_name = field_node.utf8_text_owned(bytes)?;
    if field_name.starts_with_uppercase() {
        return None;
    }

    let binding_class = binding_class_for_field_access(index, &receiver_node, bytes, uri)?;
    stale_binding_field_diagnostic(
        index,
        uri,
        &binding_class,
        &field_name,
        node_to_range(field_node, bytes),
    )
}

/// Staleness diagnostic for a bare implicit-`this` binding field, e.g. `title`
/// inside `with(binding) { title }`. Consistent with the navigation and
/// reference support for bare receiver-scope members.
fn check_stale_bare_binding_field(
    identifier_node: &tree_sitter::Node,
    bytes: &[u8],
    index: &Indexer,
    uri: &Url,
) -> Option<Diagnostic> {
    if is_non_reference_identifier(identifier_node) {
        return None;
    }
    let field_name = identifier_node.utf8_text_owned(bytes)?;
    if field_name.starts_with_uppercase() {
        return None;
    }
    let binding_class =
        binding_class_for_bare_field_access(index, identifier_node, &field_name, bytes, uri, None)?;
    stale_binding_field_diagnostic(
        index,
        uri,
        &binding_class,
        &field_name,
        node_to_range(*identifier_node, bytes),
    )
}

/// True when `node` (a `simple_identifier`) is not a standalone value
/// reference: the receiver or `.member` of a navigation expression, a call
/// callee, or the bound name of a declaration (val/var/lambda param/function
/// param). None of these is a bare implicit-`this` member usage.
fn is_non_reference_identifier(node: &tree_sitter::Node) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            KIND_NAV_EXPR | KIND_NAV_SUFFIX | KIND_CALL_EXPR | KIND_VAR_DECL | KIND_PARAMETER
        )
    })
}

/// Shared staleness check: the field exists in the module's generated binding
/// but its id is gone from every live layout variant.
fn stale_binding_field_diagnostic(
    index: &Indexer,
    uri: &Url,
    binding_class: &str,
    field_name: &str,
    range: Range,
) -> Option<Diagnostic> {
    let module_root = uri
        .to_file_path()
        .ok()
        .and_then(|path| module_root_for_source_file(&path))?;
    let layout_name = layout_name_for_binding_class(binding_class)?;

    if !binding_field_exists(index, &module_root, binding_class, field_name) {
        return None;
    }
    if view_id_live_for_binding_field(index, &module_root, &layout_name, field_name) {
        return None;
    }

    Some(Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::INFORMATION),
        source: Some(DIAGNOSTIC_SOURCE.into()),
        message: format!("Field `{field_name}` comes from a stale build; id no longer in layout"),
        ..Default::default()
    })
}

/// True when the module's own generated binding class declares `field_name`.
///
/// Pairs the lookup to `module_root` so a same-named binding in another module
/// cannot make this module's usages look like they come from a stale build.
fn binding_field_exists(
    index: &Indexer,
    module_root: &Path,
    binding_class: &str,
    field_name: &str,
) -> bool {
    let Some(module) = index.viewbinding.generated_bindings.get(module_root) else {
        return false;
    };
    let Some(entry) = module.entries.get(binding_class) else {
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

/// Skip staleness work when the module has no generated bindings and no databinding import.
fn module_has_binding_staleness_context(index: &Indexer, uri: &Url) -> bool {
    let Some(module_root) = uri
        .to_file_path()
        .ok()
        .and_then(|path| module_root_for_source_file(&path))
    else {
        return false;
    };
    if index
        .viewbinding
        .generated_bindings
        .get(&module_root)
        .is_some_and(|module| !module.entries.is_empty())
    {
        return true;
    }
    index.file_data_for(uri.as_str()).is_some_and(|file_data| {
        file_data
            .imports
            .iter()
            .any(|import| !import.is_star && import_triggers_binding_discovery(&import.full_path))
    })
}

fn node_to_range(node: tree_sitter::Node, bytes: &[u8]) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    let line_start_offsets = line_starts(bytes);
    Range {
        start: Position {
            line: start.row as u32,
            character: ts_byte_col_to_utf16(bytes, &line_start_offsets, start.row, start.column)
                as u32,
        },
        end: Position {
            line: end.row as u32,
            character: ts_byte_col_to_utf16(bytes, &line_start_offsets, end.row, end.column) as u32,
        },
    }
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
