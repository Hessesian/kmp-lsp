use crate::indexer::Indexer;
use crate::stdlib::{bare_completions, dot_completions_for};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position, Url};

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

#[test]
fn ks_3_4_001_boolean_values_are_true_and_false() {
    let completion_items = bare_completions(false);

    for literal in ["true", "false"] {
        let matching_items: Vec<_> = completion_items
            .iter()
            .filter(|completion_item| completion_item.label == literal)
            .collect();
        assert_eq!(matching_items.len(), 1, "expected one {literal} literal");
        assert_eq!(matching_items[0].kind, Some(CompletionItemKind::KEYWORD));
        assert_eq!(matching_items[0].detail.as_deref(), Some("Boolean literal"));
    }

    assert!(!completion_items
        .iter()
        .any(|completion_item| completion_item.label == "True"));
    assert!(!completion_items
        .iter()
        .any(|completion_item| completion_item.label == "False"));
}

fn enum_completion_items() -> Vec<CompletionItem> {
    let source = "enum class WorkflowSpec {\n    Ready\n}\nclass MisleadingWorkflowSpec\nfun inspect(state: WorkflowSpec) { state. }\n";
    let specification_uri = Url::parse("file:///kotlin-spec/EnumBuiltins.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let completion_line = "fun inspect(state: WorkflowSpec) { state. }";
    let completion_character = completion_line
        .find("state.")
        .map(|byte_offset| byte_offset + "state.".len())
        .expect("fixture completion marker must exist") as u32;

    indexer
        .completions(
            &specification_uri,
            Position::new(4, completion_character),
            true,
        )
        .0
}

#[test]
#[ignore = "KS-3.9-001: kmp-lsp does not synthesize kotlin.Enum as an enum class supertype"]
fn ks_3_9_001_enum_class_is_indexed_as_implicit_enum_subtype() {
    let source = "enum class WorkflowSpec { Ready }\nclass Enum\n";
    let specification_uri = Url::parse("file:///kotlin-spec/EnumSubtype.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let locations = indexer.subtypes_of("Enum");
    assert_eq!(
        locations.len(),
        1,
        "only WorkflowSpec must be an Enum subtype"
    );
    assert_eq!(locations[0].uri, specification_uri);
    assert_eq!(locations[0].range.start, Position::new(0, 11));
}

#[test]
#[ignore = "KS-3.9-003: kmp-lsp does not provide the built-in enum name property in completion"]
fn ks_3_9_003_enum_provides_name_property_completion() {
    let completion_items = enum_completion_items();
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "name")
        .collect();

    assert_eq!(matching_items.len(), 1, "expected one enum name property");
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::PROPERTY));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("val name: String")
    );
}

#[test]
#[ignore = "KS-3.9-004: kmp-lsp does not provide the built-in enum ordinal property in completion"]
fn ks_3_9_004_enum_provides_ordinal_property_completion() {
    let completion_items = enum_completion_items();
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "ordinal")
        .collect();

    assert_eq!(
        matching_items.len(),
        1,
        "expected one enum ordinal property"
    );
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::PROPERTY));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("val ordinal: Int")
    );
}

#[test]
#[ignore = "KS-3.9-006: kmp-lsp does not provide the built-in enum compareTo method in completion"]
fn ks_3_9_006_enum_provides_compare_to_completion() {
    let completion_items = enum_completion_items();
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "compareTo")
        .collect();

    assert_eq!(
        matching_items.len(),
        1,
        "expected one enum compareTo method"
    );
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("override final fun compareTo(other: WorkflowSpec): Int")
    );
}

#[test]
#[ignore = "KS-3.9-008: kmp-lsp reports the universal Any.equals signature instead of the final enum override"]
fn ks_3_9_008_enum_provides_final_equals_completion() {
    let completion_items = enum_completion_items();
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "equals")
        .collect();

    assert_eq!(matching_items.len(), 1, "expected one enum equals method");
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("override final fun equals(other: Any?): Boolean")
    );
}

#[test]
#[ignore = "KS-3.9-009: kmp-lsp reports the universal Any.hashCode signature instead of the final enum override"]
fn ks_3_9_009_enum_provides_final_hash_code_completion() {
    let completion_items = enum_completion_items();
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "hashCode")
        .collect();

    assert_eq!(matching_items.len(), 1, "expected one enum hashCode method");
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("override final fun hashCode(): Int")
    );
}

fn assert_string_array_completion_signature(
    name: &str,
    expected_kind: CompletionItemKind,
    expected_signature: &str,
) {
    let completion_items = dot_completions_for("Array<String>", false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == name)
        .collect();

    assert_eq!(matching_items.len(), 1, "expected exactly one {name} item");
    assert_eq!(matching_items[0].kind, Some(expected_kind));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some(expected_signature)
    );
}

#[test]
#[ignore = "KS-3.10-008: kmp-lsp omits the built-in Array.get method from completion"]
fn ks_3_10_008_array_provides_operator_get_completion() {
    assert_string_array_completion_signature(
        "get",
        CompletionItemKind::METHOD,
        "operator fun Array<String>.get(index: Int): String",
    );
}

#[test]
#[ignore = "KS-3.10-011: kmp-lsp omits the built-in Array.set method from completion"]
fn ks_3_10_011_array_provides_operator_set_completion() {
    assert_string_array_completion_signature(
        "set",
        CompletionItemKind::METHOD,
        "operator fun Array<String>.set(index: Int, value: String): Unit",
    );
}

#[test]
#[ignore = "KS-3.10-014: kmp-lsp reports Array.size as a Collection method instead of an Array property"]
fn ks_3_10_014_array_provides_size_property_completion() {
    assert_string_array_completion_signature(
        "size",
        CompletionItemKind::PROPERTY,
        "val Array<String>.size: Int",
    );
}

#[test]
#[ignore = "KS-3.10-016: kmp-lsp omits the built-in Array.iterator method from completion"]
fn ks_3_10_016_array_provides_operator_iterator_completion() {
    assert_string_array_completion_signature(
        "iterator",
        CompletionItemKind::METHOD,
        "operator fun Array<String>.iterator(): Iterator<String>",
    );
}

#[test]
#[ignore = "KS-3.10-003: kmp-lsp does not provide the built-in Array constructor in completion"]
fn ks_3_10_003_array_constructor_completion_has_inline_signature() {
    let completion_items = bare_completions(false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "Array")
        .collect();

    assert_eq!(matching_items.len(), 1, "expected one Array constructor");
    assert_eq!(
        matching_items[0].kind,
        Some(CompletionItemKind::CONSTRUCTOR)
    );
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("inline constructor Array<T>(size: Int, init: (Int) -> T)")
    );
}

#[test]
#[ignore = "KS-3.10.1-001: kmp-lsp does not expose specialized array types in bare completion"]
fn ks_3_10_1_001_specialized_array_types_are_available_in_completion() {
    let completion_items = bare_completions(false);
    let specialized_array_types = [
        "DoubleArray",
        "FloatArray",
        "LongArray",
        "IntArray",
        "ShortArray",
        "ByteArray",
        "CharArray",
        "BooleanArray",
    ];

    for type_name in specialized_array_types {
        let matching_items: Vec<_> = completion_items
            .iter()
            .filter(|completion_item| completion_item.label == type_name)
            .collect();
        assert_eq!(matching_items.len(), 1, "expected one {type_name} item");
        assert_eq!(matching_items[0].kind, Some(CompletionItemKind::CLASS));
    }
}

#[test]
#[ignore = "KS-3.10.1-002: kmp-lsp does not provide specialized array get, set, and size contracts"]
fn ks_3_10_1_002_int_array_reuses_specialized_array_members() {
    let completion_items = dot_completions_for("IntArray", false);
    let expected_members = [
        ("get", "operator fun IntArray.get(index: Int): Int"),
        (
            "set",
            "operator fun IntArray.set(index: Int, value: Int): Unit",
        ),
        ("size", "val IntArray.size: Int"),
    ];

    for (name, expected_signature) in expected_members {
        let matching_items: Vec<_> = completion_items
            .iter()
            .filter(|completion_item| completion_item.label == name)
            .collect();
        assert_eq!(matching_items.len(), 1, "expected one IntArray.{name} item");
        assert_eq!(
            matching_items[0].detail.as_deref(),
            Some(expected_signature)
        );
    }
}

#[test]
#[ignore = "KS-3.10.1-003: kmp-lsp does not provide specialized array constructors in completion"]
fn ks_3_10_1_003_specialized_array_constructor_accepts_size() {
    let completion_items = bare_completions(false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "IntArray")
        .collect();

    assert_eq!(matching_items.len(), 1, "expected one IntArray constructor");
    assert_eq!(
        matching_items[0].kind,
        Some(CompletionItemKind::CONSTRUCTOR)
    );
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("constructor IntArray(size: Int)")
    );
}

#[test]
#[ignore = "KS-3.10.1-006: kmp-lsp does not provide specialized array iterator completion"]
fn ks_3_10_1_006_specialized_array_provides_specialized_iterator() {
    let completion_items = dot_completions_for("IntArray", false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "iterator")
        .collect();

    assert_eq!(
        matching_items.len(),
        1,
        "expected one IntArray.iterator item"
    );
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("operator fun IntArray.iterator(): IntIterator")
    );
}

#[test]
#[ignore = "KS-3.11-002: kmp-lsp does not provide the built-in Iterator.next method in completion"]
fn ks_3_11_002_iterator_provides_operator_next_completion() {
    let completion_items = dot_completions_for("Iterator<String>", false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "next")
        .collect();

    assert_eq!(matching_items.len(), 1, "expected one Iterator.next item");
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("operator fun Iterator<String>.next(): String")
    );
}

#[test]
#[ignore = "KS-3.11-004: kmp-lsp does not provide the built-in Iterator.hasNext method in completion"]
fn ks_3_11_004_iterator_provides_operator_has_next_completion() {
    let completion_items = dot_completions_for("Iterator<String>", false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "hasNext")
        .collect();

    assert_eq!(
        matching_items.len(),
        1,
        "expected one Iterator.hasNext item"
    );
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("operator fun Iterator<String>.hasNext(): Boolean")
    );
}

#[test]
#[ignore = "KS-3.11.1-002: kmp-lsp does not provide specialized iterator nextTYPE methods"]
fn ks_3_11_1_002_int_iterator_provides_next_int_completion() {
    let completion_items = dot_completions_for("IntIterator", false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == "nextInt")
        .collect();

    assert_eq!(
        matching_items.len(),
        1,
        "expected one IntIterator.nextInt item"
    );
    assert_eq!(matching_items[0].kind, Some(CompletionItemKind::METHOD));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some("operator fun IntIterator.nextInt(): Int")
    );
}

fn assert_builtin_completion_signature(
    receiver_type: &str,
    name: &str,
    expected_kind: CompletionItemKind,
    expected_signature: &str,
) {
    let completion_items = dot_completions_for(receiver_type, false);
    let matching_items: Vec<_> = completion_items
        .iter()
        .filter(|completion_item| completion_item.label == name)
        .collect();

    assert_eq!(
        matching_items.len(),
        1,
        "expected one {receiver_type}.{name} item"
    );
    assert_eq!(matching_items[0].kind, Some(expected_kind));
    assert_eq!(
        matching_items[0].detail.as_deref(),
        Some(expected_signature)
    );
}

#[test]
#[ignore = "KS-3.12-004: kmp-lsp does not provide Throwable.message in completion"]
fn ks_3_12_004_throwable_provides_message_property_completion() {
    assert_builtin_completion_signature(
        "Throwable",
        "message",
        CompletionItemKind::PROPERTY,
        "val Throwable.message: String?",
    );
}

#[test]
#[ignore = "KS-3.12-006: kmp-lsp does not provide Throwable.cause in completion"]
fn ks_3_12_006_throwable_provides_cause_property_completion() {
    assert_builtin_completion_signature(
        "Throwable",
        "cause",
        CompletionItemKind::PROPERTY,
        "val Throwable.cause: Throwable?",
    );
}

#[test]
#[ignore = "KS-3.13-002: kmp-lsp does not provide Comparable.compareTo in completion"]
fn ks_3_13_002_comparable_provides_operator_compare_to_completion() {
    assert_builtin_completion_signature(
        "Comparable<String>",
        "compareTo",
        CompletionItemKind::METHOD,
        "operator fun Comparable<String>.compareTo(other: String): Int",
    );
}

#[test]
#[ignore = "KS-3.16.2-003: kmp-lsp does not provide KCallable.name in completion"]
fn ks_3_16_2_003_k_callable_provides_name_property_completion() {
    assert_builtin_completion_signature(
        "KCallable<String>",
        "name",
        CompletionItemKind::PROPERTY,
        "val KCallable<String>.name: String",
    );
}
