use tower_lsp::lsp_types::{Position, Url};

use crate::features::completion::param_names_from_sig;
use crate::features::traits::{LiveTreeAccess, SignatureIndex};
use crate::indexer::{
    cursor_node_at, split_params_at_depth_zero, CstQuery, Indexer, LambdaScopeInfo, ResolveIo,
};
use crate::resolver::complete::ReceiverExpr;
use crate::types::CursorPos;

const IT: &str = "it";
const THIS: &str = "this";

pub(crate) struct LambdaScope {
    /// Type of the implicit `it` parameter (if single-param lambda).
    pub it_type: Option<String>,
    /// Named lambda parameters: (name, type).
    pub named_params: Vec<(String, String)>,
    /// Lambda label — the call name just before `{`, e.g. `forEach { }` → `"forEach"`.
    /// Used to resolve `this@forEach` / `this@label` references.
    pub label: Option<String>,
}

impl From<LambdaScopeInfo> for LambdaScope {
    fn from(info: LambdaScopeInfo) -> Self {
        Self {
            it_type: info.it_type,
            named_params: info.named_params,
            label: info.label,
        }
    }
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
    /// Call argument context at the cursor, if inside a call expression.
    pub call_info: Option<CallInfo>,
}

pub(crate) struct CallInfo {
    pub callee: String,
    pub qualifier: Option<String>,
    /// Active argument index — reserved for future ranking use.
    #[allow(dead_code)]
    pub arg_index: usize,
    pub expected_name: Option<String>,
    /// Type hint at `arg_index` — reserved for future ranking use.
    #[allow(dead_code)]
    pub expected_type: Option<String>,
}

impl CompletionContext {
    /// Single analysis pass for a cache miss.
    pub(crate) fn analyse(
        before_prefix: &str,
        position: Position,
        index: &Indexer,
        uri: &Url,
        annotation_only: bool,
    ) -> Self {
        let scope = ScopeContext::build(position, index, uri);
        let call_info = build_call_info(position, index, uri);
        Self {
            receiver: ReceiverExpr::parse(before_prefix),
            annotation_only,
            scope,
            call_info,
        }
    }
}

fn build_call_info(position: Position, index: &Indexer, uri: &Url) -> Option<CallInfo> {
    let ci = index.call_info_at(position, uri)?;
    let params_text =
        index.find_fun_signature_with_receiver(uri, &ci.fn_name, ci.qualifier.as_deref())?;
    let raw = params_text.trim_matches(|c| c == '(' || c == ')');
    let arg_index = ci.active_param as usize;
    let param_names = param_names_from_sig(raw);
    let expected_name = param_names.get(arg_index).cloned();
    let expected_type = param_type_at(raw, arg_index);
    Some(CallInfo {
        callee: ci.fn_name,
        qualifier: ci.qualifier,
        arg_index,
        expected_name,
        expected_type,
    })
}

fn param_type_at(raw: &str, idx: usize) -> Option<String> {
    let part = split_params_at_depth_zero(raw)
        .into_iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .nth(idx)?;
    let colon = part.find(':')?;
    let ty = part[colon + 1..].split('=').next()?.trim();
    (!ty.is_empty()).then(|| ty.to_owned())
}

impl ScopeContext {
    /// Build scope context from the current cursor position.
    pub(crate) fn build(position: Position, index: &Indexer, uri: &Url) -> Self {
        let cursor_line = position.line as usize;
        let cursor_col = position.character as usize;
        let enclosing_class = index.enclosing_class_at(uri, position.line);
        let bare_this_type = index
            .infer_lambda_param_type_at(THIS, uri, position)
            .or_else(|| enclosing_class.clone());
        let mut lambda_scopes = collect_lambda_scopes(index, uri, position);
        let mut param_names = index.lambda_params_at_col(uri, cursor_line, cursor_col);
        if param_names.is_empty() {
            param_names = index.lambda_params_at(uri, cursor_line);
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

/// Build the scope stack for every lambda enclosing the cursor via the CST
/// ancestor-walk (`CstQuery::lambda_scope`), outermost first.
///
/// The tree comes from `live_doc_or_parse`: the live tree for open files, a
/// transient parse otherwise — completion works the same either way.
fn collect_lambda_scopes(index: &Indexer, uri: &Url, position: Position) -> Vec<LambdaScope> {
    let Some(doc) = index.live_doc_or_parse(uri) else {
        return Vec::new();
    };
    let cursor = CursorPos {
        line: position.line as usize,
        utf16_col: position.character as usize,
    };
    let Some(node) = cursor_node_at(&doc, cursor) else {
        return Vec::new();
    };
    CstQuery::new(node, &doc, index, uri, ResolveIo::NoRg)
        .lambda_scope()
        .into_iter()
        .map(LambdaScope::from)
        .collect()
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

#[cfg(test)]
#[path = "completion_context_tests.rs"]
mod tests;
