use std::sync::Arc;

use tower_lsp::lsp_types::{CodeActionOrCommand, Range, SymbolKind, Url};

use crate::features::generate_utils;
use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use crate::queries::{KIND_CLASS_DECL, KIND_TYPE_IDENT};
use crate::types::{FileData, Language, Visibility};

const CLASS_LIKE_SYMBOLS: &[SymbolKind] = &[
    SymbolKind::CLASS,
    SymbolKind::INTERFACE,
    SymbolKind::STRUCT,
    SymbolKind::ENUM,
    SymbolKind::OBJECT,
];

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
    let Some(class_node) = generate_utils::ancestor_of_kind(leaf, KIND_CLASS_DECL) else {
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

    let existing = generate_utils::existing_methods(class_node, bytes);

    let indent = generate_utils::leading_whitespace(&content, class_node.start_position().row);
    let insert_pos = generate_utils::find_insert_position(class_node);
    let method_indent = format!("{indent}    ");

    let overridable = collect_overridable_methods(indexer, uri, &class_name);
    if overridable.is_empty() {
        return Vec::new();
    }

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let mut titles: Vec<String> = Vec::new();

    for m in &overridable {
        if existing
            .iter()
            .any(|e| e.name == m.name && e.params == m.params)
        {
            continue;
        }
        let stub_text = build_override_stub(m, &method_indent, &indent);
        let final_text = if insert_pos.needs_body {
            format!(" {{{}}}\n{indent}", stub_text)
        } else {
            stub_text
        };
        actions.push(generate_utils::make_action(
            format!("Override `{}()` for `{class_name}`", m.name),
            &final_text,
            insert_pos.pos,
            uri,
        ));
        titles.push(format!("`{}()`", m.name));
    }

    if titles.len() > 1 {
        let mut combined = String::new();
        for m in &overridable {
            if existing
                .iter()
                .any(|e| e.name == m.name && e.params == m.params)
            {
                continue;
            }
            combined.push_str(&build_override_stub(m, &method_indent, &indent));
            combined.push('\n');
        }
        let final_combined = if insert_pos.needs_body {
            format!(" {{{}}}\n{indent}", combined)
        } else {
            combined
        };
        actions.push(generate_utils::make_action(
            format!("Override all ({}) for `{class_name}`", titles.join(", ")),
            &final_combined,
            insert_pos.pos,
            uri,
        ));
    }

    actions
}

struct OverridableMethod {
    name: String,
    detail: String,
    params: String,
}

/// Collect all public/protected non-private methods from the supertype hierarchy
/// of `class_name` (at `uri`), including transitive supertypes.
fn collect_overridable_methods(
    indexer: &Indexer,
    uri: &Url,
    class_name: &str,
) -> Vec<OverridableMethod> {
    let mut visited = std::collections::HashSet::new();
    let mut result = Vec::new();
    let mut queue = vec![(class_name.to_owned(), uri.clone())];

    while let Some((name, uri)) = queue.pop() {
        if !visited.insert((name.clone(), uri.to_string())) {
            continue;
        }

        let Some(file_data) = indexer.file_data_for(uri.as_str()) else {
            continue;
        };

        let found_line = file_data
            .symbols
            .iter()
            .find(|s| CLASS_LIKE_SYMBOLS.contains(&s.kind) && s.name == name)
            .map(|s| (s.selection_start(), s.kind));

        let Some((class_line, _parent_kind)) = found_line else {
            continue;
        };

        for (super_line, super_name, _) in &file_data.supers {
            if *super_line != class_line {
                continue;
            }

            for loc in indexer.definition_locations(super_name).iter() {
                queue.push((super_name.clone(), loc.uri.clone()));

                if let Some(super_data) = indexer.file_data_for(loc.uri.as_str()) {
                    let super_kind = super_data
                        .symbols
                        .iter()
                        .find(|s| CLASS_LIKE_SYMBOLS.contains(&s.kind) && s.name == *super_name)
                        .map(|s| s.kind)
                        .unwrap_or(SymbolKind::CLASS);
                    collect_methods_from(&super_data, super_name, super_kind, &mut result);
                }
            }
        }
    }

    result.sort_by(|a, b| a.name.cmp(&b.name).then(a.params.cmp(&b.params)));
    result.dedup_by(|a, b| a.name == b.name && a.params == b.params);
    result
}

/// Extract public/protected overridable method symbols from `file_data` that
/// belong to `class_name`. Only includes methods that can actually be overridden
/// in Kotlin: interface members, or concrete methods marked `open`/`abstract`.
/// Excludes `internal` visibility (only Public | Protected eligible).
fn collect_methods_from(
    file_data: &Arc<FileData>,
    class_name: &str,
    parent_kind: SymbolKind,
    out: &mut Vec<OverridableMethod>,
) {
    let is_parent_interface = parent_kind == SymbolKind::INTERFACE;

    for sym in &file_data.symbols {
        if sym.kind != SymbolKind::METHOD && sym.kind != SymbolKind::FUNCTION {
            continue;
        }
        if sym.container.as_deref() != Some(class_name) {
            continue;
        }
        if sym.visibility != Visibility::Public && sym.visibility != Visibility::Protected {
            continue;
        }
        if !is_parent_interface && !is_overridable_modifier(&sym.detail) {
            continue;
        }
        out.push(OverridableMethod {
            name: sym.name.clone(),
            detail: sym.detail.clone(),
            params: sym.params.clone(),
        });
    }
}

/// Check if the detail string indicates this is an `open` or `abstract` method.
/// In Kotlin, methods are final by default and need explicit modifier to be overridable.
fn is_overridable_modifier(detail: &str) -> bool {
    let padded = format!(" {detail} ");
    padded.contains(" open ") || padded.contains(" abstract ")
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

#[cfg(test)]
#[path = "generate_overrides_tests.rs"]
mod tests;
