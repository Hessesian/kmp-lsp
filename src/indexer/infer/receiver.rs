//! Signature-derived lambda helpers shared by the CST resolution path.
//!
//! The text-heuristic receiver inference that once lived here (the
//! `lambda_receiver_type_from_context` family) has been retired — every call
//! site now resolves lambda types from the CST. What remains are three
//! signature/type utilities the CST resolvers still call.

use tower_lsp::lsp_types::Url;

use crate::StrExt;

use super::deps::InferDeps;
use super::lambda::lambda_type_receiver;
use super::sig::last_fun_param_type_str;
use super::type_subst::is_generic_param;

/// Preserve a dot-qualified type name's prefix while dropping generics/nullable
/// suffixes.  Use when `raw` is a **type string** (e.g. "Contract.Effect",
/// "ImmutableList<T>"), not a variable or field name.
pub(super) fn uppercase_dotted_type_prefix(raw: &str) -> Option<String> {
    let base = raw.dotted_ident_prefix();
    let base = base.trim_end_matches('.');
    if base.is_empty() || is_generic_param(base) {
        return None;
    }
    let first_seg = base.split('.').next().unwrap_or(base);
    first_seg.starts_with_uppercase().then(|| base.to_owned())
}

/// Like `fun_trailing_lambda_it_type` but for `this`: only returns a type when
/// the trailing lambda parameter is a **receiver lambda** `T.() -> R`.
pub(super) fn fun_trailing_lambda_this_type(
    fn_name: &str,
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    let sig = deps.find_fun_params_text(fn_name, uri)?;
    let last_type = last_fun_param_type_str(&sig)?;
    lambda_type_receiver(&last_type)
}

/// Resolve function params with receiver awareness: if the call has a dot-receiver
/// (e.g. `factory.create(...)`), resolve the receiver's type and look up the
/// method on that type.  Falls back to global name-based lookup.
pub(super) fn resolve_call_params(
    fn_name: &str,
    receiver_type: Option<&str>,
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    if let Some(raw_type) = receiver_type {
        let dotted = raw_type.dotted_ident_prefix();
        if !dotted.is_empty() {
            if let Some(params) = deps.find_method_params_text(&dotted, fn_name) {
                return Some(params);
            }
        }
    }
    deps.find_fun_params_text(fn_name, uri)
}
