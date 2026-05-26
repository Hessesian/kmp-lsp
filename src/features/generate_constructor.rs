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

    let document_lines = indexer.mem_lines_for(uri.as_str())?;
    let document_content = document_lines.join("\n");
    let tree_sitter_language = lang_for_path(uri.path())?;
    let parsed_document = parse_live(&document_content, tree_sitter_language)?;
    let document_bytes = &parsed_document.bytes;

    // Find class declaration at cursor
    let cursor_line = range.start.line as usize;
    let line_text = document_lines.get(cursor_line)?;
    let byte_column = utf16_col_to_byte(line_text, range.start.character as usize);
    let point = tree_sitter::Point {
        row: cursor_line,
        column: byte_column,
    };
    let leaf_node = parsed_document
        .tree
        .root_node()
        .descendant_for_point_range(point, point)?;
    let class_node = ancestor_of_kind(leaf_node, KIND_CLASS_DECL)?;

    // Skip if class already has a primary constructor
    if has_child_of_kind(class_node, KIND_PRIMARY_CTOR) {
        return None;
    }

    // Find delegation specifier (the `: SuperType` part)
    let delegation_specifier = class_node
        .children(&mut class_node.walk())
        .find(|child| child.kind() == KIND_DELEGATION_SPEC)?;

    // Extract supertype name and concrete type arguments; skip if already has constructor invocation arguments
    let (supertype_name, type_arguments) =
        delegation_specifier.super_from_delegation(document_bytes)?;
    if has_child_of_kind(delegation_specifier, KIND_CONSTRUCTOR_INVOCATION) {
        return None;
    }

    // Get class name
    let class_name = class_node
        .children(&mut class_node.walk())
        .find(|child| child.kind() == KIND_TYPE_IDENT)?
        .utf8_text(document_bytes)
        .ok()?
        .to_owned();

    // Resolve supertype's constructor parameters and formal type parameters from the indexed file data
    let (supertype_constructor_parameters, formal_type_parameters) =
        resolve_supertype_constructor_parameters(indexer, &supertype_name, uri)?;
    if supertype_constructor_parameters.is_empty() {
        return None;
    }

    // Map formal type parameters (e.g., "T") to concrete type arguments (e.g., "String")
    let mut substitutions = HashMap::new();
    for (formal_parameter, concrete_argument) in
        formal_type_parameters.iter().zip(type_arguments.iter())
    {
        substitutions.insert(formal_parameter.clone(), concrete_argument.clone());
    }

    // Build parameter entries: (name, substituted_type_text)
    let parameters: Vec<(&str, String)> = supertype_constructor_parameters
        .iter()
        .filter_map(|parameter_string| {
            let (parameter_name, parameter_type) = parse_parameter(parameter_string)?;
            let substituted_type = substitute_types(parameter_type, &substitutions);
            Some((parameter_name, substituted_type))
        })
        .collect();
    if parameters.is_empty() {
        return None;
    }

    // Generate edit formatting
    let class_indentation = class_indent(&document_content, class_node.start_position().row);
    let parameter_indentation = format!("{class_indentation}    ");

    let mut parameters_text: String = String::new();
    for (index, (parameter_name, parameter_type)) in parameters.iter().enumerate() {
        let comma = if index + 1 < parameters.len() {
            ","
        } else {
            ""
        };
        parameters_text.push_str(&format!(
            "{parameter_indentation}val {parameter_name}: {parameter_type}{comma}\n"
        ));
    }

    let arguments: Vec<&str> = parameters.iter().map(|(name, _)| *name).collect();
    let constructor_block = format!("(\n{parameters_text}{class_indentation})");

    // Position: after the class name or its type parameters → insert primary constructor
    // For generic classes (e.g. `class Foo<T>`), we must insert AFTER `type_parameters`
    // to avoid producing invalid Kotlin like `class Foo (...) <T>`.
    let class_name_end_position = class_node
        .children(&mut class_node.walk())
        .find(|child| child.kind() == "type_parameters")
        .or_else(|| {
            class_node
                .children(&mut class_node.walk())
                .find(|child| child.kind() == KIND_TYPE_IDENT)
        })?
        .end_position();

    // Position: after supertype name in delegation spec → add `(arguments)`
    let user_type_end_position = delegation_specifier
        .children(&mut delegation_specifier.walk())
        .find(|child| child.kind() == KIND_USER_TYPE)?
        .end_position();

    let class_name_position = Position::new(
        class_name_end_position.row as u32,
        class_name_end_position.column as u32,
    );
    let supertype_position = Position::new(
        user_type_end_position.row as u32,
        user_type_end_position.column as u32,
    );

    let mut document_changes = HashMap::new();
    document_changes.insert(
        uri.clone(),
        vec![
            TextEdit {
                range: Range::new(class_name_position, class_name_position),
                new_text: format!(" {constructor_block} "),
            },
            TextEdit {
                range: Range::new(supertype_position, supertype_position),
                new_text: format!("({})", arguments.join(", ")),
            },
        ],
    );

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!(
            "Generate constructor `{class_name}({})`",
            arguments.join(", ")
        ),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(document_changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

// ─── helpers ──────────────────────────────────────────────────────────────────

/// Substitutes formal generic parameters with actual type arguments in a type signature.
/// Respects word boundaries to ensure partial matches are not erroneously replaced.
fn substitute_types(type_text: &str, substitutions: &HashMap<String, String>) -> String {
    let mut result = String::new();
    let mut current_word = String::new();

    for character in type_text.chars() {
        if character.is_alphanumeric() || character == '_' {
            current_word.push(character);
        } else {
            if !current_word.is_empty() {
                if let Some(substituted_value) = substitutions.get(&current_word) {
                    result.push_str(substituted_value);
                } else {
                    result.push_str(&current_word);
                }
                current_word.clear();
            }
            result.push(character);
        }
    }
    if !current_word.is_empty() {
        if let Some(substituted_value) = substitutions.get(&current_word) {
            result.push_str(substituted_value);
        } else {
            result.push_str(&current_word);
        }
    }
    result
}

/// Resolve constructor parameter text and formal generic parameters from the supertype's definition.
///
/// Tries the definitions index first (fast path), then falls back to the full
/// resolver chain (handles same-file, not-yet-indexed files, etc.).
/// Uses `ensure_file_data` to read from disk when the file is not in the index.
fn resolve_supertype_constructor_parameters(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
) -> Option<(Vec<String>, Vec<String>)> {
    // Normalize nested or dotted type names (e.g., "Outer.Inner" -> "Inner")
    // to ensure compatibility with indexer queries and symbol resolution.
    // Utilizing next_back() on DoubleEndedIterator to efficiently retrieve the final segment.
    let simple_name = name.split('.').next_back().unwrap_or(name);

    // Phase 1: definitions index (fast path)
    let locations = indexer.definition_locations(simple_name);
    if !locations.is_empty() {
        if let Some(result) =
            constructor_parameters_from_locations(indexer, simple_name, &locations, from_uri)
        {
            return Some(result);
        }
    }

    // Phase 2: full resolver chain (handles same-file, unindexed files, rg, etc.)
    let locations = resolve_symbol_inner(indexer, simple_name, from_uri, false);
    if !locations.is_empty() {
        if let Some(result) =
            constructor_parameters_from_locations(indexer, simple_name, &locations, from_uri)
        {
            return Some(result);
        }
    }

    None
}

/// Extract constructor parameters and generic type parameters from candidate locations.
///
/// Prefers the location in the same package as `from_uri`. Falls back to
/// reading from disk if the target file is not in the in-memory index.
fn constructor_parameters_from_locations(
    indexer: &Indexer,
    name: &str,
    locations: &[Location],
    from_uri: &Url,
) -> Option<(Vec<String>, Vec<String>)> {
    let current_package = indexer.package_of(from_uri);

    let target_location = current_package
        .as_ref()
        .and_then(|package| {
            locations.iter().find(|location| {
                indexer
                    .package_of(&location.uri)
                    .as_ref()
                    .is_some_and(|p| p == package)
            })
        })
        .unwrap_or(&locations[0]);

    let file_data = indexer
        .file_data_for(target_location.uri.as_str())
        .or_else(|| ensure_file_data(indexer, &target_location.uri))?;

    let class_symbol = file_data.symbols.iter().find(|symbol| {
        symbol.name == name
            && matches!(
                symbol.kind,
                SymbolKind::CLASS
                    | SymbolKind::INTERFACE
                    | SymbolKind::STRUCT
                    | SymbolKind::ENUM
                    | SymbolKind::OBJECT
            )
    })?;

    let parameters_text = class_symbol.params.as_str();
    if parameters_text.is_empty() {
        return None;
    }

    // Extract formal generic type parameters directly (already indexed as Vec<String>)
    let type_parameters = class_symbol.type_params.clone();

    Some((split_parameters(parameters_text), type_parameters))
}

/// Split comma-separated parameter text into individual parameter strings,
/// respecting nesting depth to avoid splitting on commas inside generics.
fn split_parameters(text: &str) -> Vec<String> {
    let mut depth = 0u8;
    let mut start = 0usize;
    let mut result = Vec::new();
    for (index, character) in text.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let parameter = text[start..index].trim();
                if !parameter.is_empty() {
                    result.push(parameter.to_owned());
                }
                start = index + 1;
            }
            _ => {}
        }
    }
    let last_parameter = text[start..].trim();
    if !last_parameter.is_empty() {
        result.push(last_parameter.to_owned());
    }
    result
}

/// Extract `(name, type)` from a single parameter string.
///
/// Handles forms:
/// - `"val name: Type"` → `("name", "Type")`
/// - `"name: Type"` → `("name", "Type")`
/// - `"vararg name: Type"` → `("name", "Type")`
/// - `"crossinline name: Type"` → `("name", "Type")`
fn parse_parameter(parameter: &str) -> Option<(&str, &str)> {
    let parameter = parameter.trim();
    // Strip optional val/var/vararg/crossinline/noinline/open modifiers
    let after_modifiers = parameter
        .strip_prefix("val ")
        .or_else(|| parameter.strip_prefix("var "))
        .or_else(|| parameter.strip_prefix("vararg "))
        .or_else(|| parameter.strip_prefix("crossinline "))
        .or_else(|| parameter.strip_prefix("noinline "))
        .or_else(|| parameter.strip_prefix("open "))
        .unwrap_or(parameter);
    let colon_index = after_modifiers.find(':')?;
    let name = after_modifiers[..colon_index].trim();
    let parameter_type = after_modifiers[colon_index + 1..].trim();
    // Strip default value after `=`
    let equals_index = parameter_type
        .find("= ")
        .or_else(|| parameter_type.find('='));
    let parameter_type = equals_index
        .map(|index| parameter_type[..index].trim())
        .unwrap_or(parameter_type);
    if name.is_empty() || parameter_type.is_empty() {
        return None;
    }
    Some((name, parameter_type))
}

/// Walk up from a node to find the nearest ancestor of the given kind.
fn ancestor_of_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    let mut current_node = node;
    loop {
        if current_node.kind() == kind {
            return Some(current_node);
        }
        if current_node.kind() == KIND_SOURCE_FILE {
            return None;
        }
        current_node = current_node.parent()?;
    }
}

fn has_child_of_kind(node: tree_sitter::Node, kind: &str) -> bool {
    node.children(&mut node.walk())
        .any(|child| child.kind() == kind)
}

/// Detect the indentation string for a given row (leading whitespace).
fn class_indent(content: &str, row: usize) -> String {
    let line = content.lines().nth(row).unwrap_or("");
    line.chars()
        .take_while(|character| *character == ' ' || *character == '\t')
        .collect()
}

#[cfg(test)]
#[path = "generate_constructor_tests.rs"]
mod tests;
