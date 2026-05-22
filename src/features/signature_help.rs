//! Signature help feature — extracts the active call site via CST and returns parameter info.
//!
//! The CST (`cst_call_info`) is authoritative: it walks up the live tree-sitter parse tree to
//! find the enclosing `call_expression`, counts `value_argument` children for `active_param`,
//! and handles multiline calls naturally.
//!
//! When the closing `)` is absent (live typing mid-argument) the CST cannot form a
//! `call_expression`, so `call_info_at` uses a text-scan fallback on the current line.
//!
//! When the cursor is inside a nested call (e.g. `setOf()`) whose signature cannot be resolved,
//! the outer call is tried as a fallback so the user still sees helpful parameter info.

use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation, Url,
};

use super::traits::{LiveTreeAccess, SignatureIndex};

/// Compute signature help for the call under the cursor at `pos` in `uri`.
///
/// Returns `None` when:
/// - no live parse tree exists for the file (not yet opened/edited),
/// - the cursor is not inside a `call_expression` (e.g. inside a trailing lambda body), or
/// - the function name cannot be resolved to a known signature.
pub(crate) fn compute_signature_help(
    uri: &Url,
    pos: Position,
    index: &(impl SignatureIndex + LiveTreeAccess),
) -> Option<SignatureHelp> {
    let ci = index.call_info_at(pos, uri)?;

    if let Some(params_text) =
        index.find_fun_signature_with_receiver(uri, &ci.fn_name, ci.qualifier.as_deref())
    {
        return build_signature_help(&ci.fn_name, &params_text, ci.active_param);
    }

    // Inner call's signature not found (e.g. stdlib overloaded function like `setOf`).
    // Try the enclosing call expression so the user still sees the outer parameter info.
    let outer = index.outer_call_info_at(pos, uri)?;
    let params_text =
        index.find_fun_signature_with_receiver(uri, &outer.fn_name, outer.qualifier.as_deref())?;
    build_signature_help(&outer.fn_name, &params_text, outer.active_param)
}

fn build_signature_help(
    fn_name: &str,
    params_text: &str,
    active_param: u32,
) -> Option<SignatureHelp> {
    use crate::indexer::split_params_at_depth_zero;
    let raw = params_text.trim_matches(|c| c == '(' || c == ')');
    let param_parts: Vec<String> = split_params_at_depth_zero(raw)
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let parameters: Vec<ParameterInformation> = param_parts
        .iter()
        .map(|p| ParameterInformation {
            label: ParameterLabel::Simple(p.clone()),
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

#[cfg(test)]
#[path = "signature_help_tests.rs"]
mod tests;
