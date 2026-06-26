//! Diagnostic: detect a plain `.` member access on a nullable receiver.
//!
//! Kotlin requires a safe call (`?.`) or non-null assertion (`!!.`) to access a
//! member through a nullable receiver. Walks the live CST for all
//! `navigation_expression` nodes using a plain `.`, and for each whose receiver
//! has a nullable inferred type — either a simple variable (`repo.load()`) or a
//! pure field-access chain (`holder.repo.load()`, where `repo` is a nullable
//! field) — checks whether the accessed name resolves to:
//! - a real class member (always an error on a nullable receiver), or
//! - an extension function/property whose own declared receiver is itself
//!   non-nullable (also an error — Kotlin won't pick that overload for a
//!   nullable argument).
//!
//! Skipped cases (too ambiguous without full type resolution):
//! - Receivers that aren't a plain identifier-and-field chain (e.g. a call
//!   result `getFoo().bar`, an index `xs[0]`, or a `?.`/`::` in the chain).
//! - `this`/`super` receivers.
//! - Names that don't resolve to either a member or a known extension (could be
//!   an unindexed JAR/stdlib symbol, smart-cast, etc.) — skip rather than guess.

use tower_lsp::lsp_types::*;

use crate::indexer::{live_tree::LiveDoc, Indexer, NodeExt};
use crate::queries::{KIND_NAV_EXPR, KIND_SIMPLE_IDENT};
use crate::resolver::{ReceiverKind, ReceiverType, Resolver};

/// Scan a file for plain-`.` member access on nullable receivers.
///
/// The caller provides a `LiveDoc` parsed from the *same text* that was just
/// indexed, guaranteeing the CST and the indexed signature data are consistent.
pub(crate) fn nullable_dot_call_diagnostics(
    indexer: &Indexer,
    uri: &Url,
    doc: &LiveDoc,
) -> Vec<Diagnostic> {
    // NOTE: unlike `call_arg_diagnostics`, this diagnostic is *not* gated on
    // `jar_phase.is_loading()`. JAR indexing on a large project can take many
    // seconds, and gating here meant the diagnostic stayed invisible for that
    // whole window (the symptom that surfaced this: "no diagnostics on live
    // lines"). It is safe to run during loading because:
    //   * Every true positive resolves to a workspace-local symbol (a project
    //     class member via `resolve_member_only`, or a project extension via
    //     `extension_by_receiver`), all of which are populated by the fast
    //     source scan — none depend on JAR symbols.
    //   * The diagnostic only fires when it *positively* resolves a member or a
    //     non-nullable extension; a partial index that simply lacks a symbol
    //     yields a skip, never a false flag.
    //   * Generic stdlib scope functions (`let`/`also`/`run`/…) are keyed in
    //     `extension_by_receiver` under their type-parameter receiver (`T`),
    //     not a concrete leaf type, so a concrete-leaf lookup never matches
    //     them — `s.let { }` on a nullable `s` is never flagged.
    let bytes = &doc.bytes;
    let mut diagnostics = Vec::new();
    collect_nav_nodes(doc.tree.root_node(), bytes, indexer, uri, &mut diagnostics);
    diagnostics
}

fn collect_nav_nodes(
    node: tree_sitter::Node,
    bytes: &[u8],
    indexer: &Indexer,
    uri: &Url,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if node.kind() == KIND_NAV_EXPR {
        if let Some(diag) = check_nullable_dot_call(&node, bytes, indexer, uri) {
            diagnostics.push(diag);
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_nav_nodes(cursor.node(), bytes, indexer, uri, diagnostics);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn check_nullable_dot_call(
    navigation_node: &tree_sitter::Node,
    bytes: &[u8],
    indexer: &Indexer,
    uri: &Url,
) -> Option<Diagnostic> {
    // navigation_expression named children: receiver_expr, navigation_suffix.
    let named_count = navigation_node.named_child_count();
    if named_count < 2 {
        return None;
    }
    let receiver_node = navigation_node.named_child(0)?;
    let suffix_node = navigation_node.named_child(named_count - 1)?;

    // The member-access operator is the suffix's first (anonymous) child:
    // "." for plain access, "?." for a safe call, "::" for a callable
    // reference. Only plain "." is unsafe on a nullable receiver.
    let operator = suffix_node.child(0)?;
    if operator.kind() != "." {
        return None;
    }

    let member_node = suffix_node.first_child_of_kind(KIND_SIMPLE_IDENT)?;
    let member_name = member_node.utf8_text_owned(bytes)?;

    // The receiver is either a simple variable (`repo.load()`) or a pure
    // field-access chain (`holder.repo.load()`, where `repo` is a nullable
    // field). `qualifier` is the text we hand to `resolve_member_only` to
    // locate the member; `receiver_type` carries the inferred nullability.
    let (qualifier, receiver_type) = resolve_receiver(indexer, &receiver_node, bytes, uri)?;
    if !receiver_type.nullable {
        return None;
    }

    // 1. A real class member (declared in the body or inherited) is always an
    //    error on a nullable receiver, regardless of any extension with the
    //    same name. `resolve_member_only`'s underlying file search isn't
    //    container-scoped, so verify the resolved symbol is actually nested
    //    inside the receiver's class — otherwise a same-named top-level
    //    extension declared in the same file would be mistaken for a member.
    let member_locs = indexer.resolve_member(&member_name, &qualifier, uri);
    if member_locs
        .iter()
        .any(|location| is_member_of(indexer, location, &receiver_type.leaf))
    {
        return Some(diagnostic(&member_node, &qualifier, &member_name));
    }

    // 2. An extension function/property: safe only when its own declared
    //    receiver is itself nullable. `detail` is the full signature text
    //    (e.g. `"fun String?.isBlankCustom(): Boolean"`), so checking for
    //    `"?."` before the parameter list distinguishes a nullable receiver
    //    from a `?.`/`?` appearing only in a default parameter value.
    //
    //    Only consider extensions actually *visible* from this file (same
    //    package or imported). `extension_by_receiver` is workspace-global, so
    //    an unscoped match could flag a member off an extension that isn't in
    //    scope here, or be silenced by an out-of-scope nullable-receiver
    //    overload — see `extension_is_in_scope`.
    if let Some(entries) = indexer.extension_by_receiver.get(&receiver_type.leaf) {
        let caller_file_data = indexer.file_data_for(uri.as_str());
        let caller_file_data_ref = caller_file_data.as_deref();
        let caller_package = caller_file_data.as_ref().and_then(|fd| fd.package.as_ref());
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.name == member_name)
            .filter(|entry| {
                extension_in_scope_here(entry, uri, caller_package, caller_file_data_ref)
            })
            .collect();
        if matches.is_empty() {
            return None;
        }
        let any_nullable_safe = matches
            .iter()
            .any(|entry| extension_detail_has_nullable_receiver(&entry.detail));
        if any_nullable_safe {
            return None;
        }
        return Some(diagnostic(&member_node, &qualifier, &member_name));
    }

    None
}

/// Resolve a receiver node into the text used for member lookup plus its
/// inferred [`ReceiverType`].
///
/// Handles two shapes:
/// - a simple variable (`repo` in `repo.load()`), and
/// - a pure field-access chain (`holder.repo` in `holder.repo.load()`), where
///   a nullable data-class field is the receiver.
///
/// Returns `None` for `this`/`super` roots and for any receiver that isn't a
/// plain identifier-and-field chain (e.g. a call result like `getFoo().bar`),
/// which we can't reason about without fuller type resolution.
fn resolve_receiver(
    indexer: &Indexer,
    receiver_node: &tree_sitter::Node,
    bytes: &[u8],
    uri: &Url,
) -> Option<(String, ReceiverType)> {
    match receiver_node.kind() {
        KIND_SIMPLE_IDENT => {
            let name = receiver_node.utf8_text_owned(bytes)?;
            if name == "this" || name == "super" {
                return None;
            }
            let receiver_type = indexer.infer_receiver_type(ReceiverKind::Variable(&name), uri)?;
            Some((name, receiver_type))
        }
        KIND_NAV_EXPR => {
            let chain = pure_field_chain(receiver_node, bytes)?;
            // Need a root plus at least one field segment, and not `this.x`/`super.x`.
            if chain.len() < 2 || chain[0] == "this" || chain[0] == "super" {
                return None;
            }
            let receiver_type = indexer.infer_field_chain_type(&chain, uri)?;
            Some((chain.join("."), receiver_type))
        }
        _ => None,
    }
}

/// Collect a pure field-access chain into its segment names, or `None` if the
/// node is anything other than a simple identifier optionally followed by
/// `.field` accesses (all plain `.`, no `?.`/`::`, no call/index suffixes).
///
/// `holder.repo` → `["holder", "repo"]`; `getFoo().bar` → `None`.
fn pure_field_chain(node: &tree_sitter::Node, bytes: &[u8]) -> Option<Vec<String>> {
    match node.kind() {
        KIND_SIMPLE_IDENT => Some(vec![node.utf8_text_owned(bytes)?]),
        KIND_NAV_EXPR => {
            let named_count = node.named_child_count();
            if named_count < 2 {
                return None;
            }
            let receiver = node.named_child(0)?;
            let suffix = node.named_child(named_count - 1)?;
            // Only plain `.` field access — reject `?.`, `::`, and any suffix
            // whose accessed name isn't a simple identifier.
            if suffix.child(0)?.kind() != "." {
                return None;
            }
            let field = suffix.first_child_of_kind(KIND_SIMPLE_IDENT)?;
            let mut chain = pure_field_chain(&receiver, bytes)?;
            chain.push(field.utf8_text_owned(bytes)?);
            Some(chain)
        }
        _ => None,
    }
}

/// Whether `entry` (a workspace-global extension) is actually visible from the
/// file at `uri`. An extension is in scope when it is declared in the *same
/// file*, lives in the *same package* (two files with no package declaration
/// share the root package), or is covered by an import — matching the rules
/// `extension_is_in_scope` applies to packaged/imported extensions, plus the
/// same-file/same-default-package cases it doesn't model.
fn extension_in_scope_here(
    entry: &crate::types::ExtensionEntry,
    uri: &Url,
    caller_package: Option<&String>,
    caller_file_data: Option<&crate::types::FileData>,
) -> bool {
    entry.file_uri == uri.as_str()
        || (entry.package.is_none() && caller_package.is_none())
        || crate::resolver::infer::extension_is_in_scope(
            entry.package.as_ref(),
            &entry.name,
            caller_package,
            caller_file_data,
        )
}

/// Whether the symbol declared at `location` is nested inside a container
/// named `class_name` — i.e. a real member, not a same-named top-level
/// symbol (e.g. an extension function) that happens to live in the same file.
fn is_member_of(indexer: &Indexer, location: &Location, class_name: &str) -> bool {
    let Some(file_data) = indexer.file_data_for(location.uri.as_str()) else {
        return false;
    };
    file_data
        .symbols
        .iter()
        .find(|symbol| symbol.selection_range == location.range)
        .is_some_and(|symbol| symbol.container.as_deref() == Some(class_name))
}

/// Whether an extension's signature text declares a nullable receiver, e.g.
/// `"fun String?.isBlankCustom(): Boolean"`. Checks only the text before the
/// first `(` so a `?.`/`?` inside a default parameter value doesn't count.
fn extension_detail_has_nullable_receiver(detail: &str) -> bool {
    detail.split('(').next().unwrap_or(detail).contains("?.")
}

fn diagnostic(
    member_node: &tree_sitter::Node,
    receiver_name: &str,
    member_name: &str,
) -> Diagnostic {
    let start = member_node.start_position();
    let end = member_node.end_position();
    Diagnostic {
        range: Range::new(
            Position::new(start.row as u32, start.column as u32),
            Position::new(end.row as u32, end.column as u32),
        ),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("kmp-lsp".into()),
        message: format!(
            "{member_name}: receiver '{receiver_name}' is nullable — use '?.' or '!!.' to access this member"
        ),
        ..Default::default()
    }
}

#[cfg(test)]
#[path = "nullable_call_diagnostics_tests.rs"]
mod tests;
