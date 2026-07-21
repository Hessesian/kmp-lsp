use super::{
    receiver_type_agreement, supertype_chain_contains, ReceiverTypeAgreement,
    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};
use crate::indexer::Indexer;
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
