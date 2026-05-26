use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CLASS_BODY, KIND_CLASS_PARAM, KIND_COLON, KIND_FUN_DECL, KIND_FUN_VALUE_PARAMS,
    KIND_PRIMARY_CTOR, KIND_SIMPLE_IDENT, KIND_SOURCE_FILE,
};

/// A single primary constructor parameter.
pub(crate) struct CtorParam {
    pub name: String,
    pub type_name: String,
    pub is_var: bool,
}

/// Result of finding where to insert generated code into a class.
pub(crate) struct InsertPosition {
    /// The LSP position for the insertion.
    pub pos: Position,
    /// True when the class has no body yet — caller must wrap generated
    /// members in `{ }` and insert at `pos` which is at end-of-declaration.
    pub needs_body: bool,
}

/// An existing method defined in a class: name + raw parameter text for overload disambiguation.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExistingMethod {
    pub name: String,
    pub params: String,
}

/// Walk up from a node to find the nearest ancestor of the given kind.
pub(crate) fn ancestor_of_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    let mut cur = node;
    loop {
        if cur.kind() == kind {
            return Some(cur);
        }
        if cur.kind() == KIND_SOURCE_FILE {
            return None;
        }
        cur = cur.parent()?;
    }
}

/// Extract leading whitespace from the given line.
pub(crate) fn leading_whitespace(content: &str, row: usize) -> String {
    content
        .lines()
        .nth(row)
        .unwrap_or("")
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Collect `val`/`var` parameters from the class's primary constructor,
/// returning each parameter's name, type text, and whether it's declared `var`.
pub(crate) fn extract_primary_ctor_params(
    class_node: tree_sitter::Node,
    bytes: &[u8],
) -> Vec<CtorParam> {
    let ctor = match class_node.first_child_of_kind(KIND_PRIMARY_CTOR) {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut result = Vec::new();
    for cp in ctor.children_of_kind(KIND_CLASS_PARAM) {
        let Some(name_node) = cp.first_child_of_kind(KIND_SIMPLE_IDENT) else {
            continue;
        };
        let name = match name_node.utf8_text(bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => continue,
        };

        let is_var = cp
            .children(&mut cp.walk())
            .any(|c| c.utf8_text(bytes) == Ok("var"));

        let type_text = name_node
            .next_sibling()
            .and_then(|after_name| {
                let mut cur = after_name;
                while cur.kind() == KIND_COLON {
                    cur = cur.next_sibling()?;
                }
                cur.utf8_text(bytes).ok().map(|s| s.to_owned())
            })
            .unwrap_or_default();

        if !type_text.is_empty() {
            result.push(CtorParam {
                name,
                type_name: type_text,
                is_var,
            });
        }
    }
    result
}

/// Return the set of methods already defined in the class body, including their
/// raw parameter text for distinguishing overloaded methods.
pub(crate) fn existing_methods(class_node: tree_sitter::Node, bytes: &[u8]) -> Vec<ExistingMethod> {
    let mut result = Vec::new();
    let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) else {
        return result;
    };
    for child in body.children(&mut body.walk()) {
        if child.kind() == KIND_FUN_DECL {
            let Some(name_node) = child.first_child_of_kind(KIND_SIMPLE_IDENT) else {
                continue;
            };
            let Ok(name) = name_node.utf8_text(bytes) else {
                continue;
            };
            let params = extract_params_text(child, bytes);
            result.push(ExistingMethod {
                name: name.to_owned(),
                params,
            });
        }
    }
    result
}

/// Convenience: return just the names of existing methods (without params).
/// Use `existing_methods()` when you need to distinguish overloaded methods.
pub(crate) fn existing_method_names(class_node: tree_sitter::Node, bytes: &[u8]) -> Vec<String> {
    existing_methods(class_node, bytes)
        .into_iter()
        .map(|m| m.name)
        .collect()
}

/// Extract the parameter text between `(` and `)` from a function_declaration node.
/// Returns an empty string if no parameters node is found.
fn extract_params_text(fun_decl: tree_sitter::Node, bytes: &[u8]) -> String {
    let Some(params_node) = fun_decl.first_child_of_kind(KIND_FUN_VALUE_PARAMS) else {
        return String::new();
    };
    let node_bytes = &bytes[params_node.start_byte()..params_node.end_byte()];
    let Some(open) = node_bytes.iter().position(|&b| b == b'(') else {
        return String::new();
    };
    let mut depth: i32 = 0;
    let mut close = None;
    for (i, &b) in node_bytes[open..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return String::new();
    };
    if let Ok(s) = std::str::from_utf8(&node_bytes[open + 1..close]) {
        return s
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
    }
    String::new()
}

/// Find the position to insert generated methods.
///
/// When the class has a body, inserts at the position of the closing `}`.
/// When no body exists, marks `needs_body = true` so the caller can wrap
/// the generated members in `{ }` at the returned position (end of declaration).
pub(crate) fn find_insert_position(class_node: tree_sitter::Node) -> InsertPosition {
    if let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) {
        let end = body.end_position();
        InsertPosition {
            pos: Position::new(end.row as u32, end.column.saturating_sub(1) as u32),
            needs_body: false,
        }
    } else {
        let end = class_node.end_position();
        InsertPosition {
            pos: Position::new(end.row as u32, end.column as u32),
            needs_body: true,
        }
    }
}

/// Build a `CodeActionOrCommand` with a single `TextEdit` insert at `insert_pos`.
pub(crate) fn make_action(
    title: String,
    new_text: &str,
    insert_pos: Position,
    uri: &Url,
) -> CodeActionOrCommand {
    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(insert_pos, insert_pos),
            new_text: new_text.to_owned(),
        }],
    );

    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}
