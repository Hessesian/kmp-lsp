//! Diagnostic: flag a bare class / function reference that is neither
//! reachable from the file's own scope nor available without an import.
//!
//! Flags a bare reference when BOTH hold:
//!   1. it is importable — `fqns_for_name` knows at least one concrete FQN for it
//!      (which excludes stdlib/default-import names, since their sources aren't
//!      indexed, and anything from an unindexed jar);
//!   2. it is NOT reachable from the file's own scope (`resolve_in_scope_strict`):
//!      no local/param decl, explicit import, same-package, or non-stdlib star
//!      import, and not provided by an enclosing extension/implicit-lambda
//!      receiver (`receiver_provides_member` / `all_lambda_receivers_at`).
//!
//! The candidate-collection walk here is shared with the `missing-imports` CLI
//! subcommand (`cli::missing_import_poc`), which runs the same
//! [`collect_missing_import_flags`] over an entire workspace to measure
//! precision — see that module for the aggregate false-positive methodology.

use std::collections::HashSet;

use tower_lsp::lsp_types::*;
use tree_sitter::Node;

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::{all_lambda_receivers_at, Indexer};
use crate::queries::{
    KIND_CALL_EXPR, KIND_DOT, KIND_FUN_DECL, KIND_IMPORT_HEADER, KIND_PACKAGE_HEADER,
    KIND_SIMPLE_IDENT, KIND_TYPE_IDENT, KIND_TYPE_PARAM, KIND_TYPE_PARAMS, KIND_USER_TYPE,
};
use crate::resolver::{fqns_for_name, receiver_provides_member, resolve_in_scope_strict};
use crate::types::CursorPos;

/// A flagged reference: the bare name and where it occurs.
pub(crate) struct MissingImportFlag {
    pub name: String,
    pub line: u32,
    pub col: u32,
}

/// A candidate bare reference plus the extension-receiver types in scope at that point.
struct Candidate {
    name: String,
    line: u32,
    col: u32,
    receivers: Vec<String>,
}

/// The extension-receiver type of `fun Receiver.name(...)`, if any — the `user_type`
/// immediately followed by `.` before the function name.
fn extension_receiver_of(fn_node: Node, src: &[u8]) -> Option<String> {
    let mut c = fn_node.walk();
    let children: Vec<Node> = fn_node.children(&mut c).collect();
    for (i, child) in children.iter().enumerate() {
        if child.kind() == KIND_USER_TYPE
            && children
                .get(i + 1)
                .map(|n| n.kind() == KIND_DOT)
                .unwrap_or(false)
        {
            let mut cc = child.walk();
            for sub in child.children(&mut cc) {
                if sub.kind() == KIND_TYPE_IDENT {
                    return sub.utf8_text(src).ok().map(|s| s.to_owned());
                }
            }
        }
    }
    None
}

/// Names declared by a `type_parameters` node (`<State, Effect: Bound>` → State,
/// Effect). The name is the first identifier child of each `type_parameter`; the
/// (deeper) bound identifier is left for normal collection.
fn collect_type_param_names(tp_node: Node, src: &[u8], out: &mut HashSet<String>) {
    let mut c = tp_node.walk();
    for tp in tp_node.children(&mut c) {
        if tp.kind() != KIND_TYPE_PARAM {
            continue;
        }
        let mut cc = tp.walk();
        for child in tp.children(&mut cc) {
            if child.kind() == KIND_TYPE_IDENT || child.kind() == KIND_SIMPLE_IDENT {
                if let Ok(t) = child.utf8_text(src) {
                    out.insert(t.to_owned());
                }
                break; // first identifier is the parameter name
            }
        }
    }
}

/// Walk the CST collecting candidate bare references: call-expression callees
/// (functions/constructors) and type identifiers (classes). Qualified/member refs,
/// import/package headers, and generic type parameters in scope are skipped to stay
/// high-confidence.
fn collect_candidates(
    node: Node,
    src: &[u8],
    type_params: &HashSet<String>,
    receivers: &[String],
    out: &mut Vec<Candidate>,
) {
    let kind = node.kind();

    // Don't descend into import/package declarations — their identifiers aren't uses.
    if kind == KIND_IMPORT_HEADER || kind == KIND_PACKAGE_HEADER {
        return;
    }

    // Type parameters declared on this node (`class Foo<T>` / `fun <R> bar()`) are in
    // scope for its whole subtree and must never be flagged as missing imports.
    let mut child_scope: Option<HashSet<String>> = None;
    {
        let mut c = node.walk();
        for child in node.children(&mut c) {
            if child.kind() == KIND_TYPE_PARAMS {
                let mut s = type_params.clone();
                collect_type_param_names(child, src, &mut s);
                child_scope = Some(s);
                break;
            }
        }
    }
    let active = child_scope.as_ref().unwrap_or(type_params);

    // An extension-function receiver (`fun Receiver.f()`) puts the receiver type's
    // members in scope for the function body.
    let mut child_receivers: Option<Vec<String>> = None;
    if kind == KIND_FUN_DECL {
        if let Some(r) = extension_receiver_of(node, src) {
            let mut v = receivers.to_vec();
            v.push(r);
            child_receivers = Some(v);
        }
    }
    let active_receivers = child_receivers.as_deref().unwrap_or(receivers);

    if kind == KIND_CALL_EXPR {
        if let Some(callee) = node.child(0) {
            // Only a *bare* callee (`Foo(...)` / `bar(...)`), not `recv.method(...)`.
            if callee.kind() == KIND_SIMPLE_IDENT {
                if let Ok(text) = callee.utf8_text(src) {
                    out.push(Candidate {
                        name: text.to_owned(),
                        line: callee.start_position().row as u32,
                        col: callee.start_position().column as u32,
                        receivers: active_receivers.to_vec(),
                    });
                }
            }
        }
    } else if kind == KIND_TYPE_IDENT {
        // Skip the trailing segment of a qualified type (`a.b.Foo`): if the previous
        // sibling is a `.`, this identifier is already qualified and needs no import.
        let qualified = node
            .prev_sibling()
            .map(|s| s.kind() == KIND_DOT)
            .unwrap_or(false);
        if !qualified {
            if let Ok(text) = node.utf8_text(src) {
                // A generic type parameter in scope (`<State, Effect>`) needs no import.
                if !active.contains(text) {
                    out.push(Candidate {
                        name: text.to_owned(),
                        line: node.start_position().row as u32,
                        col: node.start_position().column as u32,
                        receivers: active_receivers.to_vec(),
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_candidates(child, src, active, active_receivers, out);
    }
}

/// Detect missing-import flags for one already-parsed document.
///
/// The caller is responsible for the document's live-tree lifecycle: the
/// candidate-receiver checks (`all_lambda_receivers_at`) read
/// `indexer.live_doc(uri)`, so `uri` must already have a live tree stored —
/// true for any currently-open file (the live-diagnostics caller), or
/// explicitly arranged by the caller for an offline scan (the CLI's
/// `store_live_tree`/`remove_live_tree` bracket).
pub(crate) fn collect_missing_import_flags(
    indexer: &Indexer,
    uri: &Url,
    doc: &LiveDoc,
) -> Vec<MissingImportFlag> {
    let bytes = &doc.bytes;
    let mut candidates = Vec::new();
    collect_candidates(
        doc.tree.root_node(),
        bytes,
        &HashSet::new(),
        &[],
        &mut candidates,
    );

    // `importable` / `in-scope` are per-name; only the receiver check is per-occurrence,
    // so cache the first two and dedupe flags by name (keep the first occurrence).
    let mut importable: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    let mut in_scope: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    let mut flagged: std::collections::HashMap<&str, (u32, u32)> = std::collections::HashMap::new();
    for c in &candidates {
        // (1) importable — we know a concrete FQN to add (excludes stdlib/unindexed).
        if !*importable
            .entry(&c.name)
            .or_insert_with(|| !fqns_for_name(indexer, &c.name).is_empty())
        {
            continue;
        }
        // (2) reachable from the file's own scope.
        if *in_scope
            .entry(&c.name)
            .or_insert_with(|| resolve_in_scope_strict(indexer, &c.name, uri))
        {
            continue;
        }
        // (3) provided by an enclosing extension receiver (`fun Receiver.f()`).
        if c.receivers
            .iter()
            .any(|r| receiver_provides_member(indexer, r, &c.name))
        {
            continue;
        }
        // (4) provided by an implicit lambda receiver (`LazyColumn { item {} }`):
        // check every enclosing receiver in scope at this position — Kotlin resolves
        // an implicit-receiver call against *every* enclosing receiver (innermost
        // first), so a bare `item()` inside `with(x) { }` nested in a builder belongs
        // to the outer `LazyListScope` even if `x` lacks it.
        let pos = CursorPos {
            line: c.line as usize,
            utf16_col: c.col as usize,
        };
        if all_lambda_receivers_at(pos, indexer, uri)
            .iter()
            .any(|r| receiver_provides_member(indexer, r, &c.name))
        {
            continue;
        }
        flagged.entry(&c.name).or_insert((c.line, c.col));
    }
    flagged
        .into_iter()
        .map(|(name, (line, col))| MissingImportFlag {
            name: name.to_owned(),
            line,
            col,
        })
        .collect()
}

/// Scan a file for missing-import candidates and return diagnostics.
///
/// The caller provides a `LiveDoc` parsed from the *same text* that was just
/// indexed, guaranteeing the CST and the indexed signature data are consistent
/// (same contract as `call_arg_diagnostics` / `nullable_dot_call_diagnostics`).
pub(crate) fn missing_import_diagnostics(
    indexer: &Indexer,
    uri: &Url,
    doc: &LiveDoc,
) -> Vec<Diagnostic> {
    // Suppress while JAR indexing is in flight: this diagnostic's "importable"
    // check reads jar_definitions/jar_qualified directly, so a name whose real
    // FQN lives in a not-yet-materialized JAR looks unimportable (silently
    // skipped — safe) while a name only exemptable via a JAR-indexed
    // default-import package lookup can transiently look flaggable (a false
    // positive) until the JAR catches up. Matches call_arg_diagnostics's own
    // gate; the actor republishes diagnostics once JAR indexing reaches a
    // terminal phase, so the correct result lands as soon as symbols exist.
    if indexer
        .jar_phase
        .lock()
        .map(|p| p.is_loading())
        .unwrap_or(false)
    {
        return Vec::new();
    }
    collect_missing_import_flags(indexer, uri, doc)
        .into_iter()
        .map(|flag| Diagnostic {
            range: Range::new(
                Position::new(flag.line, flag.col),
                Position::new(
                    flag.line,
                    flag.col + flag.name.encode_utf16().count() as u32,
                ),
            ),
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("kmp-lsp".into()),
            message: format!("'{}' is not imported and not in scope", flag.name),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
#[path = "missing_import_diagnostics_tests.rs"]
mod tests;
