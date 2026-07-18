//! Cursor-context resolution helpers: word extraction, qualifier parsing,
//! lambda parameter inference, enclosing-class lookup.

use std::sync::Arc;
use tower_lsp::lsp_types::*;
use tree_sitter::Point;

use super::{
    find_as_call_arg_type, find_it_element_type_in_lines, find_named_lambda_param_type_in_lines,
    find_this_context_in_lines, lambda_brace_pos_for_param, line_has_lambda_param, Indexer,
    ThisContext,
};
use crate::indexer::live_tree::utf16_col_to_byte;
use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CATCH_BLOCK, KIND_CLASS_BODY, KIND_CLASS_DECL, KIND_COMPANION_OBJ, KIND_ENUM_CLASS_BODY,
    KIND_FOR_STMT, KIND_FUN_BODY, KIND_FUN_DECL, KIND_FUN_VALUE_PARAMS, KIND_INTERFACE_DECL,
    KIND_LAMBDA_LIT, KIND_MULTI_VAR_DECL, KIND_NULLABLE_TYPE, KIND_OBJECT_DECL, KIND_PARAMETER,
    KIND_PROP_DECL, KIND_SIMPLE_IDENT, KIND_SOURCE_FILE, KIND_USER_TYPE, KIND_VAR_DECL,
    KIND_WHEN_EXPR, KIND_WHEN_SUBJECT,
};
use crate::types::CursorPos;
use crate::StrExt;

/// Lines to scan backward when resolving variable types and lambda receivers from scope.
const SCOPE_SCAN_BACK_LINES: usize = 50;

/// Lines to scan upward when looking for a local variable declaration.
const DECL_SCAN_UP_LINES: usize = 15;

/// Lines to scan backward when looking for an enclosing call during named-argument scanning.
const ENCLOSING_CALL_SCAN_BACK: usize = 20;

impl Indexer {
    /// LSP positions are UTF-16; for ASCII-heavy Kotlin/Java identifiers the
    /// character offset is identical to the UTF-16 unit offset.
    pub(crate) fn word_at(&self, uri: &Url, position: Position) -> Option<String> {
        self.word_and_qualifier_at(uri, position).map(|(w, _)| w)
    }

    /// Like `word_at` but also returns the `Range` of the word in LSP (UTF-16) coordinates.
    pub(crate) fn word_and_range_at(
        &self,
        uri: &Url,
        position: Position,
    ) -> Option<(String, Range)> {
        let lines = self.lines_for(uri)?;
        let line_text = lines.get(position.line as usize)?;
        let target_utf16 = position.character as usize;
        let mut utf16_acc = 0usize;
        let mut char_idx = 0usize;
        for ch in line_text.chars() {
            if utf16_acc >= target_utf16 {
                break;
            }
            utf16_acc += ch.len_utf16();
            char_idx += 1;
        }
        let chars: Vec<char> = line_text.chars().collect();
        let effective = if char_idx < chars.len() && is_id_char(chars[char_idx]) {
            char_idx
        } else if char_idx > 0 && is_id_char(chars[char_idx - 1]) {
            char_idx - 1
        } else {
            return None;
        };
        let start_char = (0..=effective)
            .rev()
            .find(|&i| !is_id_char(chars[i]))
            .map(|i| i + 1)
            .unwrap_or(0);
        let end_char = (effective..chars.len())
            .find(|&i| !is_id_char(chars[i]))
            .unwrap_or(chars.len());
        if start_char >= end_char {
            return None;
        }
        let word: String = chars[start_char..end_char].iter().collect();
        if word == "_" {
            return None;
        }
        // Compute UTF-16 columns for start and end.
        let start_utf16 = chars[..start_char]
            .iter()
            .map(|c| c.len_utf16() as u32)
            .sum::<u32>();
        let end_utf16 = start_utf16
            + chars[start_char..end_char]
                .iter()
                .map(|c| c.len_utf16() as u32)
                .sum::<u32>();
        let range = Range {
            start: Position::new(position.line, start_utf16),
            end: Position::new(position.line, end_utf16),
        };
        Some((word, range))
    }

    /// Returns a clone of the live (possibly unsaved) lines for a URI.
    pub(crate) fn lines_for(&self, uri: &Url) -> Option<Arc<Vec<String>>> {
        // Prefer live (unsaved) lines, fall back to indexed file.
        if let Some(live) = self.live_lines.get(uri.as_str()) {
            return Some(live.clone());
        }
        if let Some(f) = self.files.get(uri.as_str()) {
            return Some(f.lines.clone());
        }
        // File not indexed yet (cold start / indexing in progress) — read from disk
        // so that word_at / word_and_qualifier_at work and rg fallbacks can fire.
        if let Ok(path) = uri.to_file_path() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
                return Some(Arc::new(lines));
            }
        }
        None
    }

    /// Like `word_at` but also returns the single dot-qualifier immediately
    /// preceding the word, if any.
    ///
    /// `AccountPickerMapper.Content`  cursor on `Content`
    ///   → `Some(("Content", Some("AccountPickerMapper")))`
    ///
    /// `List<StaticDocument>` cursor on `StaticDocument`
    ///   → `Some(("StaticDocument", None))`
    pub(crate) fn word_and_qualifier_at(
        &self,
        uri: &Url,
        position: Position,
    ) -> Option<(String, Option<String>)> {
        let lines = self.lines_for(uri)?;
        let line = lines.get(position.line as usize)?;

        // UTF-16 → char index
        let target_utf16 = position.character as usize;
        let mut utf16_acc = 0usize;
        let mut char_idx = 0usize;
        for ch in line.chars() {
            if utf16_acc >= target_utf16 {
                break;
            }
            utf16_acc += ch.len_utf16();
            char_idx += 1;
        }

        let chars: Vec<char> = line.chars().collect();
        let effective = if char_idx < chars.len() && is_id_char(chars[char_idx]) {
            char_idx
        } else if char_idx > 0 && is_id_char(chars[char_idx - 1]) {
            char_idx - 1
        } else {
            return None;
        };

        let start = (0..=effective)
            .rev()
            .find(|&i| !is_id_char(chars[i]))
            .map(|i| i + 1)
            .unwrap_or(0);

        let end = (effective..chars.len())
            .find(|&i| !is_id_char(chars[i]))
            .unwrap_or(chars.len());

        if start >= end {
            return None;
        }
        let word: String = chars[start..end].iter().collect();
        if word == "_" {
            return None;
        }

        // Scan back over the full dot-chain preceding the word.
        // `A.B.C.D` cursor on `D` → qualifier `"A.B.C"`, not just `"C"`.
        // `resolve_qualified` then uses the ROOT segment ("A") to locate the file
        // and searches that file for the word ("D"), handling arbitrary nesting depth.
        let qualifier = if start >= 2 && chars[start - 1] == '.' {
            let mut scan = start - 1; // pointing at the final dot
            while scan > 0 && (is_id_char(chars[scan - 1]) || chars[scan - 1] == '.') {
                scan -= 1;
            }
            let q: String = chars[scan..start - 1].iter().collect();
            let q = q.trim_start_matches('.').to_string();
            if !q.is_empty() && q != "_" {
                Some(q)
            } else {
                None
            }
        } else {
            // No dot-qualifier. Check if this looks like a named argument: `word = value`
            // (but NOT `word ==`). If so, scan backward for the enclosing call's name
            // and use that as the qualifier so we search the constructor/function's params.
            let after: String = chars[end..].iter().collect();
            let after_trimmed = after.trim_start();
            let is_named_arg = after_trimmed.starts_with('=') && !after_trimmed.starts_with("==");
            if is_named_arg {
                find_enclosing_call_name(&lines, position.line as usize, start)
                    .and_then(|callee| callee_to_qualifier(&callee))
            } else {
                None
            }
        };

        Some((word, qualifier))
    }

    /// If `name` at `position` is `it` or a named lambda parameter, return the
    /// inferred element/receiver type name (e.g. `"Product"`, `"User"`).
    ///
    /// Used by hover and go-to-definition to provide useful info for lambda params.
    /// Handles both same-line and multi-line lambda declarations by scanning
    /// backward through file lines (not just the text before the cursor).
    pub(crate) fn infer_lambda_param_type_at(
        &self,
        name: &str,
        uri: &Url,
        position: Position,
    ) -> Option<String> {
        self.infer_lambda_param_type_at_with_cache(name, uri, position, None)
    }

    pub(crate) fn infer_lambda_param_type_at_with_cache(
        &self,
        name: &str,
        uri: &Url,
        position: Position,
        parse_cache: Option<&mut super::RequestParseCache>,
    ) -> Option<String> {
        let line_no = position.line as usize;

        // Prefer live_lines (current editor content, updated synchronously on
        // did_change) over files.lines (refreshed after debounced reindex).
        // Type resolution still uses the index (definitions, files) by name —
        // that data remains valid even before reindex completes.
        let lines: Arc<Vec<String>> = self.mem_lines_for(uri.as_str()).or_else(|| {
            self.files
                .get(uri.as_str())
                .map(|file_data| file_data.lines.clone())
        })?;

        if name == "it" || name == "this" {
            let pos = CursorPos {
                line: line_no,
                utf16_col: position.character as usize,
            };
            let lambda_type = if name == "this" {
                match find_this_context_in_lines(&lines, pos, self, uri) {
                    ThisContext::Resolved(ty) => return Some(ty),
                    // Inside a receiver lambda but type unknown: `this` is the lambda
                    // receiver, not the enclosing class.  Do not fall back.
                    ThisContext::InsideReceiver => return None,
                    // Not in any receiver lambda: fall through to `find_as_call_arg_type`
                    // and the `enclosing_class_at` fallback below.
                    ThisContext::NotFound => None,
                }
            } else {
                find_it_element_type_in_lines(&lines, pos, self, uri)
            };
            if lambda_type.is_some() {
                return lambda_type;
            }
            // Type-directed fallback: if `it`/`this` is a call argument (named or
            // positional), look up the expected parameter type from the function signature.
            // Mimics Kotlin's type-directed implicit-receiver / lambda-param resolution.
            if let Some(ty) = find_as_call_arg_type(&lines, pos, self, uri) {
                return Some(ty);
            }
            // Fallback for `this` in a regular class method body (not a lambda):
            // scan backward for the enclosing class/object declaration.
            if name == "this" {
                return self.enclosing_class_at_with_cache(uri, position.line, parse_cache);
            }
            None
        } else {
            // For named params: scan backward for `{ name ->` pattern.
            // Pass the real UTF-16 column so the CST fast-path places the cursor
            // inside the correct lambda_literal (multi-line receiver chain case).
            // Snapshot live_doc ONCE here so the CST path uses the same tree
            // that produced `position` — prevents a race where did_change updates
            // live_doc between the caller's position derivation and our CST lookup.
            let utf16_col = position.character as usize;
            let live_doc_arc = self.live_doc_for_scope_query(uri, parse_cache)?;
            find_named_lambda_param_type_in_lines(
                &lines,
                name,
                line_no,
                utf16_col,
                Some(live_doc_arc.as_ref()),
                self,
                uri,
            )
        }
    }

    /// Lambda parameter names that are **in scope** at `(cursor_line, cursor_col)`.
    ///
    /// Uses the same brace-depth backward-scan algorithm as
    /// `find_it_element_type_in_lines`: `}` increments depth, `{` decrements;
    /// when depth < 0 we've found an *enclosing* `{` lambda.  Sibling/inner lambdas
    /// whose closing `}` appears before their `{` in the backward scan self-balance
    /// and never trigger depth < 0, so they are correctly excluded.
    ///
    /// Example — cursor inside `{ resultState -> … }`:
    ///   `reloadableProduct(…, { isRefresh -> … }) { resultState -> │ }`
    ///   → returns `["resultState"]`,  NOT `["isRefresh", "resultState"]`
    #[allow(dead_code)] // used by scope_tests; convenience wrapper over `lambda_params_at_col`
    pub(crate) fn lambda_params_at(&self, uri: &Url, cursor_line: usize) -> Vec<String> {
        self.lambda_params_at_col(uri, cursor_line, usize::MAX)
    }

    /// Like `lambda_params_at` but also respects `cursor_col` when scanning the
    /// cursor line.  Passing `usize::MAX` is equivalent to `lambda_params_at`.
    ///
    /// The column limit prevents the closing `}` of an inline lambda from being
    /// seen when the cursor is inside that lambda on the same line:
    ///   `loan = { loanId, isWustenrot -> setEvent(...) },`
    ///                                                  ^ cursor here
    /// Without the limit, the scan hits `}` first (depth→1), then `{` resets to 0
    /// (not <0), so the lambda params are never collected.
    pub(crate) fn lambda_params_at_col(
        &self,
        uri: &Url,
        cursor_line: usize,
        cursor_col: usize,
    ) -> Vec<String> {
        self.lambda_params_at_col_with_cache(uri, cursor_line, cursor_col, None)
    }

    pub(crate) fn lambda_params_at_col_with_cache(
        &self,
        uri: &Url,
        cursor_line: usize,
        cursor_col: usize,
        parse_cache: Option<&mut super::RequestParseCache>,
    ) -> Vec<String> {
        if let Some(params) =
            self.cst_lambda_params_at_col(uri, cursor_line, cursor_col, parse_cache)
        {
            return params;
        }

        let lines = self.lambda_param_scan_lines(uri);
        scan_lambda_params_in_lines(&lines, cursor_line, cursor_col)
    }

    fn cst_lambda_params_at_col(
        &self,
        uri: &Url,
        cursor_line: usize,
        cursor_col: usize,
        parse_cache: Option<&mut super::RequestParseCache>,
    ) -> Option<Vec<String>> {
        let doc = self.live_doc_for_scope_query(uri, parse_cache)?;
        let line_text = self
            .live_lines
            .get(uri.as_str())
            .and_then(|lines| lines.get(cursor_line).cloned())
            .unwrap_or_default();
        let point = lambda_cursor_point(&line_text, cursor_line, cursor_col);
        let node = doc
            .tree
            .root_node()
            .descendant_for_point_range(point, point)?;
        Some(collect_cst_lambda_params(node, &doc.bytes))
    }

    fn lambda_param_scan_lines(&self, uri: &Url) -> Arc<Vec<String>> {
        self.live_lines
            .get(uri.as_str())
            .map(|lines| lines.clone())
            .or_else(|| self.files.get(uri.as_str()).map(|file| file.lines.clone()))
            .unwrap_or_default()
    }

    /// Find the `{ name ->` declaration line for a lambda parameter in scope at
    /// `cursor_line`.  Returns a `Location` pointing to the opening `{` of the
    /// enclosing lambda (the parameter's declaration site).
    pub(crate) fn find_lambda_param_decl(
        &self,
        uri: &Url,
        param_name: &str,
        cursor_line: usize,
    ) -> Option<Location> {
        let lines = self
            .live_lines
            .get(uri.as_str())
            .map(|ll| ll.clone())
            .or_else(|| self.files.get(uri.as_str()).map(|f| f.lines.clone()))?;

        let scan_start = cursor_line.saturating_sub(SCOPE_SCAN_BACK_LINES);
        for ln in (scan_start..=cursor_line).rev() {
            let line = match lines.get(ln) {
                Some(l) => l,
                None => continue,
            };
            if !line_has_lambda_param(line, param_name) {
                continue;
            }
            if let Some(brace_pos) = lambda_brace_pos_for_param(line, param_name) {
                let char_col = line[..brace_pos].chars().count() as u32;
                return Some(Location {
                    uri: uri.clone(),
                    range: tower_lsp::lsp_types::Range {
                        start: tower_lsp::lsp_types::Position {
                            line: ln as u32,
                            character: char_col,
                        },
                        end: tower_lsp::lsp_types::Position {
                            line: ln as u32,
                            character: char_col + 1,
                        },
                    },
                });
            }
        }
        None
    }

    /// Infer the declared type of `var_name` visible at `position`, walking the
    /// CST from the cursor outward: function params → local `val`/`var` → class
    /// members → file-global fallback.
    pub(crate) fn variable_type_at(
        &self,
        uri: &Url,
        var_name: &str,
        position: Position,
    ) -> Option<String> {
        self.variable_type_at_from_cst(uri, var_name, position, None)
    }

    fn variable_type_at_from_cst(
        &self,
        uri: &Url,
        var_name: &str,
        position: Position,
        parse_cache: Option<&mut super::RequestParseCache>,
    ) -> Option<String> {
        let doc = self.live_doc_for_scope_query(uri, parse_cache)?;
        let line_text = self
            .lines_for(uri)
            .and_then(|lines| lines.get(position.line as usize).cloned())
            .unwrap_or_default();
        let byte_column = utf16_col_to_byte(&line_text, position.character as usize);
        let point = Point {
            row: position.line as usize,
            column: byte_column,
        };
        let cursor_node = doc
            .tree
            .root_node()
            .descendant_for_point_range(point, point)?;
        let bytes = doc.bytes.as_slice();

        if let Some(scope_root) = enclosing_local_scope_subtree(cursor_node) {
            for search_root in local_scope_search_roots(scope_root) {
                if let Some(local_type) =
                    local_type_for_name_in_subtree_before(search_root, bytes, var_name, point)
                {
                    return Some(local_type);
                }
            }
        }

        let mut node = cursor_node;
        while let Some(parent) = node.parent() {
            if matches!(
                parent.kind(),
                KIND_CLASS_BODY | KIND_ENUM_CLASS_BODY | KIND_SOURCE_FILE
            ) {
                return member_property_type(parent, var_name, bytes);
            }
            node = parent;
        }
        None
    }

    /// True when `name` at `(line, utf16_column)` is bound by a nearer local
    /// declaration — a `val`/`var`, a function parameter, or a lambda parameter —
    /// that shadows any implicit-receiver member of the same name.
    ///
    /// Kotlin resolves a bare identifier to an in-scope local before consulting
    /// an implicit receiver (`with(binding) { … }`, `binding.apply { … }`), so
    /// callers that treat a bare name as `this.name` must first rule out a
    /// shadowing local. The walk stops at the enclosing function and at type
    /// bodies: class/object/enum members and top-level declarations never shadow
    /// an inner receiver-lambda member (the innermost implicit receiver wins).
    pub(crate) fn name_shadowed_by_local_declaration(
        &self,
        uri: &Url,
        line: usize,
        utf16_column: usize,
        name: &str,
    ) -> bool {
        self.name_shadowed_by_local_declaration_with_cache(uri, line, utf16_column, name, None)
    }

    pub(crate) fn name_shadowed_by_local_declaration_with_cache(
        &self,
        uri: &Url,
        line: usize,
        utf16_column: usize,
        name: &str,
        parse_cache: Option<&mut super::RequestParseCache>,
    ) -> bool {
        let Some(doc) = self.live_doc_for_scope_query(uri, parse_cache) else {
            return false;
        };
        let line_text = self
            .lines_for(uri)
            .and_then(|lines| lines.get(line).cloned())
            .unwrap_or_default();
        let byte_column = utf16_col_to_byte(&line_text, utf16_column);
        let point = Point {
            row: line,
            column: byte_column,
        };
        let Some(cursor_node) = doc
            .tree
            .root_node()
            .descendant_for_point_range(point, point)
        else {
            return false;
        };
        let bytes = doc.bytes.as_slice();
        if let Some(scope_root) = enclosing_local_scope_subtree(cursor_node) {
            for search_root in local_scope_search_roots(scope_root) {
                if local_name_bound_in_subtree(search_root, bytes, name) {
                    return true;
                }
            }
        }
        false
    }

    /// Find the name of the innermost enclosing class/interface/object
    /// that contains `row` in the given file.
    ///
    /// Used by `references` to scope a short symbol name (e.g. `Loading`) to
    /// its parent sealed class so we can filter out unrelated `Loading` classes
    /// in other sealed hierarchies.
    pub(crate) fn enclosing_class_at(&self, uri: &Url, row: u32) -> Option<String> {
        self.enclosing_class_at_with_cache(uri, row, None)
    }

    pub(crate) fn enclosing_class_at_with_cache(
        &self,
        uri: &Url,
        row: u32,
        parse_cache: Option<&mut super::RequestParseCache>,
    ) -> Option<String> {
        self.enclosing_class_at_impl(uri, row, parse_cache)
    }

    fn enclosing_class_at_impl(
        &self,
        uri: &Url,
        row: u32,
        parse_cache: Option<&mut super::RequestParseCache>,
    ) -> Option<String> {
        let row = row as usize;

        if let Some(doc) = self.live_doc_for_scope_query(uri, parse_cache) {
            // Use the first non-whitespace byte on the row as the probe column.
            let probe_col = self
                .live_lines
                .get(uri.as_str())
                .and_then(|ll| ll.get(row).cloned())
                .map(|l| l.len() - l.trim_start().len())
                .unwrap_or(0);
            let point = Point {
                row,
                column: probe_col,
            };
            if let Some(node) = doc
                .tree
                .root_node()
                .descendant_for_point_range(point, point)
            {
                let mut cur = node;
                loop {
                    match cur.kind() {
                        KIND_CLASS_DECL | KIND_INTERFACE_DECL | KIND_OBJECT_DECL
                        | KIND_COMPANION_OBJ
                            if cur.start_position().row < row =>
                        {
                            // Guard: cursor must be inside the class body, not on the
                            // declaration header (annotations can push the start row
                            // of the declaration *above* the `class/interface` keyword
                            // line, so `start_position().row < row` is insufficient).
                            let body_inside = cur.children(&mut cur.walk()).any(|c| {
                                c.kind() == KIND_CLASS_BODY && c.start_position().row < row
                            });
                            if body_inside {
                                if let Some(name) = cur.extract_type_name(&doc.bytes) {
                                    return Some(name);
                                }
                            }
                        }
                        _ => {}
                    }
                    match cur.parent() {
                        Some(p) => cur = p,
                        None => break,
                    }
                }
            }
        }

        // ── Text fallback ────────────────────────────────────────────────────
        let file = self.files.get(uri.as_str())?;
        let mut depth = 0i32;
        let end = row.min(file.lines.len().saturating_sub(1));
        for i in (0..=end).rev() {
            let line = match file.lines.get(i) {
                Some(l) => l,
                None => continue,
            };
            for ch in line.chars().rev() {
                match ch {
                    '}' => depth += 1,
                    '{' => depth -= 1,
                    _ => {}
                }
            }
            if depth < 0 && i < row {
                let t = line.trim();
                if let Some(name) = extract_class_decl_name(t) {
                    return Some(name);
                }
                let scan_up = i.saturating_sub(DECL_SCAN_UP_LINES);
                for j in (scan_up..i).rev() {
                    if let Some(prev) = file.lines.get(j) {
                        if let Some(name) = extract_class_decl_name(prev.trim()) {
                            return Some(name);
                        }
                        let pt = prev.trim();
                        if pt.starts_with('}') || pt.ends_with('}') {
                            break;
                        }
                    }
                }
                depth = 0;
            }
        }
        None
    }
}

/// Thin wrapper around [`NodeExt::collect_lambda_param_names`] for `super::` access
/// in the companion test module.
#[cfg(test)]
fn collect_lambda_param_names(
    lambda_node: tree_sitter::Node<'_>,
    bytes: &[u8],
    existing: &[String],
) -> Vec<String> {
    lambda_node.collect_lambda_param_names(bytes, existing)
}

/// If `line` is a class/interface/object/sealed declaration, return the type name.
pub(super) fn extract_class_decl_name(line: &str) -> Option<String> {
    // Strip common modifiers: Kotlin + Java + Swift
    let mut rest = line;
    let modifiers = [
        "abstract ",
        "sealed ",
        "data ",
        "open ",
        "inner ",
        "private ",
        "protected ",
        "public ",
        "internal ",
        "inline ",
        "value ",
        "enum ",
        "companion ",
        "override ",
        "final ",
        // Swift-specific
        "fileprivate ",
        "@objc ",
        "static ",
        "final ",
    ];
    loop {
        let before = rest;
        for m in &modifiers {
            rest = rest.strip_prefix(m).unwrap_or(rest).trim_start();
        }
        // Skip @Annotations (Kotlin) and @attributes (Swift)
        if rest.starts_with('@') {
            if let Some(after) = rest.find(' ') {
                rest = rest[after..].trim_start();
            }
        }
        if rest == before {
            break;
        }
    }
    // Now rest should start with a type keyword
    let rest = rest
        .strip_prefix("class ")
        .or_else(|| rest.strip_prefix("interface "))
        .or_else(|| rest.strip_prefix("object "))
        .or_else(|| rest.strip_prefix("struct "))
        .or_else(|| rest.strip_prefix("protocol "))
        .or_else(|| rest.strip_prefix("extension "))?;
    // Extract the identifier
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() || !name.starts_with_uppercase() {
        return None;
    }
    Some(name)
}

/// Names declared by a function declaration's value-parameter list.
fn function_value_parameter_names(
    function_node: tree_sitter::Node<'_>,
    bytes: &[u8],
) -> Vec<String> {
    let Some(parameters) = function_node.first_child_of_kind(KIND_FUN_VALUE_PARAMS) else {
        return Vec::new();
    };
    parameters
        .children_of_kind(KIND_PARAMETER)
        .into_iter()
        .filter_map(|parameter| parameter.first_child_of_kind(KIND_SIMPLE_IDENT))
        .filter_map(|identifier| identifier.utf8_text_owned(bytes))
        .collect()
}

/// True when `node` is a `val`/`var` declaration whose bound name is `name`.
fn property_declaration_binds_name(node: tree_sitter::Node<'_>, bytes: &[u8], name: &str) -> bool {
    if node.kind() != KIND_PROP_DECL {
        return false;
    }
    let Some(variable_declaration) = node.first_child_of_kind(KIND_VAR_DECL) else {
        return false;
    };
    variable_declaration
        .first_child_of_kind(KIND_SIMPLE_IDENT)
        .and_then(|identifier| identifier.utf8_text_owned(bytes))
        .as_deref()
        == Some(name)
}

fn function_parameter_type(
    function_node: tree_sitter::Node<'_>,
    var_name: &str,
    bytes: &[u8],
) -> Option<String> {
    let parameters = function_node.first_child_of_kind(KIND_FUN_VALUE_PARAMS)?;
    for parameter in parameters.children_of_kind(KIND_PARAMETER) {
        if let Some(parameter_type) = parameter_type_if_named(parameter, var_name, bytes) {
            return Some(parameter_type);
        }
    }
    None
}

fn parameter_type_if_named(
    parameter: tree_sitter::Node<'_>,
    var_name: &str,
    bytes: &[u8],
) -> Option<String> {
    let identifier = parameter.first_child_of_kind(KIND_SIMPLE_IDENT)?;
    if identifier.utf8_text_owned(bytes).as_deref() != Some(var_name) {
        return None;
    }
    type_annotation_from_node(parameter, bytes)
}

fn property_declaration_type(
    node: tree_sitter::Node<'_>,
    var_name: &str,
    bytes: &[u8],
) -> Option<String> {
    if !property_declaration_binds_name(node, bytes, var_name) {
        return None;
    }
    let variable_declaration = node.first_child_of_kind(KIND_VAR_DECL)?;
    type_annotation_from_node(variable_declaration, bytes)
        .or_else(|| initializer_type_from_variable_declaration(variable_declaration, bytes))
        .or_else(|| {
            crate::viewbinding::view_binding_delegate_type_from_property(node, bytes, var_name)
        })
}

fn initializer_type_from_variable_declaration(
    variable_declaration: tree_sitter::Node<'_>,
    bytes: &[u8],
) -> Option<String> {
    let mut cursor = variable_declaration.walk();
    for child in variable_declaration.children(&mut cursor) {
        if child.kind() == "=" {
            continue;
        }
        if child.kind() == KIND_SIMPLE_IDENT {
            continue;
        }
        if let Some(type_name) = infer_type_from_initializer_node(child, bytes) {
            return Some(type_name);
        }
    }
    None
}

fn infer_type_from_initializer_node(node: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    crate::viewbinding::binding_type_from_initializer_node(node, bytes)
}

/// Innermost lambda or function body between `cursor_node` and the nearest
/// class/type-body boundary — the scope that governs local shadowing.
fn enclosing_local_scope_subtree(cursor_node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut node = cursor_node;
    let mut candidate = None;
    loop {
        match node.kind() {
            KIND_LAMBDA_LIT => candidate = Some(node),
            KIND_FUN_DECL => {
                if let Some(body) = node.first_child_of_kind(KIND_FUN_BODY) {
                    candidate = Some(body);
                }
            }
            _ => {}
        }
        let Some(parent) = node.parent() else {
            return candidate;
        };
        if matches!(
            parent.kind(),
            KIND_CLASS_BODY | KIND_ENUM_CLASS_BODY | KIND_SOURCE_FILE
        ) {
            return candidate;
        }
        node = parent;
    }
}

fn local_scope_search_roots(scope_root: tree_sitter::Node) -> Vec<tree_sitter::Node> {
    if scope_root.kind() == KIND_FUN_BODY {
        if let Some(function_declaration) = scope_root.parent() {
            return vec![function_declaration, scope_root];
        }
    }
    vec![scope_root]
}

fn local_name_bound_in_subtree(node: tree_sitter::Node<'_>, bytes: &[u8], name: &str) -> bool {
    if local_binding_binds_name(node, bytes, name) {
        return true;
    }
    if node.kind() == KIND_FUN_DECL
        && function_value_parameter_names(node, bytes)
            .iter()
            .any(|parameter| parameter == name)
    {
        return true;
    }
    if node.kind() == KIND_LAMBDA_LIT
        && node
            .lambda_param_names(bytes)
            .iter()
            .any(|parameter| parameter == name)
    {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if local_name_bound_in_subtree(child, bytes, name) {
            return true;
        }
    }
    false
}

fn local_type_for_name_in_subtree_before(
    node: tree_sitter::Node<'_>,
    bytes: &[u8],
    name: &str,
    before: Point,
) -> Option<String> {
    if node.start_position() >= before {
        return None;
    }
    if node.kind() == KIND_FUN_DECL {
        if let Some(parameter_type) = function_parameter_type(node, name, bytes) {
            return Some(parameter_type);
        }
    }
    if let Some(local_type) = property_declaration_type(node, name, bytes) {
        return Some(local_type);
    }
    let mut cursor = node.walk();
    let mut latest_match = None;
    for child in node.children(&mut cursor) {
        if child.start_position() >= before {
            continue;
        }
        if let Some(found) = local_type_for_name_in_subtree_before(child, bytes, name, before) {
            latest_match = Some(found);
        }
    }
    latest_match
}

/// True when `node` introduces a local binding for `name` (val/var, for, catch, when-subject, destructuring).
fn local_binding_binds_name(node: tree_sitter::Node<'_>, bytes: &[u8], name: &str) -> bool {
    match node.kind() {
        KIND_PROP_DECL => {
            property_declaration_binds_name(node, bytes, name)
                || destructuring_binds_name(node, bytes, name)
        }
        KIND_FOR_STMT => for_loop_binds_name(node, bytes, name),
        KIND_CATCH_BLOCK => catch_block_binds_name(node, bytes, name),
        KIND_WHEN_EXPR => when_subject_binds_name(node, bytes, name),
        _ => false,
    }
}

fn destructuring_binds_name(node: tree_sitter::Node<'_>, bytes: &[u8], name: &str) -> bool {
    let Some(multi) = node.first_child_of_kind(KIND_MULTI_VAR_DECL) else {
        return false;
    };
    let mut cursor = multi.walk();
    for child in multi.children(&mut cursor) {
        if child.kind() != KIND_VAR_DECL {
            continue;
        }
        if child
            .first_child_of_kind(KIND_SIMPLE_IDENT)
            .and_then(|identifier| identifier.utf8_text_owned(bytes))
            .as_deref()
            == Some(name)
        {
            return true;
        }
    }
    false
}

fn for_loop_binds_name(for_node: tree_sitter::Node<'_>, bytes: &[u8], name: &str) -> bool {
    if let Some(multi) = for_node.first_child_of_kind(KIND_MULTI_VAR_DECL) {
        return destructuring_in_multi_binds_name(multi, bytes, name);
    }
    let Some(variable_declaration) = for_node.first_child_of_kind(KIND_VAR_DECL) else {
        return false;
    };
    variable_declaration
        .first_child_of_kind(KIND_SIMPLE_IDENT)
        .and_then(|identifier| identifier.utf8_text_owned(bytes))
        .as_deref()
        == Some(name)
}

fn catch_block_binds_name(catch_node: tree_sitter::Node<'_>, bytes: &[u8], name: &str) -> bool {
    let mut cursor = catch_node.walk();
    for child in catch_node.children(&mut cursor) {
        if child.kind() != KIND_PARAMETER {
            continue;
        }
        if child
            .first_child_of_kind(KIND_SIMPLE_IDENT)
            .and_then(|identifier| identifier.utf8_text_owned(bytes))
            .as_deref()
            == Some(name)
        {
            return true;
        }
    }
    false
}

fn when_subject_binds_name(when_node: tree_sitter::Node<'_>, bytes: &[u8], name: &str) -> bool {
    let Some(subject) = when_node.first_child_of_kind(KIND_WHEN_SUBJECT) else {
        return false;
    };
    if let Some(variable_declaration) = subject.first_child_of_kind(KIND_VAR_DECL) {
        return variable_declaration
            .first_child_of_kind(KIND_SIMPLE_IDENT)
            .and_then(|identifier| identifier.utf8_text_owned(bytes))
            .as_deref()
            == Some(name);
    }
    subject
        .first_child_of_kind(KIND_SIMPLE_IDENT)
        .and_then(|identifier| identifier.utf8_text_owned(bytes))
        .as_deref()
        == Some(name)
}

fn destructuring_in_multi_binds_name(
    multi: tree_sitter::Node<'_>,
    bytes: &[u8],
    name: &str,
) -> bool {
    let mut cursor = multi.walk();
    for child in multi.children(&mut cursor) {
        if child.kind() != KIND_VAR_DECL {
            continue;
        }
        if child
            .first_child_of_kind(KIND_SIMPLE_IDENT)
            .and_then(|identifier| identifier.utf8_text_owned(bytes))
            .as_deref()
            == Some(name)
        {
            return true;
        }
    }
    false
}

fn member_property_type(
    member_container: tree_sitter::Node<'_>,
    var_name: &str,
    bytes: &[u8],
) -> Option<String> {
    let mut cursor = member_container.walk();
    for child in member_container.children(&mut cursor) {
        if child.kind() != KIND_PROP_DECL {
            continue;
        }
        if let Some(property_type) = property_declaration_type(child, var_name, bytes) {
            return Some(property_type);
        }
    }
    None
}

fn type_annotation_from_node(node: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut found_name = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == KIND_SIMPLE_IDENT {
            found_name = true;
            continue;
        }
        if !found_name {
            continue;
        }
        if child.kind() == ":" {
            continue;
        }
        if child.kind() == KIND_USER_TYPE {
            return user_type_name(child, bytes);
        }
        if child.kind() == KIND_NULLABLE_TYPE {
            return nullable_user_type_name(child, bytes);
        }
    }
    None
}

fn user_type_name(user_type: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let raw = user_type.utf8_text_owned(bytes)?;
    if raw.is_empty() {
        return None;
    }
    Some(raw)
}

fn nullable_user_type_name(nullable_type: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let raw = nullable_type.utf8_text_owned(bytes)?;
    Some(raw.trim_end_matches('?').to_string())
}

pub(crate) fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Return the trailing contiguous identifier slice in `s` — the longest
/// suffix whose characters all satisfy `is_id_char`.  Returns `""` if none.
///
/// Example: `last_ident_in("foo.barBaz")` → `"barBaz"`
pub(crate) fn last_ident_in(s: &str) -> &str {
    let ident_bytes: usize = s
        .chars()
        .rev()
        .take_while(|&c| is_id_char(c))
        .map(|c| c.len_utf8())
        .sum();
    &s[s.len() - ident_bytes..]
}

fn lambda_cursor_point(line_text: &str, cursor_line: usize, cursor_col: usize) -> Point {
    Point {
        row: cursor_line,
        column: crate::indexer::live_tree::utf16_col_to_byte(line_text, cursor_col),
    }
}

fn collect_cst_lambda_params(node: tree_sitter::Node<'_>, bytes: &[u8]) -> Vec<String> {
    let mut params = Vec::new();
    let mut current = node;
    loop {
        if current.kind() == KIND_LAMBDA_LIT {
            params.extend(current.collect_lambda_param_names(bytes, &params));
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    params
}

fn scan_lambda_params_in_lines(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
) -> Vec<String> {
    let scan_start = cursor_line.saturating_sub(SCOPE_SCAN_BACK_LINES);
    let mut scan = LambdaParamTextScan::default();
    for line_index in (scan_start..=cursor_line).rev() {
        let Some(line) = lines.get(line_index) else {
            continue;
        };
        scan.scan_line(line_before_cursor(
            line,
            line_index,
            cursor_line,
            cursor_col,
        ));
    }
    scan.params
}

fn line_before_cursor(
    line: &str,
    line_index: usize,
    cursor_line: usize,
    cursor_col: usize,
) -> &str {
    if line_index != cursor_line {
        return line;
    }
    let byte_end = utf16_col_to_byte(line, cursor_col);
    &line[..byte_end]
}

#[derive(Default)]
struct LambdaParamTextScan {
    params: Vec<String>,
    depth: i32,
}

impl LambdaParamTextScan {
    fn scan_line(&mut self, line: &str) {
        for (byte_index, ch) in line.char_indices().rev() {
            match ch {
                '}' => self.depth += 1,
                '{' => self.visit_lambda_open(line, byte_index),
                _ => {}
            }
        }
    }

    fn visit_lambda_open(&mut self, line: &str, byte_index: usize) {
        self.depth -= 1;
        if self.depth >= 0 || line[..byte_index].ends_with('$') {
            if line[..byte_index].ends_with('$') {
                self.depth = 0;
            }
            return;
        }
        self.collect_param_names(&line[byte_index + 1..]);
        self.depth = 0;
    }

    fn collect_param_names(&mut self, after_brace: &str) {
        let Some((names, _)) = after_brace.trim_start().split_once("->") else {
            return;
        };
        for token in names.split(',') {
            let name = token.trim().ident_prefix();
            if should_collect_lambda_param(&name, &self.params) {
                self.params.push(name);
            }
        }
    }
}

fn should_collect_lambda_param(name: &str, existing: &[String]) -> bool {
    !name.is_empty()
        && name != "it"
        && name != "_"
        && name.starts_with_lowercase()
        && !existing.iter().any(|existing_name| existing_name == name)
}

/// Scan backward from `(line_no, col)` — where `col` is the START of the cursor
/// word — to find the name of the enclosing function/constructor call.
///
/// Used to resolve named arguments: `User(name = "Alice")` with cursor on `name`
/// → scan back past the `(` → return `"User"`.
///
/// Returns the FULL dotted callee name (e.g. `"BottomSheetState.empty"`, `"User"`).
/// The caller converts this to a qualifier via `callee_to_qualifier`.
///
/// Scans at most 20 lines backward to avoid runaway on deeply nested expressions.
/// Tracks `()` and `[]` depth; lambda `{}` bodies are transparent (their inner
/// `()` still balance) so we don't need special-case brace handling.
pub(crate) fn find_enclosing_call_name(
    lines: &[String],
    line_no: usize,
    col: usize,
) -> Option<String> {
    let mut depth: i32 = 0;
    let scan_range_start = line_no.saturating_sub(ENCLOSING_CALL_SCAN_BACK);

    for ln in (scan_range_start..=line_no).rev() {
        let line_chars: Vec<char> = lines[ln].chars().collect();
        let scan_to = if ln == line_no { col } else { line_chars.len() };

        for i in (0..scan_to).rev() {
            match line_chars[i] {
                ')' | ']' => depth += 1,
                '(' | '[' => {
                    depth -= 1;
                    if depth < 0 {
                        // This `(` opened the call we're inside.
                        if i == 0 {
                            return None;
                        }
                        // Extract the identifier (possibly dotted) just before `(`.
                        let mut end = i;
                        while end > 0
                            && (is_id_char(line_chars[end - 1]) || line_chars[end - 1] == '.')
                        {
                            end -= 1;
                        }
                        if end >= i {
                            return None;
                        }
                        let name: String = line_chars[end..i].iter().collect();
                        let name = name.trim_matches('.').to_string();
                        return if name.is_empty() { None } else { Some(name) };
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Convert a raw callee name (from `find_enclosing_call_name`) to the qualifier
/// to use when resolving a named argument parameter.
///
/// Rules:
/// - Last segment uppercase → constructor call, qualifier = last segment.
///   `"User"` → `"User"`, `"com.example.User"` → `"User"`
/// - Last segment lowercase (method call) → look for the rightmost uppercase
///   segment in the receiver chain as the owner type.
///   `"BottomSheetState.empty"` → `"BottomSheetState"`
///   `"SomeClass.companion.build"` → `"SomeClass"` (last uppercase before method)
/// - Pure lowercase, no uppercase anywhere → `None` (can't resolve statically).
fn callee_to_qualifier(full_callee: &str) -> Option<String> {
    let segments: Vec<&str> = full_callee.split('.').collect();
    let last = *segments.last()?;

    // Constructor call: last segment is a type name (uppercase first char).
    if last.starts_with_uppercase() {
        return Some(last.to_string());
    }

    // Method call: find rightmost uppercase segment in the receiver chain.
    // `BottomSheetState.empty` → segments[..-1] = ["BottomSheetState"] → "BottomSheetState"
    // `viewModel.state.copy`   → no uppercase in receiver → None
    let receiver = &segments[..segments.len() - 1];
    receiver
        .iter()
        .rev()
        .find(|s| s.starts_with_uppercase())
        .map(|s| s.to_string())
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod scope_tests;
