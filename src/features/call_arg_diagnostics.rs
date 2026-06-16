//! Diagnostic: detect call sites with mismatched argument counts.
//!
//! Walks the live CST for all `call_expression` nodes, resolves the callee's
//! signature, and emits a warning when the number of provided arguments does
//! not match the required parameters.
//!
//! Skipped cases (too ambiguous without full type resolution):
//! - Calls with trailing lambdas
//! - Overloaded functions (multiple signatures found)
//! - Signatures containing `vararg`

use tower_lsp::lsp_types::*;

use crate::indexer::{
    live_tree::LiveDoc, resolve_call_signature, CallSite, Indexer, NodeExt, SignatureResult,
};
use crate::queries::{
    KIND_CALL_EXPR, KIND_CALL_SUFFIX, KIND_FUN_DECL, KIND_LAMBDA_LIT, KIND_SIMPLE_IDENT,
    KIND_VALUE_ARG,
};

/// Scan a file for call-argument count mismatches and return diagnostics.
///
/// The caller provides a `LiveDoc` parsed from the *same text* that was just
/// indexed, guaranteeing the CST and the indexed signature data are consistent.
pub(crate) fn call_arg_diagnostics(indexer: &Indexer, uri: &Url, doc: &LiveDoc) -> Vec<Diagnostic> {
    let bytes = &doc.bytes;
    let root = doc.tree.root_node();
    let mut stats = DiagStats::default();
    // Cache resolve_call_signature results within one diagnostic run.
    // Many call_expression nodes call the same function; caching avoids O(N×K)
    // symbol-index scans where N = distinct names and K = definition locations.
    let mut sig_cache: std::collections::HashMap<(String, Option<String>), SignatureResult> =
        std::collections::HashMap::new();
    let mut diagnostics = Vec::new();
    collect_call_nodes(
        root,
        bytes,
        indexer,
        uri,
        &mut diagnostics,
        &mut stats,
        &mut sig_cache,
    );
    log::debug!(
        "call_arg_diagnostics: {} call_expr nodes, {} skipped(lambda), {} skipped(scope), {} resolved ({} cache hits), {}ms",
        stats.total, stats.skipped_trailing_lambda, stats.skipped_scope, stats.resolved,
        stats.cache_hits, stats.resolve_ms
    );
    diagnostics
}

#[derive(Default)]
struct DiagStats {
    total: usize,
    skipped_trailing_lambda: usize,
    skipped_scope: usize,
    resolved: usize,
    cache_hits: usize,
    resolve_ms: u128,
}

fn collect_call_nodes(
    node: tree_sitter::Node,
    bytes: &[u8],
    indexer: &Indexer,
    uri: &Url,
    diagnostics: &mut Vec<Diagnostic>,
    stats: &mut DiagStats,
    sig_cache: &mut std::collections::HashMap<(String, Option<String>), SignatureResult>,
) {
    if node.kind() == KIND_CALL_EXPR {
        if let Some(diag) = check_call_args(&node, bytes, indexer, uri, stats, sig_cache) {
            diagnostics.push(diag);
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_call_nodes(
                cursor.node(),
                bytes,
                indexer,
                uri,
                diagnostics,
                stats,
                sig_cache,
            );
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn check_call_args(
    call_node: &tree_sitter::Node,
    bytes: &[u8],
    indexer: &Indexer,
    uri: &Url,
    stats: &mut DiagStats,
    sig_cache: &mut std::collections::HashMap<(String, Option<String>), SignatureResult>,
) -> Option<Diagnostic> {
    stats.total += 1;

    // Skip calls with trailing lambdas — too complex without SAM resolution
    if has_trailing_lambda(call_node) {
        stats.skipped_trailing_lambda += 1;
        return None;
    }

    let (fn_name, qualifier) = call_node.call_fn_and_qualifier(bytes)?;

    // Skip unqualified calls inside scope-function lambdas (apply/run/with/let/also).
    // These have an implicit `this` receiver that we cannot resolve, so global lookup
    // would match the wrong overload.
    if qualifier.is_none() && is_inside_scope_function_lambda(call_node, bytes) {
        stats.skipped_scope += 1;
        return None;
    }

    // Unqualified `copy` inside any lambda — the implicit receiver type cannot be
    // inferred without full type resolution, so cross-file lookup may pick a JAR
    // copy() with a different arity, producing false positives.
    if qualifier.is_none() && fn_name == "copy" && is_inside_any_lambda(call_node) {
        stats.skipped_scope += 1;
        return None;
    }

    let value_arguments = call_node.find_value_arguments();
    let provided_count = count_provided_args(value_arguments.as_ref(), bytes);

    // Resolve signature — use per-run cache to avoid redundant index scans for
    // the same function name called multiple times in one file.
    let cache_key = (fn_name.clone(), qualifier.clone());
    let sig_result = if let Some(cached) = sig_cache.get(&cache_key) {
        stats.cache_hits += 1;
        cached.clone()
    } else {
        let call = CallSite {
            name: &fn_name,
            qualifier: qualifier.as_deref(),
            caller_uri: uri,
        };
        stats.resolved += 1;
        let t0 = std::time::Instant::now();
        let result = resolve_call_signature(&call, indexer);
        stats.resolve_ms += t0.elapsed().as_millis();
        sig_cache.insert(cache_key, result.clone());
        result
    };
    let (params_text, (required, total)) = match sig_result {
        SignatureResult::Unique {
            params_text,
            param_counts,
        } => (params_text, param_counts),
        SignatureResult::Overloaded
        | SignatureResult::NotFound
        | SignatureResult::UnresolvableReceiver => return None,
    };

    // Skip vararg functions (Kotlin `vararg` or Java `...`)
    if params_text.contains("vararg ")
        || params_text.contains("vararg\t")
        || params_text.contains("...")
    {
        return None;
    }

    if provided_count < required {
        let range = diagnostic_range(call_node, value_arguments.as_ref());
        let message = if required == total {
            format!("{fn_name}: expected {required} argument(s), found {provided_count}")
        } else {
            format!("{fn_name}: expected at least {required} argument(s), found {provided_count}")
        };
        return Some(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("kmp-lsp".into()),
            message,
            ..Default::default()
        });
    }

    if provided_count > total {
        // Skip if this is a delegation to a parent overload:
        // `fun foo() { foo(extraArg) }` — local has 0 params, call has 1 arg.
        // Only suppress for unqualified calls inside a same-named function.
        if qualifier.is_none() && is_inside_same_named_function(call_node, bytes, &fn_name) {
            return None;
        }
        let range = diagnostic_range(call_node, value_arguments.as_ref());
        let message =
            format!("{fn_name}: expected at most {total} argument(s), found {provided_count}");
        return Some(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("kmp-lsp".into()),
            message,
            ..Default::default()
        });
    }

    None
}

/// Check if the call is inside a lambda that belongs to a scope function
/// (apply, run, with, let, also). Unqualified calls inside these lambdas
/// have an implicit `this` receiver we cannot resolve.
fn is_inside_scope_function_lambda(call_node: &tree_sitter::Node, bytes: &[u8]) -> bool {
    const SCOPE_FUNCTIONS: &[&str] = &[
        "apply",
        "run",
        "with",
        "let",
        "also",
        "takeIf",
        "takeUnless",
    ];

    let mut node = *call_node;
    for _ in 0..15 {
        let Some(parent) = node.parent() else {
            return false;
        };
        if parent.kind() == KIND_LAMBDA_LIT {
            // Walk up from the lambda to find the enclosing call_expression
            if let Some(call_ancestor) = find_enclosing_call(&parent) {
                if let Some((name, _)) = call_ancestor.call_fn_and_qualifier(bytes) {
                    if SCOPE_FUNCTIONS.contains(&name.as_str()) {
                        return true;
                    }
                }
            }
            return false;
        }
        node = parent;
    }
    false
}

fn find_enclosing_call<'a>(node: &tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut current = node.parent()?;
    for _ in 0..5 {
        if current.kind() == KIND_CALL_EXPR {
            return Some(current);
        }
        current = current.parent()?;
    }
    None
}

fn is_inside_any_lambda(call_node: &tree_sitter::Node) -> bool {
    let mut node = *call_node;
    for _ in 0..15 {
        let Some(parent) = node.parent() else {
            return false;
        };
        if parent.kind() == KIND_LAMBDA_LIT {
            return true;
        }
        node = parent;
    }
    false
}

/// Check if the call has a trailing lambda (lambda as last argument outside parens).
/// CST patterns:
/// - `foo { }` → call_suffix → annotated_lambda → lambda_literal
/// - `foo(a) { }` → outer call_expression wraps inner call_expression + call_suffix(lambda)
/// - Incomplete `foo(a) {` → tree-sitter error recovery may place `{` as a sibling
///
/// Tree-sitter splits `foo(a) { }` into nested call_expressions:
///   call_expression (outer)
///     call_expression (inner) → foo(a)
///     call_suffix → annotated_lambda → lambda_literal
/// We check both the node itself AND its parent for the lambda suffix.
fn has_trailing_lambda(call_node: &tree_sitter::Node) -> bool {
    if check_lambda_in_children(call_node) {
        return true;
    }

    // Nested call_expression: the lambda lives on the parent call_expression
    if let Some(parent) = call_node.parent() {
        if parent.kind() == KIND_CALL_EXPR && check_lambda_in_children(&parent) {
            return true;
        }
    }

    // Incomplete code: tree-sitter may place the `{` as a next sibling
    // outside the call_expression (e.g. `withContext(x) {` with no closing `}`).
    if let Some(next) = call_node.next_sibling() {
        let kind = next.kind();
        if kind == "{" || kind == KIND_LAMBDA_LIT {
            return true;
        }
        if kind == KIND_CALL_SUFFIX && contains_lambda(&next) {
            return true;
        }
        // ERROR node starting with `{` — likely an incomplete lambda
        if kind == "ERROR" {
            if let Some(first_child) = next.child(0) {
                if first_child.kind() == "{" {
                    return true;
                }
            }
        }
    }
    false
}

fn check_lambda_in_children(node: &tree_sitter::Node) -> bool {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.kind() == KIND_LAMBDA_LIT {
                return true;
            }
            if child.kind() == KIND_CALL_SUFFIX && contains_lambda(&child) {
                return true;
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    false
}

/// Recursively check if a node contains a lambda_literal (up to 3 levels deep).
fn contains_lambda(node: &tree_sitter::Node) -> bool {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.kind() == KIND_LAMBDA_LIT {
                return true;
            }

            let mut gc_cursor = child.walk();
            if gc_cursor.goto_first_child() {
                loop {
                    if gc_cursor.node().kind() == KIND_LAMBDA_LIT {
                        return true;
                    }
                    if !gc_cursor.goto_next_sibling() {
                        break;
                    }
                }
            }

            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    false
}

/// Count `value_argument` children inside a `value_arguments` node.
fn count_provided_args(value_arguments: Option<&tree_sitter::Node>, _bytes: &[u8]) -> usize {
    let Some(va) = value_arguments else {
        return 0;
    };
    va.children_of_kind(KIND_VALUE_ARG).len()
}

/// Return `true` if `node` is nested inside a `function_declaration` whose name
/// matches `fn_name`.
///
/// This catches the "delegation override" pattern where a no-arg local override
/// calls the same-named SDK function with arguments:
/// ```kotlin
/// fun setHomeAsUpEnabled() { setHomeAsUpEnabled(true) }
/// ```
/// Without this guard the 1-arg call would be flagged as "expected 0, found 1".
fn is_inside_same_named_function(node: &tree_sitter::Node, bytes: &[u8], fn_name: &str) -> bool {
    let mut cur = *node;
    for _ in 0..25 {
        let Some(parent) = cur.parent() else { break };
        if parent.kind() == KIND_FUN_DECL {
            let matches = parent
                .first_child_of_kind(KIND_SIMPLE_IDENT)
                .and_then(|n| n.utf8_text_owned(bytes))
                .is_some_and(|name| name == fn_name);
            if matches {
                return true;
            }
        }
        cur = parent;
    }
    false
}

/// Build the diagnostic range — prefer the `value_arguments` span, fall back to
/// the callee name span within the call expression.
fn diagnostic_range(
    call_node: &tree_sitter::Node,
    value_arguments: Option<&tree_sitter::Node>,
) -> Range {
    if let Some(va) = value_arguments {
        let start = va.start_position();
        let end = va.end_position();
        return Range::new(
            Position::new(start.row as u32, start.column as u32),
            Position::new(end.row as u32, end.column as u32),
        );
    }
    // Fallback: call expression start
    let start = call_node.start_position();
    let end = call_node.end_position();
    Range::new(
        Position::new(start.row as u32, start.column as u32),
        Position::new(end.row as u32, end.column as u32),
    )
}

#[cfg(test)]
#[path = "call_arg_diagnostics_tests.rs"]
mod tests;
