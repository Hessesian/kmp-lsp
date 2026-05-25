//! Generate `toString()`, `equals()`, and `hashCode()` for Kotlin classes.
//!
//! When the cursor is on a class declaration with primary constructor parameters,
//! offers code actions to generate the standard `Any` overrides based on those
//! parameters.  Returns one `CodeAction` per missing method, plus an "all" action
//! when multiple methods are missing.

use crate::features::generate_utils::{self, CtorParam};
use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use crate::queries::KIND_TYPE_IDENT;
use crate::types::Language;
use tower_lsp::lsp_types::{CodeActionOrCommand, Range, Url};

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
    let class_name = class_name.to_owned();

    let indent = generate_utils::leading_whitespace(&content, class_node.start_position().row);
    let Some(insert_pos) = generate_utils::find_insert_position(class_node) else {
        return Vec::new();
    };
    let method_indent = format!("{indent}    ");

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let mut titles: Vec<String> = Vec::new();

    if !existing.iter().any(|s| s == "toString") {
        actions.push(generate_utils::make_action(
            format!("Generate `toString()` for `{class_name}`"),
            &build_to_string(&params, &class_name, &method_indent, &indent),
            insert_pos,
            uri,
        ));
        titles.push("Generate `toString()`".to_owned());
    }

    if !existing.iter().any(|s| s == "equals") {
        actions.push(generate_utils::make_action(
            format!("Generate `equals()` for `{class_name}`"),
            &build_equals(&params, &class_name, &method_indent, &indent),
            insert_pos,
            uri,
        ));
        titles.push("Generate `equals()`".to_owned());
    }

    if !existing.iter().any(|s| s == "hashCode") {
        actions.push(generate_utils::make_action(
            format!("Generate `hashCode()` for `{class_name}`"),
            &build_hash_code(&params, &class_name, &method_indent, &indent),
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
            combined_text.push('\n');
        }
        if !existing.iter().any(|s| s == "equals") {
            combined_text.push_str(&build_equals(&params, &class_name, &method_indent, &indent));
            combined_text.push('\n');
        }
        if !existing.iter().any(|s| s == "hashCode") {
            combined_text.push_str(&build_hash_code(
                &params,
                &class_name,
                &method_indent,
                &indent,
            ));
        }
        actions.push(generate_utils::make_action(
            format!("Generate all ({combined}) for `{class_name}`"),
            &combined_text,
            insert_pos,
            uri,
        ));
    }

    actions
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

#[cfg(test)]
#[path = "generate_equals_tests.rs"]
mod tests;
