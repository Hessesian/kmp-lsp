//! ViewBinding type inference heuristics (`by viewBinding<>()`, `FooBinding.inflate()`).

use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::indexer::Indexer;
use crate::indexer::NodeExt;
use crate::queries::{KIND_CALL_EXPR, KIND_NAV_EXPR, KIND_SIMPLE_IDENT};
use crate::resolver::infer_lines::extract_type_with_generics;
use crate::viewbinding::{binding_field_type, is_view_binding_class_name};
use crate::StrExt;

/// Infer a binding field type when `class_name` is a generated `*Binding` class.
pub(crate) fn binding_field_type_in_class(
    indexer: &Indexer,
    class_name: &str,
    field_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    if !is_view_binding_class_name(class_name) {
        return None;
    }
    binding_field_type(indexer, from_uri, class_name, field_name)
}

/// Infer a `*Binding` type from a `by viewBinding<…>()` or `by viewBinding(…::inflate/bind)` delegate.
pub(crate) fn infer_view_binding_delegate_type(line: &str, var_name: &str) -> Option<String> {
    let delegate_pattern = format!("{var_name} by viewBinding");
    if !line_contains_word_boundary(line, &delegate_pattern) {
        return None;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
        return None;
    }
    let view_binding_pos = line.find("viewBinding")?;
    let after_view_binding = &line[view_binding_pos + "viewBinding".len()..];
    extract_binding_type_from_view_binding_delegate(after_view_binding)
}

/// Resolve a ViewBinding delegate type from a property node's source text.
pub(crate) fn view_binding_delegate_type_from_property(
    property_node: Node<'_>,
    bytes: &[u8],
    var_name: &str,
) -> Option<String> {
    let property_text = property_node.utf8_text_owned(bytes)?;
    for line in property_text.lines() {
        if let Some(binding_type) = infer_view_binding_delegate_type(line, var_name) {
            return Some(binding_type);
        }
    }
    None
}

/// Infer `FooBinding` from a `FooBinding.inflate(...)` call expression.
pub(crate) fn binding_type_from_inflate_call(call_node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let callee_text = call_node.utf8_text_owned(bytes)?;
    let inflate_pos = callee_text.find("Binding.inflate")?;
    let prefix = &callee_text[..inflate_pos + "Binding".len()];
    let binding_name = prefix
        .rsplit(|character: char| !character.is_alphanumeric() && character != '_')
        .next()
        .filter(|name| is_view_binding_class_name(name))?;
    Some(binding_name.to_string())
}

/// Infer binding type from a call-expression initializer node when applicable.
pub(crate) fn binding_type_from_initializer_node(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() != KIND_CALL_EXPR {
        return None;
    }
    let callee = node.first_child_of_kind(KIND_NAV_EXPR).or_else(|| {
        node.children(&mut node.walk())
            .find(|child| child.kind() == KIND_SIMPLE_IDENT)
    })?;
    let callee_name = callee.utf8_text_owned(bytes)?;
    if is_view_binding_class_name(&callee_name) {
        return Some(callee_name);
    }
    binding_type_from_inflate_call(node, bytes)
}

fn extract_binding_type_from_view_binding_delegate(after_view_binding: &str) -> Option<String> {
    let trimmed = after_view_binding.trim_start();

    if let Some(generic_args) = trimmed.strip_prefix('<') {
        let type_name = extract_type_with_generics(generic_args);
        if type_name.ends_with("Binding")
            && type_name.len() > "Binding".len()
            && type_name[..type_name.len() - "Binding".len()]
                .chars()
                .next()
                .is_some_and(|character| character.is_uppercase())
        {
            return Some(type_name);
        }
    }

    if let Some(inside) = trimmed.strip_prefix('(') {
        let inside = inside.trim_start();
        let colon_pos = inside.find("::")?;
        let before_reference = inside[..colon_pos].trim();
        let binding_name = before_reference.dotted_ident_prefix();
        let base = binding_name.last_segment().trim();
        if base.ends_with("Binding")
            && base.len() > "Binding".len()
            && base[..base.len() - "Binding".len()]
                .chars()
                .next()
                .is_some_and(|character| character.is_uppercase())
        {
            return Some(base.to_owned());
        }
    }

    None
}

fn line_contains_word_boundary(line: &str, pattern: &str) -> bool {
    let mut search_start = 0;
    while let Some(relative) = line[search_start..].find(pattern) {
        let start = search_start + relative;
        let end = start + pattern.len();
        let before_ok = start == 0
            || !line.as_bytes()[start - 1].is_ascii_alphanumeric()
                && line.as_bytes()[start - 1] != b'_';
        let after_ok = end >= line.len()
            || !line.as_bytes()[end].is_ascii_alphanumeric() && line.as_bytes()[end] != b'_';
        if before_ok && after_ok {
            return true;
        }
        search_start = end;
    }
    false
}
