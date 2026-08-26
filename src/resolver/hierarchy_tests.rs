use super::{
    receiver_type_agreement, supertype_chain_contains, supertype_targets, walk_hierarchy,
    ReceiverTypeAgreement, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
use crate::cli::extract_sources::GradleMeta;
use crate::indexer::Indexer;
use crate::types::CallerContext;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tower_lsp::lsp_types::{Location, Url};

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
    let targets = supertype_targets(&indexer, "Bar", bar_uri.as_str(), &mut budget);
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
    let targets = supertype_targets(&indexer, "Bar", bar_uri.as_str(), &mut budget);
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
    let targets = supertype_targets(&indexer, "Bar", bar_uri.as_str(), &mut budget);
    assert!(
        targets.is_empty(),
        "expected still-ambiguous decline (no denylist match), got {targets:?}"
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

/// A Gradle-cache-shaped `jar:` URI for `(group, artifact, version)`, mirroring
/// the real layout `parse_jar_meta` parses:
/// `.../modules-2/files-2.1/<group>/<artifact>/<version>/<hash>/<file>.jar`.
fn gradle_cache_jar_uri(group: &str, artifact: &str, version: &str) -> Url {
    Url::parse(&format!(
        "jar:file:///home/user/.gradle/caches/modules-2/files-2.1/{group}/{artifact}/{version}/deadbeef/{artifact}-{version}.jar"
    ))
    .unwrap()
}

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
    let targets = supertype_targets(&indexer, "Bar", bar_uri.as_str(), &mut budget);
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
    let targets = supertype_targets(&indexer, "Bar", bar_uri.as_str(), &mut budget);
    assert!(
        targets.is_empty(),
        "expected the ambiguous tail to still decline with no module \
         dependency data available, got {targets:?}"
    );
}
