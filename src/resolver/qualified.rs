//! Dot-qualified access (`receiver.member`): member/inherited-member/extension
//! precedence for both the uppercase (`Outer.Inner`) and lowercase
//! (`variable.field`) qualifier-root families.

use std::collections::HashSet;

use tower_lsp::lsp_types::{Location, Url};

use crate::indexer::Indexer;
use crate::types::CallerContext;
use crate::StrExt;

use super::container::resolve_companion_member;
use super::extension::resolve_extension_in_scope;
use super::find::{
    file_has_container_metadata, find_all_names_scoped_to_container,
    find_all_names_with_container_in_uri, find_name_in_uri, find_name_scoped_to_container,
};
use super::hierarchy::{
    walk_hierarchy, walk_hierarchy_breadth_first, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
use super::infer::{infer_field_type, infer_variable_type};
use super::resolve::{resolve_symbol, resolve_symbol_index_only, ResolveIo};

/// STAGE: parsing. What the qualifier's ROOT segment is, decided ONCE here
/// instead of re-sniffed by `starts_with_uppercase` at four separate points of
/// one function body. Takes no [`Indexer`] and does no IO, so its tests need no
/// fixture at all — before this existed there was no way to test "did we read
/// this qualifier correctly" separately from "did we find the symbol".
#[derive(Debug, PartialEq, Eq)]
pub(super) enum QualifierRoot<'a> {
    /// `this.member` — current file, then its own hierarchy.
    This,
    /// `super.member` — hierarchy only.
    Super,
    /// `Foo.member` / `Outer.Inner.member` — every segment names a type.
    TypePath {
        root: &'a str,
        /// The type segments after the root. Owned rather than the borrowed
        /// slice the design sketch named, because the split has to live
        /// somewhere and a returned value cannot borrow a local.
        nested: Vec<&'a str>,
    },
    /// `variable.field.member` — the root needs type inference before anything
    /// else can happen.
    ValuePath { root: &'a str, rest: Vec<&'a str> },
}

/// Split a dot-qualified receiver expression into its root and the segments
/// that follow it. See [`QualifierRoot`].
pub(super) fn parse_qualifier(qualifier: &str) -> QualifierRoot<'_> {
    let mut segments = qualifier.split('.');
    // `split` on a non-empty pattern always yields at least one item; the
    // fallback is unreachable and only avoids an `unwrap`.
    let root = segments.next().unwrap_or(qualifier);
    let rest: Vec<&str> = segments.collect();

    // `this`/`super` win on the ROOT segment alone, exactly as before — a
    // longer `this.field.member` still takes the keyword path.
    match root {
        "this" => QualifierRoot::This,
        "super" => QualifierRoot::Super,
        _ if root.starts_with_uppercase() => QualifierRoot::TypePath { root, nested: rest },
        _ => QualifierRoot::ValuePath { root, rest },
    }
}

pub(super) fn resolve_qualified(
    indexer: &Indexer,
    name: &str,
    qualifier: &str,
    from_uri: &Url,
    io: ResolveIo,
) -> Vec<Location> {
    let parsed = parse_qualifier(qualifier);

    // ── Keyword roots ────────────────────────────────────────────────────────
    // `this`/`super` are anchored on the CALL SITE's own file, not on a
    // resolved receiver type, so they never reach the anchor ladder below.
    match parsed {
        QualifierRoot::This => {
            let locations = find_name_in_uri(indexer, name, from_uri.as_str());
            if !locations.is_empty() {
                return locations;
            }
            return resolve_from_class_hierarchy(indexer, name, from_uri);
        }
        QualifierRoot::Super => return resolve_from_class_hierarchy(indexer, name, from_uri),
        _ => {}
    }

    // `Foo.member` with `Foo` a class name (not a variable) can only reach a
    // companion-object member in Kotlin — never an instance member of `Foo`,
    // even if one shares the name. Applies for ANY TypePath, not just a
    // single-segment one: `Outer.Inner.member` is still "member accessed on a
    // TYPE name", and `anchors_for` has already walked the anchor down to
    // `Inner` itself (the leaf `class_name`/`declaration` the loop below
    // probes) — `companion_member_on` looks up the companion of THAT leaf, so
    // gating on `nested.is_empty()` only skipped a nested type's own valid
    // companion-member access rather than protecting anything.
    let companion_applies = matches!(&parsed, QualifierRoot::TypePath { .. });

    for anchor in anchors_for(indexer, &parsed, from_uri, io) {
        if companion_applies {
            let companion_locations = companion_member_on(indexer, &anchor, name);
            if !companion_locations.is_empty() {
                return companion_locations;
            }
        }

        let candidates = candidates_on(indexer, &anchor, name, from_uri);
        if !candidates.is_empty() {
            return candidates.into_precedence_ordered();
        }
    }

    // Last resort, and only for a type root: an extension declared in a JAR
    // whose receiver is keyed on the qualifier's LEAF type name. Kept outside
    // the ladder because it keys the JAR extension registry directly by
    // spelling rather than by a resolved anchor — meaningful only when that
    // spelling is itself a type name, so a value root (`account.holder`,
    // where the root is a variable name) never reaches this branch. For
    // `Outer.Inner.member` the leaf is `Inner` (the last nested segment), not
    // `root` ("Outer") — `Inner` is the type the member is actually accessed
    // on, exactly as `anchors_for` already normalizes for the ladder above.
    if let QualifierRoot::TypePath { root, nested } = parsed {
        let leaf = nested.last().copied().unwrap_or(root);
        return jar_extension_for_type_root(indexer, leaf, name);
    }
    vec![]
}

/// STAGE: normalization output. The receiver a qualified lookup is anchored on,
/// after the root AND every nested segment have been walked. `class_name` is
/// the LEAF type's simple name — the extension-registry key — never the
/// root's. Carrying the two together by construction is what makes "probe
/// keyed on the root while the anchor has already moved to `Inner`"
/// unrepresentable rather than merely fixed.
#[derive(Debug, Clone)]
pub(super) struct ReceiverAnchor {
    /// The leaf type's own declaration `Location` (file *and* range) — `None`
    /// only for a receiver with no indexed declaration at all (a compiler
    /// built-in like `String`/`Int`, or a type this workspace cannot see),
    /// which can still carry in-scope extensions.
    ///
    /// A full `Location` rather than a bare `Url`, because
    /// [`find_all_names_scoped_to_container`] scopes its member search by
    /// matching the container's own declaration RANGE, not merely its file: a
    /// `Url` alone cannot build the own-member tier at all.
    pub(super) declaration: Option<Location>,
    /// Leaf type's simple name, nullability and package prefix already
    /// stripped.
    pub(super) class_name: String,
}

/// STAGE: normalization. Qualifier → receiver, and NOTHING else: no member
/// lookup, no extension probe, no precedence. Both of the qualifier families
/// (`Outer.Inner` and `variable.field`) collapse into this one function, which
/// is what makes the ladder in [`candidates_on`] provably share ONE anchor
/// instead of two anchors that drift apart.
///
/// Returns a `Vec` because a type root can resolve to several candidate
/// declarations; hoisting that loop to the caller makes its re-entrancy cost
/// visible in a signature instead of buried mid-body.
pub(super) fn anchors_for(
    indexer: &Indexer,
    root: &QualifierRoot<'_>,
    from_uri: &Url,
    io: ResolveIo,
) -> Vec<ReceiverAnchor> {
    match root {
        // Dispatched by `resolve_qualified` before it ever gets here.
        QualifierRoot::This | QualifierRoot::Super => vec![],
        QualifierRoot::TypePath { root, nested } => {
            type_path_anchors(indexer, root, nested, from_uri, io)
        }
        QualifierRoot::ValuePath { root, rest } => value_path_anchor(indexer, root, rest, from_uri)
            .into_iter()
            .collect(),
    }
}

/// `Outer.Inner.member`: every qualifier segment names a type, so the root
/// resolves to one or more declarations and each nested segment walks into
/// that declaration's own scope.
fn type_path_anchors(
    indexer: &Indexer,
    root: &str,
    nested: &[&str],
    from_uri: &Url,
    io: ResolveIo,
) -> Vec<ReceiverAnchor> {
    // Honors the caller's IO policy — an IndexOnly caller (the
    // resolution-accuracy benchmark's own index-only path) must not spawn
    // rg/fd resolving the qualifier root any more than it may for a bare
    // reference.
    let root_locations = if matches!(io, ResolveIo::IndexOnly) {
        resolve_symbol_index_only(indexer, root, None, from_uri)
    } else {
        resolve_symbol(indexer, root, None, from_uri)
    };

    let anchors: Vec<ReceiverAnchor> = root_locations
        .iter()
        .filter_map(|root_location| {
            // Walk any remaining nested-type segments (`Event.OverdraftInput`
            // has one: `OverdraftInput`) into that specific nested class's own
            // scope, so a same-named sibling member never shadows the
            // actually-requested nested type's own member. A segment that
            // cannot be walked drops this candidate entirely.
            let mut declaration = root_location.clone();
            let mut class_name = root;
            for &nested_segment in nested {
                declaration = find_name_scoped_to_container(indexer, nested_segment, &declaration)?;
                class_name = nested_segment;
            }
            Some(ReceiverAnchor {
                declaration: Some(declaration),
                class_name: class_name.to_owned(),
            })
        })
        .collect();
    if !anchors.is_empty() {
        return anchors;
    }

    // A required nested segment failed to resolve (`Outer.Missing.member`,
    // where `Missing` names no real nested type of `Outer`) must not fall
    // back to anchoring on `root` — that would silently resolve `member`
    // against `Outer` itself, as if `.Missing` had never been written. Only
    // an UNRESOLVED ROOT gets the declaration-less fallback below.
    if !nested.is_empty() && !root_locations.is_empty() {
        return vec![];
    }

    // The root names no indexed declaration (a built-in receiver, or a type
    // this workspace cannot see) — it can still carry in-scope extensions, so
    // hand back a declaration-less anchor rather than nothing at all. Without
    // this, the own-type extension tier would be unreachable for exactly the
    // receivers that have no member tier to begin with.
    vec![ReceiverAnchor {
        declaration: None,
        class_name: root.to_owned(),
    }]
}

/// `variable.field.member`: the root is a value whose declared type must be
/// inferred first, after which each further segment is either a nested type or
/// a field access whose own type is inferred in turn.
fn value_path_anchor(
    indexer: &Indexer,
    root: &str,
    rest: &[&str],
    from_uri: &Url,
) -> Option<ReceiverAnchor> {
    let start_type = infer_variable_type(indexer, root, from_uri)?;
    // A nullable receiver resolves members from its underlying (non-null)
    // class, so drop any trailing `?` before resolving the type to a file —
    // otherwise `resolve_symbol("Confirmation?")` would find nothing.
    let start_type = start_type.strip_nullable();

    // `start_type` may be a dotted nested type like `Outer.Inner`. Split into
    // outer (for file resolution) and optional inner (nested class).
    let (outer_type, inner_type) = match start_type.find('.') {
        Some(dot) => (&start_type[..dot], Some(&start_type[dot + 1..])),
        None => (start_type, None),
    };

    let mut declaration = resolve_symbol(indexer, outer_type, None, from_uri)
        .into_iter()
        .next();
    // The receiver's own base type name, tracked alongside `declaration` for
    // the in-scope extension tier — kept even when `declaration` is `None` (a
    // built-in/stdlib type like `String` has no indexed declaration file, but
    // can still have in-scope extensions).
    let mut class_name = outer_type.last_segment().to_owned();

    // A nested type component (`Factory` in `Outer.Factory`) is recorded as a
    // trailing qualifier segment rather than a file change, since nested types
    // live in the same file. A deeply-nested type like `Scenes.Confirmation`
    // must be split per level — searching for a literal `"Scenes.Confirmation"`
    // symbol finds nothing, since each nested class is indexed on its own name.
    let extra_segments: Vec<&str> = inner_type
        .map(|nested| nested.split('.').collect())
        .unwrap_or_default();

    for &segment in extra_segments.iter().chain(rest.iter()) {
        let current_uri = declaration.as_ref()?.uri.clone();
        if segment.starts_with_uppercase() {
            // Nested class / companion object — likely in the same file.
            // Search the current file first; fall back to a global resolve.
            let in_file = find_name_in_uri(indexer, segment, current_uri.as_str());
            declaration = if in_file.is_empty() {
                resolve_symbol(indexer, segment, None, from_uri)
                    .into_iter()
                    .next()
            } else {
                in_file.into_iter().next()
            };
            class_name = segment.to_owned();
        } else {
            // Field access: infer the declared type of this field.
            let field_type = infer_field_type(indexer, current_uri.as_str(), segment)?;
            declaration = resolve_symbol(indexer, &field_type, None, from_uri)
                .into_iter()
                .next();
            class_name = field_type.strip_nullable().last_segment().to_owned();
        }
    }

    Some(ReceiverAnchor {
        declaration,
        class_name,
    })
}

/// STAGE: aggregation. The four tiers, named and separately populated, so that
/// "which order the branches run in" is carried by field order in one struct
/// rather than by statement order in two drifted branches.
pub(super) struct QualifiedCandidates {
    /// Members declared directly inside the anchor's own body.
    own_members: Vec<Location>,
    /// Members reached through the anchor's superclass/interface hierarchy.
    inherited_members: Vec<Location>,
    /// An in-scope extension declared on the anchor's OWN leaf type.
    own_type_extension: Option<Location>,
    /// An in-scope extension declared on one of the anchor's ancestors.
    supertype_extension: Option<Location>,
}

impl QualifiedCandidates {
    fn is_empty(&self) -> bool {
        self.own_members.is_empty()
            && self.inherited_members.is_empty()
            && self.own_type_extension.is_none()
            && self.supertype_extension.is_none()
    }

    /// `own_members` → `inherited_members` → `own_type_extension` →
    /// `supertype_extension`: Kotlin's real member-over-extension precedence, in
    /// ONE place, for ONE anchor.
    fn into_precedence_ordered(self) -> Vec<Location> {
        let mut ordered = self.own_members;
        ordered.extend(self.inherited_members);
        ordered.extend(self.own_type_extension);
        ordered.extend(self.supertype_extension);
        ordered
    }
}

/// STAGE: business logic. Runs the whole precedence ladder against one anchor.
///
/// The tier ORDER is a decided behaviour, not a formality: a real member
/// (own or inherited) always wins over a same-named extension when both are
/// arity-compatible. The type-root family used to probe its own-type extension
/// FIRST and return early, before its member tier was ever computed; that was
/// the outlier and it is corrected here rather than averaged.
pub(super) fn candidates_on(
    indexer: &Indexer,
    anchor: &ReceiverAnchor,
    name: &str,
    from_uri: &Url,
) -> QualifiedCandidates {
    let (own_members, inherited_members) = member_tiers(indexer, anchor, name, from_uri);

    // Kotlin's Java-interop synthetic-property rule: `obj.fail` may really mean
    // `obj.getFail()`. Retry the identical member/inherited-member pair with the
    // getter name, but ONLY after a real `name` member came up completely
    // empty — a real member always wins. A Kotlin `fun getFail()` is never
    // exposed as `.fail` from Kotlin, so every getter candidate the retry
    // finds is filtered by ITS OWN declaring file's language, not the
    // receiver's: gating on `anchor.declaration`'s language (the earlier
    // shape) rejected a getter genuinely inherited from a Java ANCESTOR
    // whenever the receiver's own class happens to be Kotlin-declared — real
    // shape, e.g. `class KotlinFoo : JavaBase()` where only `JavaBase`
    // declares `getFail()`. `setFoo`/`isFoo`/records are deliberately out of
    // scope — see the 2026-09-09 jar-promotion-latency-budget-plan, Task 1.
    let (own_members, inherited_members) = if own_members.is_empty() && inherited_members.is_empty()
    {
        let (getter_own, getter_inherited) =
            member_tiers(indexer, anchor, &kotlin_getter_name(name), from_uri);
        (
            retain_java_declared(getter_own),
            retain_java_declared(getter_inherited),
        )
    } else {
        (own_members, inherited_members)
    };

    // Member-over-extension precedence is carried by this tier's POSITION in
    // `into_precedence_ordered`, not by refusing to compute it: a same-named
    // member doesn't always satisfy the actual call's arity, and dropping the
    // extension outright leaves a shape-aware caller with nothing to fall back
    // to. Measured on the Moneta corpus: `IMockProvider.loadJSONFromAssets`
    // (a 1-arg extension shadowed by a 2-arg member of the same name, 791 call
    // sites) resolves to the member alone under a short-circuit, fails arity
    // filtering, and lands in the ambiguous bucket instead of resolving. Same
    // reasoning as `supertype_extension` below, which has always been appended
    // alongside a winning member rather than instead of one.
    let own_type_extension =
        resolve_extension_in_scope(indexer, &anchor.class_name, name, from_uri)
            .into_iter()
            .next();

    // A same-named real member doesn't always satisfy the actual call's arity
    // (e.g. `navController.navigate(route = ...)`: a wrong-arity JVM member
    // `NavController.navigate(Uri)` vs. the wanted KTX extension
    // `NavController.navigate(route: String, ...)`), so this tier is appended
    // ALONGSIDE a winning own-member tier rather than only when everything
    // missed — members still win when arity-compatible, but a shape-aware
    // caller now has the extension to fall back to instead of an empty result.
    //
    // Computed unconditionally, same as `own_type_extension` above and for
    // the identical reason (Copilot review finding on this PR, matching the
    // exact shape `own_type_extension` was already fixed for): an inherited
    // member with the WRONG arity used to make this branch skip the walk
    // entirely, so a shape-aware caller lost the supertype extension it could
    // have fallen back to — e.g. `derived.foo()` where the inherited `foo`
    // only exists at a different arity and the applicable overload is really
    // `fun Base.foo()`. The extra hierarchy walk is not doubled cost:
    // `resolve_extension_via_supertype_hierarchy`'s own doc notes
    // `promote_candidates_bounded` memoizes per-JAR materialization, so an
    // ancestor `resolve_from_class_hierarchy_scoped` already visited is a
    // free set lookup here.
    let supertype_extension = anchor.declaration.as_ref().and_then(|declaration| {
        resolve_extension_via_supertype_hierarchy(
            indexer,
            &anchor.class_name,
            &declaration.uri,
            name,
            from_uri,
        )
        .into_iter()
        .next()
    });

    QualifiedCandidates {
        own_members,
        inherited_members,
        own_type_extension,
        supertype_extension,
    }
}

/// The direct-container then inherited-member lookup pair, as the two tiers
/// they are. Scoped to the anchor's own class and declaring file, not to the
/// call site — the qualifier and the call site are commonly different files.
///
/// Every same-named candidate, not just the first match: `name` may be an
/// overloaded Java/Kotlin function, and collapsing to one arbitrary overload
/// here (before the caller's own arity-based shape filtering ever runs) would
/// make nearly every real call site to a DIFFERENT overload resolve to nothing
/// (see `find_all_names_scoped_to_container`'s doc).
fn member_tiers(
    indexer: &Indexer,
    anchor: &ReceiverAnchor,
    name: &str,
    from_uri: &Url,
) -> (Vec<Location>, Vec<Location>) {
    // A built-in receiver has no indexed declaration, so it has no member tier
    // at all — only extensions can apply to it.
    let Some(declaration) = anchor.declaration.as_ref() else {
        return (vec![], vec![]);
    };

    let own_members = find_all_names_scoped_to_container(indexer, name, declaration);
    if !own_members.is_empty() {
        return (own_members, vec![]);
    }

    // The anchor's own body may not declare `name` directly, but a superclass
    // may (e.g. `object Manager : AbstractManager<T>()` inheriting
    // `requireComponent`) — the same situation the `this`/`super` roots handle.
    let inherited_members = resolve_from_class_hierarchy_scoped(
        indexer,
        name,
        &anchor.class_name,
        &declaration.uri,
        from_uri,
    );
    (vec![], inherited_members)
}

/// STAGE: business logic, kept separate because it answers a DIFFERENT
/// question — `Foo.member` with `Foo` a class name can only ever reach a
/// companion member. A declaration-less (built-in) anchor has no companion
/// object to look up.
pub(super) fn companion_member_on(
    indexer: &Indexer,
    anchor: &ReceiverAnchor,
    name: &str,
) -> Vec<Location> {
    match anchor.declaration.as_ref() {
        Some(declaration) => {
            resolve_companion_member(indexer, name, &anchor.class_name, declaration.uri.as_str())
        }
        None => vec![],
    }
}

/// Extension functions may live in a different file than the receiver class,
/// including inside a JAR the workspace never parsed as source. Atomic
/// promote+read (zero budget): `resolve_qualified` is on both the
/// goto-definition and the per-call-site diagnostics path.
fn jar_extension_for_type_root(indexer: &Indexer, root: &str, name: &str) -> Vec<Location> {
    let mut cache_backed_only = 0usize;
    let Some(entries) =
        crate::indexer::jar::extension_entries_for(indexer, root, &mut cache_backed_only)
    else {
        return vec![];
    };
    for entry in entries.iter() {
        if entry.name != name {
            continue;
        }
        let Ok(uri) = Url::parse(&entry.file_uri) else {
            continue;
        };
        // Look up the symbol in the declaring file for an accurate range.
        let range = indexer
            .files
            .get(&entry.file_uri)
            .or_else(|| indexer.jar_files.get(&entry.file_uri))
            .and_then(|file_data| {
                file_data
                    .symbols
                    .iter()
                    .find(|symbol| {
                        crate::resolver::infer::extension_declaration_matches(
                            symbol,
                            name,
                            root,
                            entry.container.as_ref(),
                        )
                    })
                    .map(|symbol| symbol.selection_range)
            })
            .unwrap_or_default();
        return vec![Location { uri, range }];
    }
    vec![]
}

/// Walk the superclass / interface hierarchy of the class(es) declared in
/// `from_uri` looking for a symbol named `name`.
///
/// Algorithm
/// ---------
/// 1. Extract direct supertype names from `from_uri`'s lines.
/// 2. Resolve each supertype through the normal chain (imports, same-package…).
/// 3. Search the resolved file's symbol table for `name`.
/// 4. Recurse into that file's own supertypes (depth-limited, cycle-safe).
pub(super) fn resolve_from_class_hierarchy(
    indexer: &Indexer,
    name: &str,
    from_uri: &Url,
) -> Vec<Location> {
    resolve_from_class_hierarchy_scoped(indexer, name, "", from_uri, from_uri)
}

/// Like [`resolve_from_class_hierarchy`] but scoped to one specific class's
/// own declared supertypes (`start_class`) instead of every class declared in
/// `from_uri`'s file. Needed when the class and the caller can be different
/// files — `Foo.member()` where `Foo` is a type/object name, not `this`/`super`
/// (which are always resolved from inside the class they refer to, so the
/// unscoped whole-file walk was never wrong for those callers).
///
/// `start_uri` is where `start_class` is declared (commonly a JAR for a
/// library receiver type); `origin_uri` is the real call-site file, used only
/// for `walk_hierarchy`'s module-scoped ambiguity tie-break, which needs a
/// real `file://` path to map back to an owning module. The two coincide for
/// [`resolve_from_class_hierarchy`]'s callers (`this`/`super`, always resolved
/// from inside their own file) but not for [`resolve_qualified`]'s
/// `Foo.member()` callers, where `start_uri` is `Foo`'s own declaring file.
pub(super) fn resolve_from_class_hierarchy_scoped(
    indexer: &Indexer,
    name: &str,
    start_class: &str,
    start_uri: &Url,
    origin_uri: &Url,
) -> Vec<Location> {
    // Deep enough for real Android/Kotlin hierarchies: app base classes often stack
    // several levels (`…Fragment → BaseFragment → … → androidx Fragment`) before the
    // library super that declares an inherited member like `requireActivity`. The
    // visited-set bounds total work regardless of depth.
    let results = walk_hierarchy(
        indexer,
        start_class,
        start_uri.as_str(),
        CallerContext {
            uri: Some(origin_uri.as_str()),
            cursor_line: None,
        },
        12,
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        // `find_name_in_uri` (used here previously) has no notion of the
        // ancestor CLASS at all -- it returns the first same-named symbol
        // anywhere in the file, arity-blind. That's silently wrong once a
        // JAR packs several classes into one synthetic FileData (real bug:
        // `NavHostController extends NavController`, both from the same
        // `navigation-runtime` JAR, in the same file): a same-named method
        // on an unrelated sibling class in that file could win, and even a
        // real hit collapsed to one arbitrary overload instead of every
        // arity `name` has on `class_name`. Prefer every overload actually
        // tagged as belonging to `class_name`; fall back to the old
        // whole-file behavior only when nothing is container-tagged (e.g. a
        // degenerate fixture with no container info at all).
        //
        // Copilot review finding (real): an empty `scoped` does NOT mean "no
        // container data" -- it commonly means "this specific ancestor has
        // no member named `name`", which is the normal, expected outcome
        // while walking UP a multi-level hierarchy. Falling through to an
        // unscoped `find_name_in_uri` on every such miss reintroduces the
        // exact cross-container leak `find_all_names_with_container_in_uri`
        // exists to prevent (a same-named sibling class sharing the JAR's
        // synthetic per-file symbol table wins by pure file position). Only
        // fall back when the file carries NO container tags at all.
        |index, class_name, class_uri, _| {
            let scoped = find_all_names_with_container_in_uri(index, name, class_name, class_uri);
            if !scoped.is_empty() {
                scoped
            } else if file_has_container_metadata(index, class_uri) {
                vec![]
            } else {
                find_name_in_uri(index, name, class_uri)
            }
        },
    );
    // Stable dedup via HashSet — diamond inheritance can produce the same location
    // via multiple paths; dedup_by only removes consecutive duplicates.
    let mut seen = HashSet::new();
    results
        .into_iter()
        .filter(|loc| {
            seen.insert((
                loc.uri.clone(),
                loc.range.start.line,
                loc.range.start.character,
            ))
        })
        .collect()
}

/// Kotlin's Java-interop synthetic-property name for a getter-style call:
/// `fail` -> `getFail`. Deliberately the ONE direction (read, not `setFoo`)
/// and ONE prefix (`getFoo`, not `isFoo` — Kotlin already maps `isFoo()` to
/// the *identity*-named property `isFoo`) this task implements; see the
/// jar-promotion-latency-budget-plan Task 1 brief for why the rest is out of
/// scope.
fn kotlin_getter_name(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => format!("get{}{}", first.to_uppercase(), chars.as_str()),
        None => "get".to_owned(),
    }
}

/// Whether `uri` names a Java-declared symbol: a real `.java` source file, or
/// a JAR-derived synthetic entry compiled from a `.class` (a sidecar/sources-
/// JAR per-entry URI ends `...!/<Entry>.class`). Derived from the URI's own
/// extension, per [`crate::types::Language::from_path`]'s existing
/// extension-based dispatch, rather than guessing from the symbol name — a
/// Kotlin-declared `fun getFail()` is NOT exposed as `.fail` from Kotlin, so
/// this guard is what stops the getter retry from inventing a resolution
/// Kotlin itself rejects.
fn is_java_declaring_file(uri: &Url) -> bool {
    let path = uri.as_str();
    path.ends_with(".java") || path.ends_with(".class")
}

/// Keep only the getter-retry candidates actually declared in a Java file —
/// see [`candidates_on`]'s doc comment for why this checks each candidate's
/// OWN declaring `Location`, not the receiver's.
fn retain_java_declared(locations: Vec<Location>) -> Vec<Location> {
    locations
        .into_iter()
        .filter(|location| is_java_declaring_file(&location.uri))
        .collect()
}

/// Extension-lookup counterpart to [`resolve_from_class_hierarchy_scoped`]:
/// tried only when a member/inherited-member lookup on the concrete receiver
/// type already failed, so a real member always shadows a same-named
/// ancestor extension. `extension_by_receiver`/`resolve_extension_in_scope`
/// are an exact-string-key lookup on the receiver's own leaf type name — a
/// receiver like `String` (implements `CharSequence`) never matches an
/// extension keyed `"CharSequence"` without this walk, the single largest
/// measured component of the resolution-accuracy benchmark's ambiguous
/// (FilteredCandidate) bucket on a real corpus.
///
/// `origin_uri` is the real call-site file, not `start_uri` (`anchor_uri`,
/// commonly a JAR-backed receiver type) — `walk_hierarchy`'s module-scoped
/// tie-break needs a real `file://` origin to map back to an owning module,
/// which a `jar:` URI can never provide.
///
/// This walks the same chain `resolve_from_class_hierarchy_scoped` just
/// walked, at the same budget — not a doubled cost: `promote_candidates_bounded`
/// memoizes `materialized`/`materialization_failed` per JAR, so re-visiting
/// an ancestor the first walk already attempted is a free set lookup. This
/// walk's own budget only spends anything new on ancestors beyond wherever
/// the first walk's budget ran out — the deep multi-hop case (see the real
/// 4-hop `AppCompatActivity → … → Activity` shape PR #286 fixed for member
/// lookup) this fallback exists to reach.
fn resolve_extension_via_supertype_hierarchy(
    indexer: &Indexer,
    start_class: &str,
    start_uri: &Url,
    name: &str,
    origin_uri: &Url,
) -> Vec<Location> {
    // Breadth-first (`walk_hierarchy_breadth_first`), not `walk_hierarchy`'s
    // depth-first order: Kotlin's own extension resolution prefers the most
    // specific (nearest) applicable receiver type, and depth-first fully
    // explores one direct supertype's entire chain before ever touching a
    // SIBLING direct supertype — so with multiple direct supertypes (an
    // ordinary Kotlin shape, e.g. implementing several interfaces), a
    // farther ancestor down the first branch could otherwise outrank a
    // nearer, directly-implemented one down a sibling branch.
    let matches = walk_hierarchy_breadth_first(
        indexer,
        start_class,
        start_uri.as_str(),
        CallerContext {
            uri: Some(origin_uri.as_str()),
            cursor_line: None,
        },
        12,
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        // `super_name` is already the simple leaf name — `supertype_targets`
        // (hierarchy.rs) normalizes a fully-qualified supertype spelling
        // (`class Str : com.other.Seq`) before yielding it.
        |idx, super_name, _, _| resolve_extension_in_scope(idx, super_name, name, origin_uri),
    );
    // The breadth-first walk already stops at the nearest level with any
    // match, but two SIBLING supertypes at that same level could both have
    // one (a genuine tie Kotlin itself would flag as a compile error) —
    // take just the first, matching this function's single-location
    // contract rather than surfacing a spurious multi-candidate ambiguity.
    matches.into_iter().next().into_iter().collect()
}
