//! Infer the Kotlin type of a CST expression node.
//!
//! Handles the cases that are knowable without a compiler:
//!
//! | Node kind                 | Inferred type          |
//! |---------------------------|------------------------|
//! | `integer_literal`         | `Int`                  |
//! | `long_literal`            | `Long`                 |
//! | `real_literal`            | `Float` or `Double`    |
//! | `string_literal`          | `String`               |
//! | `boolean_literal`         | `Boolean`              |
//! | `null`                    | `Nothing?`             |
//! | `character_literal`       | `Char`                 |
//! | `call_expression`         | return type from index |
//! | `check_expression`        | `Boolean`              |
//! | `comparison_expression`   | `Boolean`              |
//! | `disjunction_expression`  | `Boolean`              |
//! | `conjunction_expression`  | `Boolean`              |
//! | `prefix_expression` (`!`) | `Boolean`              |
//! | `if_expression`           | type when both branches agree |
//! | `range_expression` (int)  | `IntRange`             |
//! | `simple_identifier`       | variable type from index |
//! | `type_identifier`         | variable type from index |
//! | `navigation_expression`   | field/method return type from index |
//! | `this_expression`         | contextual lambda-receiver type |
//!
//! Simple identifiers, navigation expressions (e.g. `list.size`), and `this`
//! are now resolved through the `InferDeps` seam. `when` expressions and other
//! compound forms are not resolved — callers receive `None` and can omit the
//! type annotation.

use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::indexer::NodeExt;
use crate::queries::{
    KIND_BOOLEAN_LITERAL, KIND_CALL_EXPR, KIND_CHARACTER_LITERAL, KIND_CHECK_EXPR,
    KIND_COMPARISON_EXPR, KIND_CONJUNCTION_EXPR, KIND_CONTROL_STRUCTURE_BODY,
    KIND_DISJUNCTION_EXPR, KIND_ELSE, KIND_IF_EXPR, KIND_INTEGER_LITERAL, KIND_LONG_LITERAL,
    KIND_MULTILINE_STRING_LITERAL, KIND_NAV_EXPR, KIND_NAV_SUFFIX, KIND_NULL_LITERAL,
    KIND_PREFIX_EXPR, KIND_RANGE_EXPR, KIND_REAL_LITERAL, KIND_SIMPLE_IDENT, KIND_STRING_LITERAL,
    KIND_THIS_EXPR, KIND_TYPE_IDENT,
};
use crate::StrExt as _;

use super::deps::InferDeps;

// ─── public API ───────────────────────────────────────────────────────────────

/// Infer the Kotlin type of `node` as a human-readable string (e.g. `"Int"`).
///
/// Returns `None` when the type cannot be determined without compiler
/// type-resolution (e.g. navigation expressions, generic calls).
pub(crate) fn infer_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    match node.kind() {
        KIND_INTEGER_LITERAL => Some("Int".to_owned()),
        KIND_LONG_LITERAL => Some("Long".to_owned()),
        KIND_REAL_LITERAL => infer_real_literal(node, bytes),
        KIND_STRING_LITERAL | KIND_MULTILINE_STRING_LITERAL => Some("String".to_owned()),
        KIND_BOOLEAN_LITERAL => Some("Boolean".to_owned()),
        KIND_NULL_LITERAL => Some("Nothing?".to_owned()),
        KIND_CHARACTER_LITERAL => Some("Char".to_owned()),
        k if k == KIND_SIMPLE_IDENT || k == KIND_TYPE_IDENT => {
            infer_ident_type(node, bytes, deps, uri)
        }
        k if k == KIND_THIS_EXPR => infer_this_expr_type(node, bytes, deps, uri),
        k if k == KIND_NAV_EXPR => infer_navigation_expr_type(node, bytes, deps, uri),
        k if k == KIND_CALL_EXPR => infer_call_expr_type(node, bytes, deps, uri),
        k if k == KIND_CHECK_EXPR
            || k == KIND_COMPARISON_EXPR
            || k == KIND_DISJUNCTION_EXPR
            || k == KIND_CONJUNCTION_EXPR =>
        {
            Some("Boolean".to_owned())
        }
        k if k == KIND_PREFIX_EXPR => infer_prefix_expr_type(node, bytes),
        k if k == KIND_IF_EXPR => infer_if_expr_type(node, bytes, deps, uri),
        k if k == KIND_RANGE_EXPR => infer_range_expr_type(node, bytes, deps, uri),
        _ => None,
    }
}

// ─── private helpers ──────────────────────────────────────────────────────────

/// `3.14f` / `3.14F` → `Float`; `3.14` → `Double`.
fn infer_real_literal(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = node.utf8_text(bytes).ok()?;
    if text.ends_with('f') || text.ends_with('F') {
        Some("Float".to_owned())
    } else {
        Some("Double".to_owned())
    }
}

/// Resolve the type of a `call_expression` node by delegating to the chain
/// resolution path used by hover and semantic tokens.
fn infer_call_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    super::chain::resolve_call_expr_type(node, bytes, deps, uri)
}

/// Resolve the type of a `simple_identifier` or `type_identifier`.
///
/// Adapted from `semantic_tokens::resolve::identifier_type`.
/// Consults contextual lambda-param inference first (handles `it` / `this` /
/// named lambda params), then falls back to the declared-variable type.
/// As a last resort, a bare uppercase identifier that names a known type
/// (class, interface, enum, object, companion) resolves to itself — this
/// covers companion-object access like `Foo.CONSTANT` where `Foo` is the
/// receiver identifier.
fn infer_ident_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    let name = node.utf8_text_owned(bytes)?;
    let start = node.start_position();
    let col = crate::inlay_hints::ts_byte_col_to_utf16(bytes, &[], start.row, start.column);
    if let Some(inferred) = deps.find_contextual_type(&name, uri, start.row, col) {
        return Some(inferred);
    }
    if let Some(inferred) = deps.find_var_type(&name, uri) {
        return Some(inferred);
    }
    if name.starts_with_uppercase() && deps.has_type_definition(&name) {
        return Some(name);
    }
    None
}

/// Resolve the type of a `this_expression`.
///
/// Adapted from the `KIND_THIS_EXPR` arm of `semantic_tokens::resolve::expression_type`.
/// Delegates to contextual lambda-receiver inference at the node's position.
fn infer_this_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    let start = node.start_position();
    let col = crate::inlay_hints::ts_byte_col_to_utf16(bytes, &[], start.row, start.column);
    deps.find_contextual_type("this", uri, start.row, col)
}

/// Resolve the type of a `navigation_expression` node (e.g. `obj.field`).
///
/// Adapted from `semantic_tokens::resolve::navigation_expression_type`.
/// Recursively resolves the receiver through `infer_expr_type`, then looks up
/// the member as a field or (when the expression is a call callee) a method.
fn infer_navigation_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    let receiver = nav_receiver_node(node)?;
    let member = nav_member_ident(node)?.utf8_text_owned(bytes)?;
    let receiver_type = infer_expr_type(receiver, bytes, deps, uri)?;

    if nav_is_call_callee(node) {
        // The two-step `find_fun_return_type_reachable` → `find_fun_return_type` replicates
        // `Resolver::function_return_type`'s reachable→by_name fallback behaviour (see
        // `src/resolver/api.rs`) through the `InferDeps` seam rather than calling the
        // `Resolver` trait directly.  Together they are equivalent to the original
        // `indexer.function_return_type(&member, uri)` call in `navigation_expression_type`.
        return deps
            .find_method_return_type_for_type(&receiver_type, &member)
            .or_else(|| deps.find_fun_return_type_reachable(&member, uri))
            .or_else(|| deps.find_fun_return_type(&member));
    }

    deps.find_field_type(&receiver_type, &member)
}

// ─── navigation tree-walking helpers ─────────────────────────────────────────

/// Return the receiver sub-node of a `navigation_expression` (the part before
/// the final `.`).  Equivalent to `semantic_tokens::helpers::navigation_receiver_node`.
fn nav_receiver_node(node: Node<'_>) -> Option<Node<'_>> {
    (0..node.child_count())
        .filter_map(|i| node.child(i))
        .find(|child| child.is_named() && child.kind() != KIND_NAV_SUFFIX)
}

/// Return the member identifier inside the `navigation_suffix` of a
/// `navigation_expression`.  Equivalent to
/// `semantic_tokens::helpers::navigation_member_ident`.
fn nav_member_ident(node: Node<'_>) -> Option<Node<'_>> {
    let suffix = node.first_child_of_kind(KIND_NAV_SUFFIX)?;
    (0..suffix.child_count())
        .filter_map(|i| suffix.child(i))
        .find(|child| child.kind() == KIND_SIMPLE_IDENT || child.kind() == KIND_TYPE_IDENT)
}

/// Return `true` if `node` is the callee (first child) of a `call_expression`.
/// Equivalent to `semantic_tokens::helpers::is_call_callee`.
fn nav_is_call_callee(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == KIND_CALL_EXPR && parent.child(0).map(|child| child.id()) == Some(node.id())
}

/// `!expr` → `Boolean`; other prefix operators (`-`, `+`) are arithmetic and
/// not inferable without knowing the operand type.
fn infer_prefix_expr_type(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let op = node.child(0)?;
    let text = op.utf8_text(bytes).ok()?;
    if text == "!" {
        Some("Boolean".to_owned())
    } else {
        None
    }
}

/// For `if (cond) <then> else <else>`: emit a type hint only when both
/// branches infer to the same type. No hint is emitted for bare `if` without
/// `else`, or when either branch is ambiguous.
fn infer_if_expr_type<D: InferDeps>(
    node: Node<'_>,
    bytes: &[u8],
    deps: &D,
    uri: &Url,
) -> Option<String> {
    let has_else = (0..node.child_count())
        .filter_map(|i| node.child(i))
        .any(|child| child.kind() == KIND_ELSE);
    if !has_else {
        return None;
    }

    let mut bodies = (0..node.child_count())
        .filter_map(|i| node.child(i))
        .filter(|child| child.kind() == KIND_CONTROL_STRUCTURE_BODY);
    let then_expr = bodies.next()?.child(0)?;
    let else_expr = bodies.next()?.child(0)?;
    let then_type = infer_expr_type(then_expr, bytes, deps, uri)?;
    let else_type = infer_expr_type(else_expr, bytes, deps, uri)?;
    (then_type == else_type).then_some(then_type)
}

/// `a..b` or `a..<b`: infer `IntRange` only when both operands are integer
/// literals. Any other operand type requires the compiler.
fn infer_range_expr_type<D: InferDeps>(
    node: Node<'_>,
    bytes: &[u8],
    deps: &D,
    uri: &Url,
) -> Option<String> {
    let lhs = node.child(0)?;
    let rhs_idx = node.child_count().checked_sub(1)?;
    let rhs = node.child(rhs_idx)?;
    let lhs_ty = infer_expr_type(lhs, bytes, deps, uri)?;
    let rhs_ty = infer_expr_type(rhs, bytes, deps, uri)?;
    match (lhs_ty.as_str(), rhs_ty.as_str()) {
        ("Int", "Int") => Some("IntRange".to_owned()),
        ("Long", "Long") | ("Int", "Long") | ("Long", "Int") => Some("LongRange".to_owned()),
        ("Char", "Char") => Some("CharRange".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "expr_type_tests.rs"]
mod tests;
