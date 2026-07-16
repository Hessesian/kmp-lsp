use super::{assert_source_contains_node_kind, assert_source_parses, count_nodes_of_kind};

#[test]
fn ks_1_3_048_enum_entries_allow_comma_separation_and_trailing_comma() {
    let source = "enum class ScreenState {\nLoading,\nContent,\n}\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_ENUM_ENTRY),
        2
    );
}

#[test]
fn ks_1_3_049_enum_entry_accepts_modifiers_arguments_and_class_body() {
    assert_source_contains_node_kind(
        "enum class ScreenState(val code: Int) {\n@Deprecated(\"legacy\") Legacy(1) {\nfun label() = \"legacy\"\n},\nContent(2),\n}\n",
        crate::queries::KIND_ENUM_ENTRY,
    );
}

#[test]
fn ks_1_3_050_type_accepts_all_grammar_alternatives_and_modifiers() {
    assert_source_parses(
        "annotation class Marker\nfun <Element> types(\nfunction: (String) -> Int,\nparenthesized: (String),\nnullable: String?,\nreference: List<String>,\ndefinite: Element & Any,\nannotated: @Marker String,\n) = Unit\n",
    );
}

#[test]
fn ks_1_3_051_type_reference_accepts_user_type_and_dynamic() {
    assert_source_parses("val text: sample.model.Title\nval platformValue: dynamic\n");
}

#[test]
fn ks_1_3_052_nullable_type_accepts_one_or_more_question_marks() {
    assert_source_contains_node_kind(
        "val once: String? = null\nval twice: String?? = null\n",
        crate::queries::KIND_NULLABLE_TYPE,
    );
}

#[test]
fn ks_1_3_053_question_mark_token_accepts_following_whitespace_or_no_whitespace() {
    assert_source_parses("val compact: String?=null\nval separated: String? = null\n");
}

#[test]
fn ks_1_3_054_user_type_accepts_qualified_simple_user_types() {
    assert_source_contains_node_kind(
        "val nested: sample.model.Outer<String>.Inner<Int>? = null\n",
        crate::queries::KIND_USER_TYPE,
    );
}

#[test]
fn ks_1_3_055_simple_user_type_accepts_optional_type_arguments() {
    let tree = super::parse_kotlin_source("val plain: Title\nval generic: List<Title>\n");

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_TYPE_ARGS) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_USER_TYPE) >= 2);
}

#[test]
fn ks_1_3_056_type_projection_accepts_modified_type_and_star() {
    assert_source_parses(
        "val produced: List<out CharSequence>\nval consumed: List<in String>\nval unknown: List<*>\n",
    );
}

#[test]
#[ignore = "KS-1.3-057: tree-sitter-kotlin rejects combined annotation and variance projection modifiers"]
fn ks_1_3_057_type_projection_modifiers_accept_repeated_modifiers() {
    assert_source_parses(
        "annotation class Marker\nval values: List<@Marker out CharSequence> = emptyList()\n",
    );
}

#[test]
fn ks_1_3_058_type_projection_modifier_accepts_variance_or_annotation() {
    assert_source_parses(
        "annotation class Marker\nval produced: List<out String>\nval annotated: List<@Marker String>\n",
    );
}

#[test]
fn ks_1_3_059_function_type_accepts_receiver_parameters_arrow_and_result() {
    assert_source_contains_node_kind(
        "val predicate: String.(Int) -> Boolean = { count -> length == count }\n",
        crate::queries::KIND_FUNCTION_TYPE,
    );
}

#[test]
#[ignore = "KS-1.3-060: tree-sitter-kotlin rejects a trailing comma in function type parameters"]
fn ks_1_3_060_function_type_parameters_accept_named_unnamed_and_trailing_comma() {
    assert_source_contains_node_kind(
        "val transform: (source: String, Int,) -> String = { source, count -> source.take(count) }\n",
        crate::queries::KIND_FUNCTION_TYPE,
    );
}

#[test]
fn ks_1_3_061_parenthesized_type_wraps_another_type() {
    assert_source_parses("val title: (String?) = null\n");
}

#[test]
fn ks_1_3_062_receiver_type_accepts_type_modifiers_and_parenthesized_type() {
    assert_source_parses(
        "annotation class Marker\nfun (@Marker String).render() = this\nfun ((String)).normalized() = this\n",
    );
}

#[test]
#[ignore = "KS-1.3-063: tree-sitter-kotlin rejects a parenthesized user type in a definitely-non-nullable type"]
fn ks_1_3_063_parenthesized_user_type_may_be_nested() {
    assert_source_parses(
        "fun <Element> requireValue(value: Element): ((Element)) & Any = value as Element & Any\n",
    );
}

#[test]
fn ks_1_3_064_definitely_non_nullable_type_joins_two_user_types() {
    assert_source_parses(
        "fun <Element> requireValue(value: Element): Element & Any = value as Element & Any\n",
    );
}
