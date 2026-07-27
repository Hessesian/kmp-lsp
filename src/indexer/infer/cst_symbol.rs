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
    KIND_BINDING_PATTERN_KIND, KIND_CALL_EXPR, KIND_CATCH_BLOCK, KIND_CLASS_DECL, KIND_CLASS_PARAM,
    KIND_COMPANION_OBJ, KIND_CONTROL_STRUCTURE_BODY, KIND_ENUM_ENTRY, KIND_FINALLY_BLOCK,
    KIND_FOR_STMT, KIND_FUN_DECL, KIND_IDENTIFIER, KIND_IMPORT_HEADER, KIND_LAMBDA_LIT,
    KIND_NAV_EXPR, KIND_NAV_SUFFIX, KIND_OBJECT_DECL, KIND_PARAMETER, KIND_SIMPLE_IDENT,
    KIND_STATEMENTS, KIND_TRY_EXPR, KIND_TYPE_ALIAS, KIND_TYPE_IDENT, KIND_TYPE_PARAM,
    KIND_VAR_DECL, KIND_WHEN_EXPR,
};
use crate::resolver::api::Definitions;
use crate::semantic_tokens::is_named_argument_label;
use crate::types::CursorPos;
use tower_lsp::lsp_types::{Location, Position, Url};

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
        || pk == KIND_CATCH_BLOCK
    {
        return node.kind() == KIND_SIMPLE_IDENT;
    }
    if pk == KIND_TYPE_PARAM {
        return node.kind() == KIND_SIMPLE_IDENT || node.kind() == KIND_TYPE_IDENT;
    }
    false
}

/// Whether a declaration-site identifier (as classified by
/// [`is_declaration_site`]) names a symbol `KOTLIN_DEFINITIONS`
/// (`queries.rs`) actually indexes into `f.symbols`.
///
/// Most declaration parents (`class`/`object`/`companion`/`typealias`/`fun`/
/// `val`/`var`/enum entry) map straight onto a `KOTLIN_DEFINITIONS` pattern.
/// Three don't:
/// - `KIND_PARAMETER` — a bare function parameter; never indexed.
/// - `KIND_TYPE_PARAM` — a generic type parameter (`<T>`); never indexed.
/// - `KIND_CLASS_PARAM` — a primary-constructor parameter; indexed only when
///   it carries an explicit `val`/`var` (`KOTLIN_DEFINITIONS` patterns 18/19
///   require a `binding_pattern_kind` child). Without one it's a plain
///   constructor parameter, not a property, and stays unindexed.
///
/// These three are locally-scoped names a name-based
/// `find_definition_qualified` lookup can't safely resolve: nothing in the
/// workspace symbol index anchors the lookup to the cursor's specific
/// declaration, so it either falls through to `find_local_declaration`'s
/// unanchored same-file text scan or a full workspace-wide scan. Callers must
/// treat these as `NameScan`, not `CstResolved`.
///
/// Precondition: `is_declaration_site(node)` is `true` (so `node.parent()` is
/// known to exist and be one of the recognized declaration-parent kinds).
pub(crate) fn is_indexed_declaration_site(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        k if k == KIND_PARAMETER || k == KIND_TYPE_PARAM => false,
        k if k == KIND_CLASS_PARAM => parent
            .first_child_of_kind(KIND_BINDING_PATTERN_KIND)
            .is_some(),
        _ => true,
    }
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
#[derive(Debug, Clone)]
pub(crate) struct SymbolAtCursor {
    pub name: String,
    pub role: SymbolRole,
}

#[derive(Debug, Clone)]
pub(crate) enum SymbolRole {
    /// `indexed` is `true` when this declaration's name is captured by
    /// `KOTLIN_DEFINITIONS` (`queries.rs`) — see
    /// [`is_indexed_declaration_site`] for exactly which node kinds qualify.
    /// `false` for locally-scoped declaration sites (bare function
    /// parameters, val/var-less constructor parameters, generic type
    /// parameters) that never make it into `f.symbols`.
    Declaration {
        indexed: bool,
    },
    /// `receiver_type` is `Some` only when the reference is a member access
    /// (`x.name`) AND the receiver's type resolved via `CstQuery::expr_type`.
    /// `is_call` is true when the reference is the callee of a call_expression.
    Reference {
        receiver_type: Option<String>,
        is_call: bool,
    },
    ImportSegment,
}

/// `classify_symbol_at`, but taking an LSP `Position` directly — the
/// `Position → CursorPos` conversion every navigation-feature call site
/// otherwise repeats.
pub(crate) fn classify_cursor(
    indexer: &Indexer,
    uri: &Url,
    position: Position,
) -> Option<SymbolAtCursor> {
    classify_symbol_at(
        indexer,
        uri,
        CursorPos {
            line: position.line as usize,
            utf16_col: position.character as usize,
        },
    )
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
            role: SymbolRole::Declaration {
                indexed: is_indexed_declaration_site(node),
            },
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

/// A definitions lookup result, tagged by how much confidence its identity
/// carries.
#[derive(Debug)]
pub(crate) enum NavigationSource<T> {
    /// Identity established from the CST + index: precise, ranked first.
    CstResolved(T),
    /// Name-based scan: today's behavior, visibly labeled.
    NameScan(T),
}

/// Resolve `sym`'s identity to its definition site(s).
///
/// `CstResolved` when the CST gave enough information to trust the result
/// (a declaration is trivially its own definition; a receiver-typed member
/// reference is looked up ON that type). `NameScan` for everything the CST
/// couldn't narrow — an untyped receiver, or a bare reference resolved by
/// today's name-based `find_definition_qualified(name, None, uri)` (which
/// can span multiple same-named workspace symbols).
pub(crate) fn resolve_identity(
    sym: &SymbolAtCursor,
    indexer: &Indexer,
    uri: &Url,
) -> NavigationSource<Definitions> {
    match &sym.role {
        SymbolRole::Declaration { indexed } => {
            let locs = Definitions(indexer.find_definition_qualified(&sym.name, None, uri));
            // Only declarations `KOTLIN_DEFINITIONS` actually indexes can be
            // trusted CST-resolved; an unindexed one (bare param, val/var-less
            // constructor param, type param) falls through to an unanchored
            // same-file scan or workspace-wide scan — label it NameScan (see
            // `is_indexed_declaration_site`).
            if *indexed {
                NavigationSource::CstResolved(locs)
            } else {
                NavigationSource::NameScan(locs)
            }
        }
        SymbolRole::Reference {
            receiver_type: Some(receiver_type),
            ..
        } => {
            let locs = indexer.find_definition_qualified(&sym.name, Some(receiver_type), uri);
            if locs.is_empty() {
                NavigationSource::NameScan(Definitions(locs))
            } else {
                NavigationSource::CstResolved(Definitions(locs))
            }
        }
        SymbolRole::Reference {
            receiver_type: None,
            ..
        }
        | SymbolRole::ImportSegment => NavigationSource::NameScan(Definitions(
            indexer.find_definition_qualified(&sym.name, None, uri),
        )),
    }
}

/// For the local variable / lambda-parameter the cursor is on (either its
/// declaration or any reference to it), collect every occurrence within its
/// enclosing function/lambda body via a CST subtree walk — no rg, no index,
/// no cross-file verification. Returns `None` when the name under the cursor
/// isn't itself declared as a local inside an enclosing function/lambda body
/// — callers fall through to the cross-file path in that case.
///
/// Every returned `Location` is `CstResolved` by construction: it comes from
/// walking the actual parse tree, not a text scan. A nested function/lambda
/// that redeclares the same name shadows it — occurrences inside that nested
/// scope are excluded, since they refer to the shadowing declaration, not
/// this one.
pub(crate) fn local_scope_occurrences(
    indexer: &Indexer,
    uri: &Url,
    cursor_position: Position,
) -> Option<Vec<Location>> {
    let doc = indexer.live_doc_or_parse(uri)?;
    let cursor = CursorPos {
        line: cursor_position.line as usize,
        utf16_col: cursor_position.character as usize,
    };
    let cursor_node = crate::indexer::cursor_node_at(&doc, cursor)?;
    if is_named_argument_label(cursor_node) {
        return None; // names the callee's parameter, not the caller's local
    }
    let name = cursor_node.utf8_text_owned(&doc.bytes)?;

    // Never widen past a boundary that already owns `name` -- else an
    // unrelated sibling for/when/if reusing the same name could be pulled in.
    let body = ancestor_scope_boundaries(cursor_node)
        .find(|scope| declares_name_directly(*scope, &name, &doc.bytes).is_some())?;

    let mut occurrences: Vec<Occurrence> = Vec::new();
    visit_unshadowed_name_matches(
        body,
        &name,
        0,
        false,
        None,
        &doc.bytes,
        &mut |node, generation| occurrences.push(Occurrence { node, generation }),
    );

    let cursor_generation = occurrences
        .iter()
        .find(|occurrence| occurrence.node.id() == cursor_node.id())?
        .generation;
    let members = match generation_group(&occurrences, cursor_generation) {
        GenerationGroup::Anchored(members) => members,
        GenerationGroup::Forward => return None,
    };

    let full_text = std::str::from_utf8(&doc.bytes).ok()?;
    let locations: Vec<Location> = members
        .into_iter()
        .filter_map(|occurrence| node_to_location(uri, occurrence.node, full_text))
        .collect();
    if locations.is_empty() {
        None
    } else {
        Some(locations)
    }
}

/// One `simple_identifier` match for a name, tagged with its sequential
/// re-declaration generation (see [`visit_statements_with_generations`]).
#[derive(Clone, Copy)]
struct Occurrence<'a> {
    node: Node<'a>,
    generation: usize,
}

/// Every occurrence sharing one generation, or proof that generation has no
/// declaration of its own — a reference textually before any declaration of
/// its name in scope (invalid Kotlin, real during mid-typing). No CST trick
/// repairs `Forward`: nothing there is syntactically broken, just out of
/// order, so callers fall through to the cross-file path.
enum GenerationGroup<'a> {
    Anchored(Vec<Occurrence<'a>>),
    Forward,
}

fn generation_group<'a>(occurrences: &[Occurrence<'a>], generation: usize) -> GenerationGroup<'a> {
    let members: Vec<Occurrence<'a>> = occurrences
        .iter()
        .copied()
        .filter(|occurrence| occurrence.generation == generation)
        .collect();
    if members
        .iter()
        .any(|occurrence| is_declaration_site(occurrence.node))
    {
        GenerationGroup::Anchored(members)
    } else {
        GenerationGroup::Forward
    }
}

/// How a CST node introduces local scope for the rename local-scope walk.
///
/// Not every scope-boundary node has the same shape: a `for`/`when` binds a
/// name in its own header (outside the `{}`), while a plain block's bound
/// names are declared entirely inside it.
enum ScopeBoundary<'a> {
    /// The scope is the node's entire subtree, including a header-bound name
    /// that sits outside its inner block (`function_declaration`'s own
    /// parameters, `for_statement`'s loop variable, `when_expression`'s
    /// subject binding).
    WholeNode(Node<'a>),
    /// The scope is exactly this node's subtree — a brace-delimited block
    /// with no binding of its own outside it.
    Block(Node<'a>),
}

impl<'a> ScopeBoundary<'a> {
    fn node(self) -> Node<'a> {
        match self {
            ScopeBoundary::WholeNode(node) | ScopeBoundary::Block(node) => node,
        }
    }
}

fn scope_boundary_at(node: Node<'_>) -> Option<ScopeBoundary<'_>> {
    match node.kind() {
        kind if kind == KIND_FUN_DECL
            || kind == KIND_LAMBDA_LIT
            || kind == KIND_FOR_STMT
            || kind == KIND_WHEN_EXPR =>
        {
            Some(ScopeBoundary::WholeNode(node))
        }
        kind if kind == KIND_CONTROL_STRUCTURE_BODY
            || kind == KIND_CATCH_BLOCK
            || kind == KIND_FINALLY_BLOCK
            || kind == KIND_TRY_EXPR =>
        {
            Some(ScopeBoundary::Block(node))
        }
        _ => None,
    }
}

/// Ancestor scope boundaries of `node`, narrowest first.
fn ancestor_scope_boundaries(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut current = node;
    std::iter::from_fn(move || loop {
        let parent = current.parent()?;
        let child = current;
        current = parent;
        if let Some(boundary) = scope_boundary_at(parent) {
            if !is_functions_own_name(parent, child) {
                return Some(boundary.node());
            }
        }
    })
}

/// A function's own name is a direct `simple_identifier` child of its
/// `KIND_FUN_DECL` — the cursor sitting on that name must not treat the
/// function's own declaration as local to its own body. `for_statement`/
/// `when_expression`'s bound names sit one level deeper (through
/// `variable_declaration`), so this ambiguity is unique to `KIND_FUN_DECL`.
fn is_functions_own_name(parent: Node<'_>, child: Node<'_>) -> bool {
    parent.kind() == KIND_FUN_DECL && is_declaration_site(child)
}

/// Whether `scope` itself — not any nested scope inside it — directly
/// declares `name`: every `scope_boundary_at` match found while descending is
/// treated as opaque and not searched into. Also the shadow-check primitive:
/// a nested scope "shadows" `name` exactly when it directly declares it.
fn declares_name_directly<'a>(scope: Node<'a>, name: &str, bytes: &[u8]) -> Option<Node<'a>> {
    let mut stack = vec![scope];
    while let Some(node) = stack.pop() {
        if is_declaration_site(node) && node.utf8_text_owned(bytes).as_deref() == Some(name) {
            return Some(node);
        }
        if node.id() != scope.id() && scope_boundary_at(node).is_some() {
            continue;
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    None
}

/// Walk `statements`'s children in document order, incrementing `generation`
/// after each one that declares `name`. A declaring statement's own
/// initializer is visited at the OLD generation, then re-emitted at the new
/// one — `val x: Any = "hello"; val x = x as String`'s own initializer
/// reference must see the FIRST `x`, since the second doesn't exist yet
/// while its own initializer runs.
fn visit_statements_with_generations<'a>(
    statements: Node<'a>,
    name: &str,
    mut generation: usize,
    already_shadowed: bool,
    bytes: &[u8],
    visit: &mut impl FnMut(Node<'a>, usize),
) {
    for i in 0..statements.child_count() {
        let Some(statement) = statements.child(i) else {
            continue;
        };
        match declares_name_directly(statement, name, bytes) {
            Some(declaration_node) => {
                visit_unshadowed_name_matches(
                    statement,
                    name,
                    generation,
                    already_shadowed,
                    Some(declaration_node),
                    bytes,
                    visit,
                );
                generation += 1;
                if !already_shadowed {
                    visit(declaration_node, generation);
                }
            }
            None => visit_unshadowed_name_matches(
                statement,
                name,
                generation,
                already_shadowed,
                None,
                bytes,
                visit,
            ),
        }
    }
}

/// Walk `node`'s subtree, calling `visit` on every `simple_identifier`
/// matching `name` (tagged with `generation`), skipping the subtree of any
/// nested scope boundary that itself redeclares `name` (shadowing).
/// `exclude` suppresses one node — a declaration re-emitted separately by
/// [`visit_statements_with_generations`] at its own new generation.
fn visit_unshadowed_name_matches<'a>(
    node: Node<'a>,
    name: &str,
    generation: usize,
    already_shadowed: bool,
    exclude: Option<Node<'a>>,
    bytes: &[u8],
    visit: &mut impl FnMut(Node<'a>, usize),
) {
    let is_excluded = exclude.is_some_and(|excluded| excluded.id() == node.id());
    if !already_shadowed
        && !is_excluded
        && node.kind() == KIND_SIMPLE_IDENT
        && !is_named_argument_label(node)
        && node.utf8_text_owned(bytes).as_deref() == Some(name)
    {
        visit(node, generation);
    }
    if node.kind() == KIND_STATEMENTS {
        visit_statements_with_generations(node, name, generation, already_shadowed, bytes, visit);
        return;
    }
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else {
            continue;
        };
        let child_shadowed = already_shadowed
            || (scope_boundary_at(child).is_some()
                && declares_name_directly(child, name, bytes).is_some());
        visit_unshadowed_name_matches(
            child,
            name,
            generation,
            child_shadowed,
            exclude,
            bytes,
            visit,
        );
    }
}

/// Convert a tree-sitter node's byte-based position into an LSP `Location`
/// with UTF-16 columns. Assumes `node` is single-line (true for every
/// `simple_identifier` this module deals with).
fn node_to_location(uri: &Url, node: Node<'_>, full_text: &str) -> Option<Location> {
    let row = node.start_position().row;
    let start_byte_column = node.start_position().column;
    let end_byte_column = node.end_position().column;
    let line_text = full_text.lines().nth(row)?;
    let start_character =
        crate::features::text_utils::utf16_column(&line_text[..start_byte_column]);
    let end_character = crate::features::text_utils::utf16_column(&line_text[..end_byte_column]);
    Some(Location {
        uri: uri.clone(),
        range: tower_lsp::lsp_types::Range::new(
            Position::new(row as u32, start_character),
            Position::new(row as u32, end_character),
        ),
    })
}

#[cfg(test)]
#[path = "cst_symbol_tests.rs"]
mod tests;
