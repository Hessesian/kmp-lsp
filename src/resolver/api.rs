//! The resolution capability **catalog** for the `resolver` subsystem.
//!
//! This module exposes [`Resolver`] — the single, intent-named, documented
//! surface for resolving Kotlin/KMP receivers and members against the workspace
//! index. It exists to stop the *reinvention* that fragmented resolution in the
//! first place: consumers (diagnostics, hover, completion, semantic
//! highlighting, go-to-definition) kept hand-rolling slightly-divergent copies
//! of the same lookups because the existing capability wasn't discoverable.
//!
//! **The contract:** a consumer that needs to resolve something looks here
//! first. If the capability exists, call it — do not re-derive it inline. If it
//! is *missing*, add a method to [`Resolver`] (and implement it by delegating to
//! the canonical function), so the next consumer finds it instead of reinventing
//! it. A trait that is missing a capability is worse than none, because it sends
//! implementors back to hand-rolling.
//!
//! Return types are deliberately self-documenting so a caller knows what to
//! expect from the signature alone: a [`ReceiverType`] always carries the
//! `raw`/`qualified`/`outer`/`leaf`/`nullable` breakdown of an inferred
//! receiver; [`Definitions`] is an ordered (best-first) list of *definition*
//! sites — never references, never an error channel.
//!
//! The trait is implemented for [`Indexer`], which owns the file/symbol/
//! extension maps these methods read.

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;

use super::infer::{
    find_fun_return_type_by_name, find_fun_return_type_reachable, find_method_return_type,
    find_method_return_type_via_supertypes, infer_field_chain_type, infer_receiver_type,
};
use super::{ReceiverKind, ReceiverType};

/// An ordered list of **definition** sites (where a symbol is *declared*), best
/// match first — never references, never an error signal.
///
/// An empty `Definitions` means "no definition resolved": callers treat that as
/// *skip, don't guess*, not as a failure. Dereferences to `[Location]`, so
/// `iter()`, `first()`, `is_empty()`, and slice indexing work directly.
#[derive(Debug, Clone, Default)]
pub(crate) struct Definitions(pub Vec<Location>);

impl std::ops::Deref for Definitions {
    type Target = [Location];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The result type of a call — what a function or method *returns*.
///
/// Carried as the **as-written** type text from the signature (the text after
/// the `:`), with generics and a trailing `?` preserved (e.g. `Flow<Event>`,
/// `String?`). It is *not* normalized: feed it to [`ReceiverType::from_raw`] when
/// you need the `raw`/`qualified`/`leaf`/`nullable` breakdown. The newtype exists
/// so a signature returning `Option<ReturnType>` says "a call's return type",
/// distinct from a receiver type, a field type, or a bare identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReturnType(pub String);

impl ReturnType {
    /// Consume into the owned return-type `String` (for the downstream
    /// String-typed inference paths that haven't adopted the newtype yet).
    /// Borrow with `&*` / [`Deref`](std::ops::Deref) when you only need `&str`.
    pub(crate) fn into_inner(self) -> String {
        self.0
    }
}

impl std::ops::Deref for ReturnType {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<ReturnType> for String {
    fn from(rt: ReturnType) -> Self {
        rt.0
    }
}

/// The resolution capability catalog. See the module docs for the contract.
///
/// Implemented for [`Indexer`]; each method delegates to the canonical
/// resolution function so there is exactly one implementation of each
/// capability behind a discoverable name.
pub(crate) trait Resolver {
    /// Infer the [`ReceiverType`] of a receiver expression — a bare variable
    /// (`repo` in `repo.load()`) or a lambda/implicit-receiver
    /// ([`ReceiverKind::Contextual`]) — at `uri`.
    ///
    /// Returns `None` when the type cannot be inferred (no annotation, unindexed
    /// file, unresolvable lambda scope). Never performs a global ripgrep scan;
    /// the caller decides whether to skip or fall back.
    fn infer_receiver_type(&self, receiver: ReceiverKind<'_>, uri: &Url) -> Option<ReceiverType>;

    /// Infer the [`ReceiverType`] reached by walking a *pure field-access chain*
    /// (`["holder", "repo"]` for `holder.repo`) from the root variable through
    /// each declared field type. The leaf field's trailing `?` is preserved in
    /// [`ReceiverType::nullable`].
    ///
    /// Returns `None` if the chain has fewer than two segments or any segment's
    /// type cannot be resolved.
    fn infer_field_chain_type(&self, chain: &[String], uri: &Url) -> Option<ReceiverType>;

    /// Resolve `member` accessed through `qualifier` — a variable name
    /// (`"repo"`) or a pure field chain (`"arg.texts"`) — to its declaration
    /// site(s), searching the receiver type's owner file and its
    /// superclass/interface hierarchy.
    ///
    /// Returns [`Definitions`] (best-first, possibly empty). This is the
    /// import-aware member lookup; prefer it over reaching into the raw
    /// `definition_locations` map, which is not scope-aware.
    fn resolve_member(&self, member: &str, qualifier: &str, uri: &Url) -> Definitions;

    /// Resolve the [`ReturnType`] of a *free function* `fn_name` called from
    /// `from_uri`.
    ///
    /// Import-aware: binds `fn_name` through the scope chain (imports →
    /// same-package → star → qualified/jars) and reads the return type of the
    /// symbol the call actually binds to, falling back to a capped workspace-wide
    /// by-name lookup only when no in-scope definition is found. Prefer this over
    /// the raw by-name scan so an unrelated same-named overload in a test file or
    /// jar can't shadow the real one.
    ///
    /// Returns `None` when no definition is found or it has no declared return
    /// type.
    fn function_return_type(&self, fn_name: &str, from_uri: &Url) -> Option<ReturnType>;

    /// Resolve the [`ReturnType`] of `method_name` invoked on a receiver whose
    /// type's base name is `type_name`.
    ///
    /// This is the single composite for member resolution: it checks extension
    /// functions and member functions of the type itself, then walks the type's
    /// declared supertypes (applying type-argument substitution). When `from_uri`
    /// is `Some`, extension functions are filtered to those in scope at that file;
    /// `None` performs an unfiltered global lookup for callers without URI
    /// context.
    ///
    /// Returns `None` when no matching method (own, extension, or inherited) with
    /// a declared return type is found.
    fn method_return_type(
        &self,
        type_name: &str,
        method_name: &str,
        from_uri: Option<&Url>,
    ) -> Option<ReturnType>;
}

impl Resolver for Indexer {
    fn infer_receiver_type(&self, receiver: ReceiverKind<'_>, uri: &Url) -> Option<ReceiverType> {
        infer_receiver_type(self, receiver, uri)
    }

    fn infer_field_chain_type(&self, chain: &[String], uri: &Url) -> Option<ReceiverType> {
        infer_field_chain_type(self, chain, uri)
    }

    fn resolve_member(&self, member: &str, qualifier: &str, uri: &Url) -> Definitions {
        Definitions(self.resolve_member_only(member, qualifier, uri))
    }

    fn function_return_type(&self, fn_name: &str, from_uri: &Url) -> Option<ReturnType> {
        find_fun_return_type_reachable(self, fn_name, from_uri)
            .or_else(|| find_fun_return_type_by_name(self, fn_name))
            .map(ReturnType)
    }

    fn method_return_type(
        &self,
        type_name: &str,
        method_name: &str,
        from_uri: Option<&Url>,
    ) -> Option<ReturnType> {
        find_method_return_type(self, type_name, method_name, from_uri)
            .or_else(|| {
                find_method_return_type_via_supertypes(self, type_name, method_name, from_uri)
            })
            .map(ReturnType)
    }
}
