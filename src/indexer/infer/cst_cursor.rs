use tower_lsp::lsp_types::{Position, Url};
use tree_sitter::Point;

use crate::indexer::live_tree::utf16_col_to_byte;
use crate::indexer::{Indexer, NodeExt};
use crate::queries::{
    KIND_CALL_EXPR, KIND_FORMAL_PARAMS, KIND_FUN_DECL, KIND_FUN_VALUE_PARAMS, KIND_INIT_DECL,
    KIND_LAMBDA_LIT, KIND_LPAREN, KIND_PRIMARY_CTOR, KIND_PROTOCOL_FUNC_DECL, KIND_RPAREN,
    KIND_VALUE_ARG,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallInfo {
    pub fn_name: String,
    pub qualifier: Option<String>,
    pub active_param: u32,
}

pub(crate) fn cst_call_info(pos: Position, indexer: &Indexer, uri: &Url) -> Option<CallInfo> {
    cst_call_info_skip(pos, indexer, uri, 0)
}

/// Like `cst_call_info` but skips the innermost `skip` call_expressions.
/// `skip = 1` finds the call_expression *containing* the one the cursor is in.
/// Stops at lambda boundaries (`KIND_LAMBDA_LIT`) so it never crosses scope.
pub(crate) fn cst_outer_call_info(pos: Position, indexer: &Indexer, uri: &Url) -> Option<CallInfo> {
    cst_call_info_skip(pos, indexer, uri, 1)
}

fn cst_call_info_skip(pos: Position, indexer: &Indexer, uri: &Url, skip: u32) -> Option<CallInfo> {
    // Use `live_doc_or_parse` rather than `live_doc` to tolerate the race
    // between `textDocument/didChange` (which updates `live_lines` synchronously)
    // and the actor's `spawn_blocking` call that builds the CST into `live_trees`
    // (which is async and may not have completed when signatureHelp arrives).
    // `live_doc_or_parse` falls back to `live_lines` and parses on-demand,
    // then caches the result — so a fast editor like Zed doesn't miss the window.
    let doc = indexer.live_doc_or_parse(uri)?;
    let bytes = &doc.bytes;
    let full_text = std::str::from_utf8(bytes).ok()?;

    let line_idx = pos.line as usize;
    let line_text = full_text.lines().nth(line_idx)?;
    let byte_col = utf16_col_to_byte(line_text, pos.character as usize);
    let point = Point {
        row: line_idx,
        column: byte_col,
    };
    let start_node =
        crate::indexer::node_ext::descendant_for_point(doc.tree.root_node(), bytes, point)?;

    let mut cur = start_node;
    let mut skipped = 0u32;
    // Set to true when the walk passes through a parameter-list node in a
    // function/constructor *definition* (not a call). Used to suppress the
    // text-based fallback so that sig help does not fire while the cursor is
    // inside `fun greet(…)` or `data class User(…)`.
    let mut in_definition = false;
    let call_expr = loop {
        match cur.kind() {
            KIND_CALL_EXPR if skipped < skip => {
                skipped += 1;
                match cur.parent() {
                    Some(parent) => cur = parent,
                    None => break None,
                }
            }
            KIND_CALL_EXPR => break Some(cur),
            KIND_LAMBDA_LIT => break None,
            // Kotlin function/constructor definition parameter lists.
            // Java method/constructor formal parameter lists.
            KIND_FUN_VALUE_PARAMS | KIND_PRIMARY_CTOR | KIND_FORMAL_PARAMS => {
                in_definition = true;
                break None;
            }
            // Swift has no such wrapper node — `(`, `parameter`, `)` are direct
            // children of the declaration itself, which also encloses the
            // function/init body. Only treat this as a definition when the
            // cursor sits within the declaration's own parameter-list span,
            // not merely somewhere inside its body (a call there must still
            // resolve normally, e.g. live-typing an unclosed call).
            KIND_FUN_DECL | KIND_INIT_DECL | KIND_PROTOCOL_FUNC_DECL
                if swift_point_in_own_param_list(cur, point) =>
            {
                in_definition = true;
                break None;
            }
            _ => match cur.parent() {
                Some(parent) => cur = parent,
                None => break None,
            },
        }
    };

    if let Some(call_expr) = call_expr {
        let (fn_name, qualifier) = call_expr.call_fn_and_qualifier(bytes)?;
        let value_arguments = call_expr.find_value_arguments()?;
        let cursor_byte = full_text
            .lines()
            .take(line_idx)
            .map(|line| line.len() + 1)
            .sum::<usize>()
            + byte_col;
        let active_param = count_active_param(&value_arguments, cursor_byte);
        return Some(CallInfo {
            fn_name,
            qualifier,
            active_param,
        });
    }

    // CST fallback: when the closing `)` is absent (live typing mid-argument),
    // tree-sitter cannot build a call_expression node. Fall back to scanning
    // the text before the cursor for the innermost unmatched `(`.
    // Only applies to skip=0 (innermost); outer fallback is not attempted.
    // Suppressed when the cursor is inside a function/constructor *definition*
    // parameter list — those are not call sites.
    if skip == 0 && !in_definition {
        let cursor_byte = full_text
            .lines()
            .take(line_idx)
            .map(|line| line.len() + 1)
            .sum::<usize>()
            + byte_col;
        return text_based_call_info(full_text, cursor_byte);
    }
    None
}

/// Returns `true` when `point` falls within `decl`'s own `(...)` parameter
/// span — as opposed to elsewhere in `decl` (e.g. its function body), which
/// for Swift's flat `function_declaration`/`init_declaration`/
/// `protocol_function_declaration` nodes is also part of the same node.
fn swift_point_in_own_param_list(decl: tree_sitter::Node, point: Point) -> bool {
    let mut open: Option<tree_sitter::Node> = None;
    let mut close: Option<tree_sitter::Node> = None;
    let mut cursor = decl.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            match child.kind() {
                KIND_LPAREN if open.is_none() => open = Some(child),
                KIND_RPAREN if open.is_some() && close.is_none() => close = Some(child),
                _ => {}
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    let (Some(open), Some(close)) = (open, close) else {
        return false;
    };
    let p = (point.row, point.column);
    let start = open.start_position();
    let end = close.end_position();
    p >= (start.row, start.column) && p <= (end.row, end.column)
}

fn count_active_param(value_arguments: &tree_sitter::Node, cursor_byte: usize) -> u32 {
    let mut count = 0u32;
    let mut walker = value_arguments.walk();
    for child in value_arguments.children(&mut walker) {
        if child.kind() == KIND_VALUE_ARG {
            if child.end_byte() <= cursor_byte {
                count += 1;
            } else {
                break;
            }
        }
    }
    count
}

/// Text-based fallback for when the CST cannot find a `call_expression`
/// (e.g., the closing `)` is absent during live editing).
///
/// Scans `line_text[..byte_col]` backwards to find the innermost unclosed `(`
/// and extracts the callee name, qualifier, and active parameter index.
fn text_based_call_info(line_text: &str, byte_col: usize) -> Option<CallInfo> {
    let before = &line_text[..byte_col.min(line_text.len())];
    let open = innermost_open_paren(before)?;
    let (fn_name, qualifier) = extract_callee(&before[..open])?;
    let active_param = count_depth0_commas(&before[open + 1..]);
    Some(CallInfo {
        fn_name,
        qualifier,
        active_param,
    })
}

/// Returns the byte index of the innermost unmatched `(` in `text`.
fn innermost_open_paren(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
            b'(' => return Some(i),
            _ => {}
        }
    }
    None
}

/// Extract `(fn_name, qualifier)` from the text immediately before a `(`.
/// `"    Foo.bar"` → `("bar", Some("Foo"))`, `"    greet"` → `("greet", None)`.
fn extract_callee(before_paren: &str) -> Option<(String, Option<String>)> {
    let trimmed = before_paren.trim_end();
    let fn_name: String = trimmed
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if fn_name.is_empty() {
        return None;
    }
    let rest = trimmed[..trimmed.len() - fn_name.len()].trim_end();
    let qualifier = if let Some(before_dot) = rest.strip_suffix('.') {
        let before_dot = before_dot.trim_end();
        let q: String = before_dot
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if q.is_empty() {
            None
        } else {
            Some(q)
        }
    } else {
        None
    };
    Some((fn_name, qualifier))
}

/// Count commas at paren/bracket depth 0 in `text` (for active_param).
/// Ignores commas inside string literals, nested parens, and brackets.
fn count_depth0_commas(text: &str) -> u32 {
    let mut depth = 0i32;
    let mut count = 0u32;
    let mut in_string = false;
    let mut prev = '\0';
    for ch in text.chars() {
        if in_string {
            if ch == '"' && prev != '\\' {
                in_string = false;
            }
        } else {
            match ch {
                '"' => in_string = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' if depth > 0 => depth -= 1,
                ',' if depth == 0 => count += 1,
                _ => {}
            }
        }
        prev = ch;
    }
    count
}
