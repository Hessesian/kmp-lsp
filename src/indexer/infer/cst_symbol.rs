//! Shared CST identifier classification: declaration-vs-reference, and
//! receiver/member extraction from a `navigation_expression`.
//!
//! Originally written for semantic-token coloring (`semantic_tokens/resolve.rs`);
//! promoted here because `classify_symbol_at` (the navigation-feature
//! classifier: go-def, goto-impl, highlight) needs the identical walk —
//! two independent CST passes answering "declaration or reference?" and
//! "what's the receiver of this member access?" would drift from each other.

use tree_sitter::Node;

use crate::indexer::{CallShape, CstQuery, Indexer, NodeExt, Resolution};
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
    node.parent()
        .is_some_and(|parent| is_declaration_site_of(node, parent))
}

/// Like [`is_declaration_site`], but for callers that already hold the parent.
///
/// tree-sitter's `Node::parent()` is not a stored pointer — it re-finds the
/// node by descending from the root, so it costs O(depth) per call. A walk
/// that pushes children from a node it already has therefore knows the parent
/// for free, and asking tree-sitter for it again once per node is what turns
/// such a walk quadratic in nesting depth.
pub(crate) fn is_declaration_site_of(node: Node<'_>, parent: Node<'_>) -> bool {
    let parent_kind = parent.kind();
    if parent_kind == KIND_CLASS_DECL
        || parent_kind == KIND_OBJECT_DECL
        || parent_kind == KIND_COMPANION_OBJ
        || parent_kind == KIND_TYPE_ALIAS
    {
        return node.kind() == KIND_TYPE_IDENT;
    }
    if parent_kind == KIND_FUN_DECL
        || parent_kind == KIND_PARAMETER
        || parent_kind == KIND_ENUM_ENTRY
        || parent_kind == KIND_VAR_DECL
        || parent_kind == KIND_CLASS_PARAM
        || parent_kind == KIND_CATCH_BLOCK
    {
        return node.kind() == KIND_SIMPLE_IDENT;
    }
    if parent_kind == KIND_TYPE_PARAM {
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
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.is_named() && child.kind() != crate::queries::KIND_NAV_SUFFIX {
                return Some(child);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

pub(crate) fn navigation_member_ident(node: Node<'_>) -> Option<Node<'_>> {
    let suffix = node.first_child_of_kind(crate::queries::KIND_NAV_SUFFIX)?;
    let mut cursor = suffix.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.kind() == KIND_SIMPLE_IDENT || child.kind() == KIND_TYPE_IDENT {
                return Some(child);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

/// The enclosing `navigation_expression` when `node` is that expression's own
/// member identifier (e.g. `node` is `collect` in `triggers.collect(...)`),
/// else `None`.
///
/// `is_call_callee` (and `call_shape_of`) expect the *callee* node — for a
/// dot-qualified call the callee is the whole `nav_expr` (`triggers.collect`),
/// not the bare member identifier `collect`: `collect`'s own parent is a
/// `nav_suffix`, not the `call_expression`, so checking `is_call_callee` on
/// the identifier directly always misses qualified calls. Checking on this
/// function's result instead — as `classify_symbol_at` already does — is what
/// makes the check work identically for both `foo(...)` and `x.foo(...)`.
pub(crate) fn enclosing_nav_expr_if_member(node: Node<'_>) -> Option<Node<'_>> {
    let nav = node
        .parent()
        .and_then(|suffix| (suffix.kind() == KIND_NAV_SUFFIX).then_some(suffix))
        .and_then(|suffix| suffix.parent())?;
    (nav.kind() == KIND_NAV_EXPR
        && navigation_member_ident(nav).is_some_and(|m| m.id() == node.id()))
    .then_some(nav)
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
    /// `shape` is `Some` exactly when `is_call` is true — the call's own
    /// argument shape, used by `resolve_identity` to reject a same-named,
    /// wrong-arity candidate on the same receiver type (an explicit-receiver
    /// counterpart to `resolve_callee_definition`'s bare-call arity filter).
    Reference {
        receiver_type: Option<String>,
        is_call: bool,
        shape: Option<CallShape>,
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
    let is_import_segment = node.parent().is_some_and(|parent| {
        parent.kind() == KIND_IMPORT_HEADER
            || (parent.kind() == KIND_IDENTIFIER
                && parent
                    .parent()
                    .is_some_and(|grandparent| grandparent.kind() == KIND_IMPORT_HEADER))
    });
    if is_import_segment {
        return Some(SymbolAtCursor {
            name,
            role: SymbolRole::ImportSegment,
        });
    }

    // Member reference: the identifier is the member name of a nav_expr's suffix.
    if let Some(nav) = enclosing_nav_expr_if_member(node) {
        let call_expr = is_call_callee(nav).then(|| nav.parent()).flatten();
        let is_call = call_expr.is_some();
        let shape = call_expr.map(|expr| super::cst_lambda::call_shape_of(expr, &doc.bytes));
        // `expr_type` for a parameter/variable receiver echoes back its
        // syntactic type annotation verbatim (see `infer_ident_type` /
        // `find_var_type`) without checking that the annotated name is an
        // actual known type — `x: Unknown` resolves to `Some("Unknown")`
        // even though `Unknown` is declared nowhere. Gate on
        // `has_type_definition` so a made-up/unresolvable annotation
        // doesn't silently masquerade as a real receiver type (house
        // decoy: `untypeable_receiver_yields_no_receiver_type`).
        let receiver_type =
            navigation_receiver_node(nav).and_then(|receiver| {
                match CstQuery::new(receiver, doc, indexer, uri).expr_type() {
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
                shape,
            },
        });
    }

    // Bare reference (local var, top-level name, etc.) — no receiver, scope
    // resolution deferred (see Global Constraints). Callers fall through to
    // today's NameScan path for these.
    let bare_call_expr = node.parent().filter(|parent| {
        parent.kind() == KIND_CALL_EXPR
            && parent.child(0).map(|child| child.id()) == Some(node.id())
    });
    let is_call = bare_call_expr.is_some();
    let shape = bare_call_expr.map(|expr| super::cst_lambda::call_shape_of(expr, &doc.bytes));
    Some(SymbolAtCursor {
        name,
        role: SymbolRole::Reference {
            receiver_type: None,
            is_call,
            shape,
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

/// Resolve `symbol`'s identity to its definition site(s).
///
/// `CstResolved` when the CST gave enough information to trust the result
/// (a declaration is trivially its own definition; a receiver-typed member
/// reference is looked up ON that type). `NameScan` for everything the CST
/// couldn't narrow — an untyped receiver, or a bare reference resolved by
/// today's name-based `find_definition_qualified(name, None, uri)` (which
/// can span multiple same-named workspace symbols).
pub(crate) fn resolve_identity(
    symbol: &SymbolAtCursor,
    indexer: &Indexer,
    uri: &Url,
) -> NavigationSource<Definitions> {
    match &symbol.role {
        SymbolRole::Declaration { indexed } => {
            let locations = Definitions(indexer.find_definition_qualified(&symbol.name, None, uri));
            // Only declarations `KOTLIN_DEFINITIONS` actually indexes can be
            // trusted CST-resolved; an unindexed one (bare param, val/var-less
            // constructor param, type param) falls through to an unanchored
            // same-file scan or workspace-wide scan — label it NameScan (see
            // `is_indexed_declaration_site`).
            if *indexed {
                NavigationSource::CstResolved(locations)
            } else {
                NavigationSource::NameScan(locations)
            }
        }
        SymbolRole::Reference {
            receiver_type: Some(receiver_type),
            shape,
            ..
        } => {
            let mut locations =
                indexer.find_definition_qualified(&symbol.name, Some(receiver_type), uri);
            // A call's own shape rules out a same-named, wrong-arity member/
            // extension on the same receiver type — e.g. `triggers.collect {
            // trigger -> }` (1 arg via trailing lambda) must not resolve to a
            // same-file `Flow.collect(scope, block)` self-declaration just
            // because both are in scope on `Flow`. Filtering to empty here
            // (rather than keeping the wrong-arity candidate) demotes this to
            // `NameScan`, so `find_definition`'s later, arity-blind but
            // receiver-aware fallback (variable-type inference + hierarchy
            // walk, which never consults the extension-in-scope registry that
            // caused the wrong match) gets a chance to find the real target.
            if let Some(shape) = shape {
                retain_call_shape_compatible(indexer, *shape, &mut locations);
            }
            if locations.is_empty() {
                NavigationSource::NameScan(Definitions(locations))
            } else {
                NavigationSource::CstResolved(Definitions(locations))
            }
        }
        SymbolRole::Reference {
            receiver_type: None,
            ..
        }
        | SymbolRole::ImportSegment => NavigationSource::NameScan(Definitions(
            indexer.find_definition_qualified(&symbol.name, None, uri),
        )),
    }
}

/// Drop any `Location` whose own declared arity `shape` can't satisfy — each
/// `Location` is looked up by an exact `selection_range` match against the
/// declaring file's own symbol table (how `resolve_qualified`'s candidates
/// are always constructed), so a location that doesn't match any symbol, or
/// whose file isn't indexed, is kept unfiltered (fail open: never lose a
/// candidate this can't actually verify). Vararg declarations are exempt,
/// same reasoning as `local_symbol_satisfies_call_shape`: `param_counts`
/// can't represent a vararg's true unbounded upper end.
///
/// `pub(crate)`: also used directly by `find_definition`'s and
/// `compute_hover`'s `ctx.contextual`-based branches — `CursorContext::build`
/// populates `contextual` for *any* qualified reference via smart-cast
/// narrowing (`infer_receiver_type_at`), not just `it`/`this`/named lambda
/// params, so that path needs the identical arity filter this module's own
/// CST-resolved path does, to avoid resurrecting the same self-shadow bug.
pub(crate) fn retain_call_shape_compatible(
    indexer: &Indexer,
    shape: CallShape,
    locations: &mut Vec<Location>,
) {
    locations.retain(|location| {
        let Some(fd) = indexer
            .files
            .get(location.uri.as_str())
            .or_else(|| indexer.jar_files.get(location.uri.as_str()))
        else {
            return true;
        };
        let Some(symbol) = fd
            .symbols
            .iter()
            .find(|s| s.selection_range == location.range)
        else {
            return true;
        };
        if symbol.params.contains("vararg ") || symbol.params.contains("vararg\t") {
            return true;
        }
        shape.accepts(symbol.param_counts.0, symbol.param_counts.1)
    });
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
    let outcome = visit_unshadowed_name_matches(
        body,
        &name,
        0,
        false,
        None,
        &doc.bytes,
        &mut |node, generation| occurrences.push(Occurrence { node, generation }),
        0,
    );
    if outcome == ScopeWalk::StoppedAtDepthCap {
        // Renaming the subset we did see would corrupt the file.
        return None;
    }

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
    // Carry each node's parent alongside it: the walk already knows it, and
    // re-deriving it per node via `Node::parent()` costs O(depth) each time —
    // see [`is_declaration_site_of`].
    let mut stack = vec![(scope, scope.parent())];
    while let Some((node, parent)) = stack.pop() {
        if parent.is_some_and(|parent| is_declaration_site_of(node, parent))
            && node.utf8_text_owned(bytes).as_deref() == Some(name)
        {
            return Some(node);
        }
        if node.id() != scope.id() && scope_boundary_at(node).is_some() {
            continue;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push((child, Some(node)));
        }
    }
    None
}

/// Whether a scope walk saw every occurrence of the name, or stopped early at
/// [`crate::util::MAX_CST_DESCENT_DEPTH`].
///
/// Every other capped walker in this codebase under-reports a diagnostic when
/// it bails, which degrades gracefully. This one feeds rename, where applying
/// to some occurrences of a name and not others corrupts the file — so the
/// distinction is a type the caller must match on, not a bool it can ignore.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeWalk {
    SawEveryOccurrence,
    StoppedAtDepthCap,
}

/// Walk `statements`'s children in document order, incrementing `generation`
/// after each one that declares `name`. A declaring statement's own
/// initializer is visited at the OLD generation, then re-emitted at the new
/// one — `val x: Any = "hello"; val x = x as String`'s own initializer
/// reference must see the FIRST `x`, since the second doesn't exist yet
/// while its own initializer runs.
///
/// Stops and reports [`ScopeWalk::StoppedAtDepthCap`] the moment any nested
/// walk does — see [`visit_unshadowed_name_matches`].
#[must_use]
fn visit_statements_with_generations<'a>(
    statements: Node<'a>,
    name: &str,
    mut generation: usize,
    already_shadowed: bool,
    bytes: &[u8],
    visit: &mut impl FnMut(Node<'a>, usize),
    depth: usize,
) -> ScopeWalk {
    let mut cursor = statements.walk();
    for statement in statements.children(&mut cursor) {
        let outcome = match declares_name_directly(statement, name, bytes) {
            Some(declaration_node) => {
                let outcome = visit_unshadowed_name_matches(
                    statement,
                    name,
                    generation,
                    already_shadowed,
                    Some(declaration_node),
                    bytes,
                    visit,
                    depth + 1,
                );
                generation += 1;
                if !already_shadowed {
                    visit(declaration_node, generation);
                }
                outcome
            }
            None => visit_unshadowed_name_matches(
                statement,
                name,
                generation,
                already_shadowed,
                None,
                bytes,
                visit,
                depth + 1,
            ),
        };
        if outcome == ScopeWalk::StoppedAtDepthCap {
            return outcome;
        }
    }
    ScopeWalk::SawEveryOccurrence
}

/// Walk `node`'s subtree, calling `visit` on every `simple_identifier`
/// matching `name` (tagged with `generation`), skipping the subtree of any
/// nested scope boundary that itself redeclares `name` (shadowing).
/// `exclude` suppresses one node — a declaration re-emitted separately by
/// [`visit_statements_with_generations`] at its own new generation.
///
/// On [`ScopeWalk::StoppedAtDepthCap`] the occurrences collected so far are
/// incomplete; [`local_scope_occurrences`] gives up its fast path rather than
/// renaming the subset it did find.
#[must_use]
#[allow(clippy::too_many_arguments)]
fn visit_unshadowed_name_matches<'a>(
    node: Node<'a>,
    name: &str,
    generation: usize,
    already_shadowed: bool,
    exclude: Option<Node<'a>>,
    bytes: &[u8],
    visit: &mut impl FnMut(Node<'a>, usize),
    depth: usize,
) -> ScopeWalk {
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("visit_unshadowed_name_matches", node);
        return ScopeWalk::StoppedAtDepthCap;
    }
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
        return visit_statements_with_generations(
            node,
            name,
            generation,
            already_shadowed,
            bytes,
            visit,
            depth + 1,
        );
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let child_shadowed = already_shadowed
            || (scope_boundary_at(child).is_some()
                && declares_name_directly(child, name, bytes).is_some());
        if visit_unshadowed_name_matches(
            child,
            name,
            generation,
            child_shadowed,
            exclude,
            bytes,
            visit,
            depth + 1,
        ) == ScopeWalk::StoppedAtDepthCap
        {
            return ScopeWalk::StoppedAtDepthCap;
        }
    }
    ScopeWalk::SawEveryOccurrence
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
