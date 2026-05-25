//! Generate constructor / primary properties for classes extending a supertype.
//!
//! When the cursor is on a class declaration that extends a supertype (e.g.,
//! `class Foo : Bar`), offers a code action to generate the primary constructor
//! parameters based on the supertype's constructor.

use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Location, Position, Range, SymbolKind,
    TextEdit, Url, WorkspaceEdit,
};

use crate::indexer::live_tree::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CLASS_DECL, KIND_CONSTRUCTOR_INVOCATION, KIND_DELEGATION_SPEC, KIND_PRIMARY_CTOR,
    KIND_SOURCE_FILE, KIND_TYPE_IDENT, KIND_USER_TYPE,
};
use crate::resolver::{ensure_file_data, resolve_symbol_inner};
use crate::types::Language;

/// Try to build a "Generate constructor" code action for the cursor position.
///
/// Returns `None` if the cursor is not on a class declaration with a supertype
/// whose constructor can be resolved, or the class already has a constructor.
pub(crate) fn build_generate_constructor_action(
    indexer: &Indexer,
    uri: &Url,
    range: Range,
) -> Option<CodeActionOrCommand> {
    if Language::from_path(uri.path()) != Language::Kotlin {
        return None;
    }

    let lines = indexer.mem_lines_for(uri.as_str())?;
    let content = lines.join("\n");
    let ts_lang = lang_for_path(uri.path())?;
    let doc = parse_live(&content, ts_lang)?;
    let bytes = &doc.bytes;

    // Find class declaration at cursor
    let cursor_line = range.start.line as usize;
    let line_text = lines.get(cursor_line)?;
    let byte_col = utf16_col_to_byte(line_text, range.start.character as usize);
    let point = tree_sitter::Point {
        row: cursor_line,
        column: byte_col,
    };
    let leaf = doc
        .tree
        .root_node()
        .descendant_for_point_range(point, point)?;
    let class_node = ancestor_of_kind(leaf, KIND_CLASS_DECL)?;

    // Skip if class already has a primary constructor
    if has_child_of_kind(class_node, KIND_PRIMARY_CTOR) {
        return None;
    }

    // Find delegation specifier (the `: SuperType` part)
    let spec = class_node
        .children(&mut class_node.walk())
        .find(|c| c.kind() == KIND_DELEGATION_SPEC)?;

    // Extract supertype name; skip if already has constructor invocation args
    let (super_name, _) = spec.super_from_delegation(bytes)?;
    if has_child_of_kind(spec, KIND_CONSTRUCTOR_INVOCATION) {
        return None;
    }

    // Get class name
    let class_name = class_node
        .children(&mut class_node.walk())
        .find(|c| c.kind() == KIND_TYPE_IDENT)?
        .utf8_text(bytes)
        .ok()?
        .to_owned();

    // Resolve supertype's constructor params from the indexed file data
    let super_ctor_params = resolve_supertype_ctor_params(indexer, &super_name, uri)?;
    if super_ctor_params.is_empty() {
        return None;
    }

    // Build param entries: (name, type_text)
    let params: Vec<(&str, &str)> = super_ctor_params
        .iter()
        .filter_map(|p| parse_param(p))
        .collect();
    if params.is_empty() {
        return None;
    }

    // ── generate edit ───────────────────────────────────────────────────────
    let indent = class_indent(&content, class_node.start_position().row);
    let param_indent = format!("{indent}    ");

    let mut param_text: String = String::new();
    for (i, (name, ty)) in params.iter().enumerate() {
        let comma = if i + 1 < params.len() { "," } else { "" };
        param_text.push_str(&format!("{param_indent}val {name}: {ty}{comma}\n"));
    }

    let args: Vec<&str> = params.iter().map(|(n, _)| *n).collect();
    let ctor_block = format!("(\n{param_text}{indent})");

    // Position: after class name → insert primary constructor
    let name_end = class_node
        .children(&mut class_node.walk())
        .find(|c| c.kind() == KIND_TYPE_IDENT)?
        .end_position();

    // Position: after supertype name in delegation spec → add `(args)`
    let user_type_end = spec
        .children(&mut spec.walk())
        .find(|c| c.kind() == KIND_USER_TYPE)?
        .end_position();

    let name_lsp = Position::new(name_end.row as u32, name_end.column as u32);
    let type_lsp = Position::new(user_type_end.row as u32, user_type_end.column as u32);

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            TextEdit {
                range: Range::new(name_lsp, name_lsp),
                new_text: format!(" {ctor_block} "),
            },
            TextEdit {
                range: Range::new(type_lsp, type_lsp),
                new_text: format!("({})", args.join(", ")),
            },
        ],
    );

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Generate constructor `{class_name}({})`", args.join(", ")),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Resolve constructor parameter text from the supertype's definition.
///
/// Tries the definitions index first (fast path), then falls back to the full
/// resolver chain (handles same-file, not-yet-indexed files, etc.).
/// Uses `ensure_file_data` to read from disk when the file is not in the index.
fn resolve_supertype_ctor_params(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
) -> Option<Vec<String>> {
    // Phase 1: definitions index (fast path)
    let locs = indexer.definition_locations(name);
    if !locs.is_empty() {
        if let Some(params) = ctor_params_from_locs(indexer, name, &locs, from_uri) {
            return Some(params);
        }
    }

    // Phase 2: full resolver chain (handles same-file, unindexed files, rg, etc.)
    let locs = resolve_symbol_inner(indexer, name, from_uri, false);
    if !locs.is_empty() {
        if let Some(params) = ctor_params_from_locs(indexer, name, &locs, from_uri) {
            return Some(params);
        }
    }

    None
}

/// Extract constructor params from a set of candidate locations.
///
/// Prefers the location in the same package as `from_uri`. Falls back to
/// reading from disk if the target file is not in the in-memory index.
fn ctor_params_from_locs(
    indexer: &Indexer,
    name: &str,
    locs: &[Location],
    from_uri: &Url,
) -> Option<Vec<String>> {
    let current_pkg = indexer.package_of(from_uri);

    let target = current_pkg
        .as_ref()
        .and_then(|pkg| {
            locs.iter().find(|loc| {
                indexer
                    .package_of(&loc.uri)
                    .as_ref()
                    .is_some_and(|p| p == pkg)
            })
        })
        .unwrap_or(&locs[0]);

    let file_data = indexer
        .file_data_for(target.uri.as_str())
        .or_else(|| ensure_file_data(indexer, &target.uri))?;

    let class_sym = file_data.symbols.iter().find(|s| {
        s.name == name
            && matches!(
                s.kind,
                SymbolKind::CLASS
                    | SymbolKind::INTERFACE
                    | SymbolKind::STRUCT
                    | SymbolKind::ENUM
                    | SymbolKind::OBJECT
            )
    })?;

    let params_text = class_sym.params.as_str();
    if params_text.is_empty() {
        return None;
    }

    Some(split_params(params_text))
}

/// Split comma-separated parameter text into individual param strings,
/// respecting nesting depth to avoid splitting on commas inside generics.
fn split_params(text: &str) -> Vec<String> {
    let mut depth = 0u8;
    let mut start = 0usize;
    let mut result = Vec::new();
    for (i, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let p = text[start..i].trim();
                if !p.is_empty() {
                    result.push(p.to_owned());
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        result.push(last.to_owned());
    }
    result
}

/// Extract `(name, type)` from a single param string.
///
/// Handles forms:
/// - `"val name: Type"` → `("name", "Type")`
/// - `"name: Type"` → `("name", "Type")`
/// - `"vararg name: Type"` → `("name", "Type")`
/// - `"crossinline name: Type"` → `("name", "Type")`
fn parse_param(param: &str) -> Option<(&str, &str)> {
    let param = param.trim();
    // Strip optional val/var/vararg/crossinline/noinline/open modifiers
    let after_mod = param
        .strip_prefix("val ")
        .or_else(|| param.strip_prefix("var "))
        .or_else(|| param.strip_prefix("vararg "))
        .or_else(|| param.strip_prefix("crossinline "))
        .or_else(|| param.strip_prefix("noinline "))
        .or_else(|| param.strip_prefix("open "))
        .unwrap_or(param);
    let colon = after_mod.find(':')?;
    let name = after_mod[..colon].trim();
    let ty = after_mod[colon + 1..].trim();
    // Strip default value after `=`
    let eq = ty.find("= ").or_else(|| ty.find('='));
    let ty = eq.map(|i| ty[..i].trim()).unwrap_or(ty);
    if name.is_empty() || ty.is_empty() {
        return None;
    }
    Some((name, ty))
}

/// Walk up from a node to find the nearest ancestor of the given kind.
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

fn has_child_of_kind(node: tree_sitter::Node, kind: &str) -> bool {
    node.children(&mut node.walk()).any(|c| c.kind() == kind)
}

/// Detect the indentation string for a given row (leading whitespace).
fn class_indent(content: &str, row: usize) -> String {
    let line = content.lines().nth(row).unwrap_or("");
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

#[cfg(test)]
#[path = "generate_constructor_tests.rs"]
mod tests;
