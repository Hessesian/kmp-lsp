//! Infer the Kotlin type of a CST expression node.
//!
//! Handles the cases that are knowable without a compiler:
//!
//! | Node kind                 | Inferred type          |
//! |---------------------------|------------------------|
//! | `integer_literal`         | `Int`                  |
//! | `long_literal`            | `Long`                 |
//! | `real_literal`            | `Float` or `Double`    |
//! | `string_literal`          | `String`               |
//! | `boolean_literal`         | `Boolean`              |
//! | `null`                    | `Nothing?`             |
//! | `character_literal`       | `Char`                 |
//! | `call_expression`         | return type from index |
//! | `check_expression`        | `Boolean`              |
//! | `comparison_expression`   | `Boolean`              |
//! | `disjunction_expression`  | `Boolean`              |
//! | `conjunction_expression`  | `Boolean`              |
//! | `prefix_expression` (`!`) | `Boolean`              |
//! | `if_expression`           | type when both branches agree |
//! | `range_expression` (int)  | `IntRange`             |
//! | `as_expression` (`as`/`as?`) | the cast's own target type |
//! | `simple_identifier`       | variable type from index |
//! | `type_identifier`         | variable type from index |
//! | `navigation_expression`   | field/method return type from index |
//! | `this_expression`         | contextual lambda-receiver type |
//!
//! Simple identifiers, navigation expressions (e.g. `list.size`), and `this`
//! are now resolved through the `InferDeps` seam. `when` expressions and other
//! compound forms are not resolved — callers receive `None` and can omit the
//! type annotation.

use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::node_ext::NodeExt;
use crate::queries::{
    KIND_ADDITIVE_EXPR, KIND_AS_EXPR, KIND_BOOLEAN_LITERAL, KIND_CALL_EXPR, KIND_CHARACTER_LITERAL,
    KIND_CHECK_EXPR, KIND_COMPARISON_EXPR, KIND_CONJUNCTION_EXPR, KIND_CONTROL_STRUCTURE_BODY,
    KIND_DISJUNCTION_EXPR, KIND_ELSE, KIND_ELVIS_EXPR, KIND_IF_EXPR, KIND_INTEGER_LITERAL,
    KIND_JUMP_EXPR, KIND_KW_SAFE_CALL, KIND_LONG_LITERAL, KIND_MULTILINE_STRING_LITERAL,
    KIND_MULTIPLICATIVE_EXPR, KIND_NAV_EXPR, KIND_NAV_SUFFIX, KIND_NULL_LITERAL,
    KIND_PARENTHESIZED_EXPR, KIND_PREFIX_EXPR, KIND_RANGE_EXPR, KIND_REAL_LITERAL,
    KIND_SIMPLE_IDENT, KIND_STRING_LITERAL, KIND_THIS_EXPR, KIND_TYPE_IDENT, KIND_VALUE_ARG,
};
use crate::StrExt as _;

use super::chain::{
    resolve_field_type_on, resolve_method_return_type_on, trailing_lambda_last_expression,
};
use super::cst_symbol::{local_binding_at, local_binding_of, DestructuredValue, LocalBinding};
use super::deps::{CallShape, InferDeps, ReceiverCall};
use super::lambda::LAMBDA_RESULT_FNS;
use super::type_subst::{build_ext_fn_type_subst, split_top_level_commas, type_args_inner};
use crate::detail_text::ReturnNullability;

// ─── public API ───────────────────────────────────────────────────────────────

/// Infer the Kotlin type of `node` as a human-readable string (e.g. `"Int"`).
///
/// Returns `None` when the type cannot be determined without compiler
/// type-resolution (e.g. navigation expressions, generic calls).
pub(crate) fn infer_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    infer_expr_type_at_depth(node, bytes, deps, uri, 0).map(|(type_name, _)| type_name)
}

/// [`infer_expr_type`] together with the file the answer was written in — the
/// anchor a lookup of the type's NAME must be resolved against. `uri` itself
/// when the type has no better origin (a literal, an unannotated local).
pub(crate) fn infer_expr_type_with_origin(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<(String, Url)> {
    infer_expr_type_at_depth(node, bytes, deps, uri, 0)
}

/// Depth-guarded implementation of [`infer_expr_type`].
///
/// `infer_expr_type` and its helpers (`infer_navigation_expr_type`,
/// `infer_if_expr_type`, `infer_range_expr_type`, `infer_arithmetic_expr_type`,
/// `infer_parenthesized_expr_type`) form a hand-rolled recursive descent over
/// the expression tree — the same pattern `MAX_CST_DESCENT_DEPTH` was
/// introduced for elsewhere (`indexer::infer::chain::collect_nav_segments`,
/// `indexer::cst_folding`, the `features::*_diagnostics` walkers). This one
/// was missed by that sweep: a pathologically long `a.b.c.d…` navigation
/// chain, `((((…))))` parenthesization, or `1+1+1+…` arithmetic chain — all
/// syntactically valid, no malformed input required — recurses one frame per
/// segment with no cap, reachable from `inlay_hints` and
/// `infer_variable_type_from_cst` (the CST fallback `InferDeps::find_var_type`
/// uses). Bail out (not error) past the cap, same trade-off every other
/// capped walker makes.
///
/// Returns the inferred type together with the `Url` that should anchor
/// reachability for a member access on that type — the receiver's own
/// declaring file when known (currently only `infer_navigation_expr_type`'s
/// field case produces one), `uri` unchanged otherwise.
fn infer_expr_type_at_depth(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("infer_expr_type_at_depth", node);
        return None;
    }
    match node.kind() {
        KIND_INTEGER_LITERAL => Some(("Int".to_owned(), uri.clone())),
        KIND_LONG_LITERAL => Some(("Long".to_owned(), uri.clone())),
        KIND_REAL_LITERAL => {
            infer_real_literal(node, bytes).map(|type_name| (type_name, uri.clone()))
        }
        KIND_STRING_LITERAL | KIND_MULTILINE_STRING_LITERAL => {
            Some(("String".to_owned(), uri.clone()))
        }
        KIND_BOOLEAN_LITERAL => Some(("Boolean".to_owned(), uri.clone())),
        KIND_NULL_LITERAL => Some(("Nothing?".to_owned(), uri.clone())),
        KIND_CHARACTER_LITERAL => Some(("Char".to_owned(), uri.clone())),
        k if k == KIND_SIMPLE_IDENT || k == KIND_TYPE_IDENT => {
            infer_ident_type(node, bytes, deps, uri, depth)
        }
        k if k == KIND_THIS_EXPR => {
            infer_this_expr_type(node, bytes, deps, uri).map(|type_name| (type_name, uri.clone()))
        }
        k if k == KIND_NAV_EXPR => infer_navigation_expr_type(node, bytes, deps, uri, depth),
        k if k == KIND_CALL_EXPR => infer_call_expr_type(node, bytes, deps, uri),
        k if k == KIND_CHECK_EXPR
            || k == KIND_COMPARISON_EXPR
            || k == KIND_DISJUNCTION_EXPR
            || k == KIND_CONJUNCTION_EXPR =>
        {
            Some(("Boolean".to_owned(), uri.clone()))
        }
        k if k == KIND_PREFIX_EXPR => {
            infer_prefix_expr_type(node, bytes).map(|type_name| (type_name, uri.clone()))
        }
        k if k == KIND_AS_EXPR => {
            infer_as_expr_type(node, bytes).map(|type_name| (type_name, uri.clone()))
        }
        k if k == KIND_IF_EXPR => infer_if_expr_type(node, bytes, deps, uri, depth)
            .map(|type_name| (type_name, uri.clone())),
        k if k == KIND_RANGE_EXPR => infer_range_expr_type(node, bytes, deps, uri, depth)
            .map(|type_name| (type_name, uri.clone())),
        k if k == KIND_ADDITIVE_EXPR || k == KIND_MULTIPLICATIVE_EXPR => {
            infer_arithmetic_expr_type(node, bytes, deps, uri, depth)
                .map(|type_name| (type_name, uri.clone()))
        }
        k if k == KIND_PARENTHESIZED_EXPR => {
            infer_parenthesized_expr_type(node, bytes, deps, uri, depth)
                .map(|type_name| (type_name, uri.clone()))
        }
        KIND_ELVIS_EXPR => infer_elvis_type(node, bytes, deps, uri, depth, TypeTextForm::BareName),
        _ => None,
    }
}

/// `left ?: right` is the non-null type of `left` when `right` cannot
/// complete normally (`Nothing`) or has exactly that type, type arguments
/// included. Otherwise it is their common supertype, which this engine does
/// not compute. `form` is how `left`'s type is reported: as an expression
/// (the same form `infer_expr_type` gives `left` alone) or as a local's type.
fn infer_elvis_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
    form: TypeTextForm,
) -> Option<(String, Url)> {
    let mut cursor = node.walk();
    let mut operands = node.named_children(&mut cursor);
    let left = operands.next()?;
    let right = operands.last()?;
    let (left_written, left_uri) = infer_initializer_type(left, bytes, deps, uri, depth + 1)?;
    let non_null_left = left_written.strip_nullable();
    let right_agrees = is_nothing_typed(right, bytes, deps, uri)
        || (reports_its_nullability(right, bytes, deps, uri, depth + 1)
            && infer_initializer_type(right, bytes, deps, uri, depth + 1).is_some_and(
                |(right_written, right_uri)| {
                    let same_scope = left_uri == right_uri;
                    right_written == non_null_left
                        && (same_scope
                            || deps.same_type_in_scopes(non_null_left, &left_uri, &right_uri))
                },
            ));
    if !right_agrees {
        return None;
    }
    match form {
        TypeTextForm::AsWritten => Some((non_null_left.to_owned(), left_uri)),
        TypeTextForm::BareName => {
            let (left_type, left_uri) =
                infer_expr_type_at_depth(left, bytes, deps, uri, depth + 1)?;
            Some((left_type.strip_nullable().to_owned(), left_uri))
        }
    }
}

/// Whether the type `infer_initializer_type` gives `expression` says if it
/// can be null: a binding reads as written (`Apple?`), literals and
/// constructor calls are never null, and a plain function call is as
/// nullable as its declared return type. Other calls and member accesses read
/// as a bare name that dropped its declared `?`, so they cannot establish
/// agreement.
fn reports_its_nullability(
    expression: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> bool {
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("reports_its_nullability", expression);
        return false;
    }
    match expression.kind() {
        KIND_SIMPLE_IDENT => binding_reports_its_nullability(expression, bytes, deps, uri, depth),
        KIND_ELVIS_EXPR | KIND_PARENTHESIZED_EXPR => true,
        KIND_STRING_LITERAL
        | KIND_MULTILINE_STRING_LITERAL
        | KIND_BOOLEAN_LITERAL
        | KIND_CHARACTER_LITERAL => true,
        KIND_CALL_EXPR => {
            let is_plain_call = expression
                .named_child(0)
                .is_some_and(|callee| callee.kind() == KIND_SIMPLE_IDENT);
            let Some(callee) = expression.call_fn_name(bytes).filter(|_| is_plain_call) else {
                return false;
            };
            let is_constructor =
                callee.starts_with_uppercase() && deps.has_type_definition(&callee);
            is_constructor
                || deps.find_fun_return_nullability(&callee, uri) == ReturnNullability::NonNull
        }
        _ => false,
    }
}

/// Whether the type `infer_initializer_type` gives the identifier `use_site`
/// says if it can be null. Only a binding that writes its type down, or is
/// initialized from an expression that itself reports its nullability, does:
/// `val fallback = nullableCall()` infers a bare `Apple`. Members and
/// delegated locals are not read, so they cannot establish agreement.
fn binding_reports_its_nullability(
    use_site: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> bool {
    match local_binding_at(use_site, bytes) {
        LocalBinding::Annotated { .. } | LocalBinding::Narrowed { .. } => true,
        LocalBinding::Initialized { initializer } => {
            reports_its_nullability(initializer, bytes, deps, uri, depth + 1)
        }
        LocalBinding::Delegated { .. }
        | LocalBinding::Iterated { .. }
        | LocalBinding::Destructured { .. }
        | LocalBinding::Untyped
        | LocalBinding::NotLocal => false,
    }
}

/// `return`/`throw`/`break`/`continue`, or a call to the stdlib's
/// `error(..)` / `TODO(..)` — not a reachable function of that name that
/// returns a value.
fn is_nothing_typed(expression: Node<'_>, bytes: &[u8], deps: &impl InferDeps, uri: &Url) -> bool {
    if expression.kind() == KIND_JUMP_EXPR {
        return true;
    }
    let is_call = expression.kind() == KIND_CALL_EXPR;
    let callee = expression
        .named_child(0)
        .filter(|callee| callee.kind() == KIND_SIMPLE_IDENT)
        .and_then(|callee| callee.utf8_text(bytes).ok());
    let Some(callee @ ("error" | "TODO")) = callee.filter(|_| is_call) else {
        return false;
    };
    let is_local_callable = !matches!(
        local_binding_of(callee, expression, bytes),
        LocalBinding::NotLocal
    );
    if is_local_callable {
        return false;
    }
    match deps.find_fun_return_type_reachable(callee, uri) {
        None => true,
        Some(return_type) => {
            return_type == "Nothing"
                && deps.find_fun_return_nullability(callee, uri) == ReturnNullability::NonNull
        }
    }
}

// ─── private helpers ──────────────────────────────────────────────────────────

/// `3.14f` / `3.14F` → `Float`; `3.14` → `Double`.
fn infer_real_literal(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let text = node.utf8_text(bytes).ok()?;
    if text.ends_with('f') || text.ends_with('F') {
        Some("Float".to_owned())
    } else {
        Some("Double".to_owned())
    }
}

/// Resolve the type of a `call_expression` node by delegating to the chain
/// resolution path used by hover and semantic tokens.
fn infer_call_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<(String, Url)> {
    super::chain::resolve_call_expr_type_with_origin(node, bytes, deps, uri)
}

/// Resolve the type of a `simple_identifier` or `type_identifier`.
///
/// Adapted from `semantic_tokens::resolve::identifier_type`.
/// Consults contextual lambda-param inference first (handles `it` / `this` /
/// named lambda params), then falls back to the declared-variable type.
/// As a last resort, a bare uppercase identifier that names a known type
/// (class, interface, enum, object, companion) resolves to itself — this
/// covers companion-object access like `Foo.CONSTANT` where `Foo` is the
/// receiver identifier.
fn infer_ident_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    infer_ident_type_in_form(node, bytes, deps, uri, depth, TypeTextForm::BareName)
}

/// How a type read from a declaration is reported.
#[derive(Clone, Copy)]
enum TypeTextForm {
    /// As an expression's type: an identifier's declared type as its bare
    /// name (`"List"` for `List<Apple>?`), which the expression-type
    /// consumers look a receiver's type up by.
    BareName,
    /// The text as written, for a local's binding type.
    AsWritten,
}

fn infer_ident_type_in_form(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
    form: TypeTextForm,
) -> Option<(String, Url)> {
    let name = node.utf8_text_owned(bytes)?;
    let start = node.start_position();
    let col = crate::text_utils::ts_byte_col_to_utf16(bytes, &[], start.row, start.column);
    let binding = local_binding_at(node, bytes);
    let binding_states_its_own_type = matches!(
        binding,
        LocalBinding::Annotated { .. }
            | LocalBinding::Initialized { .. }
            | LocalBinding::Narrowed { .. }
            | LocalBinding::Delegated { .. }
            | LocalBinding::Iterated { .. }
            | LocalBinding::Destructured { .. }
    );
    if !binding_states_its_own_type {
        if let Some(inferred) = deps.find_contextual_type(&name, uri, start.row, col) {
            return Some((inferred, uri.clone()));
        }
    }
    let declared = match infer_binding_type(binding, bytes, deps, uri, depth) {
        BindingType::Typed {
            type_text,
            declaring_uri,
        } => Some((type_text, declaring_uri)),
        BindingType::LocalUnknown => return None,
        BindingType::NotLocal => deps
            .find_var_type(&name, uri)
            .map(|type_text| (type_text, uri.clone())),
    };
    if let Some((type_text, declaring_uri)) = declared {
        let type_text = match form {
            TypeTextForm::BareName => type_text.bare_type_name()?,
            TypeTextForm::AsWritten => type_text,
        };
        return Some((type_text, declaring_uri));
    }
    if name.starts_with_uppercase() && deps.has_type_definition(&name) {
        return Some((name, uri.clone()));
    }
    None
}

/// What the declaration binding a name at one point says about its type.
pub(crate) enum BindingType {
    /// The binding's type as written or inferred, with its `?` and type
    /// arguments, and the file it was written in.
    Typed {
        type_text: String,
        declaring_uri: Url,
    },
    /// A local binds the name here, but its type is not written down (a
    /// lambda parameter, `for` variable, destructuring) or is an initializer
    /// this engine cannot type. A by-name lookup would find some *other*
    /// declaration of the name.
    LocalUnknown,
    /// No enclosing local scope binds the name: a member, top-level
    /// declaration, import, or inherited symbol, which a by-name lookup finds.
    NotLocal,
}

/// The type of the local `name` in scope at `point` — read from the
/// declaration that binds it *there*, not from the first declaration of that
/// name anywhere in the file.
pub(crate) fn infer_local_type_at(
    name: &str,
    point: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> BindingType {
    infer_binding_type(local_binding_of(name, point, bytes), bytes, deps, uri, 0)
}

/// The type a [`LocalBinding`] states, for [`infer_ident_type`] and
/// [`infer_local_type_at`].
fn infer_binding_type(
    binding: LocalBinding<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> BindingType {
    let typed = match binding {
        LocalBinding::Annotated {
            declared_type: written_type,
        }
        | LocalBinding::Narrowed {
            narrowed_type: written_type,
        } => written_type
            .utf8_text_owned(bytes)
            .map(|type_text| (type_text, uri.clone())),
        LocalBinding::Initialized { initializer } => {
            infer_initializer_type(initializer, bytes, deps, uri, depth + 1)
        }
        LocalBinding::Delegated { delegate } => {
            infer_delegated_value_type(delegate, bytes, deps, uri, depth + 1)
        }
        LocalBinding::Iterated { iterable } => {
            infer_iterated_element_type(iterable, bytes, deps, uri, depth + 1)
        }
        LocalBinding::Destructured { whole, component } => {
            let whole_type = match whole {
                DestructuredValue::Initializer(initializer) => {
                    infer_initializer_type(initializer, bytes, deps, uri, depth + 1)
                }
                DestructuredValue::ElementOf(iterable) => {
                    infer_iterated_element_type(iterable, bytes, deps, uri, depth + 1)
                }
            };
            whole_type.and_then(|(whole_type, whole_uri)| {
                destructured_component_type(&whole_type, component)
                    .map(|component_type| (component_type, whole_uri))
            })
        }
        LocalBinding::Untyped => None,
        LocalBinding::NotLocal => return BindingType::NotLocal,
    };
    match typed {
        Some((type_text, declaring_uri)) => BindingType::Typed {
            type_text,
            declaring_uri,
        },
        None => BindingType::LocalUnknown,
    }
}

/// The type of one element of `iterable`, as a `for` loop reads it.
fn infer_iterated_element_type(
    iterable: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    let (iterable_type, iterable_uri) = infer_initializer_type(iterable, bytes, deps, uri, depth)?;
    iterated_element_type(&iterable_type).map(|element_type| (element_type, iterable_uri))
}

/// The type of the `component`-th (0-based) `componentN()` of `whole_type`,
/// for the types whose components are their type arguments: `Pair`,
/// `Triple` and `Map.Entry`. Unknown for anything else, including data
/// classes (their components are primary-constructor properties, which
/// this engine does not read).
fn destructured_component_type(whole_type: &str, component: usize) -> Option<String> {
    let holder = whole_type.bare_type_name()?;
    let arity = match holder.as_str() {
        "Map.Entry" | "MutableMap.MutableEntry" => 2,
        _ => match holder.last_segment() {
            "Pair" => 2,
            "Triple" => 3,
            _ => return None,
        },
    };
    let arguments = split_top_level_commas(type_args_inner(whole_type)?);
    if arguments.len() != arity {
        return None;
    }
    let written_argument = arguments.get(component)?.trim();
    let reads_as_unknown = written_argument == "*" || written_argument.starts_with("in ");
    if reads_as_unknown {
        return None;
    }
    let argument = written_argument.strip_variance();
    (!argument.is_empty()).then(|| argument.to_owned())
}

/// The element a `for` loop reads from an iterable of type `iterable_type`:
/// a range's or primitive array's scalar, a map's `Map.Entry<K, V>`, or a
/// collection's type argument. Unknown for anything else.
fn iterated_element_type(iterable_type: &str) -> Option<String> {
    let holder = iterable_type.bare_type_name()?;
    let scalar = match holder.last_segment() {
        "IntRange" | "IntProgression" | "IntArray" => Some("Int"),
        "LongRange" | "LongProgression" | "LongArray" => Some("Long"),
        "CharRange" | "CharProgression" | "CharArray" => Some("Char"),
        "BooleanArray" => Some("Boolean"),
        "ByteArray" => Some("Byte"),
        "ShortArray" => Some("Short"),
        "FloatArray" => Some("Float"),
        "DoubleArray" => Some("Double"),
        _ => None,
    };
    if let Some(scalar) = scalar {
        return Some(scalar.to_owned());
    }
    let is_map = matches!(
        holder.last_segment(),
        "Map" | "MutableMap" | "HashMap" | "LinkedHashMap" | "ImmutableMap" | "PersistentMap"
    );
    if is_map {
        let key_and_value = type_args_inner(iterable_type)?;
        return Some(format!("Map.Entry<{key_and_value}>"));
    }
    if !crate::resolver::is_collection_constructor(&holder) {
        return None;
    }
    // The loop variable keeps the argument as written, qualification and `?`
    // included, without a `out` projection.
    let written_arguments = split_top_level_commas(type_args_inner(iterable_type)?);
    let written_element = written_arguments.first()?.trim();
    let reads_as_unknown = written_element == "*" || written_element.starts_with("in ");
    let element = written_element.strip_variance();
    let element_path = element.dotted_ident_prefix();
    let names_a_type = element_path.last_segment().starts_with_uppercase();
    (!reads_as_unknown && names_a_type).then(|| element.to_owned())
}

/// How a `by <delegate>` expression determines the value its local reads as.
enum DelegateShape<'tree> {
    /// `lazy { .. }`: the lambda's value.
    LazyLambda { result: Node<'tree> },
    /// `remember { .. }`: the lambda's value is the real delegate.
    RememberedDelegate { delegate: Node<'tree> },
    /// `mutableStateOf(x)`: a state holding `x`.
    MutableStateOf { initial_value: Node<'tree> },
    /// Anything else: whatever its type's `getValue` operator returns.
    GetValueOperator,
}

/// The delegate shape `delegate` has, when its callee really is the stdlib /
/// Compose function its name suggests. A user-defined `lazy` or
/// `mutableStateOf` (or a member of that name) is an ordinary delegate whose
/// own `getValue` decides the value type.
fn delegate_shape<'tree>(
    delegate: Node<'tree>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> DelegateShape<'tree> {
    let is_plain_call = delegate.kind() == KIND_CALL_EXPR
        && delegate
            .named_child(0)
            .is_some_and(|callee| callee.kind() == KIND_SIMPLE_IDENT);
    if !is_plain_call {
        return DelegateShape::GetValueOperator;
    }
    let callee = delegate.call_fn_name(bytes);
    let lambda_result = trailing_lambda_last_expression(delegate);
    let declares_other_return = |callee: &str, expected: &str| {
        deps.find_fun_return_type_reachable(callee, uri)
            .and_then(|declared| declared.bare_type_name())
            .is_some_and(|declared| declared.last_segment() != expected)
    };
    match (callee.as_deref(), lambda_result) {
        (Some("lazy"), Some(result)) if !declares_other_return("lazy", "Lazy") => {
            DelegateShape::LazyLambda { result }
        }
        (Some(callee), Some(delegate))
            if LAMBDA_RESULT_FNS.contains(&callee)
                && returns_its_lambda_result(callee, deps, uri) =>
        {
            DelegateShape::RememberedDelegate { delegate }
        }
        (Some("mutableStateOf"), _) if !declares_other_return("mutableStateOf", "MutableState") => {
            match state_value_argument(delegate, bytes) {
                Some(initial_value) => DelegateShape::MutableStateOf { initial_value },
                None => DelegateShape::GetValueOperator,
            }
        }
        _ => DelegateShape::GetValueOperator,
    }
}

/// Whether the reachable `callee` returns its lambda's result: its declared
/// return type is one of its own type parameters (`fun <T> remember(..): T`),
/// or no declaration is reachable to say otherwise.
fn returns_its_lambda_result(callee: &str, deps: &impl InferDeps, uri: &Url) -> bool {
    let Some(declared) = deps.find_fun_return_type_reachable(callee, uri) else {
        return true;
    };
    let type_params = deps
        .find_fun_callable_info(callee, None, uri)
        .map(|info| info.type_params)
        .unwrap_or_default();
    type_params.contains(&declared)
}

/// The expression of `mutableStateOf`'s `value` parameter: the argument named
/// `value`, else the first argument written without a name.
fn state_value_argument<'tree>(call: Node<'tree>, bytes: &[u8]) -> Option<Node<'tree>> {
    let arguments = call.find_value_arguments()?;
    let value_arguments = arguments.children_of_kind(KIND_VALUE_ARG);
    let value_argument = value_arguments
        .iter()
        .find(|argument| argument.named_arg_label(bytes).as_deref() == Some("value"))
        .or_else(|| {
            value_arguments
                .iter()
                .find(|argument| argument.named_arg_label(bytes).is_none())
        })?;
    let mut argument_cursor = value_argument.walk();
    let expression = value_argument.named_children(&mut argument_cursor).last();
    expression
}

/// `lazy<Any> { .. }` / `mutableStateOf<Any>(..)`: the one type argument
/// written at the call.
fn explicit_value_type(call: Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut type_arguments = call.call_site_type_arg_strings(bytes)?;
    if type_arguments.len() != 1 {
        return None;
    }
    type_arguments.pop()
}

/// The type a `by <delegate>` local reads as. A type that does not resolve
/// from the file it was written in (an unsubstituted type parameter, say)
/// is unknown rather than a name some unrelated class happens to share.
fn infer_delegated_value_type(
    delegate: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("infer_delegated_value_type", delegate);
        return None;
    }
    let shape = delegate_shape(delegate, bytes, deps, uri);
    let holds_a_value = matches!(
        shape,
        DelegateShape::LazyLambda { .. } | DelegateShape::MutableStateOf { .. }
    );
    if holds_a_value {
        if let Some(explicit) = explicit_value_type(delegate, bytes) {
            // Written at the call site, so resolved there; unresolved (a type
            // parameter of the enclosing function, say) is unknown.
            return deps
                .names_type_in_scope(&explicit, uri)
                .then(|| (explicit, uri.clone()));
        }
    }
    match shape {
        DelegateShape::LazyLambda { result } => {
            infer_initializer_type(result, bytes, deps, uri, depth + 1)
        }
        DelegateShape::RememberedDelegate { delegate } => {
            infer_delegated_value_type(delegate, bytes, deps, uri, depth + 1)
        }
        DelegateShape::MutableStateOf { initial_value } => {
            infer_initializer_type(initial_value, bytes, deps, uri, depth + 1)
        }
        DelegateShape::GetValueOperator => {
            infer_get_value_type(delegate, bytes, deps, uri, depth + 1)
        }
    }
}

/// The return type of the delegate type's own `getValue` operator (a member,
/// or an extension such as Compose's `State<T>.getValue`), its type
/// arguments substituted.
///
/// The value type is usually one of the delegate type's type arguments, and
/// this engine tracks only where a type's head was written, not its
/// arguments (`State<State>`'s argument can come from a view model's
/// supertype list two files away). So the name must name the same type from
/// the file it was written in as from the file the delegate is used in,
/// which is also where it is then looked up; otherwise the value is unknown.
fn infer_get_value_type(
    delegate: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    let (delegate_type, delegate_uri) = infer_initializer_type(delegate, bytes, deps, uri, depth)?;
    let holder = delegate_type.bare_type_name()?;
    let (value_type, written_in) =
        member_get_value_type(&holder, &delegate_type, deps, &delegate_uri)
            .or_else(|| extension_get_value_type(&holder, &delegate_type, deps, uri))?;
    let names_the_same_type = deps.names_type_in_scope(&value_type, uri)
        && deps.same_type_in_scopes(&value_type, &written_in, uri);
    names_the_same_type.then(|| (value_type, uri.clone()))
}

/// The `getValue` member of the delegate type, with the `?` its declaration
/// states. A member whose nullability the lookup cannot read is unknown: a
/// bare name there could be a nullable value passing as a non-null one.
fn member_get_value_type(
    holder: &str,
    delegate_type: &str,
    deps: &impl InferDeps,
    delegate_uri: &Url,
) -> Option<(String, Url)> {
    let (value_type, written_in) =
        resolve_method_return_type_on(holder, delegate_type, "getValue", deps, delegate_uri)?;
    let nullability =
        deps.find_method_return_nullability_for_type(holder, "getValue", delegate_uri);
    let value_type = match nullability {
        ReturnNullability::NonNull => value_type,
        ReturnNullability::Nullable => with_nullable_marker(value_type),
        ReturnNullability::Unknown => return None,
    };
    Some((value_type, written_in))
}

fn with_nullable_marker(type_text: String) -> String {
    if type_text.ends_with('?') {
        type_text
    } else {
        format!("{type_text}?")
    }
}

/// `getValue(thisRef, property)`: what a `by` delegate is asked for.
const GET_VALUE_ARGUMENT_COUNT: usize = 2;

/// The `getValue` extension operator reachable from `uri`, when its receiver
/// is the delegate's own type: its return type with the extension's type
/// parameters (if any) bound from `delegate_type`, both read from the one
/// declaration the delegate type selects. The type is written in the
/// declaration's file — or, where a type parameter stands for it, taken to be
/// written in `uri`, since a type argument's own file is not tracked.
fn extension_get_value_type(
    holder: &str,
    delegate_type: &str,
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<(String, Url)> {
    let get_value_call = ReceiverCall {
        receiver_type: delegate_type,
        shape: CallShape {
            arg_count: GET_VALUE_ARGUMENT_COUNT,
            trailing_lambda: false,
        },
    };
    let extension = deps.find_extension_callable("getValue", get_value_call, uri)?;
    let extension_receiver = extension
        .generics
        .extension_receiver_type
        .bare_type_name()?;
    if extension_receiver.last_segment() != holder.last_segment() {
        return None;
    }
    let substitution = build_ext_fn_type_subst(
        &extension.generics.extension_receiver_type,
        delegate_type,
        &extension.generics.type_params,
    );
    let head_is_substituted = extension
        .return_type
        .bare_type_name()
        .is_some_and(|head| substitution.contains_key(&head));
    let written_in = if head_is_substituted {
        uri.clone()
    } else {
        extension.declared_in
    };
    let value_type = crate::indexer::apply_type_subst(&extension.return_type, &substitution);
    let value_type = match extension.return_nullability {
        ReturnNullability::Nullable => with_nullable_marker(value_type),
        ReturnNullability::NonNull | ReturnNullability::Unknown => value_type,
    };
    Some((value_type, written_in))
}

/// The type of `expression` as written, type arguments and `?` kept — what a
/// generic parameter is unified against (`StatefulModel<Content>` binds the
/// `T` of `data: StatefulModel<T>`).
pub(super) fn infer_expr_type_as_written(
    expression: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<(String, Url)> {
    infer_initializer_type(expression, bytes, deps, uri, 0)
}

/// The type of a local's initializer, keeping `?` and type arguments where
/// the initializer names another binding (`val copy = items`).
fn infer_initializer_type(
    initializer: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("infer_initializer_type", initializer);
        return None;
    }
    match initializer.kind() {
        KIND_SIMPLE_IDENT => infer_ident_type_in_form(
            initializer,
            bytes,
            deps,
            uri,
            depth,
            TypeTextForm::AsWritten,
        ),
        KIND_ELVIS_EXPR => infer_elvis_type(
            initializer,
            bytes,
            deps,
            uri,
            depth,
            TypeTextForm::AsWritten,
        ),
        KIND_PARENTHESIZED_EXPR => {
            let inner = initializer.named_child(0)?;
            infer_initializer_type(inner, bytes, deps, uri, depth + 1)
        }
        _ => infer_expr_type_at_depth(initializer, bytes, deps, uri, depth),
    }
}

/// Resolve the type of a `this_expression`.
///
/// Adapted from the `KIND_THIS_EXPR` arm of `semantic_tokens::resolve::expression_type`.
/// Delegates to contextual lambda-receiver inference at the node's position.
fn infer_this_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
) -> Option<String> {
    // A labelled `this@Outer` names an outer receiver, not the innermost one.
    let is_unlabelled = node.utf8_text(bytes) == Ok("this");
    if !is_unlabelled {
        return None;
    }
    let start = node.start_position();
    let col = crate::text_utils::ts_byte_col_to_utf16(bytes, &[], start.row, start.column);
    deps.find_contextual_type("this", uri, start.row, col)
}

/// Resolve the type of a `navigation_expression` node (e.g. `obj.field`).
///
/// Adapted from `semantic_tokens::resolve::navigation_expression_type`.
/// Recursively resolves the receiver through `infer_expr_type`, re-anchoring
/// reachability on the receiver's own declaring file (not the outermost
/// caller's) so a multi-segment chain like `a.b.c` resolves `.c` through
/// `a.b`'s file, not `a`'s — see `find_field_type`'s own doc comment.
fn infer_navigation_expr_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    let (member_type, member_uri) = infer_member_access_type(node, bytes, deps, uri, depth)?;
    let reads_through_safe_call = node
        .first_child_of_kind(KIND_NAV_SUFFIX)
        .and_then(|suffix| suffix.child(0))
        .is_some_and(|operator| operator.kind() == KIND_KW_SAFE_CALL);
    if reads_through_safe_call && !is_nullable_value(&member_type) {
        return Some((as_nullable_type(&member_type), member_uri));
    }
    Some((member_type, member_uri))
}

/// The declared type of the member a `navigation_expression` reads or calls.
fn infer_member_access_type(
    node: Node<'_>,
    bytes: &[u8],
    deps: &impl InferDeps,
    uri: &Url,
    depth: usize,
) -> Option<(String, Url)> {
    let receiver = nav_receiver_node(node)?;
    let member = nav_member_ident(node)?.utf8_text_owned(bytes)?;
    // As written: a receiver's type arguments substitute the member's type
    // (`store.currentState` on `Store<State>` is `State`).
    let (receiver_type, receiver_uri) =
        infer_initializer_type(receiver, bytes, deps, uri, depth + 1)?;

    // The type's own (possibly nested/qualified) name: no type arguments, no `?`.
    let lookup_name = receiver_type.dotted_ident_prefix();

    if nav_is_call_callee(node) {
        // The two-step `find_fun_return_type_reachable` → `find_fun_return_type` replicates
        // `Resolver::function_return_type`'s reachable→by_name fallback behaviour (see
        // `src/resolver/api.rs`) through the `InferDeps` seam rather than calling the
        // `Resolver` trait directly.  Together they are equivalent to the original
        // `indexer.function_return_type(&member, uri)` call in `navigation_expression_type`.
        // Neither is `Url`-aware yet, so the anchor doesn't advance past this hop.
        let type_name = resolve_method_return_type_on(
            &lookup_name,
            &receiver_type,
            &member,
            deps,
            &receiver_uri,
        )
        .map(|(substituted, _)| substituted)
        .or_else(|| deps.find_fun_return_type_reachable(&member, uri))
        .or_else(|| deps.find_fun_return_type(&member, uri))?;
        return Some((type_name, receiver_uri));
    }

    resolve_field_type_on(&lookup_name, &receiver_type, &member, deps, &receiver_uri)
}

// ─── navigation tree-walking helpers ─────────────────────────────────────────

/// Return the receiver sub-node of a `navigation_expression` (the part before
/// the final `.`).  Equivalent to `semantic_tokens::helpers::navigation_receiver_node`.
fn nav_receiver_node(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.is_named() && child.kind() != KIND_NAV_SUFFIX {
                return Some(child);
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    None
}

/// Return the member identifier inside the `navigation_suffix` of a
/// `navigation_expression`.  Equivalent to
/// `semantic_tokens::helpers::navigation_member_ident`.
fn nav_member_ident(node: Node<'_>) -> Option<Node<'_>> {
    let suffix = node.first_child_of_kind(KIND_NAV_SUFFIX)?;
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

/// Return `true` if `node` is the callee (first child) of a `call_expression`.
/// Equivalent to `semantic_tokens::helpers::is_call_callee`.
fn nav_is_call_callee(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    parent.kind() == KIND_CALL_EXPR && parent.child(0).map(|child| child.id()) == Some(node.id())
}

/// `!expr` → `Boolean`; other prefix operators (`-`, `+`) are arithmetic and
/// not inferable without knowing the operand type.
fn infer_prefix_expr_type(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let op = node.child(0)?;
    let text = op.utf8_text(bytes).ok()?;
    if text == "!" {
        Some("Boolean".to_owned())
    } else {
        None
    }
}

/// `x as Type` / `x as? Type`: the cast's own target type IS the expression's
/// type, stated outright without any inference — except that Kotlin's safe
/// cast (`as?`) always yields a NULLABLE result (`Type?`) regardless of
/// whether `Type` itself carries a `?` in source (Copilot review finding:
/// an earlier version of this function ignored the distinction entirely).
/// The target type is always the LAST named child (`as_expression`'s only
/// other named child is the base expression being cast); the operator
/// itself (`"as"` or `"as?"`) is an unnamed token among the direct children,
/// found by its own text rather than a fixed index since the base
/// expression's own child count can vary.
fn infer_as_expr_type(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let count = node.named_child_count() as u32;
    let type_node = node.named_child(count.checked_sub(1)?)?;
    let type_text = type_node.utf8_text_owned(bytes)?;

    let is_safe_cast = (0..node.child_count() as u32)
        .filter_map(|i| node.child(i))
        .any(|child| child.utf8_text(bytes) == Ok("as?"));
    if !is_safe_cast || is_nullable_value(&type_text) {
        return Some(type_text);
    }
    Some(as_nullable_type(&type_text))
}

/// Whether a value of `type_text` can be null. A bare function type ends with
/// its return type, so `(Int) -> String?` is a non-null function whose
/// result is nullable; only `((Int) -> String)?` is a nullable function value.
fn is_nullable_value(type_text: &str) -> bool {
    type_text.is_nullable() && !is_bare_function_type(type_text)
}

/// `(Int) -> String?`: a parenthesized parameter list, then `->`. A wrapped
/// `((Int) -> String)?` is not: its first group closes only at the end.
fn is_bare_function_type(type_text: &str) -> bool {
    if !type_text.starts_with('(') {
        return false;
    }
    let mut depth = 0usize;
    for (index, char) in type_text.char_indices() {
        match char {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return type_text[index + 1..].trim_start().starts_with("->");
                }
            }
            _ => {}
        }
    }
    false
}

/// `type_text` made nullable. A bare function type is parenthesized first:
/// `(String) -> Int` becomes `((String) -> Int)?` (a nullable function
/// value), not `(String) -> Int?` (a nullable return). A function type is the
/// only type shape that starts with `(`.
fn as_nullable_type(type_text: &str) -> String {
    if type_text.starts_with('(') {
        format!("({type_text})?")
    } else {
        format!("{type_text}?")
    }
}

/// For `if (cond) <then> else <else>`: emit a type hint only when both
/// branches infer to the same type. No hint is emitted for bare `if` without
/// `else`, or when either branch is ambiguous.
fn infer_if_expr_type<D: InferDeps>(
    node: Node<'_>,
    bytes: &[u8],
    deps: &D,
    uri: &Url,
    depth: usize,
) -> Option<String> {
    node.first_child_of_kind(KIND_ELSE)?;

    let mut bodies = node
        .children_of_kind(KIND_CONTROL_STRUCTURE_BODY)
        .into_iter();
    let then_expr = bodies.next()?.child(0)?;
    let else_expr = bodies.next()?.child(0)?;
    let then_type = infer_expr_type_at_depth(then_expr, bytes, deps, uri, depth + 1)?.0;
    let else_type = infer_expr_type_at_depth(else_expr, bytes, deps, uri, depth + 1)?.0;
    (then_type == else_type).then_some(then_type)
}

/// `a..b` or `a..<b`: infer `IntRange` only when both operands are integer
/// literals. Any other operand type requires the compiler.
fn infer_range_expr_type<D: InferDeps>(
    node: Node<'_>,
    bytes: &[u8],
    deps: &D,
    uri: &Url,
    depth: usize,
) -> Option<String> {
    let lhs = node.child(0)?;
    let rhs_idx = (node.child_count() as u32).checked_sub(1)?;
    let rhs = node.child(rhs_idx)?;
    let lhs_ty = infer_expr_type_at_depth(lhs, bytes, deps, uri, depth + 1)?.0;
    let rhs_ty = infer_expr_type_at_depth(rhs, bytes, deps, uri, depth + 1)?.0;
    match (lhs_ty.as_str(), rhs_ty.as_str()) {
        ("Int", "Int") => Some("IntRange".to_owned()),
        ("Long", "Long") | ("Int", "Long") | ("Long", "Int") => Some("LongRange".to_owned()),
        ("Char", "Char") => Some("CharRange".to_owned()),
        _ => None,
    }
}

/// `(expr)`: the parenthesized form has the same type as its inner expression.
/// tree-sitter-kotlin wraps the inner expression as this node's only named
/// child (no separate node for the parens themselves).
fn infer_parenthesized_expr_type<D: InferDeps>(
    node: Node<'_>,
    bytes: &[u8],
    deps: &D,
    uri: &Url,
    depth: usize,
) -> Option<String> {
    let inner = node.named_child(0)?;
    infer_expr_type_at_depth(inner, bytes, deps, uri, depth + 1).map(|(type_name, _)| type_name)
}

/// Numeric rank for Kotlin's arithmetic operand-promotion, highest wins
/// (`Int + Long` → `Long`, `Int + Double` → `Double`). Types outside this set
/// (custom classes, `Char`, `Boolean`, …) return `None` — arithmetic operators
/// on them are either overloads this function can't see without the operand's
/// declared class (`Char`) or not arithmetic at all.
fn numeric_rank(ty: &str) -> Option<u8> {
    match ty {
        "Byte" | "Short" => Some(0),
        "Int" => Some(1),
        "Long" => Some(2),
        "Float" => Some(3),
        "Double" => Some(4),
        _ => None,
    }
}

/// `a op b` for `+ - * / %` (`additive_expression` / `multiplicative_expression`,
/// both shaped `(lhs) (operator token) (rhs)`).
///
/// Two cases, matching real Kotlin usage found via production-code testing
/// (see `additive_identifier_operands_promote_to_long` / `multiplicative_int_literals_infers_int`):
/// - Both operands numeric → the higher-ranked type wins (`numeric_rank`).
/// - `+` with a `String` left operand → `String` (`String.plus(Any?): String`;
///   Kotlin has no analogous `Any.plus(String)`, so only the left side counts).
///
/// Anything else (unresolvable operand, non-numeric non-`String` operand,
/// `String` only on the right) returns `None` rather than guessing.
fn infer_arithmetic_expr_type<D: InferDeps>(
    node: Node<'_>,
    bytes: &[u8],
    deps: &D,
    uri: &Url,
    depth: usize,
) -> Option<String> {
    let lhs = node.child(0)?;
    let op = node.child(1)?.utf8_text(bytes).ok()?;
    let rhs = node.child(2)?;
    let lhs_ty = infer_expr_type_at_depth(lhs, bytes, deps, uri, depth + 1)?.0;
    if op == "+" && lhs_ty == "String" {
        return Some("String".to_owned());
    }
    let rhs_ty = infer_expr_type_at_depth(rhs, bytes, deps, uri, depth + 1)?.0;
    let lhs_rank = numeric_rank(&lhs_ty)?;
    let rhs_rank = numeric_rank(&rhs_ty)?;
    // Kotlin has no same-rank arithmetic overload for `Byte`/`Short`: unlike
    // `Int`/`Long`/`Float`/`Double`, which each define e.g. `Int.plus(Int): Int`,
    // `Byte`/`Short` only define `plus`/`minus`/etc. overloads that return `Int`
    // (`Byte.plus(Byte): Int`, `Byte.plus(Short): Int`, …) — so when neither
    // operand outranks `Byte`/`Short`, the result promotes to `Int` rather than
    // staying at the narrower operand type.
    if lhs_rank == 0 && rhs_rank == 0 {
        return Some("Int".to_owned());
    }
    Some(if lhs_rank >= rhs_rank { lhs_ty } else { rhs_ty })
}

#[cfg(test)]
#[path = "expr_type_tests.rs"]
mod tests;
