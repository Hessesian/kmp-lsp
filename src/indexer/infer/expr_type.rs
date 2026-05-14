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
//!
//! Navigation expressions (e.g. `list.size`), `when` expressions, and other
//! compound forms are not resolved — callers receive `None` and can omit the
//! type annotation.

use tree_sitter::Node;

use crate::indexer::NodeExt;
use crate::queries::{
    KIND_BOOLEAN_LITERAL, KIND_CALL_EXPR, KIND_CHARACTER_LITERAL, KIND_CHECK_EXPR,
    KIND_COMPARISON_EXPR, KIND_CONJUNCTION_EXPR, KIND_CONTROL_STRUCTURE_BODY,
    KIND_DISJUNCTION_EXPR, KIND_IF_EXPR, KIND_INTEGER_LITERAL, KIND_LONG_LITERAL,
    KIND_MULTILINE_STRING_LITERAL, KIND_NULL_LITERAL, KIND_PREFIX_EXPR, KIND_RANGE_EXPR,
    KIND_REAL_LITERAL, KIND_STRING_LITERAL,
};

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
) -> Option<String> {
    match node.kind() {
        KIND_INTEGER_LITERAL => Some("Int".to_owned()),
        KIND_LONG_LITERAL => Some("Long".to_owned()),
        KIND_REAL_LITERAL => infer_real_literal(node, bytes),
        KIND_STRING_LITERAL | KIND_MULTILINE_STRING_LITERAL => Some("String".to_owned()),
        KIND_BOOLEAN_LITERAL => Some("Boolean".to_owned()),
        KIND_NULL_LITERAL => Some("Nothing?".to_owned()),
        KIND_CHARACTER_LITERAL => Some("Char".to_owned()),
        k if k == KIND_CALL_EXPR => infer_call_expr_type(node, bytes, deps),
        k if k == KIND_CHECK_EXPR
            || k == KIND_COMPARISON_EXPR
            || k == KIND_DISJUNCTION_EXPR
            || k == KIND_CONJUNCTION_EXPR =>
        {
            Some("Boolean".to_owned())
        }
        k if k == KIND_PREFIX_EXPR => infer_prefix_expr_type(node, bytes),
        k if k == KIND_IF_EXPR => infer_if_expr_type(node, bytes, deps),
        k if k == KIND_RANGE_EXPR => infer_range_expr_type(node, bytes, deps),
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

/// For a `call_expression` whose callee is a `simple_identifier`:
/// - If the callee starts with uppercase it's a constructor call → return the class name.
/// - Otherwise look up the function's return type from the index via
///   [`InferDeps::find_fun_return_type`].
fn infer_call_expr_type(node: Node<'_>, bytes: &[u8], deps: &impl InferDeps) -> Option<String> {
    let fn_name = node.call_fn_name(bytes)?;
    if fn_name.starts_with(|c: char| c.is_uppercase()) {
        return Some(fn_name);
    }
    let raw = deps.find_fun_return_type(&fn_name)?;
    Some(raw.trim_start_matches(':').trim().to_owned())
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
fn infer_if_expr_type<D: InferDeps>(node: Node<'_>, bytes: &[u8], deps: &D) -> Option<String> {
    let has_else = (0..node.child_count())
        .filter_map(|i| node.child(i))
        .any(|child| child.kind() == "else");
    if !has_else {
        return None;
    }

    let mut bodies = (0..node.child_count())
        .filter_map(|i| node.child(i))
        .filter(|child| child.kind() == KIND_CONTROL_STRUCTURE_BODY);
    let then_expr = bodies.next()?.child(0)?;
    let else_expr = bodies.next()?.child(0)?;
    let then_type = infer_expr_type(then_expr, bytes, deps)?;
    let else_type = infer_expr_type(else_expr, bytes, deps)?;
    (then_type == else_type).then_some(then_type)
}

/// `a..b` or `a..<b`: infer `IntRange` only when both operands are integer
/// literals. Any other operand type requires the compiler.
fn infer_range_expr_type<D: InferDeps>(node: Node<'_>, bytes: &[u8], deps: &D) -> Option<String> {
    let lhs = node.child(0)?;
    let rhs_idx = node.child_count().checked_sub(1)?;
    let rhs = node.child(rhs_idx)?;
    let lhs_ty = infer_expr_type(lhs, bytes, deps)?;
    let rhs_ty = infer_expr_type(rhs, bytes, deps)?;
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
