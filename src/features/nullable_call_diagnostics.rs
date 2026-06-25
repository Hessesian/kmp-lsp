//! Diagnostic: detect a plain `.` member access on a nullable receiver.
//!
//! Kotlin requires a safe call (`?.`) or non-null assertion (`!!.`) to access a
//! member through a nullable receiver. Walks the live CST for all
//! `navigation_expression` nodes using a plain `.`, and for each whose receiver
//! is a simple variable with a nullable declared type, checks whether the
//! accessed name resolves to:
//! - a real class member (always an error on a nullable receiver), or
//! - an extension function/property whose own declared receiver is itself
//!   non-nullable (also an error — Kotlin won't pick that overload for a
//!   nullable argument).
//!
//! Skipped cases (too ambiguous without full type resolution):
//! - Multi-level chains (`a.b.c`) — only a simple identifier receiver is handled.
//! - `this`/`super` receivers.
//! - Names that don't resolve to either a member or a known extension (could be
//!   an unindexed JAR/stdlib symbol, smart-cast, etc.) — skip rather than guess.

use tower_lsp::lsp_types::*;

use crate::indexer::{live_tree::LiveDoc, Indexer, NodeExt};
use crate::queries::{KIND_NAV_EXPR, KIND_SIMPLE_IDENT};
use crate::resolver::{infer_receiver_type, ReceiverKind};

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

    // Multi-level chains (`a.b.c`) — only handle a direct simple-identifier
    // receiver, matching the scope of the existing call-arg diagnostic helpers.
    if receiver_node.kind() != KIND_SIMPLE_IDENT {
        return None;
    }
    let receiver_name = receiver_node.utf8_text_owned(bytes)?;
    if receiver_name == "this" || receiver_name == "super" {
        return None;
    }

    // The member-access operator is the suffix's first (anonymous) child:
    // "." for plain access, "?." for a safe call, "::" for a callable
    // reference. Only plain "." is unsafe on a nullable receiver.
    let operator = suffix_node.child(0)?;
    if operator.kind() != "." {
        return None;
    }

    let member_node = suffix_node.first_child_of_kind(KIND_SIMPLE_IDENT)?;
    let member_name = member_node.utf8_text_owned(bytes)?;

    let receiver_type = infer_receiver_type(indexer, ReceiverKind::Variable(&receiver_name), uri)?;
    if !receiver_type.nullable {
        return None;
    }

    // 1. A real class member (declared in the body or inherited) is always an
    //    error on a nullable receiver, regardless of any extension with the
    //    same name. `resolve_member_only`'s underlying file search isn't
    //    container-scoped, so verify the resolved symbol is actually nested
    //    inside the receiver's class — otherwise a same-named top-level
    //    extension declared in the same file would be mistaken for a member.
    let member_locs = indexer.resolve_member_only(&member_name, &receiver_name, uri);
    if member_locs
        .iter()
        .any(|location| is_member_of(indexer, location, &receiver_type.leaf))
    {
        return Some(diagnostic(&member_node, &receiver_name, &member_name));
    }

    // 2. An extension function/property: safe only when its own declared
    //    receiver is itself nullable. `detail` is the full signature text
    //    (e.g. `"fun String?.isBlankCustom(): Boolean"`), so checking for
    //    `"?."` before the parameter list distinguishes a nullable receiver
    //    from a `?.`/`?` appearing only in a default parameter value.
    if let Some(entries) = indexer.extension_by_receiver.get(&receiver_type.leaf) {
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.name == member_name)
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
        return Some(diagnostic(&member_node, &receiver_name, &member_name));
    }

    None
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
