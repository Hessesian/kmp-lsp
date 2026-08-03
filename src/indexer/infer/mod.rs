//! Type-inference helpers for the Kotlin indexer.
//!
//! # Catalogue
//!
//! `mod.rs` is the catalogue: it re-exports the rich, self-documenting types
//! and `CstQuery` so callers import from a single place.
//!
//! ## Types produced
//!
//! | Type              | Role                                                         |
//! |-------------------|--------------------------------------------------------------|
//! | `CstQuery`        | Bound CST query: node + doc + deps + URI                     |
//! | `Resolution<T>`   | Three-way outcome: `Resolved(T)` / `Ambiguous` / `Unresolved` |
//! | `ResolvedType`    | A resolved expression type with its nullable flag            |
//!
//! ## Known gaps
//!
//! These capability families are still exported flat from `src/indexer.rs` instead of through
//! `CstQuery` — an agent looking for "type of X" should know they exist before reinventing them:
//!
//! - `it_this` (`find_it_element_type`, `find_this_context`, `find_this_element_type`,
//!   `find_named_lambda_param_type`, `is_lambda_param`) — CST-driven internally already
//!   (delegates to `cst_lambda`), but takes a `CursorPos` + does its own repair-gated node
//!   acquisition; folding into `CstQuery`'s bound-`Node` model needs a `CstQuery::at_position`
//!   bridge — deferred, see the design doc's lambda-triad/`LambdaScope`-promotion step.
//!   `all_lambda_receivers_at` is the one exception: its position→node bridge now constructs a
//!   `CstQuery` and calls `all_this_receivers()` directly (2026-08-03) — still a flat
//!   `CursorPos`-taking export by name, but no longer bypasses the catalogue underneath.
//! - `sig` (signature/param-text helpers) — pure string/slice helpers, several IO-bound
//!   (`find_fun_signature_full` may trigger on-demand rg indexing); not expression-type
//!   resolution, out of `CstQuery`'s "type of a bound node" remit.
//! - `cst_symbol` (`classify_cursor`, `resolve_identity`, navigation helpers) — the symbol-identity
//!   navigation family's own facade (design doc step 6, already CST-first with string+rg
//!   fallback); intentionally a peer of `CstQuery`, not a submodule of it.
//! - `args`, `type_subst`, `lambda` — low-level primitives (`extract_first_arg`,
//!   generic-substitution string ops, lambda-type-string decomposition) consumed mostly *by* the
//!   CST engine's own submodules (`cst_lambda.rs`, `chain.rs`); the one exception is
//!   `find_as_call_arg_type` (from `args.rs`), which reaches a feature directly through
//!   `Indexer::infer_lambda_param_type_at` in `src/indexer/scope.rs` (used by hover and
//!   go-to-definition for lambda params) — not judged to need a facade.
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
//! - `cst_cursor`  — shared tree-sitter cursor-walk helpers
//! - `cst_symbol`  — CST identifier classification (declaration vs. reference, receiver type)
//! - `cst_lambda`         — CST-backed ThisLambdaCtx + lambda context helpers
//! - `lambda_resolution`  — `LambdaParamResolution` typed intermediate (Phase 2)
//! - `expr_type`   — expression-node type inference (backs `CstQuery::expr_type`)
//! - `speculative`  — marker-insertion speculative reparse for dot-completion receivers

pub(super) mod args;
pub(super) mod chain;
pub(super) mod cst_cursor;
pub(super) mod cst_lambda;
pub(super) mod cst_symbol;
pub(super) mod deps;
pub(super) mod expr_type;
pub(super) mod it_this;
pub(super) mod lambda;
pub(super) mod lambda_resolution;
pub(super) mod sig;
pub(super) mod speculative;
pub(super) mod type_subst;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

// ─── catalogue types ──────────────────────────────────────────────────────────

use tower_lsp::lsp_types::Url;
use tree_sitter::Node;

use crate::indexer::live_tree::LiveDoc;
use crate::StrExt as _;

use self::deps::InferDeps;

/// Outcome of resolving something to `T`. Reused across the catalogue so an
/// agent learns the three outcomes once and reads them off every signature.
#[derive(Debug, Clone)]
pub(crate) enum Resolution<T> {
    Resolved(T),
    /// Multiple candidates — the caller should skip rather than guess.
    Ambiguous,
    Unresolved,
}

impl<T> Resolution<T> {
    /// `Resolved(t) -> Some(t)`, else `None`.
    /// Bridges callers not yet ambiguity-aware.
    pub(crate) fn resolved(self) -> Option<T> {
        match self {
            Resolution::Resolved(value) => Some(value),
            Resolution::Ambiguous | Resolution::Unresolved => None,
        }
    }
}

/// A resolved expression type. Carries the inferred type *as-written*
/// (no lossy normalization); the RawTypeName/TypeName split is slice 5.
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
    pub(crate) fn as_type_str(&self) -> &str {
        &self.type_name
    }

    #[allow(dead_code)] // read only by tests pinning `from_inferred`'s nullable computation
    pub(crate) fn is_nullable(&self) -> bool {
        self.nullable
    }
}

// ─── CstQuery — the unified CST resolution context ───────────────────────────

/// A bound CST query: a single expression node together with its document,
/// dependency seam, and URI.
///
/// Constructing a `CstQuery` is cheap (no allocation); the per-request cost is
/// in the methods that call through to the inference engine.
///
/// # Generics
///
/// `D: InferDeps` keeps `TestDeps` as a valid driver so the inference engine
/// can be unit-tested without a live `Indexer`.
#[derive(Clone, Copy)]
pub(crate) struct CstQuery<'a, D: InferDeps> {
    node: Node<'a>,
    doc: &'a LiveDoc,
    deps: &'a D,
    uri: &'a Url,
}

impl<'a, D: InferDeps> CstQuery<'a, D> {
    /// Construct a query for `node` within `doc`, using `deps` for index
    /// lookups and `uri` to identify the file.
    pub(crate) fn new(node: Node<'a>, doc: &'a LiveDoc, deps: &'a D, uri: &'a Url) -> Self {
        Self {
            node,
            doc,
            deps,
            uri,
        }
    }

    /// Build the completion scope for every lambda enclosing the bound node.
    ///
    /// Walks the CST ancestor chain from the node, producing one
    /// [`cst_lambda::LambdaScopeInfo`] per enclosing `lambda_literal` that
    /// contributes an `it` type or named parameters — ordered outermost first,
    /// innermost last (the order the completion scope stack consumes).
    pub(crate) fn lambda_scope(&self) -> Vec<cst_lambda::LambdaScopeInfo> {
        cst_lambda::cst_lambda_scopes(self.node, self.doc, self.deps, self.uri)
    }

    /// Infer the type of the bound expression node.
    ///
    /// Covers literals, identifiers, navigation expressions, call expressions,
    /// boolean operators, `if` expressions, and `this`.  Returns
    /// `Resolution::Unresolved` for compound forms not yet handled.
    pub(crate) fn expr_type(&self) -> Resolution<ResolvedType> {
        match crate::indexer::infer::expr_type::infer_expr_type(
            self.node,
            &self.doc.bytes,
            self.deps,
            self.uri,
        ) {
            Some(raw) => Resolution::Resolved(ResolvedType::from_inferred(raw)),
            None => Resolution::Unresolved,
        }
    }

    /// Every enclosing lambda receiver type at the bound node, innermost-first —
    /// the order Kotlin resolves an implicit-receiver call in (nearest wins). An
    /// unresolvable or non-receiver lambda is skipped; the walk continues outward.
    ///
    /// Used to check a bare call/type reference against every candidate receiver
    /// in scope (e.g. `item()` inside `with(x) { }` nested in a builder belongs to
    /// the outer receiver even when `x`'s type can't be resolved).
    pub(crate) fn all_this_receivers(&self) -> Vec<String> {
        cst_lambda::all_this_receivers_at(self.node, self.doc, self.deps, self.uri)
    }
}
