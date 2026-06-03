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

use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::indexer::NodeExt;
use crate::queries::{
    KIND_BOOLEAN_LITERAL, KIND_CALL_EXPR, KIND_CHARACTER_LITERAL, KIND_CHECK_EXPR,
    KIND_COMPARISON_EXPR, KIND_CONJUNCTION_EXPR, KIND_CONTROL_STRUCTURE_BODY,
    KIND_DISJUNCTION_EXPR, KIND_ELSE, KIND_IF_EXPR, KIND_INTEGER_LITERAL, KIND_LONG_LITERAL,
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

/// Resolve the type of a `call_expression` node.
///
/// Strategy:
/// 1. If the call has a `::class` literal argument (Retrofit pattern:
///    `recv.create(Api::class.java)`), extract the type from the argument directly.
/// 2. Otherwise delegate to the chain resolution path used by hover/semantic tokens.
fn infer_call_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    // Pattern: `receiver.method(TypeName::class.java)` — extract type from argument.
    // This handles the common Retrofit/Koin pattern where the generic type parameter
    // is inferred from a class literal argument rather than explicit type args.
    if let Some(class_type) = extract_type_from_class_literal_arg(node, bytes) {
        return Some(class_type);
    }

    super::chain::resolve_call_expr_type(node, bytes, deps, uri)
}

/// Extract a type name from a `::class` literal argument inside a call expression.
///
/// Matches: `retrofit.create(GoldConversionSecuredApi::class.java)`
/// Returns: `Some("GoldConversionSecuredApi")`
fn extract_type_from_class_literal_arg<'a>(
    call_node: tree_sitter::Node<'a>,
    bytes: &[u8],
) -> Option<String> {
    // call_expression → call_suffix → value_arguments → value_argument → expr
    let call_suffix = call_node.first_child_of_kind(crate::queries::KIND_CALL_SUFFIX)?;
    let value_args = call_suffix.first_child_of_kind(crate::queries::KIND_VALUE_ARGS)?;

    let mut ac = value_args.walk();
    for arg in value_args.children(&mut ac) {
        // value_argument wraps the actual expression
        let node = if arg.kind() == crate::queries::KIND_VALUE_ARG {
            arg.child(0).filter(|c| c.is_named())?
        } else if arg.is_named() {
            arg
        } else {
            continue;
        };
        let text = node.utf8_text(bytes).ok()?;
        if let Some(class_pos) = text.find("::class") {
            let before = &text[..class_pos];
            let type_name = before.rsplit(&['.', ':']).next().unwrap_or(before);
            let type_name = type_name.trim();
            if !type_name.is_empty() && type_name.starts_with(|c: char| c.is_uppercase()) {
                return Some(type_name.to_owned());
            }
        }
    }
    None
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
