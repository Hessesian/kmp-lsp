//! CST navigation-chain resolution helpers.

use tower_lsp::lsp_types::Url;

use crate::indexer::NodeExt;
use crate::queries::{
    KIND_CALL_EXPR, KIND_CALL_SUFFIX, KIND_CLASS_DECL, KIND_LAMBDA_LIT, KIND_NAV_EXPR,
    KIND_NAV_SUFFIX, KIND_OBJECT_DECL, KIND_SIMPLE_IDENT, KIND_STATEMENTS, KIND_TYPE_IDENT,
    KIND_VALUE_ARGS,
};
use crate::resolver::extract_collection_element_type;
use crate::StrExt;

use super::deps::InferDeps;
use super::lambda::{
    GENERIC_FACTORY_FNS, LAMBDA_RESULT_FNS, NUMERIC_CONVERSION_FNS, SCOPE_FUNCTIONS,
};
use super::type_subst::{
    apply_simple_subst, build_fn_subst, build_type_arg_subst, capitalize_first_char,
    first_type_arg_raw, is_generic_param, split_top_level_commas, type_args_inner,
};

/// A segment in a navigation chain: either a root identifier or a suffix member.
#[derive(Debug)]
pub(super) enum NavSegment<'a> {
    /// Root identifier node (leftmost expression in the chain)
    Root(tree_sitter::Node<'a>),
    /// A navigation suffix member name (the identifier after `.` or `?.`).
    /// `safe_call` is true for `?.` navigation (strips nullability).
    Suffix { name: String, safe_call: bool },
    /// A call_expression intermediate (e.g. previous `.let { }` in a chain)
    CallExpr(tree_sitter::Node<'a>),
}

/// Collect navigation segments from a navigation_expression tree, left to right.
/// The structure is nested: `(a.b).c` is nav_expr(nav_expr(a, .b), .c)
pub(super) fn collect_nav_segments<'a>(
    node: tree_sitter::Node<'a>,
    bytes: &[u8],
) -> Vec<NavSegment<'a>> {
    let mut segments = Vec::new();
    collect_nav_segments_recursive(node, bytes, &mut segments, 0);
    segments
}

fn collect_nav_segments_recursive<'a>(
    node: tree_sitter::Node<'a>,
    bytes: &[u8],
    segments: &mut Vec<NavSegment<'a>>,
    depth: usize,
) {
    if node.kind() != KIND_NAV_EXPR {
        // Base case: not a navigation expression
        segments.push(NavSegment::Root(node));
        return;
    }

    // Kind-bounded (only ever descends through KIND_NAV_EXPR/KIND_CALL_EXPR),
    // but an arbitrarily long `a.b.c.d…` / `a.let{}.let{}…` chain is still an
    // arbitrarily deep recursion — cap it rather than overflow the stack.
    // See `crate::util::MAX_CST_DESCENT_DEPTH`. Treat the cut point as an
    // opaque root, same as the ordinary non-nav-expression base case above.
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        segments.push(NavSegment::Root(node));
        return;
    }

    // Left child: either another nav_expr, call_expr, or identifier
    if let Some(left) = node.named_child(0) {
        match left.kind() {
            k if k == KIND_NAV_EXPR => {
                collect_nav_segments_recursive(left, bytes, segments, depth + 1);
            }
            k if k == KIND_CALL_EXPR => {
                // Intermediate call expression (e.g. `a.let { }.let { }`)
                // Recurse into its callee to get the chain up to that point
                if let Some(inner_callee) = left.child(0) {
                    collect_nav_segments_recursive(inner_callee, bytes, segments, depth + 1);
                }
                segments.push(NavSegment::CallExpr(left));
            }
            _ => {
                segments.push(NavSegment::Root(left));
            }
        }
    }

    // Right child: navigation_suffix → extract member name
    if let Some(suffix) = node.first_child_of_kind(KIND_NAV_SUFFIX) {
        // Detect safe-call `?.` by checking the raw text of the suffix node.
        let suffix_text = suffix.utf8_text(bytes).unwrap_or("");
        let is_safe = suffix_text.starts_with("?.");
        let mut suffix_cursor = suffix.walk();
        let member = suffix
            .children(&mut suffix_cursor)
            .find(|child| {
                let kind = child.kind();
                kind == KIND_SIMPLE_IDENT || kind == KIND_TYPE_IDENT
            })
            .and_then(|child| child.utf8_text_owned(bytes));
        if let Some(name) = member {
            segments.push(NavSegment::Suffix {
                name,
                safe_call: is_safe,
            });
        }
    }
}

/// How an unresolvable navigation suffix is handled during a forward walk.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum SuffixStrictness {
    /// Receiver-position semantics: an unresolved suffix leaves the receiver
    /// type in place (best-effort; the caller probes members next).
    LeakReceiver,
    /// Expression-position semantics: an unresolved suffix fails the walk —
    /// the expression's own type is unknown (matches the deleted text
    /// walker's per-segment `?`).
    Fail,
}

/// Forward-resolve a chain of segments to get (receiver_type_before_last, last_method_name).
pub(super) fn forward_resolve_segments(
    segments: &[NavSegment<'_>],
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    strictness: SuffixStrictness,
) -> Option<(String, String)> {
    if segments.is_empty() {
        return None;
    }

    let mut current_type: Option<String> = None;
    let mut last_suffix: Option<String> = None;
    // Track whether the last Suffix actually changed current_type.
    // Used by the CallExpr dedup check: only skip re-resolution when the Suffix
    // already resolved the type (avoids false dedup when the Suffix found nothing).
    let mut last_suffix_resolved = false;

    for segment in segments {
        match segment {
            NavSegment::Root(node) => {
                current_type = resolve_root_node_type(*node, bytes, deps, uri);
                // If the root is a call_expression, record its fn name so that
                // a subsequent CallExpr for the same call (trailing-lambda wrapper)
                // is recognized as redundant by the dedup check below.
                if node.kind() == KIND_CALL_EXPR {
                    last_suffix = node.call_fn_name(bytes);
                    last_suffix_resolved = current_type.is_some();
                }
            }
            NavSegment::Suffix {
                ref name,
                safe_call,
            } => {
                if *safe_call {
                    if let Some(ref mut t) = current_type {
                        if t.ends_with('?') {
                            t.pop();
                        }
                    }
                }
                last_suffix_resolved = false;
                if let Some(ref cur) = current_type {
                    if let Some(resolved) = resolve_member_type_on(cur, name, deps, uri) {
                        current_type = Some(resolved);
                        last_suffix_resolved = true;
                    } else if SCOPE_FUNCTIONS.contains(&name.as_str()) {
                        // Scope function: receiver type flows through.
                    } else if strictness == SuffixStrictness::Fail {
                        return None;
                    }
                } else if strictness == SuffixStrictness::Fail {
                    return None;
                }
                last_suffix = Some(name.clone());
            }
            NavSegment::CallExpr(call_node) => {
                let fn_name = call_node.call_fn_name(bytes);
                if let Some(ref name) = fn_name {
                    if SCOPE_FUNCTIONS.contains(&name.as_str()) {
                        continue;
                    }
                    // If the preceding Suffix already resolved this method's return type,
                    // the CallExpr is redundant — skip re-resolution.
                    if last_suffix.as_deref() == Some(name.as_str()) && last_suffix_resolved {
                        continue;
                    }
                    if let Some(ref cur) = current_type {
                        if let Some(resolved) = resolve_member_type_on(cur, name, deps, uri) {
                            current_type = Some(resolved);
                            continue;
                        }
                    }
                    if let Some(ret_ty) = deps.find_fun_return_type(name, uri) {
                        let ret_base = ret_ty.strip_nullable().dotted_ident_prefix();
                        let ret_base = ret_base.trim_end_matches('.');
                        if !is_generic_param(ret_base) {
                            current_type = Some(ret_ty);
                            continue;
                        }
                        // Generic return type — fall through to first_type_arg_raw fallback.
                    } else if let Some(class_name) = enclosing_class_name(*call_node, bytes) {
                        if let Some(ret_ty) =
                            deps.find_method_return_type_for_type(&class_name, name, uri)
                        {
                            current_type = Some(ret_ty);
                            continue;
                        }
                    }
                    // Method not indexed (or returns generic): use first concrete type arg
                    // of the receiver as a best-effort return type.  Only applies when the
                    // receiver has exactly one type parameter (e.g. `Optional<T>`) — for
                    // multi-param types like `Map<String, Order>` the first arg would be
                    // wrong (it would infer `String` instead of `Order`).
                    let is_single_param = current_type
                        .as_deref()
                        .and_then(type_args_inner)
                        .is_some_and(|inner| split_top_level_commas(inner).len() == 1);
                    if is_single_param {
                        if let Some(first_arg) =
                            current_type.as_deref().and_then(first_type_arg_raw)
                        {
                            if first_arg.starts_with_uppercase() && !is_generic_param(&first_arg) {
                                current_type = Some(first_arg);
                            }
                        }
                    }
                }
            }
        }
    }

    // Return (type_before_last_suffix, last_suffix_name)
    let method = last_suffix?;
    let receiver_type = current_type?;
    Some((receiver_type, method))
}

/// How `resolve_callee_chain` must treat a navigation chain's final segment
/// to report `(receiver_type, method_name)` correctly.
///
/// A chain's final segment plays one of two distinct roles, and
/// `forward_resolve_segments`'s Suffix handling treats them very
/// differently: it tries `resolve_member_type_on` first and only falls back
/// to "flow the receiver type through unchanged" for names in
/// `SCOPE_FUNCTIONS`. Deciding which role applies up front — instead of
/// always walking the full list and hoping the fallback fires — is what this
/// type exists to make explicit.
enum FinalCalleeSegment<'segments, 'node> {
    /// The final segment is a plain (non-scope-function) method name being
    /// invoked as the call itself (e.g. "map" in `container.items.map { }`),
    /// not a member to fold into the receiver's type. It must never reach
    /// `forward_resolve_segments`'s member-lookup: were it walked, a
    /// same-named field or method on the receiver would be resolved as if
    /// it were a further navigation step, corrupting the receiver type.
    /// Resolve `receiver_segments` alone to get `receiver_type`, and report
    /// `method_name` as-is.
    CallTarget {
        receiver_segments: &'segments [NavSegment<'node>],
        method_name: String,
    },
    /// The final segment must be visited by a full forward walk over every
    /// segment, not excluded: either it is a scope function
    /// (`let`/`also`/`run`/`apply`/`takeIf`/`takeUnless`), whose Suffix
    /// handling both flows the receiver type through unchanged *and* applies
    /// side effects tied to actually visiting that segment (e.g.
    /// `?.`-driven nullability stripping) — dropping it via slicing silently
    /// loses those effects, which is what broke
    /// `nullable_let_chain_it_type_resolves`,
    /// `this_type_apply_on_constructor_call_infers_receiver`, and 7 other
    /// scope-function-terminated chain tests the one time this was tried —
    /// or the chain's last element isn't a Suffix at all (`Root`/`CallExpr`).
    /// `forward_resolve_segments` already reports `(receiver_type,
    /// method_name)` for the whole chain, so hand it the untouched list.
    WalkFull,
}

/// Classify how `resolve_callee_chain` should treat `segments`' final entry.
/// See `FinalCalleeSegment` for what each outcome means and why.
///
/// This classification must NOT be pushed down into
/// `forward_resolve_segments`/`resolve_segments_type` themselves: their
/// "resolve every segment I'm handed" contract is relied on elsewhere (see
/// mod_tests.rs's `unresolved_final_suffix_fails_the_strict_walk`) and by
/// `resolve_call_expr_type`'s own already-correct pre-sliced call.
fn classify_final_callee_segment<'segments, 'node>(
    segments: &'segments [NavSegment<'node>],
) -> FinalCalleeSegment<'segments, 'node> {
    match segments.last() {
        Some(NavSegment::Suffix { name, .. }) if !SCOPE_FUNCTIONS.contains(&name.as_str()) => {
            FinalCalleeSegment::CallTarget {
                receiver_segments: &segments[..segments.len() - 1],
                method_name: name.clone(),
            }
        }
        _ => FinalCalleeSegment::WalkFull,
    }
}

/// Resolve the callee navigation chain left-to-right, returning the type
/// of the expression before the final method call, and the final method name.
///
/// For `settings.familyCreationDate?.let`:
///   - root = "settings" → type "IFamilySettings"
///   - ".familyCreationDate" → type "Long" (field on IFamilySettings)
///   - returns ("Long", "let")
pub(super) fn resolve_callee_chain(
    callee: tree_sitter::Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<(String, String)> {
    match callee.kind() {
        k if k == KIND_NAV_EXPR => {
            let segments = collect_nav_segments(callee, bytes);
            if segments.is_empty() {
                return None;
            }
            match classify_final_callee_segment(&segments) {
                FinalCalleeSegment::CallTarget {
                    receiver_segments,
                    method_name,
                } => {
                    let receiver_type = resolve_segments_type(
                        receiver_segments,
                        bytes,
                        deps,
                        uri,
                        SuffixStrictness::LeakReceiver,
                    )?;
                    Some((receiver_type, method_name))
                }
                FinalCalleeSegment::WalkFull => forward_resolve_segments(
                    &segments,
                    bytes,
                    deps,
                    uri,
                    SuffixStrictness::LeakReceiver,
                ),
            }
        }
        k if k == KIND_SIMPLE_IDENT || k == KIND_TYPE_IDENT => {
            let name = callee.utf8_text_owned(bytes)?;
            None.or_else(|| {
                let _ = name;
                None
            })
        }
        // `receiver.method(args) { lambda }` nests as
        // `outer_call(inner_call(receiver.method, args), call_suffix{lambda})`.
        // The receiver chain lives on the inner call's own callee — unwrap one level
        // so `items.mapNotNull(::transform) { it }` still resolves `items`'s type.
        k if k == KIND_CALL_EXPR => {
            let inner_callee = callee.child(0)?;
            resolve_callee_chain(inner_callee, bytes, deps, uri)
        }
        _ => None,
    }
}

/// Forward-walk chain resolution for the receiver type of a lambda's enclosing
/// call expression. Given `a.b.method { lambda }`, resolves left-to-right:
///   1. Find root identifier (`a`) → resolve its type
///   2. Walk through each navigation suffix (`.b`, `.method`) tracking the type
///   3. Return the type of the expression just before the final method call
///
/// This handles arbitrary chains like `settings.familyCreationDate?.let { }`
/// without backward heuristics.
pub(super) fn cst_forward_resolve_receiver_type(
    lambda: &tree_sitter::Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    let call_expr = lambda.enclosing_call_expression()?;
    let callee = call_expr.child(0)?;

    // Collect the chain segments: (root_node, [suffix_member_names])
    // For `settings.familyCreationDate?.let`, we get:
    //   root = "settings", segments = ["familyCreationDate", "let"]
    let (root_type, final_method) = resolve_callee_chain(callee, bytes, deps, uri)?;

    // For scope functions, `it` type IS the receiver type (`user.let { it }` → User).
    if SCOPE_FUNCTIONS.contains(&final_method.as_str()) {
        return Some(root_type);
    }
    // Otherwise, when the receiver itself is a known collection type, `it` is its
    // element type (`items: List<Product>` → `Product` for `forEach`/`map`/…).
    // The decision is on the receiver *type*, not the method name — keyed off
    // the collection type. Non-collection receivers yield `None` (unchanged).
    extract_collection_element_type(&root_type)
}

/// Given a current receiver type string, resolve a member access (field or method) and
/// return the resulting type with type substitution applied.
///
/// When `build_type_arg_subst` returns an empty map because the class type params are
/// not in the index, `apply_type_subst` leaves the generic placeholder (e.g. `T`) intact.
/// In that case we fall back to `first_concrete_type_arg_str` — the same strategy used
/// by the text path in `chain_with_type_subst` — to extract the first concrete type
/// argument from `current_type`.  This prevents `:T` from leaking through as a hover
/// result for chains like `resultState.value.getOrNull()?.also { param -> }` when
/// `ResultState.Success` type params are not indexed.
pub(super) fn resolve_member_type_on(
    current_type: &str,
    member: &str,
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    let type_name = current_type.dotted_ident_prefix();
    let type_base = type_name.last_segment();
    let effective_type = if !type_base.is_empty() && type_base.starts_with_uppercase() {
        type_base.to_owned()
    } else if !type_base.is_empty() {
        capitalize_first_char(type_base)
    } else {
        return None;
    };
    if let Some(field_ty) = deps.find_field_type(&effective_type, member) {
        let subst = build_type_arg_subst(deps, &effective_type, current_type);
        let applied = crate::indexer::apply_type_subst(&field_ty, &subst);
        if is_generic_param(applied.strip_nullable()) {
            return first_type_arg_raw(current_type);
        }
        return Some(applied);
    }
    if let Some(ret_ty) = deps.find_method_return_type_for_type(&effective_type, member, uri) {
        let subst = build_type_arg_subst(deps, &effective_type, current_type);
        let applied = crate::indexer::apply_type_subst(&ret_ty, &subst);
        if is_generic_param(applied.strip_nullable()) {
            return first_type_arg_raw(current_type);
        }
        return Some(applied);
    }
    None
}

/// Walk up from a node to find the enclosing class/object declaration name.
pub(super) fn enclosing_class_name(node: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut cur = node;
    loop {
        match cur.kind() {
            KIND_CLASS_DECL | KIND_OBJECT_DECL => {
                return cur.extract_type_name(bytes);
            }
            _ => {
                cur = cur.parent()?;
            }
        }
    }
}

/// Resolve the type of a root node (identifier, navigation_expression for dotted access).
pub(super) fn resolve_root_node_type(
    node: tree_sitter::Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    match node.kind() {
        k if k == KIND_SIMPLE_IDENT || k == KIND_TYPE_IDENT => {
            let name = node.utf8_text_owned(bytes)?;
            if let Some(raw) = deps.find_var_type(&name, uri) {
                // Validate it is a type name (starts with uppercase, not a generic
                // placeholder like `T`), then return the FULL raw string including
                // generics so that downstream `build_type_arg_subst` can extract
                // type arguments (e.g. `ResultState.Success<Optional<FamilyAccount>>`).
                let base = raw.ident_prefix();
                if !base.is_empty() && !is_generic_param(&base) && base.starts_with_uppercase() {
                    return Some(raw);
                }
            }
            // Implicit lambda parameters ("it", "this") are not declared as variables
            // — resolve them via contextual lambda-param inference at their position.
            if name == "it" || name == "this" {
                let start = node.start_position();
                let utf16_col =
                    crate::inlay_hints::ts_byte_col_to_utf16(bytes, &[], start.row, start.column);
                if let Some(resolved) = deps.find_contextual_type(&name, uri, start.row, utf16_col)
                {
                    return Some(resolved);
                }
            }
            Some(name)
        }
        k if k == KIND_NAV_EXPR => {
            let segments = collect_nav_segments(node, bytes);
            resolve_segments_type(&segments, bytes, deps, uri, SuffixStrictness::Fail)
        }
        k if k == KIND_CALL_EXPR => resolve_call_expr_type(node, bytes, deps, uri),
        _ => None,
    }
}

// ─── resolve_call_expr_type: strategy list ───────────────────────────────────
//
// This resolves "what does calling `fn_name(...)` produce" through a fixed,
// ORDERED list of strategies (`strategies` below), each strictly more
// authoritative than the ones after it. That order is the whole contract —
// every entry exists because a production bug proved it must run exactly
// where it sits, not earlier and not later:
//   - authoritative-before-heuristic: a real, indexed declaration
//     (`receiver_based_method`/`reachable_return_type`) must win even when
//     its name coincides with a curated heuristic pattern.
//   - heuristic-before-broad-scan: the curated heuristics
//     (`numeric_conversion`/`di_factory_type_argument`/`retrofit_class_literal`)
//     must run before `global_name_scan`, whose bare-name match has no
//     regard for import reachability and can hit a same-named, unrelated
//     symbol anywhere in the workspace.
// Regression tests pinning each ordering live in
// `src/resolver/infer_tests.rs` (`di_factory_heuristic_does_not_override_*`,
// `class_literal_*_does_not_override_*`, `class_literal_arg_fallback_not_shadowed_by_*`,
// `numeric_conversion_heuristic_does_not_fire_on_receiver_less_call`).
//
// Adding a new strategy means picking its slot in `strategies` deliberately
// — that placement IS the review question, not an incidental `if` position.

/// Inputs every strategy reads, computed once so no strategy re-derives
/// `fn_name`/`callee` (and risks a subtly different re-derivation).
struct CallCtx<'a, D: InferDeps> {
    node: tree_sitter::Node<'a>,
    bytes: &'a [u8],
    deps: &'a D,
    uri: &'a Url,
    fn_name: &'a str,
    callee: tree_sitter::Node<'a>,
}

/// What a strategy found, and what post-processing it still needs before
/// becoming the final answer. Keeping this as data (rather than each
/// strategy applying its own substitution inline) is what makes call-site
/// type-argument substitution apply uniformly to every strategy that needs
/// it: no strategy can be added ahead of an existing one and forget it — the
/// bug fixed by `receiver_based_resolution_also_substitutes_call_site_type_argument`.
enum StrategyOutcome {
    /// A signature's raw return type resolved against a concrete receiver —
    /// needs receiver-generic substitution, then call-site substitution.
    ReceiverDerived {
        raw_return: String,
        effective_type: String,
        receiver_type: String,
    },
    /// A signature's raw return type resolved with no receiver in play —
    /// needs call-site substitution only.
    SignatureDerived(String),
    /// Already the final, concrete answer (fixed stdlib type, a call-site
    /// type argument read directly, a class-literal argument, the bare
    /// constructor name, or a scope-function's passthrough) — no
    /// substitution applies.
    Final(String),
}

impl StrategyOutcome {
    fn finalize<D: InferDeps>(self, ctx: &CallCtx<'_, D>) -> String {
        match self {
            StrategyOutcome::Final(s) => s,
            StrategyOutcome::SignatureDerived(raw) => apply_call_site_type_args(raw, ctx),
            StrategyOutcome::ReceiverDerived {
                raw_return,
                effective_type,
                receiver_type,
            } => {
                let subst = build_type_arg_subst(ctx.deps, &effective_type, &receiver_type);
                let substituted = crate::indexer::apply_type_subst(&raw_return, &subst);
                apply_call_site_type_args(substituted, ctx)
            }
        }
    }
}

/// A strategy's verdict for `ctx.fn_name(...)`.
enum StrategyVerdict {
    /// This strategy's precondition doesn't match — try the next strategy.
    NotApplicable,
    /// This strategy's precondition matched; its answer — `Some` or `None`
    /// — is FINAL. No later, less-authoritative strategy may run instead
    /// (e.g. a scope-function name whose receiver type didn't resolve must
    /// not fall through and match some unrelated same-named function).
    Terminal(Option<StrategyOutcome>),
}

/// Substitute the CALLED FUNCTION's own generic type parameter(s) from an
/// explicit call-site type argument (`filterIsInstance<Foo>()`'s own `<T>`)
/// — distinct from receiver-type substitution, which only covers the
/// receiver's own generic argument. Real example: `fun <R> Flow<*>.filterIsInstance(): Flow<R>`
/// — `R` is the function's own type parameter, supplied only by the
/// caller's explicit `<T>`, never derivable from the (possibly
/// star-projected) receiver. The one place this runs, so every strategy
/// that goes through `StrategyOutcome::finalize` gets it for free.
fn apply_call_site_type_args<D: InferDeps>(ret: String, ctx: &CallCtx<'_, D>) -> String {
    let Some(call_type_args) = ctx.node.call_site_type_arg_strings(ctx.bytes) else {
        return ret;
    };
    let Some(callable_info) = ctx.deps.find_fun_callable_info(ctx.fn_name, ctx.uri) else {
        return ret;
    };
    if callable_info.type_params.is_empty() {
        return ret;
    }
    let fn_subst = build_fn_subst(&callable_info.type_params, &call_type_args);
    apply_simple_subst(&ret, &fn_subst)
}

/// Scope functions (`let`/`also`/`run`/`apply`/`takeIf`/`takeUnless`): the
/// call's type isn't a return-type lookup at all — it's the receiver's own
/// type flowing through unchanged. Exclusive: a workspace's own unrelated
/// `let`/`also` must never override stdlib scope-function semantics.
fn scope_function_identity<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    if !SCOPE_FUNCTIONS.contains(&ctx.fn_name) {
        return StrategyVerdict::NotApplicable;
    }
    let resolved_type = resolve_root_node_type(ctx.callee, ctx.bytes, ctx.deps, ctx.uri);
    StrategyVerdict::Terminal(resolved_type.map(StrategyOutcome::Final))
}

/// Lambda-result functions (e.g. Compose `remember { Foo() }`) return their
/// trailing lambda's value. Exclusive, for the same reason as scope
/// functions: falling through would let the global same-name lookup pick an
/// unrelated overload (the Kotlin compiler ships an internal `remember`
/// returning `RealVariable`).
fn lambda_result<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    if !LAMBDA_RESULT_FNS.contains(&ctx.fn_name) {
        return StrategyVerdict::NotApplicable;
    }
    let resolved_type = infer_lambda_result_type(ctx.node, ctx.bytes, ctx.deps, ctx.uri);
    StrategyVerdict::Terminal(resolved_type.map(StrategyOutcome::Final))
}

/// The receiver's own (indexed) type has a matching method — the most
/// authoritative signature-based strategy.
fn receiver_based_method<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    let receiver_type = if ctx.callee.kind() == KIND_NAV_EXPR {
        let segments = collect_nav_segments(ctx.callee, ctx.bytes);
        if segments.len() >= 2 {
            resolve_segments_type(
                &segments[..segments.len() - 1],
                ctx.bytes,
                ctx.deps,
                ctx.uri,
                SuffixStrictness::LeakReceiver,
            )
        } else {
            None
        }
    } else {
        resolve_root_node_type(ctx.callee, ctx.bytes, ctx.deps, ctx.uri)
    };
    let Some(receiver_type) = receiver_type else {
        return StrategyVerdict::NotApplicable;
    };
    let type_base = receiver_type
        .dotted_ident_prefix()
        .last_segment()
        .to_owned();
    let effective_type = if type_base.starts_with_uppercase() {
        type_base
    } else {
        capitalize_first_char(&type_base)
    };
    if effective_type.is_empty() {
        return StrategyVerdict::NotApplicable;
    }
    let Some(raw_return) =
        ctx.deps
            .find_method_return_type_for_type(&effective_type, ctx.fn_name, ctx.uri)
    else {
        return StrategyVerdict::NotApplicable;
    };
    StrategyVerdict::Terminal(Some(StrategyOutcome::ReceiverDerived {
        raw_return,
        effective_type,
        receiver_type,
    }))
}

/// The import-aware lookup: binds to the symbol actually reachable from this
/// file (same-package / imported), so a real declaration wins even when its
/// name coincides with a curated heuristic name below.
fn reachable_return_type<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    match ctx
        .deps
        .find_fun_return_type_reachable(ctx.fn_name, ctx.uri)
    {
        Some(raw) => StrategyVerdict::Terminal(Some(StrategyOutcome::SignatureDerived(raw))),
        None => StrategyVerdict::NotApplicable,
    }
}

/// `x.toLong()`/`.toInt()`/etc. — the function name alone fixes the return
/// type (see `NUMERIC_CONVERSION_FNS`'s doc comment) once the reachable
/// lookup above has already failed. Gated on `callee.kind() == KIND_NAV_EXPR`
/// (an actual `.` receiver in the source, even if its type didn't resolve):
/// a genuinely receiver-less `toLong()` naming an unrelated top-level
/// function must not be guessed as the stdlib intrinsic.
fn numeric_conversion<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    if ctx.callee.kind() != KIND_NAV_EXPR {
        return StrategyVerdict::NotApplicable;
    }
    match NUMERIC_CONVERSION_FNS
        .iter()
        .find(|(name, _)| *name == ctx.fn_name)
    {
        Some((_, return_type)) => {
            StrategyVerdict::Terminal(Some(StrategyOutcome::Final((*return_type).to_owned())))
        }
        None => StrategyVerdict::NotApplicable,
    }
}

/// `get<Foo>()`/`inject<Foo>()`/etc. — read the type argument straight off
/// the call site once the reachable lookup above has already failed. Gated
/// on both a known factory-function name AND an explicit `<T>` at the call
/// site, so it's a strong, self-verifying signal, not a guess.
fn di_factory_type_argument<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    if !GENERIC_FACTORY_FNS.contains(&ctx.fn_name) {
        return StrategyVerdict::NotApplicable;
    }
    let Some(type_args) = ctx.node.call_site_type_arg_strings(ctx.bytes) else {
        return StrategyVerdict::NotApplicable;
    };
    let [single] = type_args.as_slice() else {
        return StrategyVerdict::NotApplicable;
    };
    if !single.starts_with_uppercase() {
        return StrategyVerdict::NotApplicable;
    }
    StrategyVerdict::Terminal(Some(StrategyOutcome::Final(single.clone())))
}

/// Retrofit-style class-literal: `retrofit.create(Foo::class.java)` with
/// neither the receiver's type nor `create` itself indexed. The argument
/// itself names the answer — see `find_class_literal_arg_type`. Also gated
/// on `GENERIC_FACTORY_FNS` (same known-factory-name list as above): without
/// it, this would wrongly override a real, correctly-indexed,
/// differently-named function that merely happens to take a class-literal
/// argument for an unrelated reason (e.g. a logging/reflection helper).
fn retrofit_class_literal<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    if !GENERIC_FACTORY_FNS.contains(&ctx.fn_name) {
        return StrategyVerdict::NotApplicable;
    }
    match find_class_literal_arg_type(ctx.node, ctx.bytes) {
        Some(class_literal_type) => {
            StrategyVerdict::Terminal(Some(StrategyOutcome::Final(class_literal_type)))
        }
        None => StrategyVerdict::NotApplicable,
    }
}

/// The receiver-agnostic global-name scan — tried only when NOTHING above
/// found anything. A same-named match here has no regard for import/package
/// reachability (a real production bug had this scan bare-name-match an
/// unrelated `create` on a completely different, unimported class — a KSP
/// `SymbolProcessorProvider.create(): SymbolProcessor` — before the
/// heuristics above got a chance to run, back when they were ordered after
/// this scan instead of before it).
fn global_name_scan<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    match ctx.deps.find_fun_return_type(ctx.fn_name, ctx.uri) {
        Some(raw) => StrategyVerdict::Terminal(Some(StrategyOutcome::SignatureDerived(raw))),
        None => StrategyVerdict::NotApplicable,
    }
}

/// Constructor fallback: `Foo(...)` with no resolvable function return type
/// is a constructor call whose type is `Foo`. Only when the callee is a bare
/// (unqualified or dotted) identifier whose leaf starts uppercase.
fn constructor_fallback<D: InferDeps>(ctx: &CallCtx<'_, D>) -> StrategyVerdict {
    let is_bare_or_dotted_ident = matches!(
        ctx.callee.kind(),
        k if k == KIND_SIMPLE_IDENT || k == KIND_NAV_EXPR || k == KIND_TYPE_IDENT
    );
    if !ctx.fn_name.starts_with_uppercase() || !is_bare_or_dotted_ident {
        return StrategyVerdict::NotApplicable;
    }
    StrategyVerdict::Terminal(Some(StrategyOutcome::Final(ctx.fn_name.to_owned())))
}

pub(super) fn resolve_call_expr_type<D: InferDeps>(
    node: tree_sitter::Node<'_>,
    bytes: &[u8],
    deps: &D,
    uri: &Url,
) -> Option<String> {
    let fn_name = node.call_fn_name(bytes)?;
    // Guaranteed `Some`: `call_fn_name` itself reads `node.child(0)` to
    // produce a name, so a resolved `fn_name` implies a callee exists.
    let callee = node.child(0)?;
    let ctx = CallCtx {
        node,
        bytes,
        deps,
        uri,
        fn_name: fn_name.as_str(),
        callee,
    };

    let strategies: [fn(&CallCtx<'_, D>) -> StrategyVerdict; 9] = [
        scope_function_identity,
        lambda_result,
        receiver_based_method,
        reachable_return_type,
        numeric_conversion,
        di_factory_type_argument,
        retrofit_class_literal,
        global_name_scan,
        constructor_fallback,
    ];
    for strategy in strategies {
        if let StrategyVerdict::Terminal(outcome) = strategy(&ctx) {
            return outcome.map(|o| o.finalize(&ctx));
        }
    }
    None
}

/// Find a `Foo::class` (optionally `.java`-suffixed, optionally
/// package-qualified) argument inside a call's argument list and return
/// `Foo`.
///
/// Deliberately TEXTUAL, not a CST walk of the argument's structure --
/// mirroring `infer_lines::infer_from_rhs_assignment`'s "Pattern 3" exactly
/// (same substring search), for a concrete reason: a bare `Foo::class`
/// parses as a single `callable_reference(type_identifier)` node, but a
/// qualified `com.example.Foo::class` does NOT -- tree-sitter-kotlin instead
/// produces a nested `navigation_expression` chain (`com.example.Foo` as
/// dotted navigation suffixes, with `::class` itself becoming just another
/// suffix carrying the literal `class` keyword token, not a
/// `callable_reference` at all). Chasing that shape recursively is much more
/// fragile than reading the argument list's own text and applying the same
/// substring search already proven correct on the STRING side.
///
/// Unconditional on the callee/receiver resolving to anything, since the
/// pattern's whole point is recovering a type when the actual
/// `create`/factory method is not indexed (e.g. an external Retrofit-style
/// library). Returns the first class-literal argument found; callers only
/// reach this after every signature-based resolution has failed.
fn find_class_literal_arg_type(call: tree_sitter::Node<'_>, bytes: &[u8]) -> Option<String> {
    let call_suffix = call.first_child_of_kind(KIND_CALL_SUFFIX)?;
    let value_args = call_suffix.first_child_of_kind(KIND_VALUE_ARGS)?;
    let args_text = value_args.utf8_text(bytes).ok()?;
    let class_pos = args_text.find("::class")?;
    let before_class = args_text[..class_pos].trim_end();
    let leaf = before_class
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next_back()
        .unwrap_or("");
    if leaf.starts_with_uppercase() {
        Some(leaf.to_owned())
    } else {
        None
    }
}

/// Locate the trailing `lambda_literal` of a call expression, handling the
/// `call_suffix → annotated_lambda → lambda_literal` nesting that tree-sitter
/// produces for `f { … }` and `f(args) { … }`.
fn find_trailing_lambda_literal(call: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut call_cursor = call.walk();
    for child in call.children(&mut call_cursor) {
        if child.kind() == KIND_LAMBDA_LIT {
            return Some(child);
        }
        if child.kind() == KIND_CALL_SUFFIX {
            let mut suffix_cursor = child.walk();
            for suffix_child in child.children(&mut suffix_cursor) {
                if suffix_child.kind() == KIND_LAMBDA_LIT {
                    return Some(suffix_child);
                }
                // call_suffix → annotated_lambda → lambda_literal
                let mut annotated_cursor = suffix_child.walk();
                for annotated_child in suffix_child.children(&mut annotated_cursor) {
                    if annotated_child.kind() == KIND_LAMBDA_LIT {
                        return Some(annotated_child);
                    }
                }
            }
        }
    }
    None
}

/// Infer the type of a lambda-result call (`remember { … }`) as the type of the
/// trailing lambda's last expression. Returns `None` for an empty lambda or when
/// the last expression's type can't be determined.
fn infer_lambda_result_type(
    call: tree_sitter::Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    let lambda = find_trailing_lambda_literal(call)?;
    let statements = lambda.first_child_of_kind(KIND_STATEMENTS)?;
    let count = statements.named_child_count();
    if count == 0 {
        return None;
    }
    let last = statements.named_child(count - 1)?;
    resolve_root_node_type(last, bytes, deps, uri)
}

/// Resolve a chain of segments to a type (without returning method name).
/// Used when we need just the final type after processing all segments.
pub(super) fn resolve_segments_type(
    segments: &[NavSegment<'_>],
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    strictness: SuffixStrictness,
) -> Option<String> {
    if segments.is_empty() {
        return None;
    }
    // If there's just a root, resolve it directly.
    if segments.len() == 1 {
        if let NavSegment::Root(node) = &segments[0] {
            return resolve_root_node_type(*node, bytes, deps, uri);
        }
    }
    // Otherwise use forward_resolve_segments which returns (final_type, last_suffix).
    // The final type after all segments is what we want.
    forward_resolve_segments(segments, bytes, deps, uri, strictness)
        .map(|(resolved_type, _)| resolved_type)
}
