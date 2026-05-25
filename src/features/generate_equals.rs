//! Generate `toString()`, `equals()`, and `hashCode()` for Kotlin classes.
//!
//! When the cursor is on a class declaration with primary constructor parameters,
//! offers code actions to generate the standard `Any` overrides based on those
//! parameters.  Returns one `CodeAction` per missing method, plus an "all" action
//! when multiple methods are missing.

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CLASS_BODY, KIND_CLASS_DECL, KIND_CLASS_PARAM, KIND_FUN_DECL, KIND_PRIMARY_CTOR,
    KIND_SIMPLE_IDENT, KIND_SOURCE_FILE,
};
use crate::types::Language;

struct CtorParam {
    name: String,
    type_name: String,
}

/// Build "Generate toString / equals / hashCode" code actions for the class at `range`.
///
/// Returns an empty vec when the cursor is not on a suitable class, the class
/// has no primary constructor parameters, or all three methods already exist.
pub(crate) fn build_generate_equals_action(
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
        .find(|c| c.kind() == crate::queries::KIND_TYPE_IDENT)
    else {
        return Vec::new();
    };
    let Ok(class_name) = class_name_node.utf8_text(bytes) else {
        return Vec::new();
    };
    let class_name = class_name.to_owned();

    let indent = leading_whitespace(&content, class_node.start_position().row);
    let Some(insert_pos) = find_insert_position(class_node) else {
        return Vec::new();
    };
    let method_indent = format!("{indent}    ");

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let mut titles: Vec<String> = Vec::new();

    if !existing.iter().any(|s| s == "toString") {
        let text = build_to_string(&params, &class_name, &method_indent, &indent);
        actions.push(make_action(
            format!("Generate `toString()` for `{class_name}`"),
            &text,
            insert_pos,
            uri,
        ));
        titles.push("Generate `toString()`".to_owned());
    }

    if !existing.iter().any(|s| s == "equals") {
        let text = build_equals(&params, &class_name, &method_indent, &indent);
        actions.push(make_action(
            format!("Generate `equals()` for `{class_name}`"),
            &text,
            insert_pos,
            uri,
        ));
        titles.push("Generate `equals()`".to_owned());
    }

    if !existing.iter().any(|s| s == "hashCode") {
        let text = build_hash_code(&params, &class_name, &method_indent, &indent);
        actions.push(make_action(
            format!("Generate `hashCode()` for `{class_name}`"),
            &text,
            insert_pos,
            uri,
        ));
        titles.push("Generate `hashCode()`".to_owned());
    }

    if titles.len() > 1 {
        let combined = titles.join(", ");
        let mut combined_text = String::new();
        if !existing.iter().any(|s| s == "toString") {
            combined_text.push_str(&build_to_string(
                &params,
                &class_name,
                &method_indent,
                &indent,
            ));
        }
        if !existing.iter().any(|s| s == "equals") {
            combined_text.push_str(&build_equals(&params, &class_name, &method_indent, &indent));
        }
        if !existing.iter().any(|s| s == "hashCode") {
            combined_text.push_str(&build_hash_code(
                &params,
                &class_name,
                &method_indent,
                &indent,
            ));
        }
        actions.push(make_action(
            format!("Generate all ({combined}) for `{class_name}`"),
            &combined_text,
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

/// Extract leading whitespace from the given line.
fn leading_whitespace(content: &str, row: usize) -> String {
    content
        .lines()
        .nth(row)
        .unwrap_or("")
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Collect `val`/`var` parameters from the class's primary constructor,
/// returning each parameter's name and the raw type text as written in source.
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
            });
        }
    }
    result
}

/// Return the set of method names already defined in the class body.
fn existing_method_names(class_node: tree_sitter::Node, bytes: &[u8]) -> Vec<String> {
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
fn find_insert_position(class_node: tree_sitter::Node) -> Option<Position> {
    if let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) {
        let end = body.end_position();
        Some(Position::new(end.row as u32 - 1, 0))
    } else {
        let end = class_node.end_position();
        Some(Position::new(end.row as u32, end.column as u32))
    }
}

fn build_to_string(
    params: &[CtorParam],
    class_name: &str,
    indent: &str,
    outer_indent: &str,
) -> String {
    let parts: Vec<String> = params
        .iter()
        .map(|p| format!("{}=${{{}}}", p.name, p.name))
        .collect();
    let inner = parts.join(", ");
    format!(
        "\n{indent}override fun toString(): String = \"{class_name}({inner})\"\n{outer_indent}",
        indent = indent,
        class_name = class_name,
        inner = inner,
        outer_indent = outer_indent,
    )
}

fn build_equals(
    params: &[CtorParam],
    class_name: &str,
    indent: &str,
    outer_indent: &str,
) -> String {
    let conditions: Vec<String> = params
        .iter()
        .map(|p| {
            format!(
                "{indent}    {lhs} == other.{rhs}",
                lhs = p.name,
                rhs = p.name
            )
        })
        .collect();
    let cond_body = if conditions.is_empty() {
        String::from("true")
    } else {
        conditions.join(" &&\n")
    };
    format!(
        "\n{indent}override fun equals(other: Any?): Boolean {{\
         \n{indent}    if (this === other) return true\
         \n{indent}    if (other == null || this::class != other::class) return false\
         \n{indent}    other as {class_name}\
         \n{indent}    return {cond_body}\
         \n{indent}}}\n{outer_indent}",
        indent = indent,
        class_name = class_name,
        cond_body = cond_body,
        outer_indent = outer_indent,
    )
}

fn build_hash_code(
    params: &[CtorParam],
    _class_name: &str,
    indent: &str,
    outer_indent: &str,
) -> String {
    let mut lines = Vec::new();
    for (i, p) in params.iter().enumerate() {
        // Use safe call `.hashCode() ?: 0` for nullable types to avoid NPE.
        let rhs = if p.type_name.ends_with('?') {
            format!("{}?.hashCode() ?: 0", p.name)
        } else {
            format!("{}.hashCode()", p.name)
        };
        if i == 0 {
            lines.push(format!("{indent}    var result = {rhs}"));
        } else {
            lines.push(format!("{indent}    result = 31 * result + {rhs}"));
        }
    }
    let body = if lines.is_empty() {
        format!("{indent}    return 0")
    } else {
        lines.join("\n")
    };
    format!(
        "\n{indent}override fun hashCode(): Int {{\
         \n{body}\
         \n{indent}    return result\
         \n{indent}}}\n{outer_indent}",
        indent = indent,
        body = body,
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
#[path = "generate_equals_tests.rs"]
mod tests;
