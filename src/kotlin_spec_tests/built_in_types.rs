use crate::stdlib::dot_completions_for;
use tower_lsp::lsp_types::CompletionItemKind;

fn assert_any_completion_signature(name: &str, expected_signature: &str) {
    let completion_items = dot_completions_for("Any", false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == name)
        .collect();

    assert_eq!(matching_items.len(), 1, "expected exactly one {name} item");
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some(expected_signature)
    );
}

#[test]
#[ignore = "KS-3.1-001: kmp-lsp omits operator from the kotlin.Any.equals completion signature"]
fn ks_3_1_001_any_provides_operator_equals_signature() {
    assert_any_completion_signature(
        "equals",
        "open operator fun Any.equals(other: Any?): Boolean",
    );
}

#[test]
fn ks_3_1_008_any_provides_hash_code_signature() {
    assert_any_completion_signature("hashCode", "open fun Any.hashCode(): Int");
}

#[test]
fn ks_3_1_010_any_provides_to_string_signature() {
    assert_any_completion_signature("toString", "open fun Any.toString(): String");
}
