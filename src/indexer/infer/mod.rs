//! Type-inference helpers for the Kotlin indexer.
//!
//! # Catalogue
//!
//! `mod.rs` is the catalogue: it re-exports the rich, self-documenting types and
//! the `CstResolve` facade trait so callers import from a single place.
//!
//! ## Types produced (Phase 1)
//!
//! | Type              | Role                                                         |
//! |-------------------|--------------------------------------------------------------|
//! | `CstCtx`          | Per-request input: bytes + URI + IO policy                   |
//! | `Fqn`             | Importable fully-qualified name (newtype over `String`)      |
//! | `Resolution<T>`   | Three-way outcome: `Resolved(T)` / `Ambiguous(Vec<Fqn>)` / `Unresolved` |
//! | `ResolvedType`    | A resolved expression type with its nullable flag            |
//!
//! ## Submodules
//!
//! - `deps`        — `InferDeps` trait + `TestDeps` stub for unit-testing leaf helpers
//! - `lambda`      — decomposing lambda/function types (`(T) -> R`, receiver lambdas, etc.)
//! - `sig`         — function signature extraction (pure string/slice functions)
//! - `args`        — call argument parsing (pure)
//! - `it_this`     — resolving `it`/`this` element types inside Kotlin lambda bodies
//! - `type_subst`  — generic type-parameter substitution
//! - `chain`       — CST navigation-chain type resolution
//! - `receiver`    — lambda receiver type inference from text context
//! - `cst_lambda`         — CST-backed ThisLambdaCtx + lambda context helpers
//! - `lambda_resolution`  — `LambdaParamResolution` typed intermediate (Phase 2)

pub(super) mod args;
pub(super) mod chain;
pub(super) mod cst_cursor;
pub(super) mod cst_lambda;
pub(super) mod deps;
pub(super) mod expr_type;
pub(super) mod it_this;
pub(super) mod lambda;
pub(super) mod lambda_resolution;
pub(super) mod receiver;
pub(super) mod sig;
pub(super) mod type_subst;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

// ─── re-exports from resolver (IO policy) ─────────────────────────────────────

pub(crate) use crate::resolver::resolve::ResolveIo;

// ─── catalogue types ──────────────────────────────────────────────────────────

use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::StrExt as _;

// Phase 1 types + trait: consumed only via tests now; Task 3 wires production
// callers. Suppress dead-code lints for the seam until that slice lands.
#[allow(dead_code)]
/// Per-request CST resolution input: document bytes + URI + IO policy.
pub(crate) struct CstCtx<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) uri: &'a Url,
    /// IO policy: carried now for the seam; later catalogue methods branch on it.
    pub(crate) io: ResolveIo,
}

/// Importable fully-qualified name (newtype over the FQN string).
#[allow(dead_code)] // produced by Ambiguous; consumed by later catalogue methods
pub(crate) struct Fqn(pub(crate) String);

/// Outcome of resolving something to `T`. Reused across the catalogue so an
/// agent learns the three outcomes once and reads them off every signature.
#[allow(dead_code)] // variants wired by Task 3+; all three present for completeness
pub(crate) enum Resolution<T> {
    Resolved(T),
    /// Multiple candidates — callers may surface all or pick one heuristically.
    /// Unused by `expr_type` in Phase 1; present for later catalogue methods.
    Ambiguous(Vec<Fqn>),
    Unresolved,
}

impl<T> Resolution<T> {
    /// `Resolved(t) -> Some(t)`, else `None`.
    /// Bridges callers not yet ambiguity-aware.
    #[allow(dead_code)] // wired by Task 3; suppressed until then
    pub(crate) fn resolved(self) -> Option<T> {
        match self {
            Resolution::Resolved(value) => Some(value),
            Resolution::Ambiguous(_) | Resolution::Unresolved => None,
        }
    }

    /// `Resolved(t) -> Some(&t)`, else `None`.
    #[allow(dead_code)] // wiring seam; used by later catalogue methods
    pub(crate) fn resolved_ref(&self) -> Option<&T> {
        match self {
            Resolution::Resolved(value) => Some(value),
            Resolution::Ambiguous(_) | Resolution::Unresolved => None,
        }
    }
}

/// A resolved expression type. Phase 1: carries the inferred type *as-written*
/// (no lossy normalization); the RawTypeName/TypeName split is slice 5.
#[allow(dead_code)] // wired by Task 3; suppressed until then
pub(crate) struct ResolvedType {
    type_name: String,
    nullable: bool,
}

impl ResolvedType {
    /// Construct from an inferred type string.
    /// Nullability is derived via `StrExt::is_nullable` (the canonical place).
    pub(crate) fn from_inferred(raw: String) -> Self {
        let nullable = raw.is_nullable();
        ResolvedType {
            type_name: raw,
            nullable,
        }
    }

    /// The type as-written (what the old `Option<String>` callers consumed).
    #[allow(dead_code)] // wired by Task 3; suppressed until then
    pub(crate) fn as_type_str(&self) -> &str {
        &self.type_name
    }

    #[allow(dead_code)] // wired by Task 3; suppressed until then
    pub(crate) fn is_nullable(&self) -> bool {
        self.nullable
    }
}

// ─── catalogue facade trait ───────────────────────────────────────────────────

/// The catalogue of CST-driven type resolution. (Phase 1: `expr_type` only.)
#[allow(dead_code)] // wired by Task 3; suppressed until then
pub(crate) trait CstResolve {
    /// Type of any expression node.
    /// Covers ident / nav / call / literals / this / if / range.
    fn expr_type(&self, node: Node, ctx: &CstCtx) -> Resolution<ResolvedType>;
}

// ─── impl for Indexer ─────────────────────────────────────────────────────────

impl CstResolve for crate::indexer::Indexer {
    fn expr_type(&self, node: Node, ctx: &CstCtx) -> Resolution<ResolvedType> {
        match crate::indexer::infer::expr_type::infer_expr_type(node, ctx.bytes, self, ctx.uri) {
            Some(raw) => Resolution::Resolved(ResolvedType::from_inferred(raw)),
            None => Resolution::Unresolved,
        }
    }
}
