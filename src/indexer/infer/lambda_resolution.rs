//! Typed intermediate for lambda parameter type resolution.
//!
//! Replaces raw `Option<String>` flowing between resolution stages with an
//! explicit struct that carries classification and callable context.
//!
//! The pipeline has two stages:
//!
//! ```text
//! Stage 1 — locate_and_extract:
//!   lambda node → find enclosing call → look up signature → extract param type → classify
//!
//! Stage 2 — finalize_resolution:
//!   Concrete → return extracted_type directly
//!   GenericParam + resolved receiver → substitute → return concrete type
//!   GenericParam + unresolved receiver → return None (fall through to text path)
//! ```

use super::deps::CallableInfo;

/// Why an extracted type was classified as a generic parameter.
///
/// This distinction controls the fallback behaviour when substitution fails.
#[derive(Debug)]
pub(super) enum GenericParamSource {
    /// Matched against the callable's explicit `type_params` list.
    ///
    /// On substitution failure, return the extracted type as-is — it IS the
    /// concrete type (e.g. `"Effect"` in `type_params = ["Effect"]` that was
    /// matched by a wrong overload but still names the concrete sealed class).
    DeclaredInCallable,
    /// Heuristic: short all-uppercase name (T, R, IN, …) with no callable info.
    ///
    /// On substitution failure, return `None` so the text path can retry.
    ShapeHeuristic,
}

/// Classification of a type extracted from a function signature's lambda parameter.
pub(super) enum ExtractedTypeKind {
    /// A concrete type requiring no substitution (e.g. `"Contract.Effect"`, `"String"`).
    Concrete,
    /// A generic type parameter that must be substituted before returning.
    GenericParam(GenericParamSource),
}

/// Structured result of stage 1 (LOCATE + EXTRACT) of lambda `it`/`this` resolution.
///
/// Carries all context needed for stage 2 (generic substitution) without
/// re-deriving it, and without scattering `is_generic_param` heuristics across
/// multiple call sites.
pub(super) struct LambdaParamResolution<'tree> {
    /// Raw type extracted from the function signature.
    pub extracted_type: String,
    /// Classification determined once at extraction time.
    pub kind: ExtractedTypeKind,
    /// Callable info providing `type_params` and `extension_receiver_type`.
    pub callable: Option<CallableInfo>,
    /// The enclosing `call_expression` CST node.
    pub call_expr: tree_sitter::Node<'tree>,
    /// Resolved receiver type from the call-site chain, if available.
    ///
    /// `None` when the chain resolves to a raw name (e.g. function parameter
    /// whose type is not in `type_annotations`). Stage 2 checks this before
    /// attempting generic substitution — no point substituting against an
    /// unresolved receiver.
    pub receiver_type: Option<String>,
}
