//! Signature help feature — extracts the active call site and returns parameter info.

use crate::backend::actions::is_non_call_keyword;
use crate::indexer::CallInfo;
use crate::StrExt;

use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation, Url,
};

use super::traits::{CallInfoAccess, DocumentAccess, SignatureIndex};

/// Compute signature help for the call under the cursor at `pos` in `uri`.
///
/// Returns `None` when the cursor is not inside a call expression or when the
/// function name cannot be resolved to a known signature.
pub(crate) fn compute_signature_help(
    uri: &Url,
    pos: Position,
    index: &(impl DocumentAccess + SignatureIndex + CallInfoAccess),
) -> Option<SignatureHelp> {
    let lines_owned = index.mem_lines_for(uri.as_str())?;
    let lines: &[String] = &lines_owned;

    let line_idx = pos.line as usize;
    if line_idx >= lines.len() {
        return None;
    }
    let line_text = &lines[line_idx];
    let col = crate::indexer::live_tree::utf16_col_to_byte(line_text, pos.character as usize);
    let before = &line_text[..col];

    let ci = index
        .call_info_at(pos, uri)
        .or_else(|| text_call_info(lines, before, line_idx))?;

    let params_text =
        index.find_fun_signature_with_receiver(uri, &ci.fn_name, ci.qualifier.as_deref());
    if params_text.is_empty() {
        return None;
    }

    build_signature_help(&ci.fn_name, &params_text, ci.active_param)
}

fn build_signature_help(
    fn_name: &str,
    params_text: &str,
    active_param: u32,
) -> Option<SignatureHelp> {
    let raw = params_text.trim_matches(|c| c == '(' || c == ')');
    let param_parts: Vec<&str> = raw
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let parameters: Vec<ParameterInformation> = param_parts
        .iter()
        .map(|p| ParameterInformation {
            label: ParameterLabel::Simple(p.to_string()),
            documentation: None,
        })
        .collect();
    let label = format!("{}({})", fn_name, param_parts.join(", "));
    let active_param = active_param.min(parameters.len().saturating_sub(1) as u32);
    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    })
}

/// Text-scan fallback: extract `(fn_name, qualifier, active_param)` by walking
/// backwards through `before` (and up to `MAX_SCAN_BACK_LINES` previous lines
/// for multiline calls).
fn text_call_info(lines: &[String], before: &str, line_idx: usize) -> Option<CallInfo> {
    let mut depth: i32 = 0;
    let mut active_param: u32 = 0;
    let mut call_name: Option<String> = None;
    let mut call_qualifier: Option<String> = None;

    let chars: Vec<char> = before.chars().collect();
    let mut i = chars.len();
    while i > 0 {
        i -= 1;
        match chars[i] {
            ')' | ']' => {
                depth += 1;
            }
            '{' | '}' => {
                break;
            }
            '(' => {
                if depth == 0 {
                    let mut j = i;
                    while j > 0 && (chars[j - 1].is_alphanumeric() || chars[j - 1] == '_') {
                        j -= 1;
                    }
                    let candidate: String = chars[j..i].iter().collect();
                    if !candidate.is_empty() && !is_non_call_keyword(&candidate) {
                        call_name = Some(candidate);
                        call_qualifier = extract_dot_qualifier(&chars, j);
                    }
                    break;
                }
                depth -= 1;
            }
            ',' if depth == 0 => {
                active_param += 1;
            }
            _ => {}
        }
    }

    let in_block_body = before.contains('{')
        || before.contains('}')
        || lines[line_idx].trim_start().starts_with('}');
    if call_name.is_none() && line_idx > 0 && !in_block_body {
        if let Some((name, qual, extra)) = scan_multiline_call_open(lines, line_idx) {
            call_name = Some(name);
            call_qualifier = qual;
            active_param += extra;
        }
    }

    let fn_name = call_name.filter(|n| !n.is_empty())?;
    Some(CallInfo {
        fn_name,
        qualifier: call_qualifier,
        active_param,
    })
}

const MAX_SCAN_BACK_LINES: usize = 10;

fn scan_multiline_call_open(
    lines: &[String],
    line_idx: usize,
) -> Option<(String, Option<String>, u32)> {
    let scan_start = line_idx.saturating_sub(MAX_SCAN_BACK_LINES);
    for scan_line in (scan_start..line_idx).rev() {
        let l = &lines[scan_line];
        if l.contains('{') || l.contains('}') {
            break;
        }
        if let Some((name, qualifier)) = find_call_open_on_line(l) {
            let mut extra: u32 = 0;
            if scan_line + 1 < line_idx {
                for mid in &lines[(scan_line + 1)..line_idx] {
                    extra += mid.chars().filter(|&c| c == ',').count() as u32;
                }
            }
            return Some((name, qualifier, extra));
        }
    }
    None
}

fn find_call_open_on_line(line: &str) -> Option<(String, Option<String>)> {
    for (p, _) in line
        .char_indices()
        .filter(|&(_, c)| c == '(')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let before_paren = &line[..p];
        let name = before_paren.last_ident_in();
        if !name.is_empty() && !is_non_call_keyword(name) {
            let net: i32 = line[p..]
                .chars()
                .map(|c| match c {
                    '(' => 1,
                    ')' => -1,
                    _ => 0,
                })
                .sum();
            if net > 0 {
                let before_name = &before_paren[..before_paren.len() - name.len()];
                let qualifier = if before_name.ends_with('.') {
                    let q = before_name
                        .strip_suffix('.')
                        .unwrap_or(before_name)
                        .last_ident_in();
                    if q.is_empty() {
                        None
                    } else {
                        Some(q.to_owned())
                    }
                } else {
                    None
                };
                return Some((name.to_owned(), qualifier));
            }
        }
    }
    None
}

fn extract_dot_qualifier(chars: &[char], j: usize) -> Option<String> {
    if j > 0 && chars[j - 1] == '.' {
        let mut k = j - 1;
        while k > 0 && (chars[k - 1].is_alphanumeric() || chars[k - 1] == '_') {
            k -= 1;
        }
        let q: String = chars[k..j - 1].iter().collect();
        if !q.is_empty() {
            Some(q)
        } else {
            None
        }
    } else {
        None
    }
}
