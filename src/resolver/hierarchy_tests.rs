use super::super::shared_fixture_tests::gradle_cache_jar_uri;
use super::{
    receiver_type_agreement, supertype_chain_contains, supertype_targets, walk_hierarchy,
    ReceiverTypeAgreement, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
use crate::cli::extract_sources::GradleMeta;
use crate::indexer::Indexer;
use crate::types::{CallerContext, FileData};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tower_lsp::lsp_types::{Location, Position, Range, Url};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///t{path}")).unwrap()
}

fn indexed(path: &str, source: &str) -> (Url, Indexer) {
    let file_uri = uri(path);
    let indexer = Indexer::new();
    indexer.index_content(&file_uri, source);
    (file_uri, indexer)
}

#[test]
fn exact_type_match_is_exact() {
    let (file_uri, indexer) = indexed("/D.kt", "class User\n");
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "User",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Exact
    );
}

#[test]
fn subtype_of_target_is_inherited() {
    let (file_uri, indexer) = indexed("/D.kt", "open class User\nclass DerivedUser : User()\n");
    assert!(supertype_chain_contains(
        &indexer,
        "DerivedUser",
        file_uri.as_str(),
        "User",
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
    ));
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "DerivedUser",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Inherited
    );
}

#[test]
fn unrelated_indexed_type_is_unrelated() {
    let (file_uri, indexer) = indexed("/D.kt", "class User\nclass File\n");
    assert!(!supertype_chain_contains(
        &indexer,
        "File",
        file_uri.as_str(),
        "User",
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
    ));
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "File",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Unrelated
    );
}

#[test]
fn unindexed_type_is_unresolvable_not_unrelated() {
    let (file_uri, indexer) = indexed("/D.kt", "class User\n");
    // "Ghost" is never declared anywhere — has_type_definition fails, so we
    // must NOT claim to have proven it's unrelated to User.
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "Ghost",
            file_uri.as_str(),
            "User",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Unresolvable
    );
}

/// House decoy: a two-level hierarchy — the target is a grandparent, not the
/// immediate supertype.
#[test]
fn transitive_supertype_is_inherited() {
    let (file_uri, indexer) = indexed(
        "/D.kt",
        "open class Base\nopen class Middle : Base()\nclass Leaf : Middle()\n",
    );
    assert_eq!(
        receiver_type_agreement(
            &indexer,
            "Leaf",
            file_uri.as_str(),
            "Base",
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK
        ),
        ReceiverTypeAgreement::Inherited
    );
}

// ─── Primitive 1: ambiguity-safe hierarchy-walk tail ───────────────────────

/// An unscoped tail lookup that finds two same-named, non-denylisted
/// candidates must decline rather than pick one arbitrarily. Pre-fix, this
/// picked whichever candidate the index happened to insert first
/// (`resolve_symbol_no_rg`'s raw first-match tail); post-fix it returns no
/// supertype target for this hop at all.
#[test]
fn ambiguous_same_name_supertype_declines_instead_of_guessing() {
    let indexer = Indexer::new();
    // Two unrelated classes both named `Activity`, indexed before the class
    // whose supertype walk will collide on that bare name — insertion order
    // is exactly what the old first-match tail picked from.
    indexer.index_content(
        &uri("/decoy/Activity.kt"),
        "package com.example.decoy\nclass Activity\n",
    );
    indexer.index_content(
        &uri("/real/Activity.kt"),
        "package com.example.real\nclass Activity\n",
    );
    // `Bar`'s own file has no import for `Activity`, so resolving its
    // supertype falls all the way through to the ambiguous tail.
    let bar_uri = uri("/app/Bar.kt");
    indexer.index_content(&bar_uri, "class Bar : Activity()\n");

    let mut budget = MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK;
    let targets = supertype_targets(
        &indexer,
        "Bar",
        bar_uri.as_str(),
        &mut budget,
        bar_uri.as_str(),
    );
    assert!(
        targets.is_empty(),
        "expected the ambiguous tail to decline, got {targets:?}"
    );
}

// ─── Primitive 3: narrow com.android.internal.* denylist tie-break ────────

/// The one evidenced denylist entry: a `com.android.internal.*` candidate
/// is dropped from an otherwise-ambiguous 2-way tie, leaving the other
/// candidate as the unique (and therefore usable) match.
#[test]
fn denylisted_package_candidate_is_deprioritized_in_favor_of_the_other() {
    let indexer = Indexer::new();
    // The denylisted decoy is indexed first, matching the real repro's
    // insertion order — a naive first-match tail would have picked it.
    indexer.index_content(
        &uri("/internal/Activity.kt"),
        "package com.android.internal.telephony\nclass Activity\n",
    );
    indexer.index_content(
        &uri("/android/Activity.kt"),
        "package android.app\nclass Activity\n",
    );
    let bar_uri = uri("/app/Bar.kt");
    indexer.index_content(&bar_uri, "class Bar : Activity()\n");

    let mut budget = MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK;
    let targets = supertype_targets(
        &indexer,
        "Bar",
        bar_uri.as_str(),
        &mut budget,
        bar_uri.as_str(),
    );
    assert_eq!(
        targets.len(),
        1,
        "expected exactly one target, got {targets:?}"
    );
    assert_eq!(targets[0].0, "Activity");
    assert!(
        targets[0].1.ends_with("/android/Activity.kt"),
        "expected the non-denylisted android.app.Activity, got {targets:?}"
    );
}

/// Regression guard: a package prefix that is NOT on the denylist must
/// never be deprioritized, even when it ties with another candidate — this
/// guards against the denylist silently growing into a broad preference
/// system (see the design doc's self-critique).
#[test]
fn unlisted_package_prefix_is_never_deprioritized() {
    let indexer = Indexer::new();
    // Neither candidate matches the denylist — both are equally
    // legitimate-looking third-party packages. The tie-break must not
    // invent a preference between them.
    indexer.index_content(
        &uri("/vendor1/Activity.kt"),
        "package com.vendor.one\nclass Activity\n",
    );
    indexer.index_content(
        &uri("/vendor2/Activity.kt"),
        "package com.vendor.two\nclass Activity\n",
    );
    let bar_uri = uri("/app/Bar.kt");
    indexer.index_content(&bar_uri, "class Bar : Activity()\n");

    let mut budget = MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK;
    let targets = supertype_targets(
        &indexer,
        "Bar",
        bar_uri.as_str(),
        &mut budget,
        bar_uri.as_str(),
    );
    assert!(
        targets.is_empty(),
        "expected still-ambiguous decline (no denylist match), got {targets:?}"
    );
}

/// Regression guard for the Copilot-flagged gap in PR #286:
/// `is_denylisted_package_prefix` only ever looked up `indexer.files`, so
/// a candidate declared in a compiled-only JAR entry (no `-sources.jar`
/// companion — its parsed `FileData` lives in `indexer.jar_files` instead)
/// was never recognized as denylisted even when its real package matched
/// `com.android.internal.*`. Same two-candidate shape as
/// `denylisted_package_candidate_is_deprioritized_in_favor_of_the_other`
/// above, but both candidates are JAR-only (`jar_definitions` +
/// `jar_files`, never `indexer.files`).
#[test]
fn denylisted_jar_only_package_candidate_is_deprioritized_in_favor_of_the_other() {
    let indexer = Indexer::new();
    let decoy_jar_uri = gradle_cache_jar_uri("com.android.internal", "telephony-stubs", "1.0.0");
    let real_jar_uri = gradle_cache_jar_uri("android", "android-stubs", "34.0.0");
    // The denylisted decoy is indexed first, matching this file's established
    // convention of seeding the wrong candidate first.
    indexer.jar_definitions.insert(
        "Activity".to_owned(),
        vec![
            Location {
                uri: decoy_jar_uri.clone(),
                range: Default::default(),
            },
            Location {
                uri: real_jar_uri.clone(),
                range: Default::default(),
            },
        ],
    );
    indexer.jar_files.insert(
        decoy_jar_uri.to_string(),
        Arc::new(FileData {
            package: Some("com.android.internal.telephony".to_owned()),
            ..Default::default()
        }),
    );
    indexer.jar_files.insert(
        real_jar_uri.to_string(),
        Arc::new(FileData {
            package: Some("android.app".to_owned()),
            ..Default::default()
        }),
    );

    let bar_uri = uri("/app/Bar.kt");
    indexer.index_content(&bar_uri, "class Bar : Activity()\n");

    let mut budget = MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK;
    let targets = supertype_targets(
        &indexer,
        "Bar",
        bar_uri.as_str(),
        &mut budget,
        bar_uri.as_str(),
    );
    assert_eq!(
        targets,
        vec![("Activity".to_owned(), real_jar_uri.to_string())],
        "expected the JAR-only com.android.internal.* candidate to be \
         deprioritized in favor of the non-denylisted one, got {targets:?}"
    );
}

/// Copilot review finding on PR #289: `is_denylisted_package_prefix` reads
/// `FileData.package` -- a single value covering the WHOLE synthetic file a
/// compiled JAR is indexed under, derived (`build_jar_file_data`) from just
/// the FIRST class-like symbol it happens to find. A real JAR spans many
/// packages across its symbols, so that single value can be wrong for any
/// OTHER symbol in the same JAR. The codebase already has an accurate
/// per-symbol lookup for this (`jar_symbol_package`, backed by the
/// `jar_symbol_packages` side table) -- the denylist check must consult it
/// first, not just the file-level approximation.
#[test]
fn denylisted_check_uses_the_real_per_symbol_package_not_the_jars_first_symbol_guess() {
    let indexer = Indexer::new();
    // Both candidates live in the SAME jar-derived synthetic file -- the
    // realistic multi-package-JAR shape. The file-level `package` is a
    // first-symbol guess that happens to equal the REAL candidate's package,
    // not the decoy's -- exactly the case a naive file-level check gets wrong.
    let same_jar_uri = gradle_cache_jar_uri("android", "multi-pkg-stubs", "1.0.0");
    indexer.jar_definitions.insert(
        "Activity".to_owned(),
        vec![
            Location {
                uri: same_jar_uri.clone(),
                range: Range {
                    start: Position::new(0, 0),
                    end: Position::new(0, 8),
                },
            },
            Location {
                uri: same_jar_uri.clone(),
                range: Range {
                    start: Position::new(1, 0),
                    end: Position::new(1, 8),
                },
            },
        ],
    );
    indexer.jar_files.insert(
        same_jar_uri.to_string(),
        Arc::new(FileData {
            package: Some("android.app".to_owned()),
            ..Default::default()
        }),
    );
    // Per-symbol ground truth: line 0 (the decoy) is really
    // com.android.internal.*, contradicting the file-level guess above.
    indexer.jar_symbol_packages.insert(
        same_jar_uri.to_string(),
        vec![
            "com.android.internal.telephony".to_owned(),
            "android.app".to_owned(),
        ],
    );

    let bar_uri = uri("/app/Bar.kt");
    indexer.index_content(&bar_uri, "class Bar : Activity()\n");

    // `supertype_targets`'s own `(name, uri)` output can't distinguish which
    // of two same-URI candidates survived (both share `same_jar_uri`) --
    // call the ambiguity-safe resolver directly instead, so the surviving
    // candidate's own `range` (line 1, the real one) can be asserted on.
    let locs = super::super::resolve_symbol_hierarchy_ambiguity_safe(
        &indexer,
        "Activity",
        &bar_uri,
        Some(&bar_uri),
    );
    assert_eq!(
        locs,
        vec![Location {
            uri: same_jar_uri,
            range: Range {
                start: Position::new(1, 0),
                end: Position::new(1, 8),
            },
        }],
        "expected the per-symbol-denylisted line-0 candidate to be excluded \
         using its real per-symbol package, not the file-level first-symbol \
         guess, got {locs:?}"
    );
}

// ─── Acceptance: combined 4-hop walk with a denylisted decoy at the tail ──

/// Synthetic reproduction of the real Moneta corpus shape (`AppCompatActivity
/// → FragmentActivity → ComponentActivity → Activity`) with a deliberate
/// `com.android.internal.*` decoy at the last hop, standing in for the real
/// `DctConstants.Activity` collision. Does not depend on a real corpus or a
/// live Gradle cache.
#[test]
fn four_hop_walk_reaches_the_real_ancestor_past_a_denylisted_decoy() {
    let indexer = Indexer::new();
    let call_us_uri = uri("/app/CallUsActivity.kt");
    indexer.index_content(
        &call_us_uri,
        "package com.example.app\nclass CallUsActivity : AppCompatActivity()\n",
    );
    indexer.index_content(
        &uri("/appcompat/AppCompatActivity.kt"),
        "package androidx.appcompat.app\nclass AppCompatActivity : FragmentActivity()\n",
    );
    indexer.index_content(
        &uri("/fragment/FragmentActivity.kt"),
        "package androidx.fragment.app\nclass FragmentActivity : ComponentActivity()\n",
    );
    indexer.index_content(
        &uri("/core/ComponentActivity.kt"),
        "package androidx.core.app\nclass ComponentActivity : Activity()\n",
    );
    // Decoy indexed first, matching the real repro's insertion order.
    indexer.index_content(
        &uri("/internal/Activity.kt"),
        "package com.android.internal.telephony\nclass Activity\n",
    );
    indexer.index_content(
        &uri("/android/Activity.kt"),
        "package android.app\nclass Activity\n",
    );

    let items = walk_hierarchy(
        &indexer,
        "CallUsActivity",
        call_us_uri.as_str(),
        CallerContext::default(),
        12,
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        |_, super_name, super_uri, _| vec![(super_name.to_string(), super_uri.to_string())],
    );

    assert!(
        items
            .iter()
            .any(|(name, target_uri)| name == "Activity"
                && target_uri.ends_with("/android/Activity.kt")),
        "expected the walk to reach android.app.Activity, got {items:?}"
    );
    assert!(
        !items
            .iter()
            .any(|(name, target_uri)| name == "Activity" && target_uri.contains("/internal/")),
        "must never resolve to the com.android.internal decoy, got {items:?}"
    );
}

// ─── Module-scoped narrowing (real-workspace-json-schema wiring) ──────────
//
// See `docs/superpowers/specs/2026-08-25-real-workspace-json-schema-and-
// consumption-design.md` §5: when the `com.android.internal.*` denylist
// alone still leaves more than one candidate, a second tie-break narrows
// using the calling file's own module's real Gradle dependency set (loaded
// from `workspace.json` into `Indexer::module_dependencies` — see
// `workspace_json::load_module_dependencies`). Neither candidate here has a
// package known to `indexer.files` (both are JAR-only `Location`s, never
// indexed as real files), so the denylist tie-break is a no-op in both
// tests below and the module-scoped tie-break is what's actually exercised.

/// Two same-named JAR-backed candidates for `Activity`, one from a dependency
/// of the calling module (`com.example.real:real-lib:2.0.0`), one from an
/// unrelated library (`com.example.decoy:decoy-lib:1.0.0`) that the calling
/// module does NOT depend on. `Indexer::module_dependencies` records only the
/// real dependency for the calling file's own content root (`/t/app`).
#[test]
fn module_scoped_tie_break_narrows_jar_collision_to_the_calling_modules_dependency() {
    let indexer = Indexer::new();
    let decoy_jar_uri = gradle_cache_jar_uri("com.example.decoy", "decoy-lib", "1.0.0");
    let real_jar_uri = gradle_cache_jar_uri("com.example.real", "real-lib", "2.0.0");
    // Decoy indexed first, matching this file's established convention of
    // seeding the wrong candidate first so a first-match tail would pick it.
    indexer.jar_definitions.insert(
        "Activity".to_owned(),
        vec![
            Location {
                uri: decoy_jar_uri,
                range: Default::default(),
            },
            Location {
                uri: real_jar_uri.clone(),
                range: Default::default(),
            },
        ],
    );

    let mut dependencies_by_content_root: HashMap<PathBuf, HashSet<GradleMeta>> = HashMap::new();
    dependencies_by_content_root.insert(
        PathBuf::from("/t/app"),
        HashSet::from([GradleMeta {
            group: "com.example.real".to_owned(),
            artifact: "real-lib".to_owned(),
            version: "2.0.0".to_owned(),
        }]),
    );
    *indexer.module_dependencies.write().unwrap() = dependencies_by_content_root;

    let bar_uri = uri("/app/Bar.kt");
    indexer.index_content(&bar_uri, "class Bar : Activity()\n");

    let mut budget = MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK;
    let targets = supertype_targets(
        &indexer,
        "Bar",
        bar_uri.as_str(),
        &mut budget,
        bar_uri.as_str(),
    );
    assert_eq!(
        targets,
        vec![("Activity".to_owned(), real_jar_uri.to_string())],
        "expected the tie-break to narrow to the calling module's own \
         real-lib dependency, got {targets:?}"
    );
}

/// Same two-candidate JAR collision as above, but with no `workspace.json`
/// module data loaded at all (`Indexer::module_dependencies` stays at its
/// default empty map). Behavior must be unchanged from before this wiring
/// existed: still decline on ambiguity, exactly like the plain
/// `com.android.internal.*`-denylist-only behavior.
#[test]
fn no_module_dependency_data_still_declines_on_jar_collision() {
    let indexer = Indexer::new();
    let decoy_jar_uri = gradle_cache_jar_uri("com.example.decoy", "decoy-lib", "1.0.0");
    let real_jar_uri = gradle_cache_jar_uri("com.example.real", "real-lib", "2.0.0");
    indexer.jar_definitions.insert(
        "Activity".to_owned(),
        vec![
            Location {
                uri: decoy_jar_uri,
                range: Default::default(),
            },
            Location {
                uri: real_jar_uri,
                range: Default::default(),
            },
        ],
    );
    // `indexer.module_dependencies` is left at its `Indexer::new()` default:
    // an empty map, standing in for "no workspace.json present."

    let bar_uri = uri("/app/Bar.kt");
    indexer.index_content(&bar_uri, "class Bar : Activity()\n");

    let mut budget = MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK;
    let targets = supertype_targets(
        &indexer,
        "Bar",
        bar_uri.as_str(),
        &mut budget,
        bar_uri.as_str(),
    );
    assert!(
        targets.is_empty(),
        "expected the ambiguous tail to still decline with no module \
         dependency data available, got {targets:?}"
    );
}

// ─── Module-scoped narrowing must reach a hop-4 collision, not just hop 1 ──
//
// Regression guard for the wiring bug found on PR #286: beyond hop 1 of a
// hierarchy walk, the per-hop `class_uri`/`from_uri` a candidate is resolved
// from is the PREVIOUS hop's own resolved location — a `jar:` URI once any
// ancestor lives in a compiled JAR, which `owning_module_dependencies` can
// never map to a module. Unlike the test above (which resolves every hop via
// `indexer.index_content`, so every `class_uri` stays a real `file://` URI
// the whole way and never actually exercises the jar: case), this test makes
// hops 1-3 JAR-backed so hop 4 — where the real collision lives — is reached
// with a `jar:` `class_uri`, and only the calling file at hop 0 has a real
// `file://` URI.

/// Minimal jar-backed `FileData`: `symbols` stays empty, so
/// `super_names_for_class`'s class-name lookup misses and it falls back to
/// returning every entry in `supers` — exactly what a single-super JAR entry
/// needs here, without hand-building a full symbol table.
fn jar_backed_file_data(super_name: &str) -> FileData {
    FileData {
        supers: vec![(0, super_name.to_owned(), Vec::new())],
        ..Default::default()
    }
}

#[test]
fn module_scoped_tie_break_narrows_a_hop_four_jar_collision_using_the_walks_real_origin() {
    let indexer = Indexer::new();
    let call_us_uri = uri("/app/CallUsActivity.kt");
    indexer.index_content(
        &call_us_uri,
        "package com.example.app\nclass CallUsActivity : AppCompatActivity()\n",
    );

    let appcompat_uri = gradle_cache_jar_uri("androidx.appcompat", "appcompat", "1.6.1");
    let fragment_uri = gradle_cache_jar_uri("androidx.fragment", "fragment", "1.6.1");
    let component_uri = gradle_cache_jar_uri("androidx.activity", "activity", "1.7.0");
    let decoy_activity_uri = gradle_cache_jar_uri("com.example.decoy", "decoy-lib", "1.0.0");
    let real_activity_uri = gradle_cache_jar_uri("android", "android-stubs", "34.0.0");

    indexer.jar_definitions.insert(
        "AppCompatActivity".to_owned(),
        vec![Location {
            uri: appcompat_uri.clone(),
            range: Default::default(),
        }],
    );
    indexer.jar_definitions.insert(
        "FragmentActivity".to_owned(),
        vec![Location {
            uri: fragment_uri.clone(),
            range: Default::default(),
        }],
    );
    indexer.jar_definitions.insert(
        "ComponentActivity".to_owned(),
        vec![Location {
            uri: component_uri.clone(),
            range: Default::default(),
        }],
    );
    // The hop-4 collision: two same-named `Activity` JAR candidates. Decoy
    // indexed first, matching this file's established insertion-order convention.
    indexer.jar_definitions.insert(
        "Activity".to_owned(),
        vec![
            Location {
                uri: decoy_activity_uri,
                range: Default::default(),
            },
            Location {
                uri: real_activity_uri.clone(),
                range: Default::default(),
            },
        ],
    );

    indexer.jar_files.insert(
        appcompat_uri.to_string(),
        Arc::new(jar_backed_file_data("FragmentActivity")),
    );
    indexer.jar_files.insert(
        fragment_uri.to_string(),
        Arc::new(jar_backed_file_data("ComponentActivity")),
    );
    indexer.jar_files.insert(
        component_uri.to_string(),
        Arc::new(jar_backed_file_data("Activity")),
    );

    // Only the calling file's module (`/t/app`, `CallUsActivity`'s own content
    // root) depends on the real `android:android-stubs` artifact — the decoy
    // belongs to an unrelated dependency the calling module never declared.
    let mut dependencies_by_content_root: HashMap<PathBuf, HashSet<GradleMeta>> = HashMap::new();
    dependencies_by_content_root.insert(
        PathBuf::from("/t/app"),
        HashSet::from([GradleMeta {
            group: "android".to_owned(),
            artifact: "android-stubs".to_owned(),
            version: "34.0.0".to_owned(),
        }]),
    );
    *indexer.module_dependencies.write().unwrap() = dependencies_by_content_root;

    let items = walk_hierarchy(
        &indexer,
        "CallUsActivity",
        call_us_uri.as_str(),
        CallerContext::default(),
        12,
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        |_, super_name, super_uri, _| vec![(super_name.to_string(), super_uri.to_string())],
    );

    assert!(
        items
            .iter()
            .any(|(name, target_uri)| name == "Activity"
                && *target_uri == real_activity_uri.to_string()),
        "expected the walk to reach the calling module's real dependency past \
         the hop-4 collision using the walk's real origin file, got {items:?}"
    );
}
