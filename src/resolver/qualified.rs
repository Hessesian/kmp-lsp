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
    find_all_names_scoped_to_container, find_all_names_with_container_in_uri, find_name_in_uri,
    find_name_scoped_to_container,
};
use super::hierarchy::{
    walk_hierarchy, walk_hierarchy_breadth_first, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
use super::infer::{infer_field_type, infer_variable_type};
use super::resolve::{resolve_symbol, resolve_symbol_index_only, ResolveIo};

/// Step 0 — dot-qualified access.
///
/// Handles two families of chains:
///
/// **Uppercase root** (`Outer.Inner`, `A.B.C.D`): all segments are class/object
/// names; the root identifies the file and all nested types live in the same
/// file, so we resolve root → file and search that file for `name`.
///
/// **Lowercase root** (`variable.field`, `account.account.interestPlanCode`):
/// the first segment is a variable/parameter — we infer its declared type, then
/// traverse every subsequent lowercase segment as a field access (inferring each
/// field's type in turn) until we have a file to search `name` in.
/// Uppercase segments inside a lowercase chain are treated as nested class names
/// within the current file.
pub(super) fn resolve_qualified(
    indexer: &Indexer,
    name: &str,
    qualifier: &str,
    from_uri: &Url,
    io: ResolveIo,
) -> Vec<Location> {
    let segments: Vec<&str> = qualifier.split('.').collect();
    let root = segments[0];

    // ── `this.member` — search current file and its superclass hierarchy ──────
    if root == "this" {
        let locs = find_name_in_uri(indexer, name, from_uri.as_str());
        if !locs.is_empty() {
            return locs;
        }
        return resolve_from_class_hierarchy(indexer, name, from_uri);
    }

    // ── `super.member` — search superclass hierarchy only ────────────────────
    if root == "super" {
        return resolve_from_class_hierarchy(indexer, name, from_uri);
    }

    if root.starts_with_uppercase() {
        let root_base = root.last_segment();

        // Extension functions take precedence over member functions,
        // but only when they are in scope (same package or imported).
        let ext_locs = resolve_extension_in_scope(indexer, root_base, name, from_uri);
        if !ext_locs.is_empty() {
            return ext_locs;
        }

        // Then check member functions (same-file). Honors the caller's IO
        // policy — an IndexOnly caller (the resolution-accuracy benchmark's
        // own index-only path) must not spawn rg/fd resolving the qualifier
        // root any more than it may for a bare reference.
        let qual_locs = if matches!(io, ResolveIo::IndexOnly) {
            resolve_symbol_index_only(indexer, root, None, from_uri)
        } else {
            resolve_symbol(indexer, root, None, from_uri)
        };
        for qual_loc in &qual_locs {
            // `Foo.member` with `Foo` a class name (not a variable) can only reach a
            // companion-object member in Kotlin — never an instance member of `Foo`,
            // even if one shares the name. Try the companion first so a same-named
            // instance member declared earlier in the file can't shadow it.
            //
            // Only the single-segment `Foo.member` form names `root` as the
            // qualifying class. For a multi-segment qualifier like
            // `Outer.Inner.member`, `root` is `Outer` — not the class the member
            // is accessed on — so probing `Outer`'s companion would mis-resolve;
            // fall through to the nested-segment handling instead.
            if segments.len() == 1 {
                let companion_locs =
                    resolve_companion_member(indexer, name, root, qual_loc.uri.as_str());
                if !companion_locs.is_empty() {
                    return companion_locs;
                }
            }

            // Walk any remaining nested-type segments (`Event.OverdraftInput` has
            // one: `OverdraftInput`) to that specific nested class's own scope
            // before searching for `name`, so a same-named sibling member never
            // shadows the actually-requested nested type's own member.
            let mut anchor = qual_loc.clone();
            let mut anchor_class_name = root_base;
            let mut nested_segments_resolved = true;
            for &nested_segment in &segments[1..] {
                match find_name_scoped_to_container(indexer, nested_segment, &anchor) {
                    Some(location) => {
                        anchor = location;
                        anchor_class_name = nested_segment;
                    }
                    None => {
                        nested_segments_resolved = false;
                        break;
                    }
                }
            }
            if !nested_segments_resolved {
                continue;
            }

            // Every same-named candidate, not just the first match — `name`
            // may be an overloaded Java/Kotlin function, and collapsing to
            // one arbitrary overload here (before the caller's own
            // arity-based shape filtering ever runs) would make nearly
            // every real call site to a DIFFERENT overload resolve to
            // nothing (see `find_all_names_scoped_to_container`'s doc).
            //
            // `anchor`'s own body may not declare `name` directly, but it may
            // live on a superclass instead (e.g. `object Manager :
            // AbstractManager<T>()` inheriting `requireComponent`), the same
            // situation the `this`/`super` branches above already handle.
            // Scoped to `anchor`'s own class and declaring file, not
            // `from_uri` — the qualifier and the call site are commonly
            // different files. `member_or_inherited_member` bundles both
            // lookups so the Java-getter retry below can run the identical
            // pair under a different name instead of duplicating it.
            let real_member_locs =
                member_or_inherited_member(indexer, name, &anchor, anchor_class_name, from_uri);
            if !real_member_locs.is_empty() {
                return real_member_locs;
            }

            // Kotlin's Java-interop synthetic-property rule: `obj.fail` may
            // really mean `obj.getFail()`. Retry the identical member/
            // inherited-member pair with the getter name, but ONLY when the
            // declaring file is Java and ONLY after a real `name` member came
            // up completely empty — a real member always wins, and a Kotlin
            // `fun getFail()` is never exposed as `.fail` from Kotlin (the
            // Java-file guard is load-bearing, not a nicety). `setFoo`/`isFoo`/
            // records are deliberately out of scope for this task — see the
            // 2026-09-09 jar-promotion-latency-budget-plan, Task 1.
            if is_java_declaring_file(&anchor.uri) {
                let getter_name = kotlin_getter_name(name);
                let getter_locs = member_or_inherited_member(
                    indexer,
                    &getter_name,
                    &anchor,
                    anchor_class_name,
                    from_uri,
                );
                if !getter_locs.is_empty() {
                    return getter_locs;
                }
            }

            // `anchor`'s own class has no member or inherited member named
            // `name` (resolve_from_class_hierarchy_scoped's callback is a pure
            // member lookup — it has NOT ruled out an in-scope extension on
            // `anchor`'s own class) — check `anchor`'s ancestors for a
            // supertype extension.
            let supertype_ext_locs = resolve_extension_via_supertype_hierarchy(
                indexer,
                anchor_class_name,
                &anchor.uri,
                name,
                from_uri,
            );
            if !supertype_ext_locs.is_empty() {
                return supertype_ext_locs;
            }
        }
        // Extension functions may live in a different file than the receiver class.
        // Atomic promote+read (zero budget): `resolve_qualified` is on both the
        // goto-definition and the per-call-site diagnostics path.
        let root_base = root.last_segment();
        let mut cache_backed_only = 0usize;
        if let Some(entries) =
            crate::indexer::jar::extension_entries_for(indexer, root_base, &mut cache_backed_only)
        {
            for entry in entries.iter() {
                if entry.name == name {
                    if let Ok(uri) = Url::parse(&entry.file_uri) {
                        // Look up the symbol in the declaring file for accurate range.
                        let range = indexer
                            .files
                            .get(&entry.file_uri)
                            .or_else(|| indexer.jar_files.get(&entry.file_uri))
                            .and_then(|fd| {
                                fd.symbols
                                    .iter()
                                    .find(|s| {
                                        crate::resolver::infer::extension_declaration_matches(
                                            s,
                                            name,
                                            root_base,
                                            entry.container.as_ref(),
                                        )
                                    })
                                    .map(|s| s.selection_range)
                            })
                            .unwrap_or_default();
                        return vec![Location { uri, range }];
                    }
                }
            }
        }
        return vec![];
    }

    // ── Lowercase root: variable / parameter type inference ──────────────────
    let Some(start_type) = infer_variable_type(indexer, root, from_uri) else {
        return vec![];
    };
    // A nullable receiver resolves members from its underlying (non-null) class,
    // so drop any trailing `?` before resolving the type to a file — otherwise
    // `resolve_symbol("Confirmation?")` would find nothing.
    let start_type = start_type.strip_nullable();

    // `start_type` may be a dotted nested type like `Outer.Inner`.
    // Split into outer (for file resolution) and optional inner (nested class).
    let (outer_type, inner_type) = match start_type.find('.') {
        Some(dot) => (&start_type[..dot], Some(&start_type[dot + 1..])),
        None => (start_type, None),
    };

    // Resolve the variable's type to its source file.
    let type_locs = resolve_symbol(indexer, outer_type, None, from_uri);
    let mut current_file: Option<String> = type_locs.first().map(|l| l.uri.to_string());
    // The receiver's own base type name, tracked alongside `current_file` for
    // the in-scope extension-function fallback below — kept even when
    // `current_file` is `None` (a built-in/stdlib type like `String` has no
    // indexed declaration file, but can still have in-scope extensions).
    let mut current_type_base: String = outer_type.last_segment().to_string();

    // If there's a nested type component (e.g. `Factory` in `Outer.Factory`),
    // the members we want to search are inside that nested type.
    // We don't need to change `current_file` because nested types live in the
    // same file; instead we record each nested level as a trailing qualifier
    // segment to process. A deeply-nested type like `Scenes.Confirmation` must
    // be split per-level — searching for a literal `"Scenes.Confirmation"`
    // symbol finds nothing, since each nested class is indexed on its own name.
    let extra_segments: Vec<&str> = inner_type
        .map(|t| t.split('.').collect())
        .unwrap_or_default();

    // Traverse remaining qualifier segments (plus any from the nested type).
    for &seg in extra_segments.iter().chain(segments[1..].iter()) {
        let Some(ref uri) = current_file else {
            return vec![];
        };
        if seg.starts_with_uppercase() {
            // Nested class / companion object — likely in the same file.
            // Search current file first; fall back to a global resolve.
            let locs = find_name_in_uri(indexer, seg, uri);
            current_file = if !locs.is_empty() {
                locs.first().map(|l| l.uri.to_string())
            } else {
                resolve_symbol(indexer, seg, None, from_uri)
                    .first()
                    .map(|l| l.uri.to_string())
            };
            current_type_base = seg.to_string();
        } else {
            // Field access: infer the declared type of this field.
            let Some(field_type) = infer_field_type(indexer, uri, seg) else {
                return vec![];
            };
            let locs = resolve_symbol(indexer, &field_type, None, from_uri);
            current_file = locs.first().map(|l| l.uri.to_string());
            current_type_base = field_type.strip_nullable().last_segment().to_string();
        }
    }

    // Search the resolved type's file for the target member, then its
    // superclass/interface hierarchy — Kotlin member (including inherited)
    // resolution always shadows a same-named extension function, so both are
    // tried before falling to the extension-in-scope lookup below.
    if let Some(ref resolved_uri) = current_file {
        let locs = find_name_in_uri(indexer, name, resolved_uri);
        if !locs.is_empty() {
            return match Url::parse(resolved_uri) {
                Ok(parsed_uri) => with_supertype_extension_fallback(
                    indexer,
                    locs,
                    &current_type_base,
                    &parsed_uri,
                    name,
                    from_uri,
                ),
                Err(_) => locs,
            };
        }
        if let Ok(parsed_uri) = Url::parse(resolved_uri) {
            let hierarchy_locs = resolve_from_class_hierarchy(indexer, name, &parsed_uri);
            if !hierarchy_locs.is_empty() {
                return hierarchy_locs;
            }
        }
    }

    // No member or inherited member named `name` on the receiver's type (this
    // also covers built-in/stdlib receivers like `String`/`Int`, which have
    // no indexed declaration file at all, so `current_file` is `None`) — the
    // call may still be a same-named, receiver-scoped extension function
    // declared elsewhere in the workspace. Without this, callers fell
    // straight through to the receiver-blind global bare-name search, which
    // can't distinguish `String.toViewText` from an unrelated
    // `SomeEnum.toViewText` and simply declines when both exist — a real,
    // measured source of ambiguous member-call resolution (see the
    // 2026-08-26 resolution-accuracy investigation).
    resolve_extension_in_scope(indexer, &current_type_base, name, from_uri)
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
fn resolve_from_class_hierarchy_scoped(
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
        |index, class_name, class_uri, _| {
            let scoped = find_all_names_with_container_in_uri(index, name, class_name, class_uri);
            if !scoped.is_empty() {
                scoped
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

/// The direct-container then inherited-member lookup pair every `Foo.member`
/// qualified lookup tries before falling to extension fallbacks. Extracted
/// into one named helper so the Java-getter synthetic-property retry
/// (`obj.fail` -> `obj.getFail()`) can run the identical two calls under a
/// different name, instead of duplicating the pair — and so the ordering
/// ("real member first, synthetic getter second") reads as two sequential
/// calls in `resolve_qualified` rather than a boolean flag threaded through
/// one shared body.
fn member_or_inherited_member(
    indexer: &Indexer,
    name: &str,
    anchor: &Location,
    anchor_class_name: &str,
    from_uri: &Url,
) -> Vec<Location> {
    let member_locs = find_all_names_scoped_to_container(indexer, name, anchor);
    if !member_locs.is_empty() {
        return with_supertype_extension_fallback(
            indexer,
            member_locs,
            anchor_class_name,
            &anchor.uri,
            name,
            from_uri,
        );
    }

    resolve_from_class_hierarchy_scoped(indexer, name, anchor_class_name, &anchor.uri, from_uri)
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

/// A same-named real member doesn't always satisfy the actual call's arity
/// (e.g. `navController.navigate(route = ...)`: a wrong-arity JVM member
/// `NavController.navigate(Uri)` vs. the wanted KTX extension
/// `NavController.navigate(route: String, ...)`). Appends the supertype-walk
/// extension after `member_locs` rather than replacing it — members still
/// win when arity-compatible (Kotlin's own precedence), but a shape-aware
/// caller now has the extension to fall back to instead of an empty result.
fn with_supertype_extension_fallback(
    indexer: &Indexer,
    member_locs: Vec<Location>,
    anchor_class_name: &str,
    anchor_uri: &Url,
    name: &str,
    from_uri: &Url,
) -> Vec<Location> {
    let supertype_ext_locs = resolve_extension_via_supertype_hierarchy(
        indexer,
        anchor_class_name,
        anchor_uri,
        name,
        from_uri,
    );
    if supertype_ext_locs.is_empty() {
        return member_locs;
    }
    let mut combined = member_locs;
    combined.extend(supertype_ext_locs);
    combined
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
