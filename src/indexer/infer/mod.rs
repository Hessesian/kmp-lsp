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
//! | `CstQuery`        | Bound CST query: node + doc + deps + URI + IO policy        |
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

use crate::indexer::live_tree::LiveDoc;
use crate::StrExt as _;

use self::deps::InferDeps;

/// Importable fully-qualified name (newtype over the FQN string).
#[allow(dead_code)] // produced by Ambiguous; consumed by later catalogue methods
pub(crate) struct Fqn(pub(crate) String);

/// Outcome of resolving something to `T`. Reused across the catalogue so an
/// agent learns the three outcomes once and reads them off every signature.
pub(crate) enum Resolution<T> {
    Resolved(T),
    /// Multiple candidates — callers may surface all or pick one heuristically.
    /// Unused by `expr_type` today; present for later catalogue methods.
    #[allow(dead_code)] // present for completeness; consumed by later catalogue methods
    Ambiguous(Vec<Fqn>),
    Unresolved,
}

impl<T> Resolution<T> {
    /// `Resolved(t) -> Some(t)`, else `None`.
    /// Bridges callers not yet ambiguity-aware.
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

    #[allow(dead_code)] // not yet consumed; available for later catalogue methods
    pub(crate) fn is_nullable(&self) -> bool {
        self.nullable
    }
}

// ─── CstQuery — the unified CST resolution context ───────────────────────────

/// A bound CST query: a single expression node together with its document,
/// dependency seam, URI, and IO policy.
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
    /// IO policy carried for later catalogue methods that branch on it.
    #[allow(dead_code)]
    io: ResolveIo,
}

impl<'a, D: InferDeps> CstQuery<'a, D> {
    /// Construct a query for `node` within `doc`, using `deps` for index
    /// lookups and `uri` to identify the file.
    pub(crate) fn new(
        node: Node<'a>,
        doc: &'a LiveDoc,
        deps: &'a D,
        uri: &'a Url,
        io: ResolveIo,
    ) -> Self {
        Self {
            node,
            doc,
            deps,
            uri,
            io,
        }
    }

    /// Return a new query pointing at `node` but sharing all other context.
    /// Used by walk steps that move to a child or sibling node.
    #[allow(dead_code)] // wiring seam for later walk tasks
    fn at(&self, node: Node<'a>) -> Self {
        Self { node, ..*self }
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
}
