use super::{
    receiver_type_agreement, supertype_chain_contains, supertype_targets, walk_hierarchy,
    ReceiverTypeAgreement, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
use crate::indexer::Indexer;
use crate::types::CallerContext;
use tower_lsp::lsp_types::Url;

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
