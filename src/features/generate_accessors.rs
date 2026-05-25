use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CLASS_BODY, KIND_CLASS_DECL, KIND_CLASS_PARAM, KIND_PRIMARY_CTOR, KIND_SIMPLE_IDENT,
    KIND_SOURCE_FILE, KIND_TYPE_IDENT,
};
use crate::types::Language;

struct CtorParam {
    name: String,
    type_name: String,
    is_var: bool,
}

/// Build "Generate Getter/Setter" code actions for the class at `range`.
///
/// Returns one action per missing getter/setter, plus combined actions
/// ("Generate all getters", "Generate all setters") when multiple apply.
pub(crate) fn build_generate_accessors_action(
    indexer: &Indexer,
    uri: &Url,
    range: Range,
) -> Vec<CodeActionOrCommand> {
    if Language::from_path(uri.path()) != Language::Kotlin {
        return Vec::new();
    }

    let Some(lines) = indexer.mem_lines_for(uri.as_str()) else {
        return Vec::new();
    };
    let content = lines.join("\n");
    let Some(ts_lang) = lang_for_path(uri.path()) else {
        return Vec::new();
    };
    let Some(doc) = parse_live(&content, ts_lang) else {
        return Vec::new();
    };
    let bytes = &doc.bytes;

    let cursor_line = range.start.line as usize;
    let Some(line_text) = lines.get(cursor_line) else {
        return Vec::new();
    };
    let byte_col = utf16_col_to_byte(line_text, range.start.character as usize);
    let point = tree_sitter::Point {
        row: cursor_line,
        column: byte_col,
    };
    let Some(leaf) = doc
        .tree
        .root_node()
        .descendant_for_point_range(point, point)
    else {
        return Vec::new();
    };
    let Some(class_node) = ancestor_of_kind(leaf, KIND_CLASS_DECL) else {
        return Vec::new();
    };

    let params = extract_primary_ctor_params(class_node, bytes);
    if params.is_empty() {
        return Vec::new();
    }

    let existing = existing_method_names(class_node, bytes);

    let Some(class_name_node) = class_node
        .children(&mut class_node.walk())
        .find(|c| c.kind() == KIND_TYPE_IDENT)
    else {
        return Vec::new();
    };
    let Ok(class_name) = class_name_node.utf8_text(bytes) else {
        return Vec::new();
    };
    let indent = leading_whitespace(&content, class_node.start_position().row);
    let Some(insert_pos) = find_insert_position(class_node) else {
        return Vec::new();
    };
    let method_indent = format!("{indent}    ");

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let mut getter_titles: Vec<String> = Vec::new();
    let mut setter_titles: Vec<String> = Vec::new();

    for p in &params {
        let getter = getter_name(&p.name);
        if !existing.contains(&getter) {
            let text = build_getter(p, &method_indent, &indent);
            actions.push(make_action(
                format!("Generate getter `{getter}()` for `{}`", p.name),
                &text,
                insert_pos,
                uri,
            ));
            getter_titles.push(format!("`{getter}()`"));
        }

        if p.is_var {
            let setter = setter_name(&p.name);
            if !existing.contains(&setter) {
                let text = build_setter(p, &method_indent, &indent);
                actions.push(make_action(
                    format!("Generate setter `{setter}()` for `{}`", p.name),
                    &text,
                    insert_pos,
                    uri,
                ));
                setter_titles.push(format!("`{setter}()`"));
            }
        }
    }

    if getter_titles.len() > 1 {
        let mut combined = String::new();
        for p in &params {
            let getter = getter_name(&p.name);
            if !existing.contains(&getter) {
                combined.push_str(&build_getter(p, &method_indent, &indent));
            }
        }
        actions.push(make_action(
            format!("Generate all getters ({}) for `{class_name}`", getter_titles.join(", ")),
            &combined,
            insert_pos,
            uri,
        ));
    }

    if setter_titles.len() > 1 {
        let mut combined = String::new();
        for p in &params {
            if p.is_var {
                let setter = setter_name(&p.name);
                if !existing.contains(&setter) {
                    combined.push_str(&build_setter(p, &method_indent, &indent));
                }
            }
        }
        actions.push(make_action(
            format!("Generate all setters ({}) for `{class_name}`", setter_titles.join(", ")),
            &combined,
            insert_pos,
            uri,
        ));
    }

    actions
}

fn ancestor_of_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
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

fn leading_whitespace(content: &str, row: usize) -> String {
    content
        .lines()
        .nth(row)
        .unwrap_or("")
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

fn extract_primary_ctor_params(class_node: tree_sitter::Node, bytes: &[u8]) -> Vec<CtorParam> {
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

        let is_var = cp.children(&mut cp.walk()).any(|c| c.kind() == "var");

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

fn existing_method_names(class_node: tree_sitter::Node, bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) else {
        return names;
    };
    for child in body.children(&mut body.walk()) {
        if child.kind() == crate::queries::KIND_FUN_DECL {
            if let Some(name_node) = child.first_child_of_kind(KIND_SIMPLE_IDENT) {
                if let Ok(name) = name_node.utf8_text(bytes) {
                    names.push(name.to_owned());
                }
            }
        }
    }
    names
}

fn find_insert_position(class_node: tree_sitter::Node) -> Option<Position> {
    if let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) {
        let end = body.end_position();
        Some(Position::new(end.row as u32 - 1, 0))
    } else {
        let end = class_node.end_position();
        Some(Position::new(end.row as u32, end.column as u32))
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

fn getter_name(name: &str) -> String {
    format!("get{}", capitalize(name))
}

fn setter_name(name: &str) -> String {
    format!("set{}", capitalize(name))
}

fn build_getter(p: &CtorParam, indent: &str, outer_indent: &str) -> String {
    format!(
        "\n{indent}fun {getter}(): {type_name} = {name}\n{outer_indent}",
        indent = indent,
        getter = getter_name(&p.name),
        type_name = p.type_name,
        name = p.name,
        outer_indent = outer_indent,
    )
}

fn build_setter(p: &CtorParam, indent: &str, outer_indent: &str) -> String {
    format!(
        "\n{indent}fun {setter}(value: {type_name}) {{\
         \n{indent}    {name} = value\
         \n{indent}}}\n{outer_indent}",
        indent = indent,
        setter = setter_name(&p.name),
        type_name = p.type_name,
        name = p.name,
        outer_indent = outer_indent,
    )
}

fn make_action(
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

#[cfg(test)]
#[path = "generate_accessors_tests.rs"]
mod tests;
