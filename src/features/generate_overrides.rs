use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, SymbolKind, TextEdit, Url,
    WorkspaceEdit,
};

use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CLASS_BODY, KIND_CLASS_DECL, KIND_FUN_DECL, KIND_SIMPLE_IDENT, KIND_SOURCE_FILE,
    KIND_TYPE_IDENT,
};
use crate::types::{FileData, Language, Visibility};

/// Build "Implement methods" / "Override methods" code actions.
///
/// When the cursor is on a class declaration, walks the supertype hierarchy
/// and offers to generate `override fun` stubs for public/protected methods
/// not yet present in the current class.
pub(crate) fn build_generate_overrides_action(
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

    let Some(class_name_node) = class_node
        .children(&mut class_node.walk())
        .find(|c| c.kind() == KIND_TYPE_IDENT)
    else {
        return Vec::new();
    };
    let Ok(class_name) = class_name_node.utf8_text(bytes) else {
        return Vec::new();
    };
    let class_name = class_name.to_owned();

    let existing = existing_method_names(class_node, bytes);

    let indent = leading_whitespace(&content, class_node.start_position().row);
    let Some(insert_pos) = find_insert_position(class_node) else {
        return Vec::new();
    };
    let method_indent = format!("{indent}    ");

    let overridable = collect_overridable_methods(indexer, uri.as_str(), &class_name);
    if overridable.is_empty() {
        return Vec::new();
    }

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let mut titles: Vec<String> = Vec::new();

    for m in &overridable {
        if existing.contains(&m.name) {
            continue;
        }
        let text = build_override_stub(m, &method_indent, &indent);
        actions.push(make_action(
            format!("Override `{}()` for `{class_name}`", m.name),
            &text,
            insert_pos,
            uri,
        ));
        titles.push(format!("`{}()`", m.name));
    }

    if titles.len() > 1 {
        let mut combined = String::new();
        for m in &overridable {
            if existing.contains(&m.name) {
                continue;
            }
            combined.push_str(&build_override_stub(m, &method_indent, &indent));
        }
        actions.push(make_action(
            format!("Override all ({}) for `{class_name}`", titles.join(", ")),
            &combined,
            insert_pos,
            uri,
        ));
    }

    actions
}

/// A method from a supertype that can be overridden.
struct OverridableMethod {
    name: String,
    /// Full signature from `SymbolEntry.detail`, e.g. `"fun toString(): String"`.
    detail: String,
}

/// Collect all public/protected non-private methods from the supertype hierarchy
/// of `class_name` (at `uri_str`), including transitive supertypes.
fn collect_overridable_methods(
    indexer: &Indexer,
    uri_str: &str,
    class_name: &str,
) -> Vec<OverridableMethod> {
    let mut visited = std::collections::HashSet::new();
    let mut result = Vec::new();
    let mut queue = vec![(class_name.to_owned(), uri_str.to_owned())];

    while let Some((name, uri)) = queue.pop() {
        if !visited.insert((name.clone(), uri.clone())) {
            continue;
        }

        let Some(file_data) = indexer.file_data_for(&uri) else {
            continue;
        };

        let found_line = file_data
            .symbols
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.selection_start());

        let Some(class_line) = found_line else {
            continue;
        };

        for (super_line, super_name, _) in &file_data.supers {
            if *super_line != class_line {
                continue;
            }

            for loc in indexer.definition_locations(super_name).iter() {
                let super_uri = loc.uri.as_str().to_owned();
                queue.push((super_name.clone(), super_uri));
            }

            for loc in indexer.definition_locations(super_name).iter() {
                if let Some(super_data) = indexer.file_data_for(loc.uri.as_str()) {
                    collect_methods_from(&super_data, super_name, &mut result);
                }
            }
        }
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    result.dedup_by(|a, b| a.name == b.name);
    result
}

/// Extract public/protected method symbols from `file_data` that belong to `class_name`.
fn collect_methods_from(
    file_data: &Arc<FileData>,
    class_name: &str,
    out: &mut Vec<OverridableMethod>,
) {
    for sym in &file_data.symbols {
        if sym.kind != SymbolKind::METHOD && sym.kind != SymbolKind::FUNCTION {
            continue;
        }
        if sym.container.as_deref() != Some(class_name) {
            continue;
        }
        if sym.visibility == Visibility::Private {
            continue;
        }
        out.push(OverridableMethod {
            name: sym.name.clone(),
            detail: sym.detail.clone(),
        });
    }
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

fn find_insert_position(class_node: tree_sitter::Node) -> Option<Position> {
    if let Some(body) = class_node.first_child_of_kind(KIND_CLASS_BODY) {
        let end = body.end_position();
        Some(Position::new(end.row as u32 - 1, 0))
    } else {
        let end = class_node.end_position();
        Some(Position::new(end.row as u32, end.column as u32))
    }
}

fn build_override_stub(m: &OverridableMethod, indent: &str, outer_indent: &str) -> String {
    format!(
        "\n{indent}override {detail} {{\
         \n{indent}    TODO(\"Not yet implemented\")\
         \n{indent}}}\n{outer_indent}",
        indent = indent,
        detail = m.detail,
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
#[path = "generate_overrides_tests.rs"]
mod tests;
