use std::sync::Arc;

use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::features::fill_when::when_diagnostics;
use crate::indexer::Indexer;
use crate::inlay_hints::compute_inlay_hints;
use tower_lsp::lsp_types::{InlayHintLabel, Position, Range, Url};

fn inlay_hint_labels(source: &str) -> Vec<String> {
    let specification_uri =
        Url::parse("file:///kotlin-spec/Expressions.kt").expect("specification URI must be valid");
    let indexer = Arc::new(Indexer::new());
    indexer.index_content(&specification_uri, source);
    let line_count = source.lines().count() as u32;
    compute_inlay_hints(
        &indexer,
        &specification_uri,
        Range::new(Position::new(0, 0), Position::new(line_count, 0)),
    )
    .into_iter()
    .filter_map(|hint| match hint.label {
        InlayHintLabel::String(label) => Some(label),
        InlayHintLabel::LabelParts(_) => None,
    })
    .collect()
}

fn when_diagnostic_messages(source: &str) -> Vec<String> {
    let specification_uri = Url::parse("file:///kotlin-spec/WhenExpressions.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    indexer.store_live_tree(&specification_uri, source);
    indexer.set_live_lines(&specification_uri, source);
    when_diagnostics(&indexer, &specification_uri)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

#[test]
fn ks_8_001_expression_context_is_determined_by_statement_position() {
    assert_source_parses(
        "fun consumeSpec(valueSpec: Int) {}\nfun renderSpec() {\n    1 + 2\n    consumeSpec(1 + 2)\n}\n",
    );
}

#[test]
fn ks_8_1_1_001_true_and_false_have_boolean_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec() {\n    val enabledSpec = true\n    val disabledSpec = false\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-8.1.1-002: tree-sitter-kotlin accepts Boolean keywords as unescaped identifiers"]
fn ks_8_1_1_002_boolean_keywords_require_escaping_when_used_as_identifiers() {
    assert_source_parses("val `true`: Boolean = false\nval copiedSpec = `true`\n");
    assert_source_has_syntax_error("val true: Boolean = false\n");
    assert_source_has_syntax_error("val false: Boolean = true\n");
}

#[test]
#[ignore = "KS-8.1.2-001: kmp-lsp does not reject misplaced decimal underscores"]
fn ks_8_1_2_001_decimal_literal_accepts_internal_underscores_only() {
    assert_source_parses("val valuesSpec = listOf(0, 7, 1_000, 12_34_56)\n");
    for invalid_source in ["val valueSpec = _1\n", "val valueSpec = 1_\n"] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
#[ignore = "KS-8.1.2-002: tree-sitter-kotlin accepts leading-zero decimal literals"]
fn ks_8_1_2_002_decimal_literal_cannot_use_leading_zero_or_octal_form() {
    assert_source_parses("val zeroSpec = 0\nval eightSpec = 8\n");
    for invalid_source in ["val valueSpec = 01\n", "val valueSpec = 077\n"] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_8_1_2_003_hexadecimal_literal_requires_prefix_digits_and_internal_underscores() {
    assert_source_parses("val valuesSpec = listOf(0x0, 0XfF, 0xCA_FE)\n");
    for invalid_source in [
        "val valueSpec = 0x\n",
        "val valueSpec = 0x_FF\n",
        "val valueSpec = 0xFF_\n",
        "val valueSpec = 0xGG\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
#[ignore = "KS-8.1.2-004: tree-sitter-kotlin rejects valid binary digit separators"]
fn ks_8_1_2_004_binary_literal_requires_prefix_binary_digits_and_internal_underscores() {
    assert_source_parses("val valuesSpec = listOf(0b0, 0B1, 0b1010)\n");
    assert_source_parses("val separatedSpec = 0b1010_0110\n");
    for invalid_source in [
        "val valueSpec = 0b\n",
        "val valueSpec = 0b_10\n",
        "val valueSpec = 0b10_\n",
        "val valueSpec = 0b102\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_8_1_3_001_long_suffix_gives_all_integer_radices_long_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec() {\n    val decimalSpec = 1L\n    val hexadecimalSpec = 0x1L\n    val binarySpec = 0b1L\n}\n",
    );
    assert_eq!(labels, vec![": Long", ": Long", ": Long"]);
}

#[test]
#[ignore = "KS-8.1.3-002: kmp-lsp does not diagnose integer literals above Long maximum"]
fn ks_8_1_3_002_integer_above_long_maximum_is_illegal() {
    assert_source_parses("val maximumSpec = 9223372036854775807L\n");
    assert_source_has_syntax_error("val overflowSpec = 9223372036854775808\n");
}

#[test]
#[ignore = "KS-8.1.3-003: kmp-lsp infers every unsuffixed integer literal as Int"]
fn ks_8_1_3_003_unsuffixed_integer_above_int_maximum_has_long_type() {
    let labels = inlay_hint_labels("fun valueSpec() { val largeSpec = 2147483648 }\n");
    assert_eq!(labels, vec![": Long"]);
}

#[test]
fn ks_8_1_4_001_real_literal_accepts_decimal_fraction_exponent_and_float_suffix_forms() {
    assert_source_parses("val valuesSpec = listOf(1.0, .5, 1e3, 1E+3, 1e-3, 1.0e3, 1f, 1F, .5f)\n");
}

#[test]
fn ks_8_1_4_002_real_literal_is_decimal_and_cannot_omit_fraction_after_dot() {
    assert_source_parses("val valuesSpec = listOf(1.0, 1e2, 1f)\n");
    for invalid_source in ["val valueSpec = 1.\n", "val valueSpec = 0x1.0\n"] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
#[ignore = "KS-8.1.4-003: kmp-lsp does not reject misplaced real-literal underscores"]
fn ks_8_1_4_003_real_literal_allows_underscores_only_inside_numeric_parts() {
    assert_source_parses("val valuesSpec = listOf(1_000.0, 1.0_25, 1.0e1_0)\n");
    for invalid_source in [
        "val valueSpec = _1.0\n",
        "val valueSpec = 1_.0\n",
        "val valueSpec = 1._0\n",
        "val valueSpec = 1.0_\n",
        "val valueSpec = 1.0e_3\n",
        "val valueSpec = 1.0e3_\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_8_1_4_004_real_literal_suffix_determines_float_or_double_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec() {\n    val doubleSpec = 1.0\n    val exponentSpec = 1e3\n    val lowerFloatSpec = 1.0f\n    val upperFloatSpec = 1F\n}\n",
    );
    assert_eq!(labels, vec![": Double", ": Double", ": Float", ": Float"]);
}

#[test]
fn ks_8_1_5_001_simple_character_literal_contains_one_allowed_character_and_has_char_type() {
    let labels = inlay_hint_labels("fun valueSpec() { val characterSpec = 'A' }\n");
    assert_eq!(labels, vec![": Char"]);
    for invalid_source in [
        "val characterSpec = ''\n",
        "val characterSpec = 'AB'\n",
        "val characterSpec = '\n'\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_8_1_5_002_character_literal_accepts_all_simple_escape_sequences() {
    assert_source_parses(
        r#"val valuesSpec = listOf('\t', '\b', '\r', '\n', '\'', '\"', '\\', '\$')
"#,
    );
}

#[test]
fn ks_8_1_5_003_unicode_character_escape_requires_exactly_four_hex_digits() {
    assert_source_parses("val valuesSpec = listOf('\\u0000', '\\u0041', '\\uFFFF')\n");
    for invalid_source in [
        "val valueSpec = '\\u041'\n",
        "val valueSpec = '\\u00000'\n",
        "val valueSpec = '\\uGGGG'\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_8_1_6_001_string_literal_is_a_string_interpolation_expression() {
    let labels = inlay_hint_labels(
        "fun valueSpec(nameSpec: String) { val messageSpec = \"Hello, $nameSpec\" }\n",
    );
    assert_eq!(labels, vec![": String"]);
}

#[test]
fn ks_8_1_7_001_null_literal_has_nothing_nullable_type() {
    let labels = inlay_hint_labels("fun valueSpec() { val absentSpec = null }\n");
    assert_eq!(labels, vec![": Nothing?"]);
}

#[test]
#[ignore = "KS-8.1.7-002: kmp-lsp does not diagnose null assigned to non-null types"]
fn ks_8_1_7_002_null_literal_is_valid_only_for_nullable_types() {
    assert_source_parses("val validSpec: String? = null\n");
    assert_source_has_syntax_error("val invalidSpec: String = null\n");
}

#[test]
fn ks_8_3_001_string_interpolation_has_line_and_multiline_forms() {
    assert_source_parses(
        "fun valuesSpec(nameSpec: String) {\n    val lineSpec = \"Hello, $nameSpec\"\n    val multilineSpec = \"\"\"Hello,\n$nameSpec\"\"\"\n}\n",
    );
}

#[test]
fn ks_8_3_002_string_interpolation_combines_content_and_expression_fragments() {
    assert_source_parses(
        "fun valueSpec(nameSpec: String, countSpec: Int) = \"Name: $nameSpec; next: ${countSpec + 1}.\"\n",
    );
}

#[test]
fn ks_8_3_003_simple_interpolation_path_requires_braces_for_qualified_path() {
    let tree = super::parse_kotlin_source(
        "class ModelSpec(val nameSpec: String)\nfun valuesSpec(modelSpec: ModelSpec) {\n    val simpleSpec = \"$modelSpec.nameSpec\"\n    val qualifiedSpec = \"${modelSpec.nameSpec}\"\n}\n",
    );
    assert!(!tree.root_node().has_error());
    assert_eq!(
        super::count_nodes_of_kind(&tree, crate::queries::KIND_INTERP_IDENT),
        1
    );
    assert_eq!(
        super::count_nodes_of_kind(&tree, crate::queries::KIND_NAV_EXPR),
        1
    );
}

#[test]
#[ignore = "KS-8.3-004: tree-sitter-kotlin accepts raw newlines inside line strings"]
fn ks_8_3_004_line_strings_escape_newlines_while_multiline_strings_allow_raw_content() {
    assert_source_parses(
        "val lineSpec = \"first\\nsecond\"\nval multilineSpec = \"\"\"first\nsecond \\n\"\"\"\n",
    );
    assert_source_has_syntax_error("val invalidSpec = \"first\nsecond\"\n");
}

#[test]
fn ks_8_3_005_string_interpolation_always_has_string_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec(nameSpec: String) {\n    val lineSpec = \"$nameSpec\"\n    val multilineSpec = \"\"\"$nameSpec\"\"\"\n}\n",
    );
    assert_eq!(labels, vec![": String", ": String"]);
}

#[test]
fn ks_8_4_001_try_expression_accepts_catches_optional_finally_or_finally_only() {
    assert_source_parses(
        "fun readSpec() {\n    try { println(1) } catch (failureSpec: IllegalStateException) { println(failureSpec) } catch (failureSpec: RuntimeException) { println(failureSpec) } finally { println(2) }\n    try { println(3) } finally { println(4) }\n}\n",
    );
}

#[test]
#[ignore = "KS-8.4-002: tree-sitter-kotlin rejects a trailing comma in catch parameters"]
fn ks_8_4_002_catch_has_one_annotated_typed_parameter_with_optional_trailing_comma() {
    assert_source_parses(
        "annotation class MarkerSpec\nfun validSpec() {\n    try { println(1) } catch (@MarkerSpec failureSpec: RuntimeException) { println(failureSpec) }\n}\n",
    );
    assert_source_parses(
        "annotation class MarkerSpec\nfun readSpec() {\n    try { println(1) } catch (@MarkerSpec failureSpec: RuntimeException,) { println(failureSpec) }\n}\n",
    );
}

#[test]
fn ks_8_4_003_try_expression_requires_catch_or_finally_block() {
    assert_source_parses("fun validSpec() { try { println(1) } finally { println(2) } }\n");
    assert_source_has_syntax_error("fun invalidSpec() { try { println(1) } }\n");
}

#[test]
fn ks_8_5_001_conditional_expression_accepts_single_two_and_empty_branch_forms() {
    assert_source_parses(
        "fun renderSpec(flagSpec: Boolean) {\n    if (flagSpec) println(1)\n    if (flagSpec) { println(2) } else println(3)\n    if (flagSpec);\n}\n",
    );
}

#[test]
fn ks_8_5_002_branchless_conditional_with_else_semicolon_is_valid() {
    assert_source_parses("fun renderSpec(flagSpec: Boolean) { if (flagSpec) else; }\n");
}

#[test]
#[ignore = "KS-8.5-003: kmp-lsp does not diagnose branch-incomplete if in expression context"]
fn ks_8_5_003_conditional_missing_a_branch_cannot_be_used_as_expression() {
    assert_source_parses("val validSpec = if (true) 1 else 2\n");
    assert_source_has_syntax_error("val invalidSpec = if (true) 1\n");
}

#[test]
#[ignore = "KS-8.5-004: kmp-lsp does not type-check conditional conditions"]
fn ks_8_5_004_conditional_condition_must_be_boolean() {
    assert_source_parses("val validSpec = if (true) 1 else 2\n");
    assert_source_has_syntax_error("val invalidSpec = if (1) 1 else 2\n");
}

#[test]
fn ks_8_5_005_conditional_expression_has_side_dependent_binary_precedence() {
    assert_source_parses(
        "fun updateSpec() {\n    var valueSpec = 0\n    valueSpec = if (true) 1 else 2\n    if (true) valueSpec = 1 else valueSpec = 2\n}\n",
    );
}

#[test]
fn ks_8_6_001_when_expression_accepts_subjectless_and_bound_value_forms() {
    assert_source_parses(
        "fun readSpec(valueSpec: Int): String {\n    val subjectlessSpec = when { valueSpec > 0 -> \"positive\"; else -> \"other\" }\n    return when (valueSpec) { 0 -> \"zero\"; else -> subjectlessSpec }\n}\n",
    );
}

#[test]
#[ignore = "KS-8.6-002: tree-sitter-kotlin rejects a trailing comma in when conditions"]
fn ks_8_6_002_when_entry_accepts_multiple_conditions_trailing_comma_and_else() {
    assert_source_parses(
        "fun validSpec(valueSpec: Int) = when (valueSpec) {\n    1, 2 -> \"small\"\n    else -> \"other\"\n}\n",
    );
    assert_source_parses(
        "fun readSpec(valueSpec: Int) = when (valueSpec) {\n    1, 2, -> \"small\"\n    else -> \"other\"\n}\n",
    );
}

#[test]
fn ks_8_6_003_bound_when_accepts_type_contains_equality_and_else_conditions() {
    assert_source_parses(
        "fun readSpec(valueSpec: Any, valuesSpec: List<Any>) = when (valueSpec) {\n    is String -> \"string\";\n    !is Number -> \"not number\";\n    in valuesSpec -> \"contained\";\n    !in valuesSpec -> \"not contained\";\n    0 -> \"equal\";\n    else -> \"other\";\n}\n",
    );
}

#[test]
#[ignore = "KS-8.6-004: kmp-lsp does not diagnose else before later when entries"]
fn ks_8_6_004_else_condition_must_be_last_when_entry() {
    assert_source_parses(
        "fun validSpec(valueSpec: Int) = when (valueSpec) { 0 -> \"zero\"; else -> \"other\" }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Int) = when (valueSpec) { else -> \"other\"; 0 -> \"zero\" }\n",
    );
}

#[test]
#[ignore = "KS-8.6-005: kmp-lsp does not diagnose non-exhaustive when in value context"]
fn ks_8_6_005_non_exhaustive_when_cannot_be_used_as_expression() {
    assert_source_parses(
        "fun validSpec(valueSpec: Int) = when (valueSpec) { 0 -> \"zero\"; else -> \"other\" }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Int): String = when (valueSpec) { 0 -> \"zero\" }\n",
    );
}

#[test]
fn ks_8_6_006_when_subject_may_be_immutable_property_declaration_with_initializer() {
    assert_source_parses(
        "fun readSpec(inputSpec: Int) = when (val subjectSpec = inputSpec + 1) {\n    0 -> subjectSpec\n    else -> subjectSpec + 1\n}\n",
    );
}

#[test]
#[ignore = "KS-8.6-007: kmp-lsp does not enforce when-subject property scope"]
fn ks_8_6_007_when_subject_property_scope_is_limited_to_when_expression() {
    assert_source_parses(
        "fun validSpec(inputSpec: Int) = when (val subjectSpec = inputSpec) { 0 -> subjectSpec; else -> subjectSpec + 1 }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(inputSpec: Int) {\n    when (val subjectSpec = inputSpec) { else -> println(subjectSpec) }\n    println(subjectSpec)\n}\n",
    );
}

#[test]
fn ks_8_6_008_when_subject_property_forbids_var_delegation_accessors_and_destructuring() {
    assert_source_parses(
        "fun validSpec(inputSpec: Int) = when (val subjectSpec = inputSpec) { else -> subjectSpec }\n",
    );
    for invalid_source in [
        "fun invalidSpec(inputSpec: Int) = when (var subjectSpec = inputSpec) { else -> subjectSpec }\n",
        "fun invalidSpec(inputSpec: Int) = when (val subjectSpec by lazy { inputSpec }) { else -> subjectSpec }\n",
        "fun invalidSpec(inputSpec: Int) = when (val subjectSpec get() = inputSpec) { else -> subjectSpec }\n",
        "fun invalidSpec(pairSpec: Pair<Int, Int>) = when (val (firstSpec, secondSpec) = pairSpec) { else -> firstSpec + secondSpec }\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_8_6_1_001_boolean_when_is_exhaustive_when_true_and_false_are_covered() {
    let incomplete_source = "fun readSpec(flagSpec: Boolean) = when (flagSpec) { true -> 1 }\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: false"]
    );
    let complete_source =
        "fun readSpec(flagSpec: Boolean) = when (flagSpec) { true -> 1; false -> 0 }\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_8_6_1_002_enum_when_is_exhaustive_when_every_entry_is_covered() {
    let incomplete_source = "enum class StateSpec {\n    READY, DONE\n}\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) {\n    StateSpec.READY -> 1\n}\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: DONE"]
    );
    let complete_source = "enum class StateSpec {\n    READY, DONE\n}\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) {\n    StateSpec.READY -> 1\n    StateSpec.DONE -> 0\n}\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_8_6_1_003_sealed_when_is_exhaustive_when_direct_non_sealed_subtypes_are_covered() {
    let incomplete_source = "sealed interface StateSpec\ndata class ReadySpec(val valueSpec: Int) : StateSpec\ndata object DoneSpec : StateSpec\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) { is ReadySpec -> 1 }\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: DoneSpec"]
    );
    let complete_source = "sealed interface StateSpec\ndata class ReadySpec(val valueSpec: Int) : StateSpec\ndata object DoneSpec : StateSpec\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) { is ReadySpec -> 1; DoneSpec -> 0 }\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_8_6_1_004_else_entry_makes_bounded_when_exhaustive() {
    let source = "enum class StateSpec { READY, DONE }\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) { else -> 0 }\n";
    assert!(when_diagnostic_messages(source).is_empty());
}

#[test]
#[ignore = "KS-8.6.1-005: kmp-lsp exhaustiveness diagnostics omit nullable null branches"]
fn ks_8_6_1_005_nullable_exhaustive_when_requires_null_branch() {
    let incomplete_source = "enum class StateSpec { READY, DONE }\nfun readSpec(stateSpec: StateSpec?) = when (stateSpec) { StateSpec.READY -> 1; StateSpec.DONE -> 0 }\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: null"]
    );
    let complete_source = "enum class StateSpec { READY, DONE }\nfun readSpec(stateSpec: StateSpec?) = when (stateSpec) { StateSpec.READY -> 1; StateSpec.DONE -> 0; null -> -1 }\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_8_7_001_logical_disjunction_accepts_newlines_and_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun valueSpec() {\n    val resultSpec = true\n        || false\n        || true\n}\n",
    );
    assert_eq!(labels, vec![": Boolean"]);
}

#[test]
#[ignore = "KS-8.7-002: kmp-lsp does not type-check logical disjunction operands"]
fn ks_8_7_002_logical_disjunction_operands_must_be_boolean() {
    assert_source_parses("val validSpec = true || false\n");
    assert_source_has_syntax_error("val invalidSpec = 1 || true\n");
}

#[test]
fn ks_8_8_001_logical_conjunction_accepts_newlines_and_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun valueSpec() {\n    val resultSpec = true\n        && true\n        && false\n}\n",
    );
    assert_eq!(labels, vec![": Boolean"]);
}

#[test]
#[ignore = "KS-8.8-002: kmp-lsp does not type-check logical conjunction operands"]
fn ks_8_8_002_logical_conjunction_operands_must_be_boolean() {
    assert_source_parses("val validSpec = true && false\n");
    assert_source_has_syntax_error("val invalidSpec = true && 1\n");
}

#[test]
fn ks_8_9_001_equality_expression_accepts_all_four_operators() {
    assert_source_parses(
        "fun compareSpec(firstSpec: Any?, secondSpec: Any?) {\n    val equalSpec = firstSpec == secondSpec\n    val unequalSpec = firstSpec != secondSpec\n    val identicalSpec = firstSpec === secondSpec\n    val distinctSpec = firstSpec !== secondSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-8.9.1-001: kmp-lsp does not infer reference equality result types"]
fn ks_8_9_1_001_reference_equality_expression_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun compareSpec(firstSpec: Any?, secondSpec: Any?) {\n    val identicalSpec = firstSpec === secondSpec\n    val distinctSpec = firstSpec !== secondSpec\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-8.9.1-002: kmp-lsp does not reject reference equality between unrelated types"]
fn ks_8_9_1_002_reference_equality_rejects_definitely_distinct_unrelated_types() {
    assert_source_parses(
        "open class BaseSpec\nclass FirstSpec : BaseSpec()\nclass SecondSpec : BaseSpec()\nval validSpec = FirstSpec() === BaseSpec()\n",
    );
    assert_source_has_syntax_error(
        "class FirstSpec\nclass SecondSpec\nval invalidSpec = FirstSpec() === SecondSpec()\n",
    );
}

#[test]
#[ignore = "KS-8.9.2-001: kmp-lsp does not infer value equality result types"]
fn ks_8_9_2_001_value_equality_expression_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun compareSpec(firstSpec: Any?, secondSpec: Any?) {\n    val equalSpec = firstSpec == secondSpec\n    val unequalSpec = firstSpec != secondSpec\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-8.9.2-002: kmp-lsp does not reject value equality between unrelated types"]
fn ks_8_9_2_002_value_equality_rejects_definitely_distinct_unrelated_types() {
    assert_source_parses(
        "open class BaseSpec\nclass FirstSpec : BaseSpec()\nclass SecondSpec : BaseSpec()\nval validSpec = FirstSpec() == BaseSpec()\n",
    );
    assert_source_has_syntax_error(
        "class FirstSpec\nclass SecondSpec\nval invalidSpec = FirstSpec() == SecondSpec()\n",
    );
}

#[test]
fn ks_8_10_001_comparison_expression_accepts_four_operators_and_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun compareSpec(firstSpec: Int, secondSpec: Int) {\n    val lessSpec = firstSpec < secondSpec\n    val greaterSpec = firstSpec > secondSpec\n    val atMostSpec = firstSpec <= secondSpec\n    val atLeastSpec = firstSpec >= secondSpec\n}\n",
    );
    assert_eq!(
        labels,
        vec![": Boolean", ": Boolean", ": Boolean", ": Boolean"]
    );
}

#[test]
#[ignore = "KS-8.10-002: kmp-lsp does not validate compareTo return types"]
fn ks_8_10_002_compare_to_operator_must_return_int() {
    assert_source_parses(
        "class ValidSpec { operator fun compareTo(otherSpec: ValidSpec): Int = 0; }\nval validResultSpec = ValidSpec() < ValidSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec { operator fun compareTo(otherSpec: InvalidSpec): String = \"zero\"; }\nval invalidResultSpec = InvalidSpec() < InvalidSpec()\n",
    );
}

#[test]
fn ks_8_11_1_001_type_checking_accepts_is_and_not_is_and_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun checkSpec(valueSpec: Any) {\n    val stringSpec = valueSpec is String\n    val otherSpec = valueSpec !is Number\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-8.11.1-002: kmp-lsp does not validate runtime-available type-check targets"]
fn ks_8_11_1_002_type_check_requires_runtime_available_target_type() {
    assert_source_parses("fun validSpec(valueSpec: Any) = valueSpec is List<*>\n");
    assert_source_has_syntax_error("fun invalidSpec(valueSpec: Any) = valueSpec is List<String>\n");
}

#[test]
fn ks_8_11_2_001_containment_checking_accepts_in_and_not_in_and_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun checkSpec(valueSpec: Int, valuesSpec: List<Int>) {\n    val presentSpec = valueSpec in valuesSpec\n    val absentSpec = valueSpec !in valuesSpec\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-8.11.2-002: kmp-lsp does not validate contains return types"]
fn ks_8_11_2_002_contains_operator_must_return_boolean() {
    assert_source_parses(
        "class ValidSpec { operator fun contains(valueSpec: Int): Boolean = true; }\nval validResultSpec = 1 in ValidSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec { operator fun contains(valueSpec: Int): String = \"yes\"; }\nval invalidResultSpec = 1 in InvalidSpec()\n",
    );
}

#[test]
fn ks_8_12_001_elvis_expression_accepts_chains_and_newlines() {
    assert_source_parses(
        "fun chooseSpec(firstSpec: String?, secondSpec: String?): String = firstSpec\n    ?: secondSpec\n    ?: \"fallback\"\n",
    );
}

#[test]
fn ks_8_13_001_range_expression_accepts_closed_and_until_operators_with_inferred_types() {
    let labels = inlay_hint_labels(
        "fun rangesSpec() {\n    val closedSpec = 1..3\n    val untilSpec = 1..<3\n    val longSpec = 1L..3L\n    val characterSpec = 'a'..'z'\n}\n",
    );
    assert_eq!(
        labels,
        vec![": IntRange", ": IntRange", ": LongRange", ": CharRange"]
    );
}

#[test]
fn ks_8_14_001_additive_expression_accepts_plus_minus_and_newlines() {
    assert_source_parses(
        "fun calculateSpec(firstSpec: Int, secondSpec: Int): Int = firstSpec\n    + secondSpec\n    - 1\n",
    );
}

#[test]
fn ks_8_15_001_multiplicative_expression_accepts_times_division_and_remainder() {
    assert_source_parses("fun calculateSpec(valueSpec: Int): Int = valueSpec * 6 / 3 % 2\n");
}

#[test]
fn ks_8_16_001_cast_expression_accepts_unchecked_and_checked_operators() {
    assert_source_parses(
        "fun castSpec(valueSpec: Any) {\n    val uncheckedSpec = valueSpec as String\n    val checkedSpec = valueSpec as? String\n}\n",
    );
}

#[test]
#[ignore = "KS-8.16-002: kmp-lsp does not infer cast expression types"]
fn ks_8_16_002_cast_expression_has_the_specified_nullable_or_non_nullable_type() {
    assert_source_parses(
        "fun castSpec(valueSpec: Any) {\n    val uncheckedSpec = valueSpec as String\n    val checkedSpec = valueSpec as? String\n}\n",
    );
    let labels = inlay_hint_labels(
        "fun castSpec(valueSpec: Any) {\n    val uncheckedSpec = valueSpec as String\n    val checkedSpec = valueSpec as? String\n}\n",
    );
    assert_eq!(labels, vec![": String", ": String?"]);
}

#[test]
#[ignore = "KS-8.16-003: kmp-lsp does not warn about unchecked generic casts"]
fn ks_8_16_003_checked_cast_warns_about_an_unchecked_generic_target() {
    assert_source_parses("fun validSpec(valueSpec: Any) = valueSpec as? List<*>\n");
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Any) = valueSpec as? List<String>\n",
    );
}

#[test]
fn ks_8_17_1_001_expression_accepts_multiple_prefix_annotations() {
    assert_source_parses(
        "@Target(AnnotationTarget.EXPRESSION) annotation class MarkerSpec\nfun annotateSpec(valueSpec: Int): Int = @MarkerSpec @MarkerSpec valueSpec\n",
    );
}

#[test]
#[ignore = "KS-8.17.2-001: kmp-lsp does not diagnose non-assignable prefix increment operands"]
fn ks_8_17_2_001_prefix_increment_requires_an_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; ++valueSpec }\n");
    assert_source_has_syntax_error("fun invalidSpec() { ++1 }\n");
}

#[test]
#[ignore = "KS-8.17.2-002: kmp-lsp does not validate prefix inc return types"]
fn ks_8_17_2_002_prefix_increment_result_must_be_assignable_to_the_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun inc(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); ++valueSpec }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun inc(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); ++valueSpec }\n",
    );
}

#[test]
#[ignore = "KS-8.17.3-001: kmp-lsp does not diagnose non-assignable prefix decrement operands"]
fn ks_8_17_3_001_prefix_decrement_requires_an_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; --valueSpec }\n");
    assert_source_has_syntax_error("fun invalidSpec() { --1 }\n");
}

#[test]
#[ignore = "KS-8.17.3-002: kmp-lsp does not validate prefix dec return types"]
fn ks_8_17_3_002_prefix_decrement_result_must_be_assignable_to_the_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun dec(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); --valueSpec }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun dec(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); --valueSpec }\n",
    );
}

#[test]
fn ks_8_17_4_001_prefix_arithmetic_and_logical_operators_accept_builtin_operands() {
    assert_source_parses(
        "fun prefixSpec(numberSpec: Int, flagSpec: Boolean) {\n    val negativeSpec = -numberSpec\n    val positiveSpec = +numberSpec\n    val invertedSpec = !flagSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-8.17.4-002: kmp-lsp does not infer unary plus or minus expression types"]
fn ks_8_17_4_002_prefix_arithmetic_and_logical_operators_have_operator_return_types() {
    assert_source_parses(
        "fun prefixSpec(numberSpec: Int, flagSpec: Boolean) {\n    val negativeSpec = -numberSpec\n    val positiveSpec = +numberSpec\n    val invertedSpec = !flagSpec\n}\n",
    );
    let labels = inlay_hint_labels(
        "fun prefixSpec(numberSpec: Int, flagSpec: Boolean) {\n    val negativeSpec = -numberSpec\n    val positiveSpec = +numberSpec\n    val invertedSpec = !flagSpec\n}\n",
    );
    assert_eq!(labels, vec![": Int", ": Int", ": Boolean"]);
}

#[test]
fn ks_8_18_001_postfix_increment_and_decrement_accept_assignable_operands() {
    assert_source_parses(
        "fun postfixSpec() {\n    var valueSpec = 1\n    valueSpec++\n    valueSpec--\n}\n",
    );
}

#[test]
#[ignore = "KS-8.18-002: kmp-lsp does not diagnose non-assignable postfix increment operands"]
fn ks_8_18_002_postfix_increment_requires_an_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; valueSpec++ }\n");
    assert_source_has_syntax_error("fun invalidSpec() { 1++ }\n");
}

#[test]
#[ignore = "KS-8.18-003: kmp-lsp does not diagnose non-assignable postfix decrement operands"]
fn ks_8_18_003_postfix_decrement_requires_an_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; valueSpec-- }\n");
    assert_source_has_syntax_error("fun invalidSpec() { 1-- }\n");
}

#[test]
#[ignore = "KS-8.18-004: kmp-lsp does not validate postfix inc return types"]
fn ks_8_18_004_postfix_increment_result_must_be_assignable_to_the_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun inc(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); valueSpec++ }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun inc(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); valueSpec++ }\n",
    );
}

#[test]
#[ignore = "KS-8.18-005: kmp-lsp does not validate postfix dec return types"]
fn ks_8_18_005_postfix_decrement_result_must_be_assignable_to_the_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun dec(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); valueSpec-- }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun dec(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); valueSpec-- }\n",
    );
}

#[test]
fn ks_8_19_001_not_null_assertion_accepts_a_nullable_operand() {
    assert_source_parses(
        "fun assertSpec(valueSpec: String?) {\n    val assertedSpec = valueSpec!!\n}\n",
    );
}

#[test]
#[ignore = "KS-8.19-002: kmp-lsp does not infer not-null assertion expression types"]
fn ks_8_19_002_not_null_assertion_has_the_non_nullable_operand_type() {
    assert_source_parses(
        "fun assertSpec(valueSpec: String?) {\n    val assertedSpec = valueSpec!!\n}\n",
    );
    let labels = inlay_hint_labels(
        "fun assertSpec(valueSpec: String?) {\n    val assertedSpec = valueSpec!!\n}\n",
    );
    assert_eq!(labels, vec![": String"]);
}

#[test]
#[ignore = "KS-8.20-001: tree-sitter-kotlin rejects trailing commas in indexing expressions"]
fn ks_8_20_001_indexing_expression_accepts_multiple_indices_and_a_trailing_comma() {
    assert_source_parses(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) = gridSpec[0, 1]\n",
    );
    assert_source_parses(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) = gridSpec[\n    0,\n    1,\n]\n",
    );
}

#[test]
#[ignore = "KS-8.20-002: kmp-lsp does not infer indexing expression types"]
fn ks_8_20_002_indexing_expression_has_the_selected_get_return_type() {
    assert_source_parses(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) { val cellSpec = gridSpec[0, 1] }\n",
    );
    let labels = inlay_hint_labels(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) { val cellSpec = gridSpec[0, 1] }\n",
    );
    assert_eq!(labels, vec![": String"]);
}

#[test]
fn ks_8_20_003_indexing_expression_is_an_assignable_expression() {
    assert_source_parses(
        "class GridSpec {\n    operator fun set(rowSpec: Int, columnSpec: Int, valueSpec: String) {}\n}\nfun writeSpec(gridSpec: GridSpec) { gridSpec[0, 1] = \"cell\" }\n",
    );
}

#[test]
fn ks_8_21_1_001_navigation_expressions_accept_direct_safe_and_reference_operators() {
    assert_source_parses(
        "class HolderSpec(val textSpec: String) {\n    fun lengthSpec(): Int = textSpec.length\n}\nfun navigateSpec(holderSpec: HolderSpec?) {\n    val directSpec = HolderSpec(\"value\").textSpec\n    val safeSpec = holderSpec?.lengthSpec()\n    val referenceSpec = HolderSpec::textSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-8.21.1-002: kmp-lsp drops nullability from safe-navigation result hints"]
fn ks_8_21_1_002_safe_navigation_expression_has_a_nullable_result_type() {
    assert_source_parses(
        "class HolderSpec(val textSpec: String)\nfun navigateSpec(holderSpec: HolderSpec?) { val safeSpec = holderSpec?.textSpec }\n",
    );
    let labels = inlay_hint_labels(
        "class HolderSpec(val textSpec: String)\nfun navigateSpec(holderSpec: HolderSpec?) { val safeSpec = holderSpec?.textSpec }\n",
    );
    assert_eq!(labels, vec![": String?"]);
}

#[test]
fn ks_8_21_2_001_callable_references_accept_type_and_value_properties_and_functions() {
    assert_source_parses(
        "class CallableSpec(val valueSpec: Int) {\n    fun renderSpec(): String = valueSpec.toString()\n}\nfun referencesSpec(callableSpec: CallableSpec) {\n    val typePropertySpec = CallableSpec::valueSpec\n    val typeFunctionSpec = CallableSpec::renderSpec\n    val valuePropertySpec = callableSpec::valueSpec\n    val valueFunctionSpec = callableSpec::renderSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-8.21.2-002: kmp-lsp does not reject member-extension callable references"]
fn ks_8_21_2_002_callable_reference_forbids_a_member_extension() {
    assert_source_parses(
        "class ValidSpec {\n    fun memberSpec(): Unit {}\n}\nfun validSpec() { val referenceSpec = ValidSpec::memberSpec }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun String.memberExtensionSpec(): Unit {}\n}\nfun invalidSpec() { val referenceSpec = InvalidSpec::memberExtensionSpec }\n",
    );
}

#[test]
fn ks_8_21_3_001_class_literals_accept_type_and_value_receivers() {
    assert_source_parses(
        "fun classLiteralsSpec(valueSpec: Any) {\n    val typeLiteralSpec = String::class\n    val valueLiteralSpec = valueSpec::class\n}\n",
    );
}

#[test]
fn ks_8_21_3_002_type_class_literal_requires_a_non_nullable_runtime_available_type() {
    assert_source_parses("val validSpec = String::class\n");
    assert_source_has_syntax_error("val invalidSpec = String?::class\n");
}

#[test]
#[ignore = "KS-8.21.3-003: kmp-lsp does not infer class-literal KClass types"]
fn ks_8_21_3_003_class_literal_has_a_kclass_type() {
    assert_source_parses("fun literalSpec() { val typeLiteralSpec = String::class }\n");
    let labels = inlay_hint_labels("fun literalSpec() { val typeLiteralSpec = String::class }\n");
    assert_eq!(labels, vec![": KClass<String>"]);
}

#[test]
fn ks_8_21_4_001_function_call_accepts_receiver_named_vararg_default_and_trailing_lambda_arguments()
{
    assert_source_parses(
        "class CallerSpec {\n    fun callSpec(firstSpec: Int = 1, vararg restSpec: String, blockSpec: () -> Unit) {}\n}\nfun invokeSpec(callerSpec: CallerSpec) {\n    callerSpec.callSpec(restSpec = arrayOf(\"a\", \"b\")) { println(\"done\") }\n}\n",
    );
}

#[test]
fn ks_8_21_5_001_spread_arguments_mix_with_regular_vararg_arguments() {
    assert_source_parses(
        "fun consumeSpec(vararg valuesSpec: String) {}\nfun spreadSpec(valuesSpec: Array<String>) { consumeSpec(\"before\", *valuesSpec, \"after\") }\n",
    );
}

#[test]
#[ignore = "KS-8.21.5-002: kmp-lsp does not restrict spread expressions to value arguments"]
fn ks_8_21_5_002_spread_expression_is_allowed_only_as_a_value_argument() {
    assert_source_parses(
        "fun consumeSpec(vararg valuesSpec: String) {}\nfun validSpec(valuesSpec: Array<String>) { consumeSpec(*valuesSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(valuesSpec: Array<String>) { val copiedSpec = *valuesSpec }\n",
    );
}

#[test]
#[ignore = "KS-8.21.5-003: kmp-lsp does not validate spread argument array types"]
fn ks_8_21_5_003_spread_argument_type_must_match_the_vararg_array_type() {
    assert_source_parses(
        "fun consumeSpec(vararg valuesSpec: String) {}\nfun validSpec(valuesSpec: Array<String>) { consumeSpec(*valuesSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun consumeSpec(vararg valuesSpec: String) {}\nfun invalidSpec(valuesSpec: IntArray) { consumeSpec(*valuesSpec) }\n",
    );
}
