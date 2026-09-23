//! Symbol resolution for Kotlin, Java, and Swift.
//!
//! See [`resolve`] for the resolution chain and strategy documentation.

pub(crate) mod api;
pub(crate) mod complete;
pub(crate) mod container;
mod extension;
mod fd;
pub(crate) mod find;
mod hierarchy;
mod import_edit;
mod imports;
pub(crate) mod infer;
pub(crate) mod infer_lines;
mod package;
mod package_scope;
mod platform_types;
mod qualified;
pub(crate) mod resolve;
mod scope_check;
#[cfg(test)]
mod shared_fixture_tests;
#[cfg(test)]
mod tests;
mod tie_break;

// ─── re-exports ───────────────────────────────────────────────────────────────

pub(crate) use api::{Resolver, ReturnType};
pub(crate) use complete::symbols_from_uri_as_completions_pub;
#[cfg(test)]
pub(crate) use complete::{complete_symbol, complete_symbol_with_context, is_annotation_context};
pub(crate) use extension::resolve_implicit_receiver_callee;
pub(crate) use hierarchy::ReceiverTypeAgreement;
pub(crate) use hierarchy::{walk_hierarchy, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK};
pub(crate) use import_edit::{already_imported, import_insertion_line, make_import_edit};
pub(crate) use infer::{
    infer_receiver_type, infer_receiver_type_at, infer_variable_type_from_cst,
    infer_variable_type_raw, ReceiverKind, ReceiverType,
};
pub(crate) use infer_lines::{
    extract_collection_element_type, extract_property_type_from_detail,
    extract_return_type_from_detail,
};
pub(crate) use package_scope::find_symbol_in_package;
pub(crate) use resolve::{
    ensure_file_data, fqns_for_name, resolve_callee_definition,
    resolve_symbol_hierarchy_ambiguity_safe, resolve_symbol_no_rg, resolve_symbol_scoped_only,
};
pub(crate) use scope_check::{receiver_provides_member, resolve_in_scope_strict};

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
pub(crate) use platform_types::resolve_kotlin_builtin_type_platform_equivalent;
#[cfg(test)]
pub(crate) use resolve::resolve_symbol;
#[cfg(test)]
pub(crate) use resolve::resolve_symbol_index_only;
