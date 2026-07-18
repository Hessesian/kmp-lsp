//! Shared ViewBinding receiver resolution for cursor, references, definition, and diagnostics.
//!
//! One implementation for implicit-`this` detection, shadow checks, and receiver-type
//! inference so the four LSP surfaces cannot drift.

use tower_lsp::lsp_types::{Position, Url};
use tree_sitter::Node;

use crate::indexer::live_tree::{utf16_col_to_byte, RequestParseCache};
use crate::indexer::{find_this_context_in_lines, Indexer, NodeExt, ThisContext};
use crate::inlay_hints::{line_starts, ts_byte_col_to_utf16};
use crate::queries::{KIND_NAV_EXPR, KIND_SIMPLE_IDENT, KIND_THIS_EXPR};
use crate::resolver::{
    infer::infer_field_chain_type, infer_receiver_type, infer_receiver_type_at, ReceiverKind,
    ReceiverType,
};
use crate::types::CursorPos;
use crate::viewbinding::binding_field_type;
pub(crate) use crate::viewbinding::is_view_binding_class_name;

/// True when `receiver_type` names a generated ViewBinding class.
pub(crate) fn binding_class_from_receiver_type(receiver_type: &ReceiverType) -> Option<String> {
    if is_view_binding_class_name(&receiver_type.leaf) {
        return Some(receiver_type.leaf.clone());
    }
    if is_view_binding_class_name(&receiver_type.qualified) {
        return Some(receiver_type.leaf.clone());
    }
    None
}

pub(crate) fn receiver_matches_binding_class(
    receiver_type: &ReceiverType,
    expected_binding_class: &str,
) -> bool {
    receiver_type.leaf == expected_binding_class
        || receiver_type.qualified == expected_binding_class
        || receiver_type
            .qualified
            .ends_with(&format!(".{expected_binding_class}"))
}

/// Binding class for `receiver.field` navigation expression.
pub(crate) fn binding_class_for_field_access(
    index: &Indexer,
    receiver_node: &Node<'_>,
    bytes: &[u8],
    uri: &Url,
) -> Option<String> {
    let receiver_type = infer_receiver_type_for_node(index, receiver_node, bytes, uri)?;
    binding_class_from_receiver_type(&receiver_type)
}

/// Binding class of the implicit `this` receiver at a bare member usage
/// (`title` inside `with(binding) { title }`). Returns `None` when the bare name
/// is not an implicit binding-field access — including when a local
/// declaration shadows it.
pub(crate) fn binding_class_for_bare_field_access(
    index: &Indexer,
    identifier_node: &Node<'_>,
    field_name: &str,
    bytes: &[u8],
    uri: &Url,
    parse_cache: Option<&mut RequestParseCache>,
) -> Option<String> {
    let receiver_type = implicit_receiver_type_for_bare_field_at_node(
        index,
        identifier_node,
        field_name,
        bytes,
        uri,
        parse_cache,
    )?;
    binding_class_from_receiver_type(&receiver_type)
}

/// Implicit `this` receiver type for a bare member at `(line, utf16_column)`.
pub(crate) fn implicit_receiver_type_for_bare_member_at(
    index: &Indexer,
    uri: &Url,
    line: usize,
    utf16_column: usize,
    word: &str,
    parse_cache: Option<&mut RequestParseCache>,
) -> Option<ReceiverType> {
    if index.name_shadowed_by_local_declaration_with_cache(
        uri,
        line,
        utf16_column,
        word,
        parse_cache,
    ) {
        return None;
    }
    let lines = index.mem_lines_for(uri.as_str())?;
    let this_context = find_this_context_in_lines(
        &lines,
        CursorPos {
            line,
            utf16_col: utf16_column,
        },
        index,
        uri,
    );
    match this_context {
        ThisContext::Resolved(resolved_type) => Some(ReceiverType::from_raw(resolved_type)),
        ThisContext::InsideReceiver | ThisContext::NotFound => None,
    }
}

/// True when `member_name` exists on a ViewBinding receiver (layout or generated Java).
pub(crate) fn bare_member_exists_on_binding_receiver(
    index: &Indexer,
    uri: &Url,
    receiver_type: &ReceiverType,
    member_name: &str,
) -> bool {
    if !member_name
        .chars()
        .next()
        .is_some_and(|character| character.is_lowercase())
    {
        return false;
    }
    let binding_class = receiver_type.leaf.as_str();
    if is_view_binding_class_name(binding_class) {
        return binding_field_type(index, Some(uri), binding_class, member_name).is_some();
    }
    crate::resolver::infer::find_field_type_in_class_from(index, binding_class, member_name, uri)
        .is_some()
}

/// Receiver type for a qualified or bare binding-field reference at `location`.
pub(crate) fn receiver_type_for_binding_field_reference(
    index: &Indexer,
    parse_cache: &mut RequestParseCache,
    tree: &tree_sitter::Tree,
    bytes: &[u8],
    location: &tower_lsp::lsp_types::Location,
    field_name: &str,
) -> Option<ReceiverType> {
    if let Some(navigation_node) = navigation_expression_at_position(
        index,
        tree,
        bytes,
        &location.uri,
        location.range.start,
        field_name,
    ) {
        return infer_receiver_for_navigation(index, &navigation_node, bytes, &location.uri);
    }
    implicit_receiver_type_for_bare_field_at_location(
        index,
        parse_cache,
        tree,
        bytes,
        location,
        field_name,
    )
}

/// Walk a receiver chain through generated binding Java fields (`binding.header` →
/// `ViewHeaderBinding`) using the importing source file for module pairing.
pub(crate) fn binding_class_for_receiver_chain(
    index: &Indexer,
    uri: &Url,
    position: Position,
    segments: &[&str],
) -> Option<String> {
    if segments.is_empty() {
        return None;
    }
    let root_type = if segments[0] == "it" || segments[0] == "this" {
        infer_receiver_type(
            index,
            ReceiverKind::Contextual {
                name: segments[0],
                position,
            },
            uri,
        )?
    } else {
        infer_receiver_type_at(index, segments[0], uri, position)?
    };
    let mut binding_class = binding_class_from_receiver_type(&root_type)?;
    for field in &segments[1..] {
        let field_type = binding_field_type(index, Some(uri), &binding_class, field)?;
        if !is_view_binding_class_name(&field_type) {
            return None;
        }
        binding_class = field_type;
    }
    Some(binding_class)
}

fn implicit_receiver_type_for_bare_field_at_node(
    index: &Indexer,
    identifier_node: &Node<'_>,
    field_name: &str,
    bytes: &[u8],
    uri: &Url,
    parse_cache: Option<&mut RequestParseCache>,
) -> Option<ReceiverType> {
    let start = identifier_node.start_position();
    let line_start_offsets = line_starts(bytes);
    let utf16_column = ts_byte_col_to_utf16(bytes, &line_start_offsets, start.row, start.column);
    implicit_receiver_type_for_bare_member_at(
        index,
        uri,
        start.row,
        utf16_column,
        field_name,
        parse_cache,
    )
}

fn implicit_receiver_type_for_bare_field_at_location(
    index: &Indexer,
    parse_cache: &mut RequestParseCache,
    tree: &tree_sitter::Tree,
    bytes: &[u8],
    location: &tower_lsp::lsp_types::Location,
    field_name: &str,
) -> Option<ReceiverType> {
    let root = tree.root_node();
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
    let byte_column = utf16_col_to_byte(&line_text, location.range.start.character as usize);
    let target_point = tree_sitter::Point {
        row: location.range.start.line as usize,
        column: byte_column,
    };
    let node = root.descendant_for_point_range(target_point, target_point)?;
    if node.kind() != KIND_SIMPLE_IDENT {
        return None;
    }
    if node.utf8_text_owned(bytes).as_deref() != Some(field_name) {
        return None;
    }
    implicit_receiver_type_for_bare_field_at_node(
        index,
        &node,
        field_name,
        bytes,
        &location.uri,
        Some(parse_cache),
    )
}

fn navigation_expression_at_position<'tree>(
    index: &Indexer,
    tree: &'tree tree_sitter::Tree,
    bytes: &[u8],
    uri: &Url,
    position: Position,
    field_name: &str,
) -> Option<Node<'tree>> {
    let root = tree.root_node();
    let line_text = index
        .mem_lines_for(uri.as_str())
        .or_else(|| {
            index
                .files
                .get(uri.as_str())
                .map(|file_data| file_data.lines.clone())
        })
        .and_then(|lines| lines.get(position.line as usize).cloned())
        .unwrap_or_default();
    let byte_column = utf16_col_to_byte(&line_text, position.character as usize);
    let target_point = tree_sitter::Point {
        row: position.line as usize,
        column: byte_column,
    };
    let mut node = root.descendant_for_point_range(target_point, target_point)?;
    while let Some(parent) = node.parent() {
        if parent.kind() == KIND_NAV_EXPR
            && navigation_member_name(&parent, bytes).as_deref() == Some(field_name)
        {
            return Some(parent);
        }
        node = parent;
    }
    None
}

fn navigation_member_name(navigation_node: &Node<'_>, bytes: &[u8]) -> Option<String> {
    let named_count = navigation_node.named_child_count();
    if named_count < 2 {
        return None;
    }
    let suffix_node = navigation_node.named_child(named_count - 1)?;
    suffix_node
        .first_child_of_kind(KIND_SIMPLE_IDENT)?
        .utf8_text_owned(bytes)
}

fn infer_receiver_for_navigation(
    index: &Indexer,
    navigation_node: &Node<'_>,
    bytes: &[u8],
    uri: &Url,
) -> Option<ReceiverType> {
    let named_count = navigation_node.named_child_count();
    if named_count < 2 {
        return None;
    }
    let receiver_node = navigation_node.named_child(0)?;
    let suffix_node = navigation_node.named_child(named_count - 1)?;
    let operator = suffix_node.child(0)?;
    if operator.kind() != "." {
        return None;
    }
    infer_receiver_type_for_node(index, &receiver_node, bytes, uri)
}

fn infer_receiver_type_for_node(
    index: &Indexer,
    receiver_node: &Node<'_>,
    bytes: &[u8],
    uri: &Url,
) -> Option<ReceiverType> {
    match receiver_node.kind() {
        KIND_THIS_EXPR => infer_contextual_receiver_type(index, receiver_node, bytes, uri, "this"),
        KIND_SIMPLE_IDENT => {
            let name = receiver_node.utf8_text_owned(bytes)?;
            if name == "super" {
                return None;
            }
            if name == "this" || name == "it" {
                return infer_contextual_receiver_type(index, receiver_node, bytes, uri, &name);
            }
            infer_receiver_type(index, ReceiverKind::Variable(&name), uri)
        }
        KIND_NAV_EXPR => {
            let chain = pure_field_chain(receiver_node, bytes)?;
            if chain.len() < 2 || chain[0] == "this" || chain[0] == "super" {
                return None;
            }
            if let Some(receiver_type) = infer_field_chain_type(index, &chain, uri) {
                return Some(receiver_type);
            }
            let start = receiver_node.start_position();
            let line_start_offsets = line_starts(bytes);
            let utf16_column =
                ts_byte_col_to_utf16(bytes, &line_start_offsets, start.row, start.column);
            let position = Position::new(start.row as u32, utf16_column as u32);
            let segments: Vec<&str> = chain.iter().map(String::as_str).collect();
            binding_class_for_receiver_chain(index, uri, position, &segments)
                .map(ReceiverType::from_raw)
        }
        _ => None,
    }
}

fn infer_contextual_receiver_type(
    index: &Indexer,
    receiver_node: &Node<'_>,
    bytes: &[u8],
    uri: &Url,
    contextual_name: &str,
) -> Option<ReceiverType> {
    let start = receiver_node.start_position();
    let line_start_offsets = line_starts(bytes);
    let utf16_column = ts_byte_col_to_utf16(bytes, &line_start_offsets, start.row, start.column);
    let position = Position::new(start.row as u32, utf16_column as u32);
    infer_receiver_type(
        index,
        ReceiverKind::Contextual {
            name: contextual_name,
            position,
        },
        uri,
    )
}

fn pure_field_chain(receiver_node: &Node<'_>, bytes: &[u8]) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut current = *receiver_node;
    loop {
        if current.kind() == KIND_SIMPLE_IDENT {
            segments.insert(0, current.utf8_text_owned(bytes)?);
            break;
        }
        if current.kind() != KIND_NAV_EXPR {
            return None;
        }
        let named_count = current.named_child_count();
        if named_count < 2 {
            return None;
        }
        let suffix_node = current.named_child(named_count - 1)?;
        if suffix_node.child(0)?.kind() != "." {
            return None;
        }
        let member = suffix_node.first_child_of_kind(KIND_SIMPLE_IDENT)?;
        segments.insert(0, member.utf8_text_owned(bytes)?);
        current = current.named_child(0)?;
    }
    Some(segments)
}

#[cfg(test)]
#[path = "receiver_tests.rs"]
mod tests;
