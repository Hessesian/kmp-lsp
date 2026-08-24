use tower_lsp::lsp_types::{Location, Position, SymbolKind, Url};

use crate::indexer::{Indexer, InferDeps, NodeExt};
use crate::types::FileData;
use crate::LinesExt;
use crate::StrExt;

use super::ensure_file_data;
use super::infer_lines::{
    extract_property_type_from_detail, extract_return_type_from_detail, find_rhs_str,
    has_dot_after_first_call,
};
use super::{walk_hierarchy, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK};

// ─── Type-string helpers ──────────────────────────────────────────────────────

/// Strip generic parameters and nullability markers from a type string.
///
/// `"List<Product>"` → `"List"`, `"String?"` → `"String"`, `"Outer.Inner<T>"` → `"Outer.Inner"`
///
/// Mirrors the stripping done by [`infer_type_in_lines`](super::infer_lines::infer_type_in_lines)
/// so that `type_annotations` lookups return the same shape as line-scan results.
fn strip_generics(type_str: &str) -> String {
    let stripped: String = type_str
        .chars()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '.')
        .collect();
    stripped.trim_end_matches('.').to_owned()
}

// ─── Receiver type resolution ─────────────────────────────────────────────────

/// How the receiver expression should be resolved.
///
/// - `Variable`: a named val/var (e.g. `interactor`, `viewModel`).
///   Resolved via line-scan type annotation (`val name: Type`).
/// - `Contextual`: `it`, `this`, or a named lambda parameter.
///   Requires cursor `position` for scope analysis; falls back to
///   `infer_variable_type_raw` only if scope analysis returns nothing.
pub(crate) enum ReceiverKind<'a> {
    Variable(&'a str),
    Contextual { name: &'a str, position: Position },
}

/// A fully-normalised receiver type with multiple access forms.
///
/// All forms are derived from a single raw string (e.g. `"Outer.Inner<Param>"`):
/// - `raw`       — original with generics: `"Outer.Inner<Param>"`
/// - `qualified` — no generics, dots preserved: `"Outer.Inner"`
/// - `outer`     — first dot-segment: `"Outer"`  (used for file lookup)
/// - `leaf`      — last dot-segment: `"Inner"`   (used for fallback member lookup)
#[derive(Clone)]
pub(crate) struct ReceiverType {
    /// Full raw type string as inferred, e.g. `"StateFlow<UiState>?"`.
    pub raw: String,
    /// Type name with no generics and no `?`, e.g. `"StateFlow"` or `"Outer.Inner"`.
    pub qualified: String,
    /// Outermost segment of `qualified`, e.g. `"Outer"`.
    pub outer: String,
    /// Innermost segment of `qualified`, e.g. `"Inner"`.
    pub leaf: String,
    /// Whether the type was annotated as nullable (`?`), e.g. `val x: User?`.
    /// Used by the nullable-dot-call diagnostic; lookup sites use `qualified`.
    pub nullable: bool,
}

impl ReceiverType {
    pub(crate) fn from_raw(raw: String) -> Self {
        // Strip generics and outer `?` — stop at first `<` or `?`.
        let qualified: String = raw.chars().take_while(|&c| c != '<' && c != '?').collect();
        // Only a *trailing* `?` makes the outer type nullable. A `?` inside a
        // generic argument (`Box<String?>`) does not — so test the end, not the
        // whole string.
        let nullable = raw.is_nullable();
        let outer = qualified
            .split('.')
            .next()
            .unwrap_or(&qualified)
            .to_string();
        let leaf = qualified
            .rsplit('.')
            .next()
            .unwrap_or(&qualified)
            .to_string();
        ReceiverType {
            raw,
            qualified,
            outer,
            leaf,
            nullable,
        }
    }
}

/// Infer the type of a receiver expression and normalise it into a
/// [`ReceiverType`].
///
/// Returns `None` when type inference fails (no annotation, unindexed file,
/// or lambda scope not resolvable).  Call sites then decide whether to skip
/// or fall back; this function never performs a global rg scan.
pub(crate) fn infer_receiver_type(
    indexer: &Indexer,
    kind: ReceiverKind<'_>,
    uri: &Url,
) -> Option<ReceiverType> {
    let raw = match kind {
        ReceiverKind::Variable(name) => match infer_variable_type_raw(indexer, name, uri) {
            Some(raw) => raw,
            // CST fallback for initializers the line heuristics miss (e.g.
            // `val x = remember { Foo() }` → `Foo`).
            None => infer_variable_type_from_cst(indexer, name, uri)?,
        },
        ReceiverKind::Contextual { name, position } => {
            // Lambda / implicit-receiver path.
            if let Some(type_str) = indexer.infer_lambda_param_type_at(name, uri, position) {
                type_str
            } else {
                // Contextual fallback: ordinary annotated var that happens to
                // appear in a lambda context (e.g. captured val with explicit type).
                infer_variable_type_raw(indexer, name, uri)?
            }
        }
    };
    Some(ReceiverType::from_raw(raw))
}

/// Infer the type of a pure field-access chain such as `holder.repo` (a root
/// variable followed by one or more field names), preserving the leaf field's
/// trailing `?` so the caller can observe nullability.
///
/// `["holder", "repo"]` where `holder: Holder` and
/// `data class Holder(val repo: Repository?)` →
/// `ReceiverType { qualified: "Repository", nullable: true, .. }`.
///
/// Returns `None` if the chain has no field segment (`segments.len() < 2`) or
/// any segment's type can't be resolved. Used by the nullable-dot-call
/// diagnostic to flag `holder.repo.load()` where `repo` is a nullable field,
/// and by the `when`-exhaustiveness diagnostic for a chained subject like
/// `event.event`.
///
/// `line` is the 0-based line of the chain expression itself, used only as a
/// fallback (see below). `cst_point`, when given, is the chain expression's
/// own CST node plus source bytes — Kotlin smart-casts a whole *stable path*,
/// not just a simple variable (`when (event.events) { is X -> when
/// (event.events) { ... } }` narrows the entire path `event.events`, not just
/// `event`), so with a CST node this finds whatever prefix of `segments` an
/// enclosing `when` narrows via [`enclosing_smart_cast_type`] and starts the
/// field walk from there (an exact whole-chain match walks zero further
/// fields). Without a CST node — or when no prefix matches — falls back to
/// the much narrower line-scanning `smart_cast_narrowed_type` (root only) and
/// then the plain declared type.
///
/// The returned `Url` is the file that declares the *leaf* type — the correct
/// reachability context for resolving anything about that type further (e.g.
/// its own members via `resolve::resolve_type_index_only`). A caller that
/// keeps using its own `uri` for that instead reintroduces exactly the bug
/// this function exists to avoid: the leaf type can be a class duplicated
/// workspace-wide, reachable only through the chain (never imported by the
/// original caller directly), so only the leaf's own declaring file's
/// imports/package can disambiguate it.
pub(crate) fn infer_field_chain_type(
    indexer: &Indexer,
    segments: &[String],
    uri: &Url,
    line: u32,
    cst_point: Option<(tree_sitter::Node, &[u8])>,
) -> Option<(ReceiverType, Url)> {
    if segments.len() < 2 {
        return None;
    }
    // Find the prefix of `segments` that's smart-cast-narrowed (if any), and
    // the type it narrows to — the un-narrowed declared type would send the
    // field lookup below down the wrong, far more collision-prone bare-name
    // class (see `smart_cast_narrowed_type` doc comment).
    let (narrowed_prefix_len, mut current) = 'narrowed: {
        if let Some((point, source)) = cst_point {
            if let Some((narrowed_type, prefix_len)) =
                enclosing_smart_cast_type(point, segments, source)
            {
                break 'narrowed (prefix_len, narrowed_type);
            }
        }
        // No prefix is smart-cast-narrowed — fall back to the root's plain
        // declared type. With a CST node, prefer the scope-correct
        // `resolve_declared_type_from_cst` (a whole-file scan can find an
        // unrelated same-named parameter/local in a *different* function —
        // see its own doc comment) before the line-scanning smart-cast check
        // and the unscoped whole-file scan.
        let root = segments.first()?;
        let declared_type = match cst_point {
            Some((point, source)) => resolve_declared_type_from_cst(point, root, source)
                .or_else(|| smart_cast_narrowed_type(indexer, root, uri, line))
                .or_else(|| infer_variable_type(indexer, root, uri))?,
            None => smart_cast_narrowed_type(indexer, root, uri, line)
                .or_else(|| infer_variable_type(indexer, root, uri))?,
        };
        (1, declared_type)
    };
    // The file whose imports/package govern the *next* lookup's reachability.
    // Starts as the caller's own file (correct for the root: its declared type
    // is reachable from wherever it's declared/used) and is updated to each
    // resolved class's own declaring file as the chain descends — a field's
    // type must resolve through the *declaring class's* imports, not the
    // original caller's (see `find_field_type_in_class`). A *qualified*
    // narrowed type (`Event.OverdraftInput`) carries its own reachability
    // signal, resolved here up front.
    let mut reachability_uri = declaring_uri_for_type(indexer, &current, uri);
    let mut leaf_raw = current.clone();
    for field in &segments[narrowed_prefix_len..] {
        // Reduce the running type to a bare class name for the field lookup:
        // drop generics, any package/outer qualifier, and a trailing `?`.
        let class_base = current
            .split('<')
            .next()
            .unwrap_or(&current)
            .rsplit('.')
            .next()
            .unwrap_or(&current)
            .strip_nullable();
        let (field_raw, declaring_uri) =
            find_field_type_in_class(indexer, class_base, field, &reachability_uri)?;
        current = field_raw.clone();
        leaf_raw = field_raw;
        reachability_uri = declaring_uri;
    }
    Some((ReceiverType::from_raw(leaf_raw), reachability_uri))
}

/// Like [`infer_receiver_type`] but checks smart-cast narrowing at the given
/// position first.  If the variable is inside a `when (var)` branch or an
/// `if (var is Type)` block, returns the narrowed type.
pub(crate) fn infer_receiver_type_at(
    indexer: &Indexer,
    name: &str,
    uri: &Url,
    position: Position,
) -> Option<ReceiverType> {
    if let Some(narrowed) = smart_cast_narrowed_type(indexer, name, uri, position.line) {
        return Some(ReceiverType::from_raw(narrowed));
    }
    // Fallback to normal inference
    infer_receiver_type(indexer, ReceiverKind::Variable(name), uri)
}

/// If `name` is the subject of an enclosing `when (name) { is Type -> ... }`
/// branch or `if (name is Type)` block at `line`, returns the narrowed type —
/// the *static* declared type of `name` (e.g. a sealed interface) is wrong
/// inside such a branch, and callers that skip this and use the declared type
/// instead risk a far more collision-prone bare-name class lookup downstream
/// (a common sealed-interface name like `Event` can match dozens of unrelated
/// classes workspace-wide; the narrowed subtype name rarely does).
fn smart_cast_narrowed_type(indexer: &Indexer, name: &str, uri: &Url, line: u32) -> Option<String> {
    use super::infer_lines::SmartCast;

    let lines = indexer
        .live_lines
        .get(uri.as_str())
        .map(|ll| (*ll).clone())
        .or_else(|| indexer.files.get(uri.as_str()).map(|d| d.lines.clone()))?;
    match super::infer_lines::smart_cast_type_at_line(&lines, name, line)? {
        SmartCast::TypeTest(type_name) => Some(type_name),
        // Only an object's own name is also a type; an enum entry or a
        // constant matches by value and leaves the subject's type alone.
        SmartCast::ObjectEquality(label) => {
            names_object_declaration(indexer, &label, uri).then_some(label)
        }
    }
}

/// Real Kotlin smart-cast subjects (`event`, `event.events`) are never more
/// than a handful of segments, so bailing out for anything longer is free —
/// a defensive bound on [`enclosing_smart_cast_type`]'s input, independent of
/// the tree-depth cost documented on its own `Node::parent()` warning: a
/// long segment count does *not* imply `point` is deeply nested (for a
/// left-recursive chain `a.b0.b1…`, it's the *opposite* — the longest-chain
/// node is the outermost, shallowest one), so this bound alone would not
/// have protected the pathological-chain case that motivated it. That case
/// was fixed at the call site instead (`nullable_call_diagnostics` stopped
/// passing a CST point) — see `enclosing_smart_cast_type`'s doc comment.
const MAX_SMART_CAST_CHAIN_LEN: usize = 16;

/// CST-based counterpart to [`smart_cast_narrowed_type`]: if `point` sits
/// inside an immediately-enclosing `when (subject) { is Type -> ... }` branch
/// whose subject is exactly `target_segments` (a bare variable `["event"]` or
/// a stable dotted path `["event", "events"]` — Kotlin smart-casts a whole
/// property path, not just a simple variable), returns `Type` in full (e.g.
/// `"Event.OverdraftInput"` from `is Event.OverdraftInput ->`) — the
/// qualifier matters: it lets `resolve_type_index_only` resolve `Event`
/// reachability-first, then find `OverdraftInput` scoped to *that* file,
/// rather than a flat bare-name scan for `OverdraftInput` alone (which can't
/// disambiguate if the enclosing `Event` itself is duplicated under two
/// same-named outer types in different packages, e.g. two contracts each
/// declaring their own `Event`).
///
/// Unlike the line-scanning heuristic in `infer_lines::smart_cast_type_at_line`
/// — which recovers `when`-branch nesting from indented text and can be fooled
/// by a *sibling* branch that also happens to contain its own nested
/// `when (...)` sharing the same subject — this walks real tree-sitter parent
/// pointers, which only ever reach `point`'s true ancestors. Callers with a
/// CST node in hand should prefer this over `smart_cast_narrowed_type`.
///
/// Handles only a `when (subject) { is Type -> ... }` type test — not
/// `if (subject is Type)` or object-equality branches — matching the shape
/// every current caller needs; extend if a caller needs those too.
///
/// Walks *multiple* enclosing `when` levels outward, not just the immediate
/// one: a `when`'s own subject can be a *different, unrelated* path than
/// `target_segments` (e.g. an ancestor `when (foo)` while querying
/// `["event", "events"]`), in which case that level doesn't narrow
/// `target_segments` at all and the search must continue past it to whichever
/// ancestor `when` actually has (a prefix of) `target_segments` as its
/// subject (real shape this guards: `is Banner -> when (event.events) { is X
/// -> when (event.events) { ... } }` — the innermost level's own subject
/// `event.events` already matches the whole 2-segment target directly; a
/// query for just `["event"]` from a *different* nested spot would instead
/// need to see through an `event.events`-subject level in between to reach an
/// outer `when (event)`'s narrowing).
///
/// Each ancestor level is checked exactly once — a level's subject is
/// whatever length it naturally is, so it can only ever match the
/// correspondingly-sized prefix of `target_segments`; there's no need to
/// separately try shorter prefixes at the same level (an earlier version of
/// this function did, making the walk `O(depth × segments.len())` per call).
///
/// **Costly per call from a deep node — `Node::parent()` is not a stored
/// pointer.** Unlike a bounded-recursion walk (`pure_field_chain_at`,
/// `collect_navigation_segments`), tree-sitter's `Node::parent()` is
/// `O(depth from root)` internally, every call, regardless of how many hops
/// the caller intends to make (see `Node::parent` in tree-sitter's
/// `node.c`). One call from a node 5,000 levels deep already costs
/// `O(5000)`; a caller that calls this once per node while walking a whole
/// deep tree pays that cost once per node, making the *whole walk*
/// quadratic. `MAX_SMART_CAST_CHAIN_LEN` bounds `target_segments.len()` as a
/// cheap sanity check, but does **not** by itself bound `point`'s depth — a
/// short segment count doesn't imply a shallow node (see that constant's own
/// doc comment). **Only call this from a position that's known to be
/// shallow** (e.g. `fill_when`'s one call per `when` node — a `when`
/// expression itself is never deeply nested even when its subject chain is);
/// a caller that might invoke this once per node of a deep chain must not
/// pass a CST point at all (`nullable_call_diagnostics` doesn't, precisely
/// for this reason) — see `nullable_diagnostics_survives_a_pathologically_deep_field_chain`,
/// the regression test that caught this the first time it was tried.
///
/// Returns the narrowed type together with how many leading segments of
/// `target_segments` it covers, so the caller knows how many (if any) remain
/// to walk as plain fields.
pub(crate) fn enclosing_smart_cast_type(
    point: tree_sitter::Node,
    target_segments: &[String],
    source: &[u8],
) -> Option<(String, usize)> {
    use crate::queries::{
        KIND_TYPE_IDENT, KIND_TYPE_TEST, KIND_USER_TYPE, KIND_WHEN_CONDITION, KIND_WHEN_ENTRY,
        KIND_WHEN_EXPR, KIND_WHEN_SUBJECT,
    };

    if target_segments.len() > MAX_SMART_CAST_CHAIN_LEN {
        return None;
    }

    let mut current = point;
    for _ in 0..crate::util::MAX_CST_DESCENT_DEPTH {
        let when_entry = ancestor_of_kind(current, KIND_WHEN_ENTRY)?;
        let owning_when = ancestor_of_kind(when_entry, KIND_WHEN_EXPR)?;
        let subject = owning_when
            .children(&mut owning_when.walk())
            .find(|child| child.kind() == KIND_WHEN_SUBJECT)?;

        let consumed = subject_segments(subject, source).filter(|subject_segments| {
            !subject_segments.is_empty()
                && subject_segments.len() <= target_segments.len()
                && subject_segments.as_slice() == &target_segments[..subject_segments.len()]
        });
        if let Some(consumed) = consumed {
            let condition = when_entry
                .children(&mut when_entry.walk())
                .find(|child| child.kind() == KIND_WHEN_CONDITION)?;
            let type_test = condition
                .children(&mut condition.walk())
                .find(|child| child.kind() == KIND_TYPE_TEST)?;
            let user_type = type_test.first_child_of_kind(KIND_USER_TYPE)?;
            let identifiers: Vec<&str> = user_type
                .children(&mut user_type.walk())
                .filter(|child| child.kind() == KIND_TYPE_IDENT)
                .map(|child| child.utf8_text(source))
                .collect::<Result<_, _>>()
                .ok()?;
            if identifiers.is_empty() {
                return None;
            }
            return Some((identifiers.join("."), consumed.len()));
        }

        // This level's subject isn't a prefix of `target_segments` (a
        // different variable, or a different path) — it doesn't narrow it;
        // keep looking further out.
        current = owning_when;
    }
    crate::util::report_cst_depth_exceeded!("enclosing_smart_cast_type", point);
    None
}

/// The identifier chain of a `when_subject` (or any node wrapping a plain
/// identifier / dotted navigation expression): `state` → `["state"]`,
/// `event.events` → `["event", "events"]`. `None` for anything else (a call,
/// an index, a literal) — those need real expression inference, not a name
/// chain. Depth-bounded like [`enclosing_smart_cast_type`]'s own walk.
///
/// The one shared implementation — `fill_when::analyze_when` uses this
/// directly for its own subject-segment extraction rather than re-deriving
/// it, so there's a single CST chain-flattening walk instead of two.
pub(crate) fn subject_segments(node: tree_sitter::Node, source: &[u8]) -> Option<Vec<String>> {
    use crate::queries::{KIND_NAV_EXPR, KIND_NAV_SUFFIX, KIND_SIMPLE_IDENT};

    fn collect(
        node: tree_sitter::Node,
        source: &[u8],
        out: &mut Vec<String>,
        depth: usize,
    ) -> Option<()> {
        if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
            crate::util::report_cst_depth_exceeded!("subject_segments", node);
            return None;
        }
        match node.kind() {
            KIND_SIMPLE_IDENT => out.push(node.utf8_text(source).ok()?.to_owned()),
            KIND_NAV_EXPR | KIND_NAV_SUFFIX => {
                for child in node.children(&mut node.walk()) {
                    // The `.` separator carries no segment of its own.
                    if child.kind() != "." {
                        collect(child, source, out, depth + 1)?;
                    }
                }
            }
            _ => return None,
        }
        Some(())
    }

    // A bare identifier/nav-expression node (as passed by a caller that
    // already resolved down to the expression itself) needs no unwrapping —
    // check it before its children. Only `when_subject` wraps its expression
    // in `"(" <expression> ")"`, so its own kind never matches here and the
    // loop below finds the wrapped expression among its children instead.
    // Checking the node first (not last) matters: a `navigation_expression`
    // node's *own* children include its receiver sub-expression, which can
    // itself be `KIND_SIMPLE_IDENT`/`KIND_NAV_EXPR` — finding that child
    // first would silently return just the receiver's segments, a prefix of
    // the real chain, instead of the whole thing.
    for candidate in std::iter::once(node).chain(node.children(&mut node.walk())) {
        if matches!(candidate.kind(), KIND_SIMPLE_IDENT | KIND_NAV_EXPR) {
            let mut segments = Vec::new();
            collect(candidate, source, &mut segments, 0)?;
            return Some(segments);
        }
    }
    None
}

/// Walk `node`'s ancestors (not itself) for the nearest one of `kind`.
fn ancestor_of_kind<'tree>(
    node: tree_sitter::Node<'tree>,
    kind: &str,
) -> Option<tree_sitter::Node<'tree>> {
    let mut current = node.parent();
    while let Some(candidate) = current {
        if candidate.kind() == kind {
            return Some(candidate);
        }
        current = candidate.parent();
    }
    None
}

/// Resolve `var_name`'s *declared* type by walking up from `point` through the
/// CST — sibling `val`/`var` declarations in the same statement block, then
/// the enclosing function's parameters, then the enclosing class's primary
/// constructor parameters — stopping at the first match.
///
/// This is scope-correct where a whole-file text/line scan (`infer_variable_type`)
/// is not: a file with many functions can have several unrelated parameters or
/// locals named the same thing (`event`, `state`, …), and a whole-file scan has
/// no way to prefer the one actually in scope at `point`. Callers with a CST
/// node in hand should try this first and fall back to `infer_variable_type`
/// only when it finds nothing (e.g. `var_name` comes from an outer/captured
/// scope this walk doesn't reach).
pub(crate) fn resolve_declared_type_from_cst(
    point: tree_sitter::Node,
    var_name: &str,
    source: &[u8],
) -> Option<String> {
    use crate::queries::{
        KIND_BOOLEAN_LITERAL, KIND_CLASS_DECL, KIND_CLASS_PARAM, KIND_FUN_DECL,
        KIND_FUN_VALUE_PARAMS, KIND_NULLABLE_TYPE, KIND_PARAMETER, KIND_PRIMARY_CTOR,
        KIND_PROP_DECL, KIND_SIMPLE_IDENT, KIND_STATEMENTS, KIND_TYPE_IDENT, KIND_USER_TYPE,
        KIND_VAR_DECL,
    };

    fn full_type_name(user_type: tree_sitter::Node, source: &[u8]) -> Option<String> {
        let parts: Vec<&str> = user_type
            .children(&mut user_type.walk())
            .filter(|child| child.kind() == KIND_TYPE_IDENT)
            .map(|child| child.utf8_text(source))
            .collect::<Result<_, _>>()
            .ok()?;
        (!parts.is_empty()).then(|| parts.join("."))
    }

    fn type_from_nullable(nullable: tree_sitter::Node, source: &[u8]) -> Option<String> {
        nullable
            .first_child_of_kind(KIND_USER_TYPE)
            .and_then(|user_type| full_type_name(user_type, source))
    }

    // Shared by a `parameter`/`class_parameter` node (`name: Type`) and a
    // `variable_declaration` node (same shape, plus an inferred-Boolean case
    // for `val x = false`/`true` handled by the caller).
    fn type_after_matching_name(
        node: tree_sitter::Node,
        var_name: &str,
        source: &[u8],
    ) -> Option<String> {
        let mut name_matched = false;
        for child in node.children(&mut node.walk()) {
            if child.kind() == KIND_SIMPLE_IDENT && child.utf8_text(source).ok() == Some(var_name) {
                name_matched = true;
            }
            if name_matched {
                if child.kind() == KIND_USER_TYPE {
                    return full_type_name(child, source);
                }
                if child.kind() == KIND_NULLABLE_TYPE {
                    return type_from_nullable(child, source);
                }
            }
        }
        None
    }

    fn find_in_sibling_declarations(
        statements: tree_sitter::Node,
        var_name: &str,
        source: &[u8],
    ) -> Option<String> {
        statements
            .children(&mut statements.walk())
            .filter(|child| child.kind() == KIND_PROP_DECL)
            .find_map(|prop| {
                let var_decl = prop.first_child_of_kind(KIND_VAR_DECL)?;
                type_after_matching_name(var_decl, var_name, source).or_else(|| {
                    // `val x = false`/`true` — no annotation, inferred Boolean.
                    let name_matches = var_decl
                        .first_child_of_kind(KIND_SIMPLE_IDENT)
                        .and_then(|ident| ident.utf8_text(source).ok())
                        == Some(var_name);
                    let has_boolean_literal = prop
                        .children(&mut prop.walk())
                        .any(|child| child.kind() == KIND_BOOLEAN_LITERAL);
                    (name_matches && has_boolean_literal).then(|| "Boolean".to_owned())
                })
            })
    }

    fn find_in_parameters(
        function_declaration: tree_sitter::Node,
        var_name: &str,
        source: &[u8],
    ) -> Option<String> {
        let params = function_declaration.first_child_of_kind(KIND_FUN_VALUE_PARAMS)?;
        params
            .children(&mut params.walk())
            .filter(|child| child.kind() == KIND_PARAMETER)
            .find_map(|param| type_after_matching_name(param, var_name, source))
    }

    fn find_in_constructor(
        class_declaration: tree_sitter::Node,
        var_name: &str,
        source: &[u8],
    ) -> Option<String> {
        let primary_constructor = class_declaration.first_child_of_kind(KIND_PRIMARY_CTOR)?;
        primary_constructor
            .children(&mut primary_constructor.walk())
            .filter(|child| child.kind() == KIND_CLASS_PARAM)
            .find_map(|param| type_after_matching_name(param, var_name, source))
    }

    let mut current = point.parent();
    while let Some(node) = current {
        let found = match node.kind() {
            KIND_STATEMENTS => find_in_sibling_declarations(node, var_name, source),
            KIND_FUN_DECL => find_in_parameters(node, var_name, source),
            KIND_CLASS_DECL => find_in_constructor(node, var_name, source),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        current = node.parent();
    }
    None
}

/// Whether the qualified `label` (e.g. `Ui.Loading`) names an `object`
/// declaration as seen from `from_uri`.
///
/// Resolved through `resolve_type_index_only`, which honours the file's own
/// imports and package, rather than by scanning every definition sharing the
/// simple name: an unrelated `object Idle` in another package would otherwise
/// validate an enum entry named `Idle` and narrow the subject to a type it
/// has nothing to do with.
fn names_object_declaration(indexer: &Indexer, label: &str, from_uri: &Url) -> bool {
    super::resolve::resolve_type_index_only(indexer, label, from_uri)
        .into_iter()
        .any(|location| {
            ensure_file_data(indexer, &location.uri).is_some_and(|file_data| {
                file_data.symbols.iter().any(|symbol| {
                    symbol.selection_range == location.range && symbol.kind == SymbolKind::OBJECT
                })
            })
        })
}

/// Shared recursion budget for `infer_variable_type`/`infer_variable_type_raw`
/// and `find_field_type_in_class`: `infer_var_from_rhs_data`'s `field_match`
/// branch re-enters `find_field_type_in_class`, whose own fallback re-enters
/// variable-type inference — each side resetting to a fresh budget on
/// re-entry, rather than decrementing one shared counter, let a real
/// unannotated-field-chain-heavy file overflow the stack (594 real
/// mutually-recursive frames, no synthetic pathological input needed).
const MAX_RAW_TYPE_INFER_DEPTH: u8 = 4;

/// Scan the current file's lines for a type annotation on `var_name` and return
/// the declared type name if found.  Delegates to [`infer_type_in_lines`] and
/// falls back to method return-type inference for `val x = receiver.method(...)`.
pub(crate) fn infer_variable_type(indexer: &Indexer, var_name: &str, uri: &Url) -> Option<String> {
    infer_variable_type_impl(indexer, var_name, uri, MAX_RAW_TYPE_INFER_DEPTH)
}

/// Like [`infer_variable_type`] but preserves generic parameters in the returned
/// type string.  e.g. `val items: List<Product>` → `"List<Product>"`.
///
/// Used by the `it`-completion path to extract the collection element type.
pub(crate) fn infer_variable_type_raw(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
) -> Option<String> {
    infer_variable_type_raw_impl(indexer, var_name, uri, MAX_RAW_TYPE_INFER_DEPTH)
}

fn infer_variable_type_impl(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
    depth: u8,
) -> Option<String> {
    infer_variable_type_core(indexer, var_name, uri, depth, false)
}

fn infer_variable_type_raw_impl(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
    depth: u8,
) -> Option<String> {
    infer_variable_type_core(indexer, var_name, uri, depth, true)
}

fn infer_variable_type_core(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
    depth: u8,
    keep_generics: bool,
) -> Option<String> {
    if depth == 0 {
        return None;
    }
    if let Some(ll) = indexer.live_lines.get(uri.as_str()) {
        let result = if keep_generics {
            ll.infer_type_raw(var_name)
        } else {
            ll.infer_type(var_name)
        };
        if result.is_some() {
            return result;
        }
        // Own the live lines and release the live_lines ref before any
        // recursion / further dashmap access (avoids re-entrant shard locks).
        let lines = (*ll).clone();
        drop(ll);
        // Live lines didn't find the type — consult the indexed snapshot.
        // This handles the case where `val x: T` is in a different source
        // section from the live editor content (e.g. sig vs code in tests,
        // or a declaration from a file indexed before the editor opened it).
        if let Some(data) = indexer.files.get(uri.as_str()) {
            if let Some(ann) = data.type_annotations.iter().find(|(_, n, _)| n == var_name) {
                return Some(if keep_generics {
                    ann.2.clone()
                } else {
                    strip_generics(&ann.2)
                });
            }
        }
        // Consult the CST-derived RHS data (parity with the indexed branch
        // below). Without this, `val x = recv.field` / `recv.method()` whose
        // declaring type lives in another file resolves to nothing in the live
        // editor path, even though the indexed/CLI path handles it.
        if let Some(t) = infer_var_from_rhs_data(indexer, var_name, uri, depth, keep_generics) {
            return Some(t);
        }
        return infer_method_return_type(indexer, var_name, &lines, uri, depth - 1)
            .or_else(|| find_extension_property_type(indexer, var_name, uri));
    }
    if let Some(data) = indexer.files.get(uri.as_str()) {
        if let Some(ann) = data.type_annotations.iter().find(|(_, n, _)| n == var_name) {
            return Some(if keep_generics {
                ann.2.clone()
            } else {
                strip_generics(&ann.2)
            });
        }
        let line_result = if keep_generics {
            data.lines.infer_type_raw(var_name)
        } else {
            data.lines.infer_type(var_name)
        };
        if line_result.is_some() {
            return line_result;
        }
        let lines = data.lines.clone();
        drop(data);
        if let Some(t) = infer_var_from_rhs_data(indexer, var_name, uri, depth, keep_generics) {
            return Some(t);
        }
        return infer_method_return_type(indexer, var_name, &lines, uri, depth - 1)
            .or_else(|| find_extension_property_type(indexer, var_name, uri));
    }
    let path = uri.to_file_path().ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let lines: Vec<String> = content.lines().map(String::from).collect();
    if keep_generics {
        lines.infer_type_raw(var_name)
    } else {
        lines.infer_type(var_name)
    }
}

/// Infer a variable's type from the file's CST-derived RHS maps
/// (`rhs_types`, `method_call_rhs`, `field_access_rhs`) for a
/// `val x = <init>` declaration. Extracts the matching entries while holding
/// the `files` ref, then drops it before recursing — so it is safe to call
/// from both the live-lines and indexed branches of
/// [`infer_variable_type_core`].
fn infer_var_from_rhs_data(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
    depth: u8,
    keep_generics: bool,
) -> Option<String> {
    let (rhs_match, method_match, field_match) = {
        let data = indexer.files.get(uri.as_str())?;
        (
            data.rhs_types
                .iter()
                .find(|(_, n, _)| n == var_name)
                .map(|(_, _, type_name)| type_name.clone()),
            data.method_call_rhs
                .iter()
                .find(|(_, n, _, _)| n == var_name)
                .map(|(_, _, recv, method)| (recv.clone(), method.clone())),
            data.field_access_rhs
                .iter()
                .find(|(_, n, _, _)| n == var_name)
                .map(|(_, _, recv, field)| (recv.clone(), field.clone())),
        )
    };
    if let Some(type_name) = rhs_match {
        return Some(type_name);
    }
    if let Some((recv, method)) = method_match {
        // Resolve the receiver's RAW type (generics kept) regardless of this
        // call's own `keep_generics`: a call-site type argument (e.g. the
        // `Unit` in `MutableSharedFlow<Unit>`) has to survive through to the
        // substitution step below, or a generic extension's return type
        // comes back with its declared type parameter still literal (e.g.
        // `SharedFlow<T>` instead of `SharedFlow<Unit>`) -- matches the raw
        // (generics-preserving) contract `rhs_match`/`field_match` above
        // already return regardless of `keep_generics` for this same reason.
        if let Some(recv_type_raw) = infer_variable_type_core(indexer, &recv, uri, depth - 1, true)
        {
            if let Some(ret) =
                resolve_method_return_type_substituted(indexer, &recv_type_raw, &method, uri)
            {
                return Some(ret);
            }
        }
    }
    if let Some((recv, field)) = field_match {
        if let Some(recv_type) =
            infer_variable_type_core(indexer, &recv, uri, depth - 1, keep_generics)
        {
            let recv_stripped = recv_type.split('<').next().unwrap_or(&recv_type);
            let recv_base = recv_stripped
                .rsplit('.')
                .next()
                .unwrap_or(recv_stripped)
                .strip_nullable();
            if let Some((field_type, _declaring_uri)) =
                find_field_type_in_class_impl(indexer, recv_base, &field, uri, depth - 1)
            {
                return Some(field_type);
            }
        }
    }
    None
}

/// Resolve `method_name`'s return type on a receiver whose raw (generics-
/// preserving) type is `recv_type_raw`, substituting the receiver's own
/// concrete type argument(s) into the result.
///
/// The "own class, then supertypes" fallback is delegated to
/// [`crate::indexer::InferDeps::find_method_return_type_for_type`] -- the
/// exact composite the CST engine's `resolve_call_expr_type`
/// (`indexer/infer/chain.rs`) reaches for the same receiver-method shape --
/// rather than hand-chaining `find_method_return_type` and
/// `find_method_return_type_via_supertypes` here too. Shared by both STRING
/// call sites below (`infer_var_from_rhs_data`'s `method_match` branch and
/// `infer_method_return_type`'s line-scan fallback) so a future change to
/// that fallback policy can't silently diverge between the two STRING call
/// sites, or between the STRING and CST engines -- which is exactly how the
/// supertype-walk fallback itself went missing from this file before it was
/// added back (see `find_method_return_type_via_supertypes`'s callers).
fn resolve_method_return_type_substituted(
    indexer: &Indexer,
    recv_type_raw: &str,
    method: &str,
    uri: &Url,
) -> Option<String> {
    let recv_base = recv_type_raw
        .dotted_ident_prefix()
        .last_segment()
        .to_owned();
    let raw_ret = indexer.find_method_return_type_for_type(&recv_base, method, uri)?;
    // Substitute the receiver's own concrete type argument(s) (e.g. `Unit` in
    // `MutableSharedFlow<Unit>`) into the raw, as-declared return type --
    // without this, a generic extension's return type keeps its literal type
    // parameter name instead of the caller's concrete instantiation.
    let subst = crate::indexer::build_type_arg_subst(indexer, &recv_base, recv_type_raw);
    Some(crate::indexer::apply_type_subst(&raw_ret, &subst))
}

/// CST fallback for variable type inference: find `val <var_name> = <init>` in
/// the live tree and infer the initializer's type via `infer_expr_type`. Catches
/// cases the line-based heuristics miss — notably lambda-result calls like
/// Compose `remember { Foo() }` (→ `Foo`) and constructor calls.
pub(crate) fn infer_variable_type_from_cst(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
) -> Option<String> {
    // Cycle guard — see `ResolutionInFlight`. Inferring a variable's type
    // infers its initializer, which resolves the identifiers inside it, which
    // infers *their* variables. A self- or mutually-referential initializer
    // closes that into a loop with no natural end.
    let _guard = ResolutionInFlight::enter(uri, var_name)?;
    let doc = indexer.live_doc_or_parse(uri)?;
    let bytes = doc.bytes.as_slice();
    let init = find_prop_initializer(doc.tree.root_node(), bytes, var_name, 0)?;
    crate::indexer::infer_expr_type(init, bytes, indexer, uri)
}

thread_local! {
    /// Variables whose type inference is currently on the stack, per thread.
    static RESOLVING_VARIABLES: std::cell::RefCell<std::collections::HashSet<(String, String)>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Breaks reference cycles in variable-type inference by refusing to re-enter
/// a resolution that is already in flight.
///
/// A depth cap cannot solve this. The loop runs
/// `infer_expr_type` → `infer_ident_type` → `infer_variable_type_from_cst` →
/// `infer_expr_type`, and `infer_expr_type` is a public entry point that
/// restarts its depth counter at zero on every lap, so the counter never
/// climbs. Only remembering what is already in flight terminates it.
///
/// Keyed on `(uri, var_name)` because that is what the resolution itself keys
/// on — [`find_prop_initializer`] searches a file by name — so re-entering the
/// same key cannot produce an answer the outer call is not already computing,
/// and returning `None` loses nothing. A resolution that became scope-aware
/// would need a correspondingly finer key here.
struct ResolutionInFlight {
    key: (String, String),
}

impl ResolutionInFlight {
    /// Claims `(uri, var_name)`, or returns `None` if it is already being
    /// resolved further up the stack — the cycle case.
    fn enter(uri: &Url, var_name: &str) -> Option<Self> {
        let key = (uri.as_str().to_owned(), var_name.to_owned());
        let claimed = RESOLVING_VARIABLES.with(|set| set.borrow_mut().insert(key.clone()));
        if !claimed {
            crate::util::report_resolution_cycle("infer_variable_type_from_cst", var_name, uri);
            return None;
        }
        Some(ResolutionInFlight { key })
    }
}

impl Drop for ResolutionInFlight {
    fn drop(&mut self) {
        RESOLVING_VARIABLES.with(|set| set.borrow_mut().remove(&self.key));
    }
}

/// Depth-first search for the initializer expression of `val/var <var_name> = …`.
fn find_prop_initializer<'a>(
    node: tree_sitter::Node<'a>,
    bytes: &[u8],
    var_name: &str,
    depth: usize,
) -> Option<tree_sitter::Node<'a>> {
    use crate::queries::{KIND_EQ, KIND_PROP_DECL};
    // A name that is never declared makes this search the whole file, so a
    // pathological input reaches its full depth here rather than returning
    // early — bail rather than overflow the stack. See
    // `crate::util::MAX_CST_DESCENT_DEPTH`.
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("find_prop_initializer", node);
        return None;
    }
    if node.kind() == KIND_PROP_DECL && prop_decl_name(node, bytes).as_deref() == Some(var_name) {
        let mut cursor = node.walk();
        let mut past_eq = false;
        for child in node.children(&mut cursor) {
            if child.kind() == KIND_EQ {
                past_eq = true;
                continue;
            }
            if past_eq {
                return Some(child);
            }
        }
        return None;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_prop_initializer(child, bytes, var_name, depth + 1) {
            return Some(found);
        }
    }
    None
}

/// Extract the declared variable name from a `property_declaration` node.
fn prop_decl_name(prop: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    use crate::queries::{KIND_SIMPLE_IDENT, KIND_VAR_DECL};
    let mut prop_cursor = prop.walk();
    let var_decl = prop
        .children(&mut prop_cursor)
        .find(|n| n.kind() == KIND_VAR_DECL)?;
    let mut var_decl_cursor = var_decl.walk();
    let ident = var_decl
        .children(&mut var_decl_cursor)
        .find(|n| n.kind() == KIND_SIMPLE_IDENT)?;
    ident.utf8_text(bytes).ok().map(str::to_owned)
}

/// Scan a specific (possibly un-indexed) file for the declared type of `field_name`.
///
/// Checks CST type annotations first (indexed files), then falls back to line
/// scanning, then reads from disk for un-indexed files.
pub(crate) fn infer_field_type(
    indexer: &Indexer,
    file_uri: &str,
    field_name: &str,
) -> Option<String> {
    let uri = tower_lsp::lsp_types::Url::parse(file_uri).ok()?;
    let file_data = ensure_file_data(indexer, &uri)?;
    if let Some(ann) = file_data
        .type_annotations
        .iter()
        .find(|(_, n, _)| n == field_name)
    {
        return Some(strip_generics(&ann.2));
    }
    file_data.lines.infer_type(field_name)
}

/// Like `infer_field_type` but preserves generic parameters in the result.
///
/// Returns `"MutableList<MbAccount>"` rather than `"MutableList"`, which is
/// needed for collection element type extraction via `extract_collection_element_type`.
/// Checks live editor lines first (most up-to-date), then CST type
/// annotations, then falls back to indexed lines and finally to a disk read
/// for un-indexed files.
///
/// `near_line` is the 0-based line of the class declaration whose field is
/// being looked up. `type_annotations` and every line-scan fallback here
/// cover every field/parameter in the *whole file*, not just the one class —
/// a sibling sealed-type member declared elsewhere in the same file with a
/// field of the same name (a common MVI pattern: many `data class
/// Foo(val event: FooEvent) : Event` variants side by side) would otherwise
/// shadow the real field via first-match. Every source here is disambiguated
/// against `near_line`: `type_annotations` (precise, per-declaration lines)
/// by picking the entry closest to it; the line-scan fallbacks by bounding
/// the scan to a window around it. Freshness still governs the *order*
/// (`live_lines` — updated synchronously on every keystroke — before the
/// debounced-reindex `type_annotations`/`data.lines`), so an edit to a
/// field's own declaration is visible immediately rather than only after the
/// next reindex settles; near_line-scoping on both sides is what keeps that
/// freshness-first order from reintroducing the sibling-shadowing bug this
/// function exists to avoid.
pub(crate) fn infer_field_type_raw(
    indexer: &Indexer,
    file_uri: &str,
    field_name: &str,
    near_line: u32,
) -> Option<String> {
    if let Some(live) = indexer.live_lines.get(file_uri) {
        if let Some(result) = windowed_infer_type_raw(&live, field_name, near_line) {
            return Some(result);
        }
    }
    if let Some(data) = indexer.files.get(file_uri) {
        if let Some(ann) = data
            .type_annotations
            .iter()
            .filter(|(_, n, _)| n == field_name)
            .min_by_key(|(line, _, _)| line.abs_diff(near_line))
        {
            return Some(ann.2.clone());
        }
        if let Some(result) = windowed_infer_type_raw(&data.lines, field_name, near_line) {
            return Some(result);
        }
        return None;
    }
    let path = tower_lsp::lsp_types::Url::parse(file_uri)
        .ok()?
        .to_file_path()
        .ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let lines: Vec<String> = content.lines().map(String::from).collect();
    windowed_infer_type_raw(&lines, field_name, near_line)
}

/// How far (in lines) around a class's own declaration to search for a field
/// via plain text scanning — generous enough for a multi-line constructor or
/// class body, far tighter than a whole file (where an unrelated sibling
/// class's same-named field would otherwise win by proximity too, just less
/// often than by first-match).
const FIELD_LOOKUP_WINDOW: u32 = 20;

fn windowed_infer_type_raw(lines: &[String], field_name: &str, near_line: u32) -> Option<String> {
    let start = near_line.saturating_sub(FIELD_LOOKUP_WINDOW) as usize;
    let end = ((near_line + FIELD_LOOKUP_WINDOW + 1) as usize).min(lines.len());
    if start >= end {
        return None;
    }
    // `infer_type_raw` (a pure per-line scan — no cross-line state, verified
    // by `infer_type_in_lines_raw`'s own implementation) returns the FIRST
    // match in the slice's order, not the closest one to `near_line`. Order
    // the window by distance first so the first match found is the closest
    // one — otherwise a sibling declaration earlier in the window (but
    // farther from `near_line` than the real field) wins by file order, the
    // exact sibling-shadowing bug this windowing exists to prevent. Mirrors
    // the `type_annotations` path's `min_by_key` disambiguation.
    let mut window_lines: Vec<usize> = (start..end).collect();
    window_lines.sort_by_key(|&line| (line as u32).abs_diff(near_line));
    let ordered: Vec<String> = window_lines
        .into_iter()
        .map(|line| lines[line].clone())
        .collect();
    ordered.infer_type_raw(field_name)
}

/// The file that declares `type_name`, resolved reachability-first from
/// `from_uri`'s own imports/package (see `resolve::resolve_type_index_only`;
/// handles a qualified `Outer.Inner` name too). Falls back to `from_uri`
/// itself when `type_name` isn't qualified (nothing to resolve) or
/// reachability resolution finds no candidate — a type genuinely declared in
/// `from_uri`'s own file, or one with no useful reachability signal at all,
/// is no worse off treated as anchored on the caller.
///
/// Shared by [`infer_field_chain_type`]'s root-anchoring and by callers that
/// resolve a *whole* smart-cast-narrowed chain directly (see
/// `enclosing_smart_cast_type`) and need the same reachability anchor for the
/// type they get back.
pub(crate) fn declaring_uri_for_type(indexer: &Indexer, type_name: &str, from_uri: &Url) -> Url {
    if !type_name.contains('.') {
        return from_uri.clone();
    }
    super::resolve::resolve_type_index_only(indexer, type_name, from_uri)
        .into_iter()
        .next()
        .map(|location| location.uri)
        .unwrap_or_else(|| from_uri.clone())
}

/// Resolve `field_name`'s declared type within `class_name`.
///
/// Candidate classes are found by name, preferring the declaration reachable
/// from `from_uri`'s own imports/package — the same reachability chain used
/// throughout the resolver (see `resolve::resolve_type_index_only`) — over an
/// arbitrary same-named class elsewhere in the workspace. A common pattern
/// this guards against: an MVI-style codebase where many unrelated features
/// each declare their own `sealed interface Event` — a bare by-name scan
/// picks whichever one the index happens to return first. Falls back to the
/// unscoped by-name search (`Indexer::workspace_def_candidates`, capped) only
/// when reachability resolution finds nothing, e.g. a class reached only
/// through a chain with no import anywhere naming it directly.
///
/// Returns the field's type together with the `Url` of the file where
/// `class_name` itself was found: the correct reachability context for
/// resolving *that field's own* type is the class's declaring file, not
/// necessarily `from_uri` — `Event.OverdraftInput.event: OverdraftInputEvent`
/// must resolve `OverdraftInputEvent` via `OverdraftInput`'s own file's
/// imports, which is why `infer_field_chain_type` re-anchors on this for the
/// next chain segment.
pub(crate) fn find_field_type_in_class(
    indexer: &Indexer,
    class_name: &str,
    field_name: &str,
    from_uri: &Url,
) -> Option<(String, Url)> {
    find_field_type_in_class_impl(
        indexer,
        class_name,
        field_name,
        from_uri,
        MAX_RAW_TYPE_INFER_DEPTH,
    )
}

/// Depth-guarded implementation of [`find_field_type_in_class`] — shares its
/// budget with `infer_var_from_rhs_data`'s `field_match` branch, the one
/// caller that re-enters this function (see `MAX_RAW_TYPE_INFER_DEPTH`).
fn find_field_type_in_class_impl(
    indexer: &Indexer,
    class_name: &str,
    field_name: &str,
    from_uri: &Url,
    depth: u8,
) -> Option<(String, Url)> {
    if depth == 0 {
        return None;
    }
    let mut candidates = super::resolve::resolve_type_index_only(indexer, class_name, from_uri);
    if candidates.is_empty() {
        candidates = indexer.workspace_def_candidates(class_name);
    }
    for location in &candidates {
        if let Some(field_type) = infer_field_type_raw(
            indexer,
            location.uri.as_str(),
            field_name,
            location.range.start.line,
        ) {
            return Some((field_type, location.uri.clone()));
        }
    }
    // Fallback: full variable inference including CST-indexed field_access_rhs
    // and method_call_rhs data (handles unannotated `val x = recv.field`).
    for location in &candidates {
        if let Some(field_type) =
            infer_variable_type_raw_impl(indexer, field_name, &location.uri, depth - 1)
        {
            return Some((field_type, location.uri.clone()));
        }
    }
    None
}

// ─── Extension property type inference ───────────────────────────────────────

/// Look up the declared type of an extension property named `prop_name` that
/// is available on any class declared in the file at `uri`.
///
/// This is the fallback path for expressions like `viewModelScope.launch` where
/// `viewModelScope` is `val ViewModel.viewModelScope: CoroutineScope` — the
/// property is not declared inside the calling file, so line-scanning returns
/// nothing.  Here we:
/// 1. Collect all class names declared in the calling file.
/// 2. Build the ancestor set for each via `walk_hierarchy`.
/// 3. Scan the index for an extension property whose `extension_receiver` is in
///    that ancestor set and whose `name == prop_name`.
/// 4. Extract the return type from the symbol's `detail` string.
fn find_extension_property_type(indexer: &Indexer, prop_name: &str, uri: &Url) -> Option<String> {
    // TODO: This fallback considers ALL classes in the file, so in files with
    // multiple top-level classes, an extension for the wrong class could match.
    // Threading the enclosing class context through the full call chain is needed
    // for a proper fix; the primary (line-scanning) path handles the common case.
    use super::{walk_hierarchy, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK};
    use crate::types::{CallerContext, Visibility};
    // Use ensure_file_data so the function works even when the file has not been
    // indexed yet (e.g. first open before the workspace scan completes).
    let file = ensure_file_data(indexer, uri)?;

    // Collect class names declared in this file as starting points.
    let class_names: Vec<(String, String)> = file
        .symbols
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                SymbolKind::CLASS | SymbolKind::OBJECT | SymbolKind::INTERFACE | SymbolKind::STRUCT
            )
        })
        .map(|s| (s.name.clone(), uri.to_string()))
        .collect();

    if class_names.is_empty() {
        return None;
    }

    // Build a set of all ancestor type names across all classes in this file.
    let caller = CallerContext {
        uri: Some(uri.as_str()),
        cursor_line: None,
    };
    let mut ancestor_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (class_name, class_uri) in &class_names {
        ancestor_set.insert(class_name.clone());
        let supers: Vec<String> = walk_hierarchy(
            indexer,
            class_name,
            class_uri,
            caller,
            8,
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
            |_idx, super_name, _super_uri, _caller| vec![super_name.to_owned()],
        );
        ancestor_set.extend(supers);
    }

    // Use the reverse index: O(ancestors) instead of O(all_files).
    // `extension_by_receiver` is Tier-2-only — promote any not-yet-
    // materialized JAR that Tier 1 says declares an extension on a walked
    // ancestor BEFORE reading it, or a Tier-1-only extension property (e.g.
    // `viewModelScope`) is invisible to type inference and chained
    // completion after it goes dark.
    //
    // Zero sidecar-IPC budget, deliberately: this runs INSIDE completion
    // requests (via receiver-type inference), so giving it its own IPC pool
    // would double the per-request blocking-IPC cap the completion sites
    // already spend. Fresh-cache-backed promotions are free regardless
    // (see `promote_candidates_bounded`), which covers the realistic
    // warm-cache case; a genuinely uncached JAR's extension property is
    // promoted by the file-open import promotion (`promote_file_imports`)
    // or by the completion sites' own budget instead.
    let mut jar_promotion_budget = 0usize;
    for ancestor in &ancestor_set {
        let Some(entries) = crate::indexer::jar::extension_entries_for(
            indexer,
            ancestor,
            &mut jar_promotion_budget,
        ) else {
            continue;
        };
        for entry in entries.iter() {
            if entry.name != prop_name {
                continue;
            }
            use tower_lsp::lsp_types::SymbolKind;
            if !matches!(entry.kind, SymbolKind::PROPERTY | SymbolKind::VARIABLE) {
                continue;
            }
            if matches!(
                entry.visibility,
                Visibility::Private | Visibility::Protected
            ) {
                continue;
            }
            let type_name = extract_property_type_from_detail(&entry.detail);
            if let Some(type_name) = type_name {
                return Some(type_name);
            }
        }
    }
    None
}

// ─── Method return-type inference ─────────────────────────────────────────────

fn infer_method_return_type(
    indexer: &Indexer,
    var_name: &str,
    lines: &[String],
    uri: &Url,
    depth: u8,
) -> Option<String> {
    let mut plain_fn_candidates: Vec<String> = Vec::new();
    let mut seen_receivers: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for line in lines {
        let rhs = match find_rhs_str(line, var_name) {
            Some(r) => r,
            None => continue,
        };

        // Match `receiver.method(` where receiver is a simple identifier.
        let paren_pos = match rhs.find('(') {
            Some(p) => p,
            None => continue,
        };
        let before_paren = &rhs[..paren_pos];
        match before_paren.rfind('.') {
            Some(dot_pos) => {
                let receiver = before_paren[..dot_pos].trim();
                let method = before_paren[dot_pos + 1..].trim();

                if receiver.is_empty() || method.is_empty() {
                    continue;
                }
                // Skip `this`/`super` and multi-segment receivers.
                if receiver == "this" || receiver == "super" || receiver.contains('.') {
                    continue;
                }
                if !method.starts_with_lowercase() {
                    continue;
                }
                // Dedup: skip if we already tried this receiver (avoids exponential blowup).
                if !seen_receivers.insert(receiver) {
                    continue;
                }

                // Recursively infer the RAW receiver type (generics kept; DashMap
                // guards already dropped) -- see `resolve_method_return_type_substituted`:
                // without the raw form, a generic extension's return type (e.g.
                // `asSharedFlow`'s `SharedFlow<T>`) would keep its literal type
                // parameter instead of the receiver's own concrete argument.
                if let Some(receiver_type_raw) =
                    infer_variable_type_raw_impl(indexer, receiver, uri, depth)
                {
                    if let Some(ret) = resolve_method_return_type_substituted(
                        indexer,
                        &receiver_type_raw,
                        method,
                        uri,
                    ) {
                        return Some(ret);
                    }
                }
            }
            None => {
                // Plain function call: `val result = getFoo(args)` — no dot-receiver.
                // Guard: skip when the first call is part of a chain (`getFoo(...).bar()`).
                let fn_name = before_paren.trim();
                if !fn_name.is_empty()
                    && fn_name.starts_with_lowercase()
                    && !has_dot_after_first_call(rhs, paren_pos)
                {
                    plain_fn_candidates.push(fn_name.to_owned());
                }
            }
        }
    }

    // Secondary pass: plain function calls. Prefer the import-aware lookup (binds
    // to the imported symbol, e.g. compose `stringResource: String`) over the loose
    // global-name scan (which may grab a test-only same-named extension).
    for fn_name in &plain_fn_candidates {
        if let Some(ret) = find_fun_return_type_reachable(indexer, fn_name, uri)
            .or_else(|| find_fun_return_type_by_name(indexer, fn_name, uri))
        {
            return Some(ret);
        }
    }

    None
}

/// Receiver-less by-name return-type lookup — the last-resort tail behind
/// [`find_fun_return_type_reachable`] at every call site.
///
/// This is the "unscoped last-resort tail" flagged by the 2026-06-30
/// CST-resolution-unification design doc's capability #1 (import/package/
/// super-aware filter — see `docs/superpowers/specs/2026-06-30-cst-resolution-
/// unification-design.md`, "Capability mapping"), which specified this filter
/// as an invariant but never implemented it here. A production bug proved the
/// gap is real: `retrofit.create(GoldConversionPublicApi::class.java)`
/// bare-name-matched an unrelated `SymbolProcessorProvider.create():
/// SymbolProcessor` (KSP, a different library entirely) purely because it
/// came first in `definitions` iteration order.
///
/// Fix: when several same-named candidates exist, prefer one whose declaring
/// file is actually reachable from `uri` (same package / explicit import /
/// star import) over first-match-in-iteration-order. When NONE are reachable,
/// this still falls back to the historical "take the first candidate with an
/// extractable return type" — every call site already tries the properly
/// scoped `find_fun_return_type_reachable` first, so by the time this runs,
/// guessing from an unrelated in-workspace symbol is judged more useful than
/// returning nothing (e.g. when the receiver's own type isn't indexed at all,
/// as with an un-promoted library receiver).
pub(crate) fn find_fun_return_type_by_name(
    indexer: &Indexer,
    fn_name: &str,
    uri: &Url,
) -> Option<String> {
    // The helper scopes to workspace defs and caps the scan (a ubiquitous name
    // like `create` has thousands of source-JAR defs, each a full symbol-list +
    // signature line scan — previously a multi-second stall).
    let candidates = indexer.workspace_def_candidates(fn_name);

    let caller_file_data = indexer.files.get(uri.as_str()).map(|r| r.value().clone());
    let caller_file_data_ref = caller_file_data.as_deref();

    candidates
        .iter()
        .filter(|loc| {
            candidate_declaration_is_reachable(indexer, loc, fn_name, caller_file_data_ref)
        })
        .find_map(|loc| return_type_of_named_fn_at(indexer, fn_name, loc))
        .or_else(|| {
            candidates
                .iter()
                .find_map(|loc| return_type_of_named_fn_at(indexer, fn_name, loc))
        })
}

/// Whether `loc`'s declaring file is reachable from a caller with
/// `caller_file_data`'s package/imports — same package, an explicit import, or
/// a star import. Reuses [`extension_is_in_scope`]'s package/import check (its
/// body is not actually extension-specific — see that function's doc comment
/// for the individual rules).
fn candidate_declaration_is_reachable(
    indexer: &Indexer,
    loc: &Location,
    fn_name: &str,
    caller_file_data: Option<&FileData>,
) -> bool {
    // Missing `FileData` means the candidate's real package is unknown (not
    // "no package") -- don't guess reachability for it either way.
    let Some(candidate_file_data) = indexer.files.get(loc.uri.as_str()) else {
        return false;
    };
    extension_is_in_scope(
        candidate_file_data.package.as_ref(),
        fn_name,
        caller_file_data,
    )
}

/// Read `fn_name`'s return type off the function/method/operator symbol at
/// `loc`, trying the (possibly truncated) `detail` string first, then a fresh
/// signature line scan.
fn return_type_of_named_fn_at(indexer: &Indexer, fn_name: &str, loc: &Location) -> Option<String> {
    let file_data = indexer.files.get(loc.uri.as_str())?;
    for symbol in &file_data.symbols {
        if symbol.name != fn_name {
            continue;
        }
        if !matches!(
            symbol.kind,
            SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::OPERATOR
        ) {
            continue;
        }
        if let Some(ret) = extract_return_type_from_detail(&symbol.detail) {
            return Some(ret);
        }
        let start_line = symbol.selection_start() as usize;
        let full_sig = file_data.lines.collect_signature(start_line);
        if let Some(ret) = extract_return_type_from_detail(&full_sig) {
            return Some(ret);
        }
    }
    None
}

/// Import-aware return-type lookup. Resolves `fn_name` via the no-rg resolver
/// (imports → same-package → star → qualified/jars, package-filtered) and reads
/// the *resolved* symbol's return type — i.e. the symbol the call actually binds
/// to, not an arbitrary same-named overload from a test file or unrelated jar.
/// Falls back to `None` so callers can defer to the looser `find_fun_return_type_by_name`.
pub(crate) fn find_fun_return_type_reachable(
    indexer: &Indexer,
    fn_name: &str,
    uri: &Url,
) -> Option<String> {
    // Promotion MUST happen before `resolve_symbol_scoped_only` at this call
    // site — unlike `find_extension_fn_return_type_scoped` below, where the
    // check guards a `jar_files` read that happens *after* it in the same
    // function, here `locations` is produced BY `resolve_symbol_scoped_only`,
    // which reads `jar_definitions` directly via `resolve_via_imports`
    // upstream. If promotion ran after this call (as it did before this
    // fix), a Tier-1-only candidate would already have produced an empty
    // `locations` Vec by the time materialization completed, so the
    // `for loc in &locations` loop below would never see the
    // freshly-materialized data on THIS call — only a later, separate call
    // would benefit. Do not move this back below `resolve_symbol_scoped_only`.
    // ZERO sidecar-IPC budget: this runs on latency-critical inference paths
    // (inlay hints call it once per name in the visible range — unbudgeted
    // blocking IPC here was observed live as a 22s inlay compute that timed
    // out every queued request behind it). Fresh-cache-backed promotions are
    // free and still happen; a genuinely uncached JAR is promoted by the
    // explicit user actions instead (completion's budget, file-open imports,
    // hover/goto-def resolution).
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(indexer, fn_name, &mut cache_backed_only);
    // Scoped-only, not `resolve_symbol_no_rg`: this function's every caller
    // already chains its own last-resort fallback (`find_fun_return_type`/
    // `find_fun_return_type_by_name`, which -- unlike this scan -- prefers an
    // import/package-reachable candidate over an arbitrary same-named one)
    // afterward, so this step should only report a match that's genuinely
    // reachable via local/import/package/hierarchy resolution, not
    // `resolve_symbol_no_rg`'s own "first workspace/JAR match, any package"
    // tail. Skipping that tail here matters: letting it fire pre-empted the
    // caller's own, more-reachability-aware fallback with a match that had
    // no more claim to correctness — a real production bug had it beat a
    // completely unrelated, unimported library's same-named function ahead
    // of the actually-intended resolution.
    let locations = crate::resolver::resolve_symbol_scoped_only(indexer, fn_name, uri);
    let mut fallback: Option<String> = None;
    for loc in &locations {
        let Some(file_data) = indexer
            .files
            .get(loc.uri.as_str())
            .or_else(|| indexer.jar_files.get(loc.uri.as_str()))
        else {
            continue;
        };
        for symbol in &file_data.symbols {
            if symbol.name != fn_name {
                continue;
            }
            if !matches!(
                symbol.kind,
                SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::OPERATOR
            ) {
                continue;
            }
            let ret = extract_return_type_from_detail(&symbol.detail);
            if symbol.selection_range.start == loc.range.start {
                // The symbol the resolver actually bound to.
                if ret.is_some() {
                    return ret;
                }
            } else if fallback.is_none() {
                fallback = ret;
            }
        }
    }
    fallback
}

pub(crate) fn find_method_return_type(
    indexer: &Indexer,
    type_name: &str,
    method_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    let type_base = type_name.last_segment();

    // Extension functions take precedence over member functions.
    if let Some(ret) = find_extension_fn_return_type(indexer, type_base, method_name, from_uri) {
        return Some(ret);
    }

    // Then check member functions (container-based), scoped + capped via the helper.
    indexer.find_in_workspace_defs(type_base, |loc| {
        let file_data = indexer.files.get(loc.uri.as_str())?;
        for symbol in &file_data.symbols {
            if symbol.name != method_name {
                continue;
            }
            if !matches!(
                symbol.kind,
                SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::OPERATOR
            ) {
                continue;
            }
            if symbol.container.as_deref() != Some(type_base) {
                continue;
            }
            // Try detail first; fall back to source lines when detail is truncated.
            if let Some(ret) = extract_return_type_from_detail(&symbol.detail) {
                return Some(ret);
            }
            // detail may be truncated (120 char limit) — try the source lines.
            let start_line = symbol.selection_start() as usize;
            let full_sig = file_data.lines.collect_signature(start_line);
            if let Some(ret) = extract_return_type_from_detail(&full_sig) {
                return Some(ret);
            }
        }
        None
    })
}

/// Returns true when an extension function declared in `entry_package` is
/// accessible from the calling file, either via same-package visibility or
/// an explicit import in `caller_file_data`.
pub(crate) fn extension_is_in_scope(
    entry_package: Option<&String>,
    entry_name: &str,
    caller_file_data: Option<&FileData>,
) -> bool {
    // Kotlin's default package (no `package` header) is a real package like
    // any other, so two default-package files are same-package to each
    // other -- but only once the caller's `FileData` is actually known.
    // `caller_file_data: None` means "unknown," not "confirmed no package";
    // conflating the two (e.g. via `caller_file_data.and_then(|fd| fd.package)`,
    // which is `None` in both cases) would treat an unloaded caller file as
    // reachable by accident.
    if entry_package.is_none() && caller_file_data.is_some_and(|fd| fd.package.is_none()) {
        return true;
    }
    let caller_package = caller_file_data.and_then(|fd| fd.package.as_ref());
    if entry_package.is_some_and(|ext_pkg| caller_package == Some(ext_pkg)) {
        return true;
    }
    // Check whether the caller has an import (star or explicit) that covers
    // the extension function's package and name.
    caller_file_data.is_some_and(|fd| {
        fd.imports.iter().any(|imp| {
            entry_package
                .as_ref()
                .is_some_and(|ext_pkg| imp.covers(ext_pkg, entry_name))
                || entry_package.is_none() && imp.local_name == entry_name
        })
    })
}

/// Find the return type of an extension function `method_name` declared with receiver
/// `ReceiverType` where `ReceiverType`'s base name == `receiver_base`.
///
/// When `from_uri` is provided, only extensions in scope (same package or imported)
/// at that URI are considered — matching the scope rules used by goto-definition.
/// When `from_uri` is `None`, a global unfiltered lookup is performed (for callers
/// that have no URI context).
///
/// Extension functions are stored with `container = None` and `extension_receiver = "Foo"`,
/// so `find_method_return_type` (which filters by `container == Some(type_base)`) misses them.
/// This function searches by the function name directly, then filters by receiver.
///
/// Example: `receiver_base = "Optional"`, `method_name = "getOrNull"` →
/// finds `public fun <T : Any> Optional<T>.getOrNull(): T?` and returns `"T?"`.
pub(crate) fn find_extension_fn_return_type(
    indexer: &Indexer,
    receiver_base: &str,
    method_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    if let Some(uri) = from_uri {
        return find_extension_fn_return_type_scoped(indexer, receiver_base, method_name, uri);
    }
    find_extension_fn_return_type_global(indexer, receiver_base, method_name)
}

fn find_extension_fn_return_type_scoped(
    indexer: &Indexer,
    receiver_base: &str,
    method_name: &str,
    from_uri: &Url,
) -> Option<String> {
    // Promotion MUST happen before the `extension_by_receiver` read below —
    // `extension_by_receiver` is populated exclusively by Tier-2
    // materialization (`build_jar_file_data`); Tier 1
    // (`populate_tier1_from_manifest`) never writes it, only writes
    // `jar_bare_names`/`jar_qualified`. So for a genuinely Tier-1-only
    // (not-yet-materialized) extension method, `extension_by_receiver.get`
    // returns `None` and this function bails out via `?` immediately — before
    // the promotion check that used to sit later, inside the loop below,
    // ever ran. That made the inner check unreachable in the real
    // "needs promotion" case (see `find_fun_return_type_reachable` above for
    // the same ordering fix applied to a sibling call site). `method_name` —
    // the extension function's own bare name — is available as a parameter
    // here and is the correct key into `jar_bare_names`, which
    // `populate_tier1_from_manifest` populates for every manifest entry
    // (member and extension functions alike).
    // ZERO sidecar-IPC budget, same rationale as `find_fun_return_type_reachable`
    // above: inference runs per-name on latency-critical paths (inlay hints)
    // — cache-backed promotions stay free, blocking IPC belongs to explicit
    // user actions.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(indexer, method_name, &mut cache_backed_only);
    let entries =
        crate::indexer::jar::extension_entries_for(indexer, receiver_base, &mut cache_backed_only)?;
    let caller_file_data = indexer.files.get(from_uri.as_str());
    let caller_file_data_ref: Option<&FileData> = caller_file_data.as_deref().map(|v| v.as_ref());
    for entry in entries.iter() {
        if entry.name != method_name {
            continue;
        }
        if !matches!(entry.kind, SymbolKind::FUNCTION) {
            continue;
        }
        if !extension_is_in_scope(entry.package.as_ref(), &entry.name, caller_file_data_ref) {
            continue;
        }
        // Try detail first; fall back to source lines when detail is truncated.
        if let Some(ret) = extract_return_type_from_detail(&entry.detail) {
            return Some(ret);
        }
        // detail may be truncated (120 char limit) — try the source lines.
        // No promotion check needed here: reaching this `entry` at all means
        // `extension_by_receiver` already had it, which — per the comment
        // above `entries` — only happens once Tier-2 materialization has
        // already populated `jar_files` for this same jar.
        let file_data = indexer
            .files
            .get(&entry.file_uri)
            .or_else(|| indexer.jar_files.get(&entry.file_uri))?;
        let start_line = file_data
            .symbols
            .iter()
            .find(|s| {
                s.name == method_name
                    && s.extension_receiver() == receiver_base
                    && s.container.is_none()
            })?
            .selection_start() as usize;
        let full_sig = file_data.lines.collect_signature(start_line);
        if let Some(ret) = extract_return_type_from_detail(&full_sig) {
            return Some(ret);
        }
    }
    None
}

fn find_extension_fn_return_type_global(
    indexer: &Indexer,
    receiver_base: &str,
    method_name: &str,
) -> Option<String> {
    // Global extension-fn lookup by bare method name: scoped + capped via the helper.
    indexer.find_in_workspace_defs(method_name, |loc| {
        let file_data = indexer.files.get(loc.uri.as_str())?;
        for symbol in &file_data.symbols {
            if symbol.name != method_name {
                continue;
            }
            if !matches!(symbol.kind, SymbolKind::FUNCTION) {
                continue;
            }
            if symbol.extension_receiver() != receiver_base {
                continue;
            }
            if let Some(ret) = extract_return_type_from_detail(&symbol.detail) {
                return Some(ret);
            }
            let start_line = symbol.selection_start() as usize;
            let full_sig = file_data.lines.collect_signature(start_line);
            if let Some(ret) = extract_return_type_from_detail(&full_sig) {
                return Some(ret);
            }
        }
        None
    })
}

pub(crate) fn find_method_return_type_via_supertypes(
    indexer: &Indexer,
    class_name: &str,
    method_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    // Strip generics AND any qualifying package prefix: `lookup_definitions`
    // is keyed by the bare symbol name, so a qualified `class_name` (e.g.
    // `com.lib.MutableSharedFlow<Event>`) would otherwise silently miss it.
    let class_base = class_name.dotted_ident_prefix().last_segment().to_owned();

    // `lookup_definitions` merges workspace + JAR locations (promoting a
    // not-yet-materialized JAR as needed) into an owned `Vec<Location>` --
    // the walk below promotes further JARs per-ancestor, and an owned Vec
    // (unlike a DashMap `Ref`) can't deadlock against that.
    indexer
        .lookup_definitions(&class_base)
        .into_iter()
        .take(crate::indexer::MAX_BY_NAME_DEFS)
        .find_map(|loc| {
            find_method_return_type_via_class_hierarchy(
                indexer,
                &class_base,
                loc.uri.as_str(),
                method_name,
                from_uri,
            )
        })
}

/// Walk every ancestor of `class_base` (declared at `class_uri`) via
/// [`walk_hierarchy`] — the same recursive, cycle-safe, JAR-promotion-aware
/// traversal `resolve_from_class_hierarchy`/completion use elsewhere —
/// looking for `method_name`. Unlike a hand-rolled single-level check, this
/// finds a method declared two or more levels up (or on a JAR-only
/// grandparent), not just the direct supertype.
///
/// The direct supertype's own generic type arguments (e.g.
/// `class Derived : Base<Int>`) are substituted into a hit found there via
/// `substitute_direct_supertype_args`; a hit on a deeper ancestor is
/// returned as-is — multi-level generic substitution isn't attempted, the
/// same scope the original single-level logic had.
fn find_method_return_type_via_class_hierarchy(
    indexer: &Indexer,
    class_base: &str,
    class_uri: &str,
    method_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    use crate::types::CallerContext;

    let caller = CallerContext {
        uri: from_uri.map(Url::as_str),
        cursor_line: None,
    };
    let hits: Vec<(String, String)> = walk_hierarchy(
        indexer,
        class_base,
        class_uri,
        caller,
        8,
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        |idx, super_name, _super_uri, _caller| {
            find_method_return_type(idx, super_name, method_name, from_uri)
                .map(|raw| (super_name.to_owned(), raw))
                .into_iter()
                .collect()
        },
    );
    let (super_name, raw) = hits.into_iter().next()?;
    Some(substitute_direct_supertype_args(
        indexer,
        class_uri,
        class_base,
        &super_name,
        &raw,
    ))
}

/// If `super_name` is `class_base`'s *direct* supertype (declared with
/// concrete type arguments, e.g. `class Derived : Base<Int>`), substitute
/// those into `raw`; otherwise (deeper ancestor, or no type args) return
/// `raw` unchanged.
fn substitute_direct_supertype_args(
    indexer: &Indexer,
    class_uri: &str,
    class_base: &str,
    super_name: &str,
    raw: &str,
) -> String {
    let Ok(uri) = Url::parse(class_uri) else {
        return raw.to_owned();
    };
    let Some(file_data) = ensure_file_data(indexer, &uri) else {
        return raw.to_owned();
    };
    let Some(class_sym) = file_data.symbols.iter().find(|s| s.name == class_base) else {
        return raw.to_owned();
    };
    let class_line = class_sym.selection_start();
    let Some((_, _, type_args)) = file_data
        .supers
        .iter()
        .find(|(line, name, _)| *line == class_line && name == super_name)
    else {
        return raw.to_owned();
    };
    if type_args.is_empty() {
        return raw.to_owned();
    }
    // `super_name` can be a dotted qualified spelling (`class X : com.lib.Base()`
    // -- see `supertype_targets` in hierarchy.rs), but `find_class_type_params`
    // matches against `FileData.symbols`' bare `name` field, so a qualified
    // spelling would silently miss and skip substitution entirely.
    let super_type_params = find_class_type_params(indexer, super_name.last_segment());
    if super_type_params.is_empty() {
        return raw.to_owned();
    }
    apply_supertype_subst(raw, &super_type_params, type_args)
}

fn find_class_type_params(indexer: &Indexer, class_name: &str) -> Vec<String> {
    indexer
        .find_in_workspace_defs(class_name, |loc| {
            let file_data = indexer.files.get(loc.uri.as_str())?;
            let symbol = file_data
                .symbols
                .iter()
                .find(|s| s.name == class_name && !s.type_params().is_empty())?;
            Some(symbol.type_params().to_vec())
        })
        .unwrap_or_default()
}

/// Replace generic type parameter names with concrete type arguments.
///
/// Given `raw = "Flow<ReducedResult<EffectType, StateType>>"`,
/// `params = ["EventType", "EffectType", "StateType"]`,
/// `args = ["BuildingSavingsInputEvent", "BuildingSavingsEffect", "Sheet"]`,
/// returns `"Flow<ReducedResult<BuildingSavingsEffect, Sheet>>"`.
fn apply_supertype_subst(raw: &str, params: &[String], args: &[String]) -> String {
    let mut result = raw.to_string();
    for (param, arg) in params.iter().zip(args.iter()) {
        // Replace whole-word occurrences only (not substrings of other type names).
        let mut new_result = String::with_capacity(result.len());
        let mut remaining = result.as_str();
        while let Some(pos) = remaining.find(param.as_str()) {
            new_result.push_str(&remaining[..pos]);
            let after = pos + param.len();
            let before_ok = pos == 0
                || !remaining.as_bytes()[pos - 1].is_ascii_alphanumeric()
                    && remaining.as_bytes()[pos - 1] != b'_';
            let after_ok = after >= remaining.len()
                || !remaining.as_bytes()[after].is_ascii_alphanumeric()
                    && remaining.as_bytes()[after] != b'_';
            if before_ok && after_ok {
                new_result.push_str(arg);
            } else {
                new_result.push_str(param);
            }
            remaining = &remaining[after..];
        }
        new_result.push_str(remaining);
        result = new_result;
    }
    result
}

#[cfg(test)]
#[path = "infer_tests.rs"]
mod infer_tests;
