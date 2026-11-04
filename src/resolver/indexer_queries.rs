//! Entry points on [`Indexer`] that are answered by name resolution. They are
//! defined here, above the index, so the index never depends on the resolver.

use std::sync::Arc;

use tower_lsp::lsp_types::{Location, Url};

use super::{
    resolve_symbol_in_type_scope, split_package_prefix, symbols_from_uri_as_completions_pub,
    ResolveIo, TypeScope,
};
use crate::indexer::infer::lambda::SCOPE_FUNCTIONS;
use crate::indexer::{Indexer, ResolvedType};
use crate::StrExt;

impl Indexer {
    /// Spawn background tasks to pre-warm the completion cache for all types
    /// declared in `uri` as constructor parameters or properties.
    ///
    /// This runs after `index_content` so that when the user types `repo.` the
    /// cache is already populated and the response is instant.
    pub(crate) fn prewarm_completion_cache(self: Arc<Self>, uri: &Url) {
        let Some(data) = self.files.get(uri.as_str()) else {
            return;
        };
        let from_uri = uri.clone();

        // Collect unique type names from this file's lines.
        let mut type_names: Vec<String> = Vec::new();
        {
            let mut seen = std::collections::HashSet::new();
            for line in data.lines.iter() {
                let t = line.trim_start();
                if t.starts_with("//") || t.starts_with('*') {
                    continue;
                }
                let mut rest = t;
                while let Some(ci) = rest.find(':') {
                    let after = rest[ci + 1..].trim_start();
                    let type_name = after.ident_prefix();
                    if !type_name.is_empty()
                        && type_name.starts_with_uppercase()
                        && seen.insert(type_name.clone())
                    {
                        type_names.push(type_name);
                    }
                    rest = &rest[ci + 1..];
                }
            }
        }
        drop(data);

        // Spawn a background task per type (capped to avoid bursts).
        let limit = Arc::new(tokio::sync::Semaphore::new(4));
        for type_name in type_names {
            let indexer = Arc::clone(&self);
            let uri2 = from_uri.clone();
            let sem = Arc::clone(&limit);
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.expect("semaphore closed");
                tokio::task::spawn_blocking(move || {
                    let locs = indexer.resolve_symbol(&type_name, None, &uri2);
                    if let Some(loc) = locs.first() {
                        let file_uri = loc.uri.to_string();
                        if indexer.completion_cache.contains_key(&file_uri) {
                            return;
                        }
                        symbols_from_uri_as_completions_pub(&indexer, &file_uri);
                    }
                })
                .await
                .ok();
            });
        }
    }

    /// Resolve an unqualified call's callee name, filtering same-file
    /// candidates whose arity can't satisfy `shape` — see
    /// [`crate::resolver::resolve_callee_definition`]. Not part of
    /// `find_definition_qualified`'s signature: every other caller of that
    /// function has no call shape to offer, and this is the one goto-definition
    /// path that does.
    pub(crate) fn find_definition_for_call(
        &self,
        name: &str,
        uri: &Url,
        shape: crate::indexer::CallShape,
    ) -> Vec<Location> {
        crate::resolver::resolve_callee_definition(self, name, uri, shape)
    }

    /// Resolve `name(...)` as an implicit `this.name(...)` against
    /// `receiver_base` — see [`crate::resolver::resolve_implicit_receiver_callee`].
    /// A separate entry point rather than folded into `find_definition_for_call`:
    /// only callers that already know they're inside an extension function's
    /// body (via `parser::enclosing_extension_receiver_at`) have a receiver to
    /// offer, and it's tried only as a fallback after the plain bare-name
    /// lookup comes up empty.
    pub(crate) fn find_definition_for_implicit_receiver_call(
        &self,
        receiver_base: &str,
        name: &str,
        uri: &Url,
        shape: crate::indexer::CallShape,
    ) -> Vec<Location> {
        crate::resolver::resolve_implicit_receiver_callee(self, receiver_base, name, uri, shape)
    }

    /// Resolve `name` as accessed through `qualifier`. When the receiver-scoped
    /// search finds nothing and `name` is a universal scope function
    /// (`SCOPE_FUNCTIONS`, e.g. `apply`/`let`) — a receiver-generic stdlib
    /// extension that can never be a member of any specific type — fall back
    /// to the same bare-name resolution a plain reference to that stdlib
    /// function would use. A receiver type that declares its own member by
    /// that name still wins, since the fallback only runs once the normal
    /// search has already failed.
    pub(crate) fn find_definition_qualified(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
    ) -> Vec<Location> {
        self.find_definition_qualified_with_io(name, qualifier, from_uri, false)
    }

    /// Like `find_definition_qualified` but never spawns `rg`/`fd` — see
    /// `resolve_symbol_index_only`'s own doc comment for why.
    pub(crate) fn find_definition_qualified_index_only(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
    ) -> Vec<Location> {
        self.find_definition_qualified_with_io(name, qualifier, from_uri, true)
    }

    /// `name` as a member of `receiver`'s type. The type's NAME is resolved in
    /// the file it was written in ([`ResolvedType::declaring_uri`]), not the
    /// caller's: an inferred receiver (`holder.model.text`) is spelled in the
    /// callee's file, which imports it — the caller usually does not.
    pub(crate) fn find_member_of_type(
        &self,
        name: &str,
        receiver: &ResolvedType,
        from_uri: &Url,
    ) -> Vec<Location> {
        self.find_member_of_type_with_io(name, receiver, from_uri, false)
    }

    /// Like `find_member_of_type` but never spawns `rg`/`fd`.
    pub(crate) fn find_member_of_type_index_only(
        &self,
        name: &str,
        receiver: &ResolvedType,
        from_uri: &Url,
    ) -> Vec<Location> {
        self.find_member_of_type_with_io(name, receiver, from_uri, true)
    }

    fn find_member_of_type_with_io(
        &self,
        name: &str,
        receiver: &ResolvedType,
        from_uri: &Url,
        index_only: bool,
    ) -> Vec<Location> {
        let type_path = receiver.type_path();
        let (package, bare_path) = split_package_prefix(&type_path);
        let type_scope = TypeScope {
            file: receiver.resolution_scope(from_uri),
            package,
        };
        self.find_qualified_in_type_scope(name, bare_path, &type_scope, from_uri, index_only)
    }

    fn find_definition_qualified_with_io(
        &self,
        name: &str,
        qualifier: Option<&str>,
        from_uri: &Url,
        index_only: bool,
    ) -> Vec<Location> {
        match qualifier {
            Some(qualifier) => self.find_qualified_in_type_scope(
                name,
                qualifier,
                &TypeScope::in_file(from_uri),
                from_uri,
                index_only,
            ),
            None if index_only => self.resolve_symbol_index_only(name, None, from_uri),
            None => self.resolve_symbol(name, None, from_uri),
        }
    }

    fn find_qualified_in_type_scope(
        &self,
        name: &str,
        qualifier: &str,
        type_scope: &TypeScope<'_>,
        from_uri: &Url,
        index_only: bool,
    ) -> Vec<Location> {
        let io = if index_only {
            ResolveIo::IndexOnly
        } else {
            ResolveIo::Full
        };
        let locations =
            resolve_symbol_in_type_scope(self, name, qualifier, type_scope, from_uri, io);
        if !locations.is_empty() {
            return locations;
        }
        if SCOPE_FUNCTIONS.contains(&name) {
            return if index_only {
                self.resolve_symbol_index_only(name, None, from_uri)
            } else {
                self.resolve_symbol(name, None, from_uri)
            };
        }
        locations
    }

    /// Apply `f` to each *workspace* (non-library) definition location of `name`,
    /// returning the first `Some`. The single chokepoint for **receiver-less,
    /// best-effort by-name inference lookups** (return/field/callable/signature
    /// resolution that has no import or receiver scope).
    ///
    /// Ubiquitous names like `create`/`build` resolve to thousands of source-JAR
    /// definitions once those are indexed; scanning them all per lookup stalled
    /// inlay/hover for seconds. Library definitions are skipped (their metadata comes
    /// from indexed symbol detail / the JAR index) and the scan is capped — without a
    /// receiver type the exact overload can't be picked anyway.
    ///
    /// The (≤ [`MAX_BY_NAME_DEFS`]) candidate locations are snapshotted and the
    /// `definitions` read guard is dropped before `f` runs, so `f` may freely resolve
    /// other names (re-enter `definitions`) without risking a reader/writer deadlock
    /// on the shard.
    /// Whether a JAR-declared `name` at `loc` sits in a package the caller
    /// could have reached — same package, or covered by one of its imports.
    ///
    /// The workspace equivalent lives in `resolver::infer`, but reads
    /// `self.files`; JAR symbols live in `jar_files`, so the lookup differs even
    /// though the package/import rule is the same.
    ///
    /// Answers `false` when the JAR file's package is unknown, which only
    /// deprioritises the candidate: callers try unreachable ones afterwards
    /// rather than discarding them.
    pub(crate) fn jar_candidate_is_reachable(
        &self,
        loc: &Location,
        name: &str,
        caller_file_data: Option<&crate::types::FileData>,
    ) -> bool {
        let Some(jar_file_data) = self.jar_files.get(loc.uri.as_str()) else {
            return false;
        };
        // Receiver-less by-name fallback over ALL JAR definitions sharing
        // `name`, not an `ExtensionEntry`-scoped lookup, so there is no
        // per-symbol container available here. `None` preserves the existing
        // package/import check unchanged for this path; the visibility/
        // same-file arguments are inert whenever `entry_container` is `None`,
        // since the member-extension branch that reads them never runs.
        crate::resolver::infer::extension_is_in_scope(
            jar_file_data.package.as_ref(),
            name,
            None,
            crate::types::Visibility::Public,
            false,
            caller_file_data,
        )
    }
}
