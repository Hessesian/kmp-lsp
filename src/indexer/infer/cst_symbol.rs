//! Shared CST identifier classification: declaration-vs-reference, and
//! receiver/member extraction from a `navigation_expression`.
//!
//! Originally written for semantic-token coloring (`semantic_tokens/resolve.rs`);
//! promoted here because `classify_symbol_at` (the navigation-feature
//! classifier: go-def, goto-impl, highlight) needs the identical walk —
//! two independent CST passes answering "declaration or reference?" and
//! "what's the receiver of this member access?" would drift from each other.

use tree_sitter::Node;

use crate::indexer::{CstQuery, Indexer, NodeExt, Resolution, ResolveIo};
use crate::queries::{
    KIND_CALL_EXPR, KIND_CLASS_DECL, KIND_CLASS_PARAM, KIND_COMPANION_OBJ, KIND_ENUM_ENTRY,
    KIND_FUN_DECL, KIND_IDENTIFIER, KIND_IMPORT_HEADER, KIND_NAV_EXPR, KIND_NAV_SUFFIX,
    KIND_OBJECT_DECL, KIND_PARAMETER, KIND_SIMPLE_IDENT, KIND_TYPE_ALIAS, KIND_TYPE_IDENT,
    KIND_TYPE_PARAM, KIND_VAR_DECL,
};
use crate::types::CursorPos;
use tower_lsp::lsp_types::Url;

use super::deps::InferDeps as _;
use super::speculative::ResolutionDoc;

pub(crate) fn is_declaration_site(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let pk = parent.kind();
    if pk == KIND_CLASS_DECL
        || pk == KIND_OBJECT_DECL
        || pk == KIND_COMPANION_OBJ
        || pk == KIND_TYPE_ALIAS
    {
        return node.kind() == KIND_TYPE_IDENT;
    }
    if pk == KIND_FUN_DECL
        || pk == KIND_PARAMETER
        || pk == KIND_ENUM_ENTRY
        || pk == KIND_VAR_DECL
        || pk == KIND_CLASS_PARAM
    {
        return node.kind() == KIND_SIMPLE_IDENT;
    }
    if pk == KIND_TYPE_PARAM {
        return node.kind() == KIND_SIMPLE_IDENT || node.kind() == KIND_TYPE_IDENT;
    }
    false
}

pub(crate) fn navigation_receiver_node(node: Node<'_>) -> Option<Node<'_>> {
    (0..node.child_count())
        .filter_map(|i| node.child(i))
        .find(|child| child.is_named() && child.kind() != crate::queries::KIND_NAV_SUFFIX)
}

pub(crate) fn navigation_member_ident(node: Node<'_>) -> Option<Node<'_>> {
    let suffix = node.first_child_of_kind(crate::queries::KIND_NAV_SUFFIX)?;
    (0..suffix.child_count())
        .filter_map(|i| suffix.child(i))
        .find(|child| child.kind() == KIND_SIMPLE_IDENT || child.kind() == KIND_TYPE_IDENT)
}

pub(crate) fn is_call_callee(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == crate::queries::KIND_CALL_EXPR
        && parent.child(0).map(|child| child.id()) == Some(node.id())
}

/// The classified identifier under the cursor, produced by [`classify_symbol_at`].
#[allow(dead_code)] // wiring seam for later navigation-feature tasks (slice 6a, tasks 3-6)
#[derive(Debug, Clone)]
pub(crate) struct SymbolAtCursor {
    pub name: String,
    pub role: SymbolRole,
}

#[allow(dead_code)] // wiring seam for later navigation-feature tasks (slice 6a, tasks 3-6)
#[derive(Debug, Clone)]
pub(crate) enum SymbolRole {
    Declaration,
    /// `receiver_type` is `Some` only when the reference is a member access
    /// (`x.name`) AND the receiver's type resolved via `CstQuery::expr_type`.
    /// `is_call` is true when the reference is the callee of a call_expression.
    Reference {
        receiver_type: Option<String>,
        is_call: bool,
    },
    ImportSegment,
}

/// Classify the identifier under `pos`: declaration, member reference (with
/// receiver type resolved via the CST where possible), or import segment.
/// Returns `None` for non-identifier positions (strings, comments,
/// whitespace) — callers treat that exactly like today's "nothing under the
/// cursor" case, never an error.
///
/// Acquisition goes through `lambda_doc_at` so mid-typing states (an
/// unclosed brace above the cursor) still classify against a repaired tree.
/// `lambda_doc_at` gates brace repair on `tree.has_error()` *tree-wide* (see
/// its docs) — a MISSING-semicolon node anywhere in the file (a common
/// tree-sitter-kotlin artifact for single-line bodies, e.g. `class User {
/// val id: Int = 0 }`) trips that gate even when the cursor sits nowhere near
/// it, and repair then fails to find an enclosing `lambda_literal` and
/// returns `None`. Unlike the narrower it/this callers, a failed repair isn't
/// authoritative here: fall back to the unrepaired live/parsed tree so an
/// unrelated error elsewhere in the file doesn't blind classification at the
/// cursor.
#[allow(dead_code)] // wiring seam for later navigation-feature tasks (slice 6a, tasks 3-6)
pub(crate) fn classify_symbol_at(
    indexer: &Indexer,
    uri: &Url,
    pos: CursorPos,
) -> Option<SymbolAtCursor> {
    let resolution = super::speculative::lambda_doc_at(indexer, uri, pos)
        .or_else(|| indexer.live_doc_or_parse(uri).map(ResolutionDoc::Parsed))?;
    let doc = resolution.doc();
    let node = super::cst_lambda::cursor_node_at(doc, pos)?;

    if !matches!(node.kind(), KIND_SIMPLE_IDENT | KIND_TYPE_IDENT) {
        return None;
    }
    let name = node.utf8_text_owned(&doc.bytes)?;

    if is_declaration_site(node) {
        return Some(SymbolAtCursor {
            name,
            role: SymbolRole::Declaration,
        });
    }

    // Import path segments are flat `simple_identifier` children of a single
    // `identifier` node (`import a.b.C` → `identifier(simple_identifier x3)`),
    // not directly nested one-per-dot — check both the node's parent (in case
    // the grammar ever emits a bare single-segment import directly under
    // `import_header`) and its grandparent through that `identifier` wrapper.
    let is_import_segment = node.parent().is_some_and(|p| {
        p.kind() == KIND_IMPORT_HEADER
            || (p.kind() == KIND_IDENTIFIER
                && p.parent().is_some_and(|gp| gp.kind() == KIND_IMPORT_HEADER))
    });
    if is_import_segment {
        return Some(SymbolAtCursor {
            name,
            role: SymbolRole::ImportSegment,
        });
    }

    // Member reference: the identifier is the member name of a nav_expr's suffix.
    if let Some(nav) = node
        .parent()
        .and_then(|suffix| (suffix.kind() == KIND_NAV_SUFFIX).then_some(suffix))
        .and_then(|suffix| suffix.parent())
    {
        if nav.kind() == KIND_NAV_EXPR
            && navigation_member_ident(nav).is_some_and(|m| m.id() == node.id())
        {
            let is_call = is_call_callee(nav);
            // `expr_type` for a parameter/variable receiver echoes back its
            // syntactic type annotation verbatim (see `infer_ident_type` /
            // `find_var_type`) without checking that the annotated name is an
            // actual known type — `x: Unknown` resolves to `Some("Unknown")`
            // even though `Unknown` is declared nowhere. Gate on
            // `has_type_definition` so a made-up/unresolvable annotation
            // doesn't silently masquerade as a real receiver type (house
            // decoy: `untypeable_receiver_yields_no_receiver_type`).
            let receiver_type = navigation_receiver_node(nav).and_then(|receiver| {
                match CstQuery::new(receiver, doc, indexer, uri, ResolveIo::IndexOnly).expr_type() {
                    Resolution::Resolved(t) if indexer.has_type_definition(t.as_type_str()) => {
                        Some(t.as_type_str().to_owned())
                    }
                    _ => None,
                }
            });
            return Some(SymbolAtCursor {
                name,
                role: SymbolRole::Reference {
                    receiver_type,
                    is_call,
                },
            });
        }
    }

    // Bare reference (local var, top-level name, etc.) — no receiver, scope
    // resolution deferred (see Global Constraints). Callers fall through to
    // today's NameScan path for these.
    let is_call = node.parent().is_some_and(|p| {
        p.kind() == KIND_CALL_EXPR && p.child(0).map(|c| c.id()) == Some(node.id())
    });
    Some(SymbolAtCursor {
        name,
        role: SymbolRole::Reference {
            receiver_type: None,
            is_call,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Indexer;
    use tower_lsp::lsp_types::Url;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file:///t{path}")).unwrap()
    }

    fn indexed_with_live(path: &str, src: &str) -> (Url, Indexer) {
        let u = uri(path);
        let idx = Indexer::new();
        idx.index_content(&u, src);
        idx.store_live_tree(&u, src);
        (u, idx)
    }

    #[test]
    fn classifies_a_class_declaration() {
        let (u, idx) = indexed_with_live("/D.kt", "class User { val id: Int = 0 }\n");
        // cursor on "User"
        let sym = classify_symbol_at(
            &idx,
            &u,
            CursorPos {
                line: 0,
                utf16_col: 8,
            },
        )
        .unwrap();
        assert_eq!(sym.name, "User");
        assert!(matches!(sym.role, SymbolRole::Declaration));
    }

    #[test]
    fn classifies_a_typed_member_reference() {
        let src = "class User { fun save() {} }\nfun f(user: User) { user.save() }\n";
        let (u, idx) = indexed_with_live("/D.kt", src);
        // cursor on "save" in "user.save()"
        let col = src.lines().nth(1).unwrap().find("save").unwrap() as u32;
        let sym = classify_symbol_at(
            &idx,
            &u,
            CursorPos {
                line: 1,
                utf16_col: col as usize,
            },
        )
        .unwrap();
        assert_eq!(sym.name, "save");
        match sym.role {
            SymbolRole::Reference {
                receiver_type: Some(t),
                is_call: true,
            } => assert_eq!(t, "User"),
            other => panic!("expected typed call reference, got {other:?}"),
        }
    }

    #[test]
    fn no_symbol_inside_a_string_literal() {
        let (u, idx) = indexed_with_live("/D.kt", "fun f() { val s = \"User\" }\n");
        let col = "fun f() { val s = \"".len() as u32;
        assert!(classify_symbol_at(
            &idx,
            &u,
            CursorPos {
                line: 0,
                utf16_col: col as usize
            }
        )
        .is_none());
    }

    #[test]
    fn classifies_an_import_segment() {
        let (u, idx) = indexed_with_live("/D.kt", "import com.example.User\n");
        let col = "import com.example.".len() as u32;
        let sym = classify_symbol_at(
            &idx,
            &u,
            CursorPos {
                line: 0,
                utf16_col: col as usize,
            },
        )
        .unwrap();
        assert_eq!(sym.name, "User");
        assert!(matches!(sym.role, SymbolRole::ImportSegment));
    }

    /// The cursor's own ancestor chain sits inside an `ERROR` node — deeply
    /// nested unclosed call args (`foo(bar(baz(qux`), not just an unrelated
    /// MISSING-semicolon artifact elsewhere in the file. `lambda_doc_at`'s
    /// brace-repair only accepts a candidate whose cursor gains an enclosing
    /// `lambda_literal`; none of these unclosed parens can ever become one
    /// (they're call-argument lists, not lambda braces), so repair exhausts
    /// `MAX_BRACE_REPAIRS` and `lambda_doc_at` returns `None` — the raw-tree
    /// fallback in `classify_symbol_at` is what actually serves this request.
    /// Verified empirically (see fix report): `lambda_doc_at` returns `None`
    /// for this exact snippet/position, and the cursor's ancestor chain is
    /// `["simple_identifier", "value_argument", "ERROR"]`.
    ///
    /// Every check in `classify_symbol_at` after acquiring the doc is an
    /// exact `node.kind() == ...` match against the identifier's *parent*
    /// kind; here that parent is `ERROR`, which matches none of them, so the
    /// function falls closed to the bare-reference case with no fabricated
    /// receiver/call info — never a wrong classification.
    #[test]
    fn safely_degrades_when_cursor_sits_inside_an_error_node() {
        let src = "class User {\nfun f() {\nif (foo(bar(baz(qux\n";
        let (u, idx) = indexed_with_live("/D.kt", src);
        let col = src.lines().nth(2).unwrap().find("qux").unwrap();
        let pos = CursorPos {
            line: 2,
            utf16_col: col,
        };

        // Empirical precondition: lambda_doc_at must actually fail here, or
        // this test isn't exercising the raw-tree fallback at all.
        assert!(
            super::super::speculative::lambda_doc_at(&idx, &u, pos).is_none(),
            "expected lambda_doc_at to return None (brace repair exhausted) \
             so classify_symbol_at's raw-tree fallback is what's under test"
        );

        // Empirical precondition: the cursor's own node sits inside an ERROR
        // node, not just somewhere unrelated in the tree.
        let doc = idx.live_doc_or_parse(&u).unwrap();
        let node = super::super::cst_lambda::cursor_node_at(&doc, pos).unwrap();
        assert_eq!(node.kind(), KIND_SIMPLE_IDENT);
        assert_eq!(node.utf8_text(&doc.bytes).unwrap(), "qux");
        assert_eq!(node.parent().unwrap().parent().unwrap().kind(), "ERROR");

        // The actual behavior under test: no panic, and no fabricated
        // classification. `qux`'s immediate parent is `value_argument`
        // inside the ERROR node — none of is_declaration_site's or the
        // member-reference branch's exact-kind checks match an ERROR
        // ancestor, so this falls through to the bare-reference case with
        // name echoed verbatim and nothing fabricated (no receiver, not
        // marked as a call).
        let sym = classify_symbol_at(&idx, &u, pos).expect("falls to bare reference, not None");
        assert_eq!(sym.name, "qux");
        match sym.role {
            SymbolRole::Reference {
                receiver_type: None,
                is_call: false,
            } => {}
            other => panic!("expected bare, unfabricated reference, got {other:?}"),
        }
    }

    /// House decoy: an untypeable receiver must not silently attach a wrong
    /// or stale receiver_type.
    #[test]
    fn untypeable_receiver_yields_no_receiver_type() {
        let src = "fun f(x: Unknown) { x.save() }\n";
        let (u, idx) = indexed_with_live("/D.kt", src);
        let col = src.find("save").unwrap() as u32;
        let sym = classify_symbol_at(
            &idx,
            &u,
            CursorPos {
                line: 0,
                utf16_col: col as usize,
            },
        )
        .unwrap();
        match sym.role {
            SymbolRole::Reference {
                receiver_type: None,
                ..
            } => {}
            other => panic!("expected no receiver_type, got {other:?}"),
        }
    }
}
