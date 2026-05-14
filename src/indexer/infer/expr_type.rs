//! Infer the Kotlin type of a CST expression node.
//!
//! Handles the cases that are knowable without a compiler:
//!
//! | Node kind           | Inferred type          |
//! |---------------------|------------------------|
//! | `integer_literal`   | `Int`                  |
//! | `long_literal`      | `Long`                 |
//! | `real_literal`      | `Float` or `Double`    |
//! | `string_literal`    | `String`               |
//! | `boolean_literal`   | `Boolean`              |
//! | `null`              | `Nothing?`             |
//! | `call_expression`   | return type from index |
//!
//! Navigation expressions (e.g. `list.size`) and other compound forms are
//! not resolved — callers receive `None` and can omit the type annotation.

use tree_sitter::Node;

use crate::indexer::NodeExt;
use crate::queries::KIND_CALL_EXPR;

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
        "integer_literal" => Some("Int".to_owned()),
        "long_literal" => Some("Long".to_owned()),
        "real_literal" => infer_real_literal(node, bytes),
        "string_literal" | "multiline_string_literal" => Some("String".to_owned()),
        "boolean_literal" => Some("Boolean".to_owned()),
        "null" => Some("Nothing?".to_owned()),
        "character_literal" => Some("Char".to_owned()),
        k if k == KIND_CALL_EXPR => infer_call_expr_type(node, bytes, deps),
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
