//! Diagnostic: detect reassignment of `val` values.
//!
//! Walks the live CST for all `assignment` nodes, resolves whether the assigned
//! variable is a `val` (immutable) or `var` (mutable), and emits a red error
//! diagnostic for val reassignments.
//!
//! Properly handles Kotlin scoping rules: innermost matching declaration wins
//! (shadowing is correctly respected).

use tower_lsp::lsp_types::*;

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::NodeExt;
use crate::queries::{
    KIND_BINDING_PATTERN_KIND, KIND_BLOCK, KIND_CLASS_BODY, KIND_CLASS_DECL, KIND_CLASS_PARAM,
    KIND_CONTROL_STRUCTURE_BODY, KIND_FORMAL_PARAM, KIND_FORMAL_PARAMS, KIND_FUN_BODY,
    KIND_FUN_DECL, KIND_FUN_VALUE_PARAMS, KIND_INTERFACE_BODY, KIND_KW_VAL, KIND_LAMBDA_LIT,
    KIND_LAMBDA_PARAMS, KIND_NAV_EXPR, KIND_NAV_SUFFIX, KIND_PARAMETER, KIND_PRIMARY_CTOR,
    KIND_PROP_DECL, KIND_SIMPLE_IDENT, KIND_TYPE_IDENT, KIND_VAR_DECL,
};

const KIND_ASSIGNMENT: &str = "assignment";
const KIND_DIRECTLY_ASSIGNABLE: &str = "directly_assignable_expression";

const SCOPE_BOUNDARIES: &[&str] = &[
    KIND_BLOCK,
    KIND_FUN_BODY,
    KIND_CLASS_BODY,
    KIND_INTERFACE_BODY,
    KIND_CONTROL_STRUCTURE_BODY,
    KIND_LAMBDA_LIT,
];

/// Scan a file for val-reassignment errors and return diagnostics.
///
/// The caller provides a `LiveDoc` parsed from the *same text* that was just
/// indexed, guaranteeing the CST and the indexed data are consistent.
pub(crate) fn reassignment_diagnostics(
    _indexer: &crate::indexer::Indexer,
    _uri: &Url,
    doc: &LiveDoc,
) -> Vec<Diagnostic> {
    let bytes = &doc.bytes;
    let root = doc.tree.root_node();
    let mut diagnostics = Vec::new();
    collect_assignments(root, bytes, &mut diagnostics);
    diagnostics
}

fn collect_assignments(node: tree_sitter::Node, bytes: &[u8], diagnostics: &mut Vec<Diagnostic>) {
    if node.kind() == KIND_ASSIGNMENT {
        if let Some(diag) = check_assignment(&node, bytes) {
            diagnostics.push(diag);
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_assignments(cursor.node(), bytes, diagnostics);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn check_assignment(assign_node: &tree_sitter::Node, bytes: &[u8]) -> Option<Diagnostic> {
    let (lhs_ident, lhs_range) = extract_lhs_identifier(assign_node, bytes)?;

    let assign_line = assign_node.start_position().row;

    let mut current = *assign_node;
    let mut checked_siblings = false;

    loop {
        if !checked_siblings {
            if let Some(diag) =
                scan_scope_for_val(&lhs_ident, &current, assign_line, bytes, &lhs_range)
            {
                return Some(diag);
            }
            checked_siblings = true;
        }

        if current.kind() == KIND_LAMBDA_LIT
            || current.kind() == KIND_FUN_DECL
            || current.kind() == KIND_CLASS_DECL
        {
            if let Some(is_val) = check_node_for_val(&lhs_ident, &current, bytes) {
                if is_val {
                    return Some(Diagnostic {
                        range: lhs_range,
                        severity: Some(DiagnosticSeverity::ERROR),
                        source: Some("kotlin-lsp".into()),
                        message: "Val cannot be reassigned".to_owned(),
                        ..Default::default()
                    });
                }
                return None;
            }

            if current.kind() == KIND_LAMBDA_LIT {
                if let Some(is_val) = check_lambda_implicit_it(&lhs_ident, &current, bytes) {
                    if is_val {
                        return Some(Diagnostic {
                            range: lhs_range,
                            severity: Some(DiagnosticSeverity::ERROR),
                            source: Some("kotlin-lsp".into()),
                            message: "Val cannot be reassigned".to_owned(),
                            ..Default::default()
                        });
                    }
                    return None;
                }
            }
        }

        let is_boundary = SCOPE_BOUNDARIES.contains(&current.kind());

        let Some(parent) = current.parent() else {
            break;
        };

        if is_boundary {
            checked_siblings = false;
        }
        current = parent;
    }

    None
}

fn first_named_child(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        if cursor.node().is_named() {
            return Some(cursor.node());
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
    None
}

fn extract_lhs_identifier(
    assign_node: &tree_sitter::Node,
    bytes: &[u8],
) -> Option<(String, Range)> {
    let lhs = first_named_child(*assign_node)?;
    if lhs.kind() != KIND_DIRECTLY_ASSIGNABLE {
        return None;
    }

    if lhs.first_child_of_kind(KIND_NAV_SUFFIX).is_some() {
        return None;
    }

    if lhs.first_child_of_kind(KIND_NAV_EXPR).is_some() {
        return None;
    }

    let first_named = first_named_child(lhs)?;

    if first_named.kind() == KIND_SIMPLE_IDENT {
        let name = first_named.utf8_text_owned(bytes)?;
        let start = first_named.start_position();
        let end = first_named.end_position();
        let range = Range::new(
            Position::new(start.row as u32, start.column as u32),
            Position::new(end.row as u32, end.column as u32),
        );
        return Some((name, range));
    }

    None
}

fn scan_scope_for_val(
    name: &str,
    from_node: &tree_sitter::Node,
    assign_line: usize,
    bytes: &[u8],
    ident_range: &Range,
) -> Option<Diagnostic> {
    let parent = from_node.parent()?;
    let mut cursor = parent.walk();

    if !cursor.goto_first_child() {
        return None;
    }

    loop {
        let child = cursor.node();
        let child_start = child.start_position().row;

        if child_start >= assign_line {
            break;
        }

        if let Some(is_val) = check_node_for_val(name, &child, bytes) {
            if is_val {
                return Some(Diagnostic {
                    range: *ident_range,
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("kotlin-lsp".into()),
                    message: "Val cannot be reassigned".to_owned(),
                    ..Default::default()
                });
            }
            return None;
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }

    None
}

fn check_node_for_val(name: &str, node: &tree_sitter::Node, bytes: &[u8]) -> Option<bool> {
    match node.kind() {
        KIND_PROP_DECL => check_prop_decl(name, node, bytes),
        KIND_FUN_DECL => check_fun_params(name, node, bytes),
        KIND_CLASS_DECL => check_class_decl_params(name, node, bytes),
        KIND_LAMBDA_LIT => check_lambda_params(name, node, bytes),
        KIND_LAMBDA_PARAMS => check_lambda_params_list(name, node, bytes),
        KIND_CLASS_PARAM => check_class_param(name, node, bytes),
        _ => None,
    }
}

fn check_prop_decl(name: &str, prop_node: &tree_sitter::Node, bytes: &[u8]) -> Option<bool> {
    let var_decl = prop_node.first_child_of_kind(KIND_VAR_DECL)?;

    let ident = var_decl.first_child_of_kind(KIND_SIMPLE_IDENT)?;
    let ident_text = ident.utf8_text_owned(bytes)?;

    if ident_text != name {
        return None;
    }

    let binding_kind = prop_node.first_child_of_kind(KIND_BINDING_PATTERN_KIND)?;
    let kind_text = binding_kind.utf8_text_owned(bytes)?;

    Some(kind_text == KIND_KW_VAL)
}

fn check_class_decl_params(
    name: &str,
    class_node: &tree_sitter::Node,
    bytes: &[u8],
) -> Option<bool> {
    let ctor = class_node.first_child_of_kind(KIND_PRIMARY_CTOR)?;

    let mut cursor = ctor.walk();
    if !cursor.goto_first_child() {
        return None;
    }

    loop {
        let child = cursor.node();

        if child.kind() == KIND_CLASS_PARAM {
            if let Some(is_val) = check_class_param(name, &child, bytes) {
                return Some(is_val);
            }
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }

    None
}

fn check_fun_params(name: &str, fun_node: &tree_sitter::Node, bytes: &[u8]) -> Option<bool> {
    let params = fun_node
        .first_child_of_kind(KIND_FUN_VALUE_PARAMS)
        .or_else(|| fun_node.first_child_of_kind(KIND_FORMAL_PARAMS))?;

    let mut cursor = params.walk();
    if !cursor.goto_first_child() {
        return None;
    }

    loop {
        let child = cursor.node();

        if child.kind() == KIND_CLASS_PARAM {
            if let Some(is_val) = check_class_param(name, &child, bytes) {
                return Some(is_val);
            }
        }

        if child.kind() == KIND_PARAMETER || child.kind() == KIND_FORMAL_PARAM {
            if let Some(is_val) = check_regular_param(name, &child, bytes) {
                return Some(is_val);
            }
        }

        if !cursor.goto_next_sibling() {
            break;
        }
    }

    None
}

fn check_class_param(name: &str, param_node: &tree_sitter::Node, bytes: &[u8]) -> Option<bool> {
    let ident = param_node
        .first_child_of_kind(KIND_SIMPLE_IDENT)
        .or_else(|| param_node.first_child_of_kind(KIND_TYPE_IDENT))?;
    let ident_text = ident.utf8_text_owned(bytes)?;

    if ident_text != name {
        return None;
    }

    let binding_kind = param_node.first_child_of_kind(KIND_BINDING_PATTERN_KIND);
    match binding_kind {
        Some(bk) => {
            let kind_text = bk.utf8_text_owned(bytes)?;
            Some(kind_text == KIND_KW_VAL)
        }
        None => Some(true),
    }
}

fn check_regular_param(name: &str, param_node: &tree_sitter::Node, bytes: &[u8]) -> Option<bool> {
    let ident = param_node
        .first_child_of_kind(KIND_SIMPLE_IDENT)
        .or_else(|| param_node.first_child_of_kind(KIND_TYPE_IDENT))?;
    let ident_text = ident.utf8_text_owned(bytes)?;

    if ident_text != name {
        return None;
    }

    Some(true)
}

fn check_lambda_params(name: &str, lambda_node: &tree_sitter::Node, bytes: &[u8]) -> Option<bool> {
    let params = lambda_node.first_child_of_kind(KIND_LAMBDA_PARAMS)?;
    check_lambda_params_list(name, &params, bytes)
}

fn check_lambda_params_list(
    name: &str,
    params_node: &tree_sitter::Node,
    bytes: &[u8],
) -> Option<bool> {
    let var_decls = params_node.children_of_kind(KIND_VAR_DECL);

    if var_decls.is_empty() {
        if name == "it" {
            let simple_idents = params_node.children_of_kind(KIND_SIMPLE_IDENT);
            if simple_idents.is_empty() {
                return Some(true);
            }
            for ident in &simple_idents {
                if let Some(ident_text) = ident.utf8_text_owned(bytes) {
                    if ident_text == "it" {
                        return Some(true);
                    }
                    if ident_text == name {
                        return Some(true);
                    }
                }
            }
        }
        return None;
    }

    for var_decl in var_decls {
        let ident = var_decl.first_child_of_kind(KIND_SIMPLE_IDENT)?;
        if let Some(ident_text) = ident.utf8_text_owned(bytes) {
            if ident_text == name {
                return Some(true);
            }
        }
    }

    None
}

fn check_lambda_implicit_it(
    name: &str,
    lambda_node: &tree_sitter::Node,
    bytes: &[u8],
) -> Option<bool> {
    if name != "it" {
        return None;
    }

    let Some(params) = lambda_node.first_child_of_kind(KIND_LAMBDA_PARAMS) else {
        return Some(true);
    };

    let var_decls = params.children_of_kind(KIND_VAR_DECL);
    if !var_decls.is_empty() {
        for var_decl in &var_decls {
            if let Some(ident) = var_decl.first_child_of_kind(KIND_SIMPLE_IDENT) {
                if let Some(ident_text) = ident.utf8_text_owned(bytes) {
                    if ident_text == "it" || ident_text == "_" {
                        return Some(true);
                    }
                    return None;
                }
            }
        }
    }

    let simple_idents = params.children_of_kind(KIND_SIMPLE_IDENT);
    if !simple_idents.is_empty() {
        for ident in &simple_idents {
            if let Some(ident_text) = ident.utf8_text_owned(bytes) {
                if ident_text == "it" || ident_text == "_" {
                    return Some(true);
                }
                return None;
            }
        }
    }

    Some(true)
}

#[cfg(test)]
#[path = "reassignment_diagnostics_tests.rs"]
mod tests;
