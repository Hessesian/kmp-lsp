use tower_lsp::lsp_types::{Position, Url};

use crate::indexer::{
    lambda_receiver_type_from_context, last_ident_in, strip_trailing_call_args, Indexer,
};
use crate::resolver::complete::ReceiverExpr;
use crate::StrExt;

const IT: &str = "it";
const THIS: &str = "this";
const SCOPE_SCAN_BACK_LINES: usize = 50;

pub(crate) struct LambdaScope {
    /// Type of the implicit `it` parameter (if single-param lambda).
    pub it_type: Option<String>,
    /// Named lambda parameters: (name, type).
    pub named_params: Vec<(String, String)>,
    /// Lambda label — the call name just before `{`, e.g. `forEach { }` → `"forEach"`.
    /// Used to resolve `this@forEach` / `this@label` references.
    pub label: Option<String>,
}

pub(crate) struct ScopeContext {
    /// Innermost enclosing class/object name, if any.
    pub enclosing_class: Option<String>,
    /// Lambda scopes, outermost first, innermost last.
    pub lambda_scopes: Vec<LambdaScope>,
    bare_this_type: Option<String>,
}

/// Semantic facts about the cursor position, computed once per cache miss.
pub(crate) struct CompletionContext {
    /// Dot-completion receiver (None = bare word context).
    pub receiver: Option<ReceiverExpr>,
    /// True when cursor is immediately after `@` — restrict to annotation types.
    pub annotation_only: bool,
    /// Lambda and class scope stack.
    pub scope: ScopeContext,
    /// Call argument context — populated in Wave 3; always None for now.
    #[expect(
        dead_code,
        reason = "Wave 3 reads call_info after this placeholder lands"
    )]
    pub call_info: Option<CallInfo>,
}

/// Placeholder for Wave 3 — kept here so Wave 3 can fill it in without touching the struct definition.
#[expect(dead_code, reason = "Wave 3 constructs and reads this placeholder")]
pub(crate) struct CallInfo {
    pub callee: String,
    pub arg_index: usize,
    pub expected_name: Option<String>,
    pub expected_type: Option<String>,
}

impl CompletionContext {
    /// Single analysis pass for a cache miss.
    pub(crate) fn analyse(
        before_prefix: &str,
        position: Position,
        index: &Indexer,
        uri: &Url,
        lines: &[String],
        annotation_only: bool,
    ) -> Self {
        Self {
            receiver: ReceiverExpr::parse(before_prefix),
            annotation_only,
            scope: ScopeContext::build(lines, position.line, position.character, index, uri),
            call_info: None,
        }
    }
}

impl ScopeContext {
    /// Build scope context from the current cursor position.
    pub(crate) fn build(
        lines: &[String],
        cursor_line: u32,
        cursor_col: u32,
        index: &Indexer,
        uri: &Url,
    ) -> Self {
        let position = Position::new(cursor_line, cursor_col);
        let enclosing_class = index.enclosing_class_at(uri, cursor_line);
        let bare_this_type = index
            .infer_lambda_param_type_at(THIS, uri, position)
            .or_else(|| enclosing_class.clone());
        let mut lambda_scopes = collect_lambda_scopes(
            lines,
            cursor_line as usize,
            cursor_col as usize,
            index,
            uri,
            position,
        );
        let mut param_names =
            index.lambda_params_at_col(uri, cursor_line as usize, cursor_col as usize);
        if param_names.is_empty() {
            param_names = index.lambda_params_at(uri, cursor_line as usize);
        }
        merge_named_params(&mut lambda_scopes, param_names, index, uri, position);
        if let Some(innermost_scope) = lambda_scopes.last_mut() {
            innermost_scope.it_type = index.infer_lambda_param_type_at(IT, uri, position);
        }
        Self {
            enclosing_class,
            lambda_scopes,
            bare_this_type,
        }
    }

    /// Resolve a receiver expression to a type string for dot-completion.
    ///
    /// Handles:
    /// - `"it"` → innermost lambda scope where `it_type.is_some()`
    /// - `"this"` → current `this` binding
    /// - `"this@Foo"` → walk lambda scopes and enclosing class looking for `Foo`
    /// - anything else → None (caller handles non-scope receivers)
    pub(crate) fn resolve_receiver(&self, expr: &str) -> Option<&str> {
        match expr {
            IT => self
                .lambda_scopes
                .iter()
                .rev()
                .find_map(|scope| scope.it_type.as_deref()),
            THIS => self.bare_this_type.as_deref(),
            _ => resolve_labeled_receiver(self, expr),
        }
    }

    pub(crate) fn named_param_type(&self, name: &str) -> Option<&str> {
        self.lambda_scopes.iter().rev().find_map(|lambda_scope| {
            lambda_scope
                .named_params
                .iter()
                .find_map(|(param_name, param_type)| {
                    (param_name == name && !param_type.is_empty()).then_some(param_type.as_str())
                })
        })
    }

    /// True if the given receiver expression is a lambda scope reference
    /// (`it`, `this`, or `this@label`).
    pub(crate) fn is_scope_receiver(&self, expr: &str) -> bool {
        matches!(expr, IT | THIS)
            || expr
                .strip_prefix("this@")
                .is_some_and(|label| !label.is_empty())
    }
}

fn resolve_labeled_receiver<'a>(scope: &'a ScopeContext, expr: &str) -> Option<&'a str> {
    let label = expr.strip_prefix("this@")?;
    scope
        .lambda_scopes
        .iter()
        .rev()
        .find(|lambda_scope| lambda_scope.label.as_deref() == Some(label))
        .and_then(|lambda_scope| lambda_scope.it_type.as_deref())
        .or_else(|| {
            scope
                .enclosing_class
                .as_deref()
                .filter(|class_name| *class_name == label)
        })
}

fn collect_lambda_scopes(
    lines: &[String],
    cursor_line: usize,
    cursor_col: usize,
    index: &Indexer,
    uri: &Url,
    position: Position,
) -> Vec<LambdaScope> {
    if lines.is_empty() {
        return Vec::new();
    }

    let scan_start = cursor_line.saturating_sub(SCOPE_SCAN_BACK_LINES);
    let mut depth = 0i32;
    let mut scopes = Vec::new();

    for line_index in (scan_start..=cursor_line).rev() {
        let Some(line) = lines.get(line_index) else {
            continue;
        };
        let scan_slice = line_before_cursor(line, line_index, cursor_line, cursor_col);
        for (byte_index, ch) in scan_slice.char_indices().rev() {
            match ch {
                '}' => depth += 1,
                '{' => {
                    depth -= 1;
                    if depth >= 0 || scan_slice[..byte_index].ends_with('$') {
                        if scan_slice[..byte_index].ends_with('$') {
                            depth = 0;
                        }
                        continue;
                    }
                    if let Some(scope) =
                        build_lambda_scope(scan_slice, byte_index, index, uri, position)
                    {
                        scopes.push(scope);
                    }
                    depth = 0;
                }
                _ => {}
            }
        }
    }

    scopes.reverse();
    scopes
}

fn merge_named_params(
    lambda_scopes: &mut Vec<LambdaScope>,
    param_names: Vec<String>,
    index: &Indexer,
    uri: &Url,
    position: Position,
) {
    if param_names.is_empty() {
        return;
    }

    if lambda_scopes.is_empty() {
        lambda_scopes.push(LambdaScope {
            it_type: None,
            named_params: Vec::new(),
            label: None,
        });
    }

    let Some(innermost_scope) = lambda_scopes.last_mut() else {
        return;
    };
    for param_name in param_names {
        if innermost_scope
            .named_params
            .iter()
            .any(|(existing_name, _)| existing_name == &param_name)
        {
            continue;
        }
        let param_type = index
            .infer_lambda_param_type_at(&param_name, uri, position)
            .unwrap_or_default();
        innermost_scope.named_params.push((param_name, param_type));
    }
}

fn build_lambda_scope(
    scan_slice: &str,
    brace_byte: usize,
    index: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<LambdaScope> {
    let before_brace = &scan_slice[..brace_byte];
    let named_params = parse_named_params(&scan_slice[brace_byte + 1..], index, uri, position);
    let it_type = lambda_receiver_type_from_context(before_brace, index, uri);
    if named_params.is_empty() && it_type.is_none() {
        return None;
    }
    Some(LambdaScope {
        it_type,
        named_params,
        label: lambda_label(before_brace),
    })
}

fn parse_named_params(
    after_brace: &str,
    index: &Indexer,
    uri: &Url,
    position: Position,
) -> Vec<(String, String)> {
    let trimmed = after_brace.trim_start();
    let Some((names, _)) = trimmed.split_once("->") else {
        return Vec::new();
    };

    let mut named_params = Vec::new();
    for token in names.split(',') {
        let name = token.trim().ident_prefix();
        if should_collect_named_param(&name, &named_params) {
            let param_type = index
                .infer_lambda_param_type_at(&name, uri, position)
                .unwrap_or_default();
            named_params.push((name, param_type));
        }
    }
    named_params
}

fn should_collect_named_param(name: &str, named_params: &[(String, String)]) -> bool {
    !name.is_empty()
        && name != IT
        && name != "_"
        && name.starts_with_lowercase()
        && !named_params
            .iter()
            .any(|(existing_name, _)| existing_name == name)
}

fn lambda_label(before_brace: &str) -> Option<String> {
    let callee = strip_trailing_call_args(before_brace)
        .replace("?.", ".")
        .trim()
        .to_owned();
    let label = last_ident_in(&callee);
    (!label.is_empty()).then(|| label.to_owned())
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
    let byte_end = crate::indexer::live_tree::utf16_col_to_byte(line, cursor_col);
    &line[..byte_end]
}

#[cfg(test)]
#[path = "completion_context_tests.rs"]
mod tests;
