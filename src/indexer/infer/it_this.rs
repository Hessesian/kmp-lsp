//! `it`/`this` type inference helpers for Kotlin lambda contexts.
//!
//! All functions take explicit `(inputs) -> output` signatures — no hidden state.
//!
//! Public surface (re-exported through `infer::mod`):
//! - `find_it_element_type_in_lines`   — multi-line `it.` (hover + completion)
//! - `find_this_element_type_in_lines` — multi-line hover `this.`
//! - `find_named_lambda_param_type`    — named lambda param (hover + completion)
//! - `is_lambda_param`                 — guard before named-param inference

use tower_lsp::lsp_types::Url;

use crate::indexer::Indexer;
#[cfg(test)]
use crate::indexer::NodeExt;
use crate::types::CursorPos;
use crate::StrExt;

#[cfg(test)]
#[allow(unused_imports)]
pub(super) use super::args::has_named_params_not_it;
#[cfg(test)]
#[allow(unused_imports)]
pub(super) use super::chain::resolve_member_type_on;
pub(crate) use super::cst_lambda::ThisContext;
#[cfg(test)]
#[allow(unused_imports)]
pub(super) use super::cst_lambda::{
    classify_this_lambda_context, cst_lambda_param_type_via_call, is_inside_receiver_lambda,
    lambda_before_brace_context, ThisLambdaCtx,
};
use super::cst_lambda::{
    cst_it_element_type, cst_named_lambda_param_type, cst_this_context, cursor_node_at,
};
use super::speculative::lambda_doc_at;
use super::type_subst::is_generic_param;

/// Guard: the inference resolved to a bare generic placeholder (T, R, E).
/// Without receiver context, this is not a meaningful type — fall through so
/// the caller returns None instead of leaking it.
fn concrete_or_none(type_opt: Option<String>) -> Option<String> {
    match type_opt {
        Some(ref t) if is_generic_param(t) => None,
        other => other,
    }
}

#[cfg(test)]
#[allow(unused_imports)]
pub(super) use super::type_subst::build_ext_fn_type_subst;
#[cfg(test)]
pub(crate) use super::type_subst::find_last_dot_at_depth_zero;

/// Resolve the element type of `it` when inside a lambda (multi-line aware).
///
/// CST-only: the tree comes from [`Indexer::live_doc_or_parse`], so open files
/// use the live tree and indexed-but-not-open files get a transient parse.
/// When the syntax at the cursor is too broken for tree-sitter to form the
/// enclosing `lambda_literal` (unclosed `{` while typing), the resolution runs
/// against an append-only brace-repaired reparse instead — same resolver,
/// repaired tree (see [`super::speculative::lambda_doc_at`]).
///
/// `_lines` is vestigial (the CST resolution reads the tree, not the raw lines);
/// it is kept so the many `_in_lines` call sites keep a stable signature.
pub(crate) fn find_it_element_type_in_lines(
    _lines: &[String],
    pos: CursorPos,
    idx: &Indexer,
    uri: &Url,
) -> Option<String> {
    let resolution_doc = lambda_doc_at(idx, uri, pos)?;
    let doc = resolution_doc.doc();
    let node = cursor_node_at(doc, pos)?;
    concrete_or_none(cst_it_element_type(node, doc, idx, uri))
}

/// Resolve the `this` context at `pos` using the CST of the file at `uri`.
///
/// Returns a [`ThisContext`] that lets callers distinguish between a resolved
/// receiver type, an unresolvable receiver lambda (must not fall back to
/// `enclosing_class_at`), and "not inside any receiver lambda" (fallback valid).
///
/// Uses [`Indexer::live_doc_or_parse`] so the CST path is taken for both open
/// files (live tree) and indexed-but-not-open files (transient parse from the
/// indexed lines).  The `lines` parameter has been removed; callers that still
/// need `lines` for `it`/named-param paths retain their own parameter.
pub(crate) fn find_this_context_in_lines(pos: CursorPos, idx: &Indexer, uri: &Url) -> ThisContext {
    // Same repair-gated tree acquisition as the `it` path: an unclosed `{`
    // (mid-typing) forms no lambda_literal, so `this` inside `with(x) { this.`
    // is only resolvable against the brace-repaired parse.
    let Some(resolution) = lambda_doc_at(idx, uri, pos) else {
        return ThisContext::NotFound;
    };
    let doc = resolution.doc();
    let Some(node) = cursor_node_at(doc, pos) else {
        return ThisContext::NotFound;
    };
    cst_this_context(node, doc, idx, uri)
}

/// Convenience wrapper: returns `Some(type)` when `find_this_context_in_lines`
/// yields a resolved receiver type, `None` otherwise.
pub(crate) fn find_this_element_type_in_lines(
    pos: CursorPos,
    idx: &Indexer,
    uri: &Url,
) -> Option<String> {
    match find_this_context_in_lines(pos, idx, uri) {
        ThisContext::Resolved(resolved_type) => Some(resolved_type),
        ThisContext::InsideReceiver | ThisContext::NotFound => None,
    }
}

/// Resolve the element/receiver type for an EXPLICITLY NAMED lambda parameter
/// (`items.forEach { item -> item.… }`, including multi-line and multi-param
/// `{ index, item -> }` lambdas).
///
/// CST-only: the tree comes from [`Indexer::live_doc_or_parse`], so open files
/// use the live tree and indexed-but-not-open files get a transient parse — the
/// same universal-CST path the `it`/`this` resolvers use.
pub(crate) fn find_named_lambda_param_type(
    param_name: &str,
    pos: CursorPos,
    idx: &Indexer,
    uri: &Url,
) -> Option<String> {
    let doc = idx.live_doc_or_parse(uri)?;
    cst_named_lambda_param_type(pos, param_name, &doc, idx, uri)
}

/// Check whether `recv` looks like an explicitly-named lambda parameter
/// in the current editing context (same line or recent lines).
///
/// Used to avoid triggering lambda inference for ordinary local variables
/// that just happen to be lowercase.  Handles single and multi-param lambdas.
pub(crate) fn is_lambda_param(
    recv: &str,
    before_cur: &str,
    idx: &Indexer,
    uri: &Url,
    cursor_line: usize,
) -> bool {
    // Fast reject: if `recv` starts with uppercase or contains `.` it's a type/qualified
    // name, never a lambda parameter name.
    if recv.starts_with_uppercase() {
        return false;
    }
    if recv.contains('.') {
        return false;
    }

    // Same-line fast check: the lambda declaration may be on the cursor line
    // itself (e.g. `items.forEach { item -> item.`).
    if line_has_lambda_param(before_cur, recv) {
        return true;
    }

    // Delegate to lambda_params_at_col for multi-line detection.  That function
    // uses the CST live-tree when available (O(depth) walk) and falls back to a
    // brace-depth text scan covering up to 50 prior lines — both more thorough
    // than the old 10-line ad-hoc scan here.
    let cursor_col = before_cur.encode_utf16().count();
    idx.lambda_params_at_col(uri, cursor_line, cursor_col)
        .iter()
        .any(|p| p == recv)
}

/// Iterator over `(brace_pos, names_str)` for each `->` in `line` that has a
/// preceding `{`. `names_str` is the text between `{` and `->` (not trimmed).
/// This is the shared scanning kernel used by all lambda-param helpers.
fn lambda_brace_arrows(line: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut search_from = 0usize;
    std::iter::from_fn(move || loop {
        let rel = line[search_from..].find("->")?;
        let arrow_pos = search_from + rel;
        search_from = arrow_pos + 2;
        if let Some(brace_pos) = line[..arrow_pos].rfind('{') {
            let names_str = &line[brace_pos + 1..arrow_pos];
            return Some((brace_pos, names_str));
        }
    })
}

fn names_has_param(names_str: &str, param_name: &str) -> bool {
    names_str.split(',').any(|tok| {
        let n = tok.trim().ident_prefix();
        n == param_name
    })
}

fn param_index_in(names_str: &str, param_name: &str) -> Option<usize> {
    names_str.split(',').enumerate().find_map(|(i, tok)| {
        let n = tok.trim().ident_prefix();
        if n == param_name {
            Some(i)
        } else {
            None
        }
    })
}

/// Returns true if `line` contains a lambda declaration that names `param_name`
/// as one of its parameters (handles single and multi-param patterns):
///   `{ param -> ... }`, `{ a, param, b -> ... }`
pub(crate) fn line_has_lambda_param(line: &str, param_name: &str) -> bool {
    lambda_brace_arrows(line).any(|(_, names)| names_has_param(names, param_name))
}

/// Find the `{` byte position in `line` for the lambda that declares `param_name`.
/// Scans all `->` occurrences (a line may have multiple lambdas).
pub(crate) fn lambda_brace_pos_for_param(line: &str, param_name: &str) -> Option<usize> {
    lambda_brace_arrows(line)
        .find(|(_, names)| names_has_param(names, param_name))
        .map(|(pos, _)| pos)
}

/// 0-based index of `param_name` in a multi-param lambda opening `{ a, b, c ->`.
/// Returns 0 for single-param lambdas.
#[allow(dead_code)]
pub(crate) fn lambda_param_position_on_line(line: &str, param_name: &str) -> usize {
    lambda_brace_arrows(line)
        .find_map(|(_, names)| param_index_in(names, param_name))
        .unwrap_or(0)
}

// ─── test helpers ─────────────────────────────────────────────────────────────

/// Returns `true` if `lambda_node` (a `lambda_literal` CST node) has a
/// `lambda_parameters` child with at least one named parameter that is
/// neither `it` nor `_`.
///
/// Thin wrapper around [`NodeExt::has_lambda_named_params`] for `super::` access
/// in the companion test module.
#[cfg(test)]
pub(super) fn has_lambda_named_params(lambda_node: tree_sitter::Node<'_>, bytes: &[u8]) -> bool {
    lambda_node.has_lambda_named_params(bytes)
}

#[cfg(test)]
#[path = "it_this_tests.rs"]
mod tests;
