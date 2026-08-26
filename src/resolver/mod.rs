//! Symbol resolution for Kotlin, Java, and Swift.
//!
//! See [`resolve`] for the resolution chain and strategy documentation.

pub(crate) mod api;
pub(crate) mod complete;
mod fd;
pub(crate) mod find;
mod hierarchy;
mod import_edit;
pub(crate) mod infer;
pub(crate) mod infer_lines;
pub(crate) mod resolve;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

// ─── re-exports ───────────────────────────────────────────────────────────────

pub(crate) use api::{Resolver, ReturnType};
pub(crate) use complete::symbols_from_uri_as_completions_pub;
#[cfg(test)]
pub(crate) use complete::{complete_symbol, complete_symbol_with_context, is_annotation_context};
pub(crate) use hierarchy::ReceiverTypeAgreement;
pub(crate) use hierarchy::{walk_hierarchy, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK};
pub(crate) use import_edit::{already_imported, import_insertion_line, make_import_edit};
pub(crate) use infer::{
    infer_receiver_type, infer_receiver_type_at, infer_variable_type_from_cst,
    infer_variable_type_raw, ReceiverKind, ReceiverType,
};
pub(crate) use infer_lines::extract_collection_element_type;
pub(crate) use resolve::{
    ensure_file_data, fqns_for_name, receiver_provides_member, resolve_callee_definition,
    resolve_implicit_receiver_callee, resolve_in_scope_strict,
    resolve_symbol_hierarchy_ambiguity_safe, resolve_symbol_no_rg, resolve_symbol_scoped_only,
};

// Re-exports used only in tests.
#[cfg(test)]
pub(crate) use crate::rg::build_rg_pattern;
#[cfg(test)]
pub(crate) use complete::{
    complete_bare, complete_dot, is_screaming_snake, match_score, COMPLETION_CAP,
    MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION,
};
#[cfg(test)]
use fd::import_file_stems;
#[cfg(test)]
use fd::{import_package_prefix, package_prefix};
#[cfg(test)]
pub(crate) use infer::infer_variable_type;
#[cfg(test)]
pub(crate) use infer_lines::{
    find_declaration_range_in_lines, infer_type_in_lines, infer_type_in_lines_raw,
};
#[cfg(test)]
pub(crate) use resolve::resolve_symbol;
#[cfg(test)]
pub(crate) use resolve::resolve_symbol_index_only;
