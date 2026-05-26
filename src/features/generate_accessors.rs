use crate::features::generate_utils::{self, CtorParam};
use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use crate::queries::KIND_TYPE_IDENT;
use crate::types::Language;
use tower_lsp::lsp_types::{CodeActionOrCommand, Range, Url};

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
    let Some(class_node) = generate_utils::ancestor_of_kind(leaf, crate::queries::KIND_CLASS_DECL)
    else {
        return Vec::new();
    };

    let params = generate_utils::extract_primary_ctor_params(class_node, bytes);
    if params.is_empty() {
        return Vec::new();
    }

    let existing = generate_utils::existing_method_names(class_node, bytes);

    let Some(class_name_node) = class_node
        .children(&mut class_node.walk())
        .find(|c| c.kind() == KIND_TYPE_IDENT)
    else {
        return Vec::new();
    };
    let Ok(class_name) = class_name_node.utf8_text(bytes) else {
        return Vec::new();
    };
    let indent = generate_utils::leading_whitespace(&content, class_node.start_position().row);
    let insert_pos = generate_utils::find_insert_position(class_node);
    let method_indent = format!("{indent}    ");

    let wrap_if_needed = |text: String| -> String {
        if insert_pos.needs_body {
            format!(" {{{}}}\n{indent}", text)
        } else {
            text
        }
    };

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let mut getter_titles: Vec<String> = Vec::new();
    let mut setter_titles: Vec<String> = Vec::new();

    for p in &params {
        let getter = getter_name(&p.name);
        if !existing.contains(&getter) {
            actions.push(generate_utils::make_action(
                format!("Generate getter `{getter}()` for `{}`", p.name),
                &wrap_if_needed(build_getter(p, &method_indent, &indent)),
                insert_pos.pos,
                uri,
            ));
            getter_titles.push(format!("`{getter}()`"));
        }

        if p.is_var {
            let setter = setter_name(&p.name);
            if !existing.contains(&setter) {
                actions.push(generate_utils::make_action(
                    format!("Generate setter `{setter}()` for `{}`", p.name),
                    &wrap_if_needed(build_setter(p, &method_indent, &indent)),
                    insert_pos.pos,
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
        actions.push(generate_utils::make_action(
            format!(
                "Generate all getters ({}) for `{class_name}`",
                getter_titles.join(", ")
            ),
            &wrap_if_needed(combined),
            insert_pos.pos,
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
        actions.push(generate_utils::make_action(
            format!(
                "Generate all setters ({}) for `{class_name}`",
                setter_titles.join(", ")
            ),
            &wrap_if_needed(combined),
            insert_pos.pos,
            uri,
        ));
    }

    actions
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

#[cfg(test)]
#[path = "generate_accessors_tests.rs"]
mod tests;
