//! Completion context — scope analysis and named-arg ranking.
//!
//! Ported from upstream kmp-lsp (Hessesian/kmp-lsp) with adaptations for
//! the kotlin-lsp codebase structure.

use tower_lsp::lsp_types::{Position, Url};

use crate::indexer::Indexer;

const IT: &str = "it";
const THIS: &str = "this";
#[allow(dead_code)]
const SCOPE_SCAN_BACK_LINES: usize = 50;

/// Type info for a single lambda scope.
pub(crate) struct LambdaScope {
    /// Type of the implicit `it` parameter (if single-param lambda).
    #[allow(dead_code)]
    pub it_type: Option<String>,
    /// Named lambda parameters: (name, type).
    #[allow(dead_code)]
    pub named_params: Vec<(String, String)>,
    /// Lambda label — the call name just before `{`, e.g. `forEach { }` → `"forEach"`.
    pub label: Option<String>,
}

/// Scope context built from the cursor position.
pub(crate) struct ScopeContext {
    /// Innermost enclosing class/object name, if any.
    #[allow(dead_code)]
    pub enclosing_class: Option<String>,
    /// Lambda scopes, outermost first, innermost last.
    #[allow(dead_code)]
    pub lambda_scopes: Vec<LambdaScope>,
    #[allow(dead_code)]
    pub(crate) bare_this_type: Option<String>,
}

impl ScopeContext {
    /// Build scope context from the current cursor position.
    #[allow(dead_code)]
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
        let lambda_scopes = collect_lambda_scopes(
            lines,
            cursor_line as usize,
            cursor_col as usize,
            index,
            uri,
            position,
        );
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
    /// - anything else → `None` (caller handles non-scope receivers)
    #[allow(dead_code)]
    pub(crate) fn resolve_receiver(&self, expr: &str) -> Option<&str> {
        match expr {
            IT => self
                .lambda_scopes
                .iter()
                .rev()
                .find_map(|s| s.it_type.as_deref()),
            THIS => self.bare_this_type.as_deref(),
            _ => {
                if let Some(label) = expr.strip_prefix("this@") {
                    // Walk innermost first, looking for a lambda or class named `label`.
                    self.lambda_scopes
                        .iter()
                        .rev()
                        .find_map(|s| {
                            s.label
                                .as_deref()
                                .filter(|l| *l == label)
                                .and(s.it_type.as_deref())
                        })
                        .or_else(|| self.enclosing_class.as_deref().filter(|c| *c == label))
                } else {
                    None
                }
            }
        }
    }

    /// Find the call site name that introduces the innermost lambda scope.
    #[allow(dead_code)]
    pub(crate) fn innermost_lambda_label(&self) -> Option<&str> {
        self.lambda_scopes.last().and_then(|s| s.label.as_deref())
    }
}

/// Collect lambda scopes by scanning backwards from the cursor position.
#[allow(dead_code)]
fn collect_lambda_scopes(
    lines: &[String],
    line: usize,
    _col: usize,
    index: &Indexer,
    uri: &Url,
    position: Position,
) -> Vec<LambdaScope> {
    let mut scopes: Vec<LambdaScope> = Vec::new();
    let mut brace_depth: i32 = 0;
    let start_line = line.saturating_sub(SCOPE_SCAN_BACK_LINES);

    for i in (start_line..=line).rev() {
        let line_text = match lines.get(i) {
            Some(l) => l,
            None => continue,
        };
        for ch in line_text.chars() {
            match ch {
                '{' => {
                    brace_depth += 1;
                    if brace_depth == 0 {
                        // We found the opening `{` of a lambda.
                        let scope = extract_lambda_scope(lines, i, index, uri, position);
                        scopes.push(scope);
                    }
                }
                '}' => brace_depth -= 1,
                _ => {}
            }
        }
    }
    scopes
}

/// Extract info for a single lambda scope at line `brace_line`.
#[allow(dead_code)]
fn extract_lambda_scope(
    lines: &[String],
    brace_line: usize,
    index: &Indexer,
    uri: &Url,
    position: Position,
) -> LambdaScope {
    // Find the call name before `{` on or before this line
    let label = find_lambda_label(lines, brace_line);

    // Infer `it` type
    let it_type = index.infer_lambda_param_type_at(IT, uri, position);

    // Collect named params from the lambda parameter list
    let named_params: Vec<(String, String)> = index
        .lambda_params_at_col(uri, brace_line, 0)
        .into_iter()
        .filter_map(|name| {
            let pos = Position::new(brace_line as u32, 0);
            let ty = index.infer_lambda_param_type_at(&name, uri, pos);
            ty.map(|t| (name, t))
        })
        .collect();

    LambdaScope {
        it_type,
        named_params,
        label,
    }
}

/// Find the call name that introduces the lambda at the given line.
#[allow(dead_code)]
fn find_lambda_label(lines: &[String], brace_line: usize) -> Option<String> {
    // Scan backwards from the `{` to find the closest identifier
    let line = lines.get(brace_line)?;
    let before_brace = &line[..line.find('{').unwrap_or(line.len())];
    let trimmed = before_brace.trim();
    // Find the last word before `{`
    trimmed
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .rfind(|s| !s.is_empty())
        .map(|s| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_resolves_to_enclosing_class() {
        let ctx = ScopeContext {
            enclosing_class: Some("Foo".to_owned()),
            lambda_scopes: vec![],
            bare_this_type: Some("Foo".to_owned()),
        };
        assert_eq!(ctx.resolve_receiver("this"), Some("Foo"));
    }

    #[test]
    fn it_resolves_to_innermost_lambda_scope() {
        let ctx = ScopeContext {
            enclosing_class: None,
            lambda_scopes: vec![
                LambdaScope {
                    it_type: None,
                    named_params: vec![],
                    label: Some("map".to_owned()),
                },
                LambdaScope {
                    it_type: Some("String".to_owned()),
                    named_params: vec![],
                    label: Some("forEach".to_owned()),
                },
            ],
            bare_this_type: None,
        };
        assert_eq!(ctx.resolve_receiver("it"), Some("String"));
    }

    #[test]
    fn this_at_label_resolves_correctly() {
        let ctx = ScopeContext {
            enclosing_class: Some("ViewModel".to_owned()),
            lambda_scopes: vec![LambdaScope {
                it_type: Some("List<Int>".to_owned()),
                named_params: vec![],
                label: Some("map".to_owned()),
            }],
            bare_this_type: Some("ViewModel".to_owned()),
        };
        assert_eq!(ctx.resolve_receiver("this@map"), Some("List<Int>"));
    }

    #[test]
    fn unknown_receiver_returns_none() {
        let ctx = ScopeContext {
            enclosing_class: None,
            lambda_scopes: vec![],
            bare_this_type: None,
        };
        assert!(ctx.resolve_receiver("viewModel").is_none());
    }
}
