use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CLASS_BODY, KIND_CLASS_PARAM, KIND_FUN_DECL, KIND_PRIMARY_CTOR, KIND_SIMPLE_IDENT,
    KIND_SOURCE_FILE,
};

/// A single primary constructor parameter.
pub(crate) struct CtorParam {
    pub name: String,
    pub type_name: String,
    pub is_var: bool,
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
                while cur.kind() == ":" {
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

/// Return the set of method names already defined in the class body.
pub(crate) fn existing_method_names(class_node: tree_sitter::Node, bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) else {
        return names;
    };
    for child in body.children(&mut body.walk()) {
        if child.kind() == KIND_FUN_DECL {
            if let Some(name_node) = child.first_child_of_kind(KIND_SIMPLE_IDENT) {
                if let Ok(name) = name_node.utf8_text(bytes) {
                    names.push(name.to_owned());
                }
            }
        }
    }
    names
}

/// Find the position to insert generated methods.
///
/// When the class has a body, inserts on the line before the closing `}`.
/// When no body exists, inserts at the end of the class declaration line.
pub(crate) fn find_insert_position(class_node: tree_sitter::Node) -> Option<Position> {
    if let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) {
        let end = body.end_position();
        Some(Position::new(end.row as u32 - 1, 0))
    } else {
        let end = class_node.end_position();
        Some(Position::new(end.row as u32, end.column as u32))
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
