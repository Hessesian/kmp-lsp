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
fn ks_expressions_0001_expression_context_is_determined_by_statement_position() {
    assert_source_parses(
        "fun consumeSpec(valueSpec: Int) {}\nfun renderSpec() {\n    1 + 2\n    consumeSpec(1 + 2)\n}\n",
    );
}

#[test]
fn ks_expressions_0006_true_and_false_have_boolean_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec() {\n    val enabledSpec = true\n    val disabledSpec = false\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0005: tree-sitter-kotlin accepts true as an unescaped identifier"]
fn ks_expressions_0005_true_keyword_requires_escaping_when_used_as_identifier() {
    assert_source_parses("val `true`: Boolean = false\nval copiedSpec = `true`\n");
    assert_source_has_syntax_error("val true: Boolean = false\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0005: tree-sitter-kotlin accepts false as an unescaped identifier"]
fn ks_expressions_0005_false_keyword_requires_escaping_when_used_as_identifier() {
    assert_source_parses("val `false`: Boolean = true\nval copiedSpec = `false`\n");
    assert_source_has_syntax_error("val false: Boolean = true\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0007: kmp-lsp does not reject misplaced decimal underscores"]
fn ks_expressions_0007_decimal_literal_accepts_internal_underscores_only() {
    assert_source_parses("val valuesSpec = listOf(0, 7, 1_000, 12_34_56)\n");
    for invalid_source in ["val valueSpec = _1\n", "val valueSpec = 1_\n"] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
#[ignore = "KS-EXPRESSIONS-0008: tree-sitter-kotlin accepts leading-zero decimal literals"]
fn ks_expressions_0008_decimal_literal_cannot_use_leading_zero_or_octal_form() {
    assert_source_parses("val zeroSpec = 0\nval eightSpec = 8\n");
    for invalid_source in ["val valueSpec = 01\n", "val valueSpec = 077\n"] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_expressions_0009_hexadecimal_literal_requires_prefix_digits_and_internal_underscores() {
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
#[ignore = "KS-EXPRESSIONS-0010: tree-sitter-kotlin rejects valid binary digit separators"]
fn ks_expressions_0010_binary_literal_requires_prefix_binary_digits_and_internal_underscores() {
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
fn ks_expressions_0011_long_suffix_is_accepted_for_all_integer_radices() {
    assert_source_parses("val valuesSpec = listOf(1L, 0x1L, 0b1L)\n");
}

#[test]
fn ks_expressions_0012_long_suffix_gives_all_integer_radices_long_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec() {\n    val decimalSpec = 1L\n    val hexadecimalSpec = 0x1L\n    val binarySpec = 0b1L\n}\n",
    );
    assert_eq!(labels, vec![": Long", ": Long", ": Long"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0013: kmp-lsp does not diagnose integer literals above Long maximum"]
fn ks_expressions_0013_integer_above_long_maximum_is_illegal() {
    assert_source_parses("val maximumSpec = 9223372036854775807L\n");
    assert_source_has_syntax_error("val overflowSpec = 9223372036854775808\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0014: kmp-lsp infers every unsuffixed integer literal as Int"]
fn ks_expressions_0014_unsuffixed_integer_above_int_maximum_has_long_type() {
    let labels = inlay_hint_labels("fun valueSpec() { val largeSpec = 2147483648 }\n");
    assert_eq!(labels, vec![": Long"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0016: kmp-lsp does not diagnose incomplete real-literal exponents"]
fn ks_expressions_0016_real_literal_accepts_decimal_fraction_exponent_and_float_suffix_forms() {
    assert_source_parses("val valuesSpec = listOf(1.0, .5, 1e3, 1E+3, 1e-3, 1.0e3, 1f, 1F, .5f)\n");
    for invalid_source in [
        "val valueSpec = 0x1.0\n",
        "val valueSpec = 1e\n",
        "val valueSpec = 1e+\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_expressions_0017_real_literal_cannot_omit_fraction_after_decimal_point() {
    assert_source_parses("val valuesSpec = listOf(1.0, 1e2, 1f)\n");
    assert_source_has_syntax_error("val valueSpec = 1.\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0018: kmp-lsp does not reject misplaced real-literal underscores"]
fn ks_expressions_0018_real_literal_allows_underscores_only_inside_numeric_parts() {
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
fn ks_expressions_0019_real_literal_suffix_determines_float_or_double_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec() {\n    val doubleSpec = 1.0\n    val exponentSpec = 1e3\n    val lowerFloatSpec = 1.0f\n    val upperFloatSpec = 1F\n}\n",
    );
    assert_eq!(labels, vec![": Double", ": Double", ": Float", ": Float"]);
}

#[test]
fn ks_expressions_0020_simple_character_literal_contains_one_allowed_character() {
    assert_source_parses("val characterSpec = 'A'\n");
    for invalid_source in [
        "val characterSpec = ''\n",
        "val characterSpec = 'AB'\n",
        "val characterSpec = '\n'\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_expressions_0021_character_literal_has_char_type() {
    let labels = inlay_hint_labels("fun valueSpec() { val characterSpec = 'A' }\n");
    assert_eq!(labels, vec![": Char"]);
}

#[test]
fn ks_expressions_0022_character_literal_accepts_all_simple_escape_sequences() {
    assert_source_parses(
        r#"val valuesSpec = listOf('\t', '\b', '\r', '\n', '\'', '\"', '\\', '\$')
"#,
    );
}

#[test]
fn ks_expressions_0024_unicode_character_escape_requires_exactly_four_hex_digits() {
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
fn ks_expressions_0028_null_literal_has_nothing_nullable_type() {
    let labels = inlay_hint_labels("fun valueSpec() { val absentSpec = null }\n");
    assert_eq!(labels, vec![": Nothing?"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0027: kmp-lsp does not diagnose null assigned to non-null types"]
fn ks_expressions_0027_null_literal_is_valid_only_for_nullable_types() {
    assert_source_parses("val validSpec: String? = null\n");
    assert_source_has_syntax_error("val invalidSpec: String = null\n");
}

#[test]
fn ks_expressions_0038_string_interpolation_has_line_and_multiline_forms() {
    assert_source_parses(
        "fun valuesSpec(nameSpec: String) {\n    val lineSpec = \"Hello, $nameSpec\"\n    val multilineSpec = \"\"\"Hello,\n$nameSpec\"\"\"\n}\n",
    );
}

#[test]
fn ks_expressions_0032_string_interpolation_combines_content_and_expression_fragments() {
    assert_source_parses(
        "fun valueSpec(nameSpec: String, countSpec: Int) = \"Name: $nameSpec; next: ${countSpec + 1}.\"\n",
    );
}

#[test]
fn ks_expressions_0033_simple_interpolation_path_requires_braces_for_qualified_path() {
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
#[ignore = "KS-EXPRESSIONS-0039: tree-sitter-kotlin accepts raw newlines inside line strings"]
fn ks_expressions_0039_line_strings_require_newlines_to_be_escaped() {
    assert_source_parses(
        "val lineSpec = \"first\\nsecond\"\nval multilineSpec = \"\"\"first\nsecond \\n\"\"\"\n",
    );
    assert_source_has_syntax_error("val invalidSpec = \"first\nsecond\"\n");
}

#[test]
fn ks_expressions_0040_multiline_strings_allow_raw_newlines() {
    assert_source_parses("val multilineSpec = \"\"\"first\nsecond\"\"\"\n");
}

#[test]
fn ks_expressions_0042_string_interpolation_always_has_string_type() {
    let labels = inlay_hint_labels(
        "fun valuesSpec(nameSpec: String) {\n    val lineSpec = \"$nameSpec\"\n    val multilineSpec = \"\"\"$nameSpec\"\"\"\n}\n",
    );
    assert_eq!(labels, vec![": String", ": String"]);
}

#[test]
fn ks_expressions_0043_try_expression_accepts_catches_optional_finally_or_finally_only() {
    assert_source_parses(
        "fun readSpec() {\n    try { println(1) } catch (failureSpec: IllegalStateException) { println(failureSpec) } catch (failureSpec: RuntimeException) { println(failureSpec) } finally { println(2) }\n    try { println(3) } finally { println(4) }\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0044: tree-sitter-kotlin rejects a trailing comma in catch parameters"]
fn ks_expressions_0044_catch_has_one_annotated_typed_parameter_with_optional_trailing_comma() {
    assert_source_parses(
        "annotation class MarkerSpec\nfun validSpec() {\n    try { println(1) } catch (@MarkerSpec failureSpec: RuntimeException) { println(failureSpec) }\n}\n",
    );
    assert_source_parses(
        "annotation class MarkerSpec\nfun readSpec() {\n    try { println(1) } catch (@MarkerSpec failureSpec: RuntimeException,) { println(failureSpec) }\n}\n",
    );
}

#[test]
fn ks_expressions_0045_try_expression_requires_catch_or_finally_block() {
    assert_source_parses("fun validSpec() { try { println(1) } finally { println(2) } }\n");
    assert_source_has_syntax_error("fun invalidSpec() { try { println(1) } }\n");
}

#[test]
fn ks_expressions_0054_conditional_expression_accepts_single_two_and_empty_branch_forms() {
    assert_source_parses(
        "fun renderSpec(flagSpec: Boolean) {\n    if (flagSpec) println(1)\n    if (flagSpec) { println(2) } else println(3)\n    if (flagSpec);\n}\n",
    );
}

#[test]
fn ks_expressions_0056_branchless_conditional_with_else_semicolon_is_valid() {
    assert_source_parses("fun renderSpec(flagSpec: Boolean) { if (flagSpec) else; }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0060: kmp-lsp does not diagnose branch-incomplete if in expression context"]
fn ks_expressions_0060_conditional_missing_a_branch_cannot_be_used_as_expression() {
    assert_source_parses("val validSpec = if (true) 1 else 2\n");
    assert_source_has_syntax_error("val invalidSpec = if (true) 1\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0061: kmp-lsp does not type-check conditional conditions"]
fn ks_expressions_0061_conditional_condition_must_be_boolean() {
    assert_source_parses("val validSpec = if (true) 1 else 2\n");
    assert_source_has_syntax_error("val invalidSpec = if (1) 1 else 2\n");
}

#[test]
fn ks_expressions_0062_conditional_expression_has_side_dependent_binary_precedence() {
    assert_source_parses(
        "fun updateSpec() {\n    var valueSpec = 0\n    valueSpec = if (true) 1 else 2\n    if (true) valueSpec = 1 else valueSpec = 2\n}\n",
    );
}

#[test]
fn ks_expressions_0063_when_expression_accepts_both_subject_forms() {
    assert_source_parses(
        "fun readSpec(valueSpec: Int): String {\n    val subjectlessSpec = when { valueSpec > 0 -> \"positive\"; else -> \"other\" }\n    return when (valueSpec) { 0 -> \"zero\"; else -> subjectlessSpec }\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0064: tree-sitter-kotlin rejects a trailing comma in when conditions"]
fn ks_expressions_0064_when_entry_accepts_condition_list_or_else() {
    assert_source_parses(
        "fun validSpec(valueSpec: Int) = when (valueSpec) {\n    1, 2 -> \"small\"\n    else -> \"other\"\n}\n",
    );
    assert_source_parses(
        "fun readSpec(valueSpec: Int) = when (valueSpec) {\n    1, 2, -> \"small\"\n    else -> \"other\"\n}\n",
    );
}

#[test]
fn ks_expressions_0069_bound_when_accepts_all_condition_forms() {
    assert_source_parses(
        "fun readSpec(valueSpec: Any, valuesSpec: List<Any>) = when (valueSpec) {\n    is String -> \"string\";\n    !is Number -> \"not number\";\n    in valuesSpec -> \"contained\";\n    !in valuesSpec -> \"not contained\";\n    0 -> \"equal\";\n    else -> \"other\";\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0068: kmp-lsp does not diagnose else before later bound-when entries"]
fn ks_expressions_0068_bound_else_condition_must_be_last_when_entry() {
    assert_source_parses(
        "fun validSpec(valueSpec: Int) = when (valueSpec) { 0 -> \"zero\"; else -> \"other\" }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Int) = when (valueSpec) { else -> \"other\"; 0 -> \"zero\" }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0068: kmp-lsp does not diagnose else before later subjectless-when entries"]
fn ks_expressions_0068_subjectless_else_condition_must_be_last_when_entry() {
    assert_source_parses(
        "fun validSpec(valueSpec: Int) = when { valueSpec == 0 -> \"zero\"; else -> \"other\" }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Int) = when { else -> \"other\"; valueSpec == 0 -> \"zero\" }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0077: kmp-lsp does not diagnose non-exhaustive when in value context"]
fn ks_expressions_0077_non_exhaustive_when_cannot_be_used_as_expression() {
    assert_source_parses(
        "fun validSpec(valueSpec: Int) = when (valueSpec) { 0 -> \"zero\"; else -> \"other\" }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Int): String = when (valueSpec) { 0 -> \"zero\" }\n",
    );
}

#[test]
fn ks_expressions_0078_when_subject_may_be_immutable_property_declaration_with_initializer() {
    assert_source_parses(
        "fun readSpec(inputSpec: Int) = when (val subjectSpec = inputSpec + 1) {\n    0 -> subjectSpec\n    else -> subjectSpec + 1\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0079: kmp-lsp does not enforce when-subject property scope"]
fn ks_expressions_0079_when_subject_property_scope_is_limited_to_when_expression() {
    assert_source_parses(
        "fun validSpec(inputSpec: Int) = when (val subjectSpec = inputSpec) { subjectSpec -> subjectSpec; else -> subjectSpec + 1 }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(inputSpec: Int) {\n    when (val subjectSpec = inputSpec) { else -> println(subjectSpec) }\n    println(subjectSpec)\n}\n",
    );
}

#[test]
fn ks_expressions_0080_when_subject_property_accepts_only_simple_initialized_val() {
    assert_source_parses(
        "fun validSpec(inputSpec: Int) = when (val subjectSpec = inputSpec) { else -> subjectSpec }\n",
    );
    for invalid_source in [
        "fun invalidSpec(inputSpec: Int) = when (var subjectSpec = inputSpec) { else -> subjectSpec }\n",
        "fun invalidSpec(inputSpec: Int) = when (val subjectSpec by lazy { inputSpec }) { else -> subjectSpec }\n",
        "fun invalidSpec(inputSpec: Int) = when (val subjectSpec get() = inputSpec) { else -> subjectSpec }\n",
        "fun invalidSpec(pairSpec: Pair<Int, Int>) = when (val (firstSpec, secondSpec) = pairSpec) { else -> firstSpec + secondSpec }\n",
        "fun invalidSpec() = when (val subjectSpec) { else -> subjectSpec }\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_expressions_0082_boolean_when_exhaustiveness_covers_both_values() {
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
fn ks_expressions_0088_enum_when_is_exhaustive_when_every_entry_is_covered() {
    let incomplete_source = "enum class StateSpec {\n    READY, DONE\n}\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) {\n    StateSpec.READY -> 1\n}\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: DONE"]
    );
    let complete_source = "enum class StateSpec {\n    READY, DONE\n}\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) {\n    StateSpec.READY -> 1\n    StateSpec.DONE -> 0\n}\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_expressions_0083_sealed_when_covers_all_direct_non_sealed_subtypes() {
    let incomplete_source = "sealed interface StateSpec\ndata class ReadySpec(val valueSpec: Int) : StateSpec\ndata class DoneSpec(val valueSpec: Int) : StateSpec\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) { is ReadySpec -> 1 }\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: DoneSpec"]
    );
    let complete_source = "sealed interface StateSpec\ndata class ReadySpec(val valueSpec: Int) : StateSpec\ndata class DoneSpec(val valueSpec: Int) : StateSpec\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) { is ReadySpec -> 1; is DoneSpec -> 0 }\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_expressions_0081_else_entry_makes_bounded_when_exhaustive() {
    let source = "enum class StateSpec { READY, DONE }\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) { else -> 0 }\n";
    assert!(when_diagnostic_messages(source).is_empty());
}

#[test]
#[ignore = "KS-EXPRESSIONS-0089: kmp-lsp exhaustiveness diagnostics omit nullable null branches"]
fn ks_expressions_0089_nullable_exhaustive_when_requires_null_branch() {
    let incomplete_source = "enum class StateSpec { READY, DONE }\nfun readSpec(stateSpec: StateSpec?) = when (stateSpec) { StateSpec.READY -> 1; StateSpec.DONE -> 0 }\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: null"]
    );
    let complete_source = "enum class StateSpec { READY, DONE }\nfun readSpec(stateSpec: StateSpec?) = when (stateSpec) { StateSpec.READY -> 1; StateSpec.DONE -> 0; null -> -1 }\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_expressions_0090_object_subtype_may_be_covered_by_equality() {
    let incomplete_source = "sealed interface StateSpec\ndata object DoneSpec : StateSpec\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) {}\n";
    assert_eq!(
        when_diagnostic_messages(incomplete_source),
        vec!["'when' is missing branches: DoneSpec"]
    );
    let complete_source = "sealed interface StateSpec\ndata object DoneSpec : StateSpec\nfun readSpec(stateSpec: StateSpec) = when (stateSpec) { DoneSpec -> 0 }\n";
    assert!(when_diagnostic_messages(complete_source).is_empty());
}

#[test]
fn ks_expressions_0092_logical_disjunction_accepts_newlines() {
    assert_source_parses("val resultSpec = true\n    || false\n    || true\n");
}

#[test]
fn ks_expressions_0096_logical_disjunction_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun valueSpec() {\n    val resultSpec = true\n        || false\n        || true\n}\n",
    );
    assert_eq!(labels, vec![": Boolean"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0095: kmp-lsp does not type-check logical disjunction operands"]
fn ks_expressions_0095_logical_disjunction_operands_must_be_boolean() {
    assert_source_parses("val validSpec = true || false\n");
    assert_source_has_syntax_error("val invalidSpec = 1 || true\n");
}

#[test]
fn ks_expressions_0097_logical_conjunction_accepts_newlines() {
    assert_source_parses("val resultSpec = true\n    && true\n    && false\n");
}

#[test]
fn ks_expressions_0101_logical_conjunction_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun valueSpec() {\n    val resultSpec = true\n        && true\n        && false\n}\n",
    );
    assert_eq!(labels, vec![": Boolean"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0100: kmp-lsp does not type-check logical conjunction operands"]
fn ks_expressions_0100_logical_conjunction_operands_must_be_boolean() {
    assert_source_parses("val validSpec = true && false\n");
    assert_source_has_syntax_error("val invalidSpec = true && 1\n");
}

#[test]
fn ks_expressions_0102_equality_expression_accepts_all_four_operators() {
    assert_source_parses(
        "fun compareSpec(firstSpec: Any?, secondSpec: Any?) {\n    val equalSpec = firstSpec == secondSpec\n    val unequalSpec = firstSpec != secondSpec\n    val identicalSpec = firstSpec === secondSpec\n    val distinctSpec = firstSpec !== secondSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0110: kmp-lsp does not infer reference equality result types"]
fn ks_expressions_0110_reference_equality_expression_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun compareSpec(firstSpec: Any?, secondSpec: Any?) {\n    val identicalSpec = firstSpec === secondSpec\n    val distinctSpec = firstSpec !== secondSpec\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0111: kmp-lsp does not reject reference equality between unrelated types"]
fn ks_expressions_0111_reference_equality_rejects_definitely_distinct_unrelated_types() {
    assert_source_parses(
        "open class BaseSpec\nclass FirstSpec : BaseSpec()\nclass SecondSpec : BaseSpec()\nval validSpec = FirstSpec() === BaseSpec()\n",
    );
    assert_source_has_syntax_error(
        "class FirstSpec\nclass SecondSpec\nval invalidSpec = FirstSpec() === SecondSpec()\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0121: kmp-lsp does not infer value equality result types"]
fn ks_expressions_0121_value_equality_expression_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun compareSpec(firstSpec: Any?, secondSpec: Any?) {\n    val equalSpec = firstSpec == secondSpec\n    val unequalSpec = firstSpec != secondSpec\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0122: kmp-lsp does not reject value equality between unrelated types"]
fn ks_expressions_0122_value_equality_rejects_definitely_distinct_unrelated_types() {
    assert_source_parses(
        "open class BaseSpec\nclass FirstSpec : BaseSpec()\nclass SecondSpec : BaseSpec()\nval validSpec = FirstSpec() == BaseSpec()\n",
    );
    assert_source_has_syntax_error(
        "class FirstSpec\nclass SecondSpec\nval invalidSpec = FirstSpec() == SecondSpec()\n",
    );
}

#[test]
fn ks_expressions_0123_comparison_expression_accepts_four_operators() {
    assert_source_parses(
        "fun compareSpec(firstSpec: Int, secondSpec: Int) {\n    val lessSpec = firstSpec < secondSpec\n    val greaterSpec = firstSpec > secondSpec\n    val atMostSpec = firstSpec <= secondSpec\n    val atLeastSpec = firstSpec >= secondSpec\n}\n",
    );
}

#[test]
fn ks_expressions_0138_comparison_expression_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun compareSpec(firstSpec: Int, secondSpec: Int) {\n    val lessSpec = firstSpec < secondSpec\n    val greaterSpec = firstSpec > secondSpec\n    val atMostSpec = firstSpec <= secondSpec\n    val atLeastSpec = firstSpec >= secondSpec\n}\n",
    );
    assert_eq!(
        labels,
        vec![": Boolean", ": Boolean", ": Boolean", ": Boolean"]
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0137: kmp-lsp does not validate compareTo return types"]
fn ks_expressions_0137_compare_to_operator_must_return_int() {
    assert_source_parses(
        "class ValidSpec { operator fun compareTo(otherSpec: ValidSpec): Int = 0; }\nval validResultSpec = ValidSpec() < ValidSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec { operator fun compareTo(otherSpec: InvalidSpec): String = \"zero\"; }\nval invalidResultSpec = InvalidSpec() < InvalidSpec()\n",
    );
}

#[test]
fn ks_expressions_0139_type_checking_accepts_is_with_not_is() {
    assert_source_parses(
        "fun checkSpec(valueSpec: Any) {\n    val stringSpec = valueSpec is String\n    val otherSpec = valueSpec !is Number\n}\n",
    );
}

#[test]
fn ks_expressions_0146_type_checking_expression_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun checkSpec(valueSpec: Any) {\n    val stringSpec = valueSpec is String\n    val otherSpec = valueSpec !is Number\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0141: kmp-lsp does not validate runtime-available type-check targets"]
fn ks_expressions_0141_type_check_requires_runtime_available_target_type() {
    assert_source_parses("fun validSpec(valueSpec: Any) = valueSpec is List<*>\n");
    assert_source_has_syntax_error("fun invalidSpec(valueSpec: Any) = valueSpec is List<String>\n");
}

#[test]
fn ks_expressions_0149_containment_checking_accepts_in_with_not_in() {
    assert_source_parses(
        "fun checkSpec(valueSpec: Int, valuesSpec: List<Int>) {\n    val presentSpec = valueSpec in valuesSpec\n    val absentSpec = valueSpec !in valuesSpec\n}\n",
    );
}

#[test]
fn ks_expressions_0156_containment_checking_expression_has_boolean_type() {
    let labels = inlay_hint_labels(
        "fun checkSpec(valueSpec: Int, valuesSpec: List<Int>) {\n    val presentSpec = valueSpec in valuesSpec\n    val absentSpec = valueSpec !in valuesSpec\n}\n",
    );
    assert_eq!(labels, vec![": Boolean", ": Boolean"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0155: kmp-lsp does not validate contains return types"]
fn ks_expressions_0155_contains_operator_must_return_boolean() {
    assert_source_parses(
        "class ValidSpec { operator fun contains(valueSpec: Int): Boolean = true; }\nval validResultSpec = 1 in ValidSpec()\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec { operator fun contains(valueSpec: Int): String = \"yes\"; }\nval invalidResultSpec = 1 in InvalidSpec()\n",
    );
}

#[test]
fn ks_expressions_0157_elvis_expression_accepts_chains_with_newlines() {
    assert_source_parses(
        "fun chooseSpec(firstSpec: String?, secondSpec: String?): String = firstSpec\n    ?: secondSpec\n    ?: \"fallback\"\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0161: tree-sitter-kotlin rejects the range-until operator"]
fn ks_expressions_0161_range_expression_accepts_closed_with_until_operator() {
    assert_source_parses("val closedSpec = 1..3\n");
    assert_source_parses("val untilSpec = 1..<3\n");
}

#[test]
fn ks_expressions_0167_range_expression_uses_selected_operator_return_type() {
    let labels = inlay_hint_labels(
        "fun rangesSpec() {\n    val integerSpec = 1..3\n    val longSpec = 1L..3L\n    val characterSpec = 'a'..'z'\n}\n",
    );
    assert_eq!(labels, vec![": IntRange", ": LongRange", ": CharRange"]);
}

#[test]
fn ks_expressions_0168_additive_expression_accepts_plus_with_minus_across_newlines() {
    assert_source_parses(
        "fun calculateSpec(firstSpec: Int, secondSpec: Int): Int = firstSpec\n    + secondSpec\n    - 1\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0174: kmp-lsp does not infer additive expression result types"]
fn ks_expressions_0174_additive_expression_uses_selected_operator_return_type() {
    let labels = inlay_hint_labels(
        "fun calculateSpec() {\n    val sumSpec = 1 + 2\n    val differenceSpec = 3L - 1L\n}\n",
    );
    assert_eq!(labels, vec![": Int", ": Long"]);
}

#[test]
fn ks_expressions_0175_multiplicative_expression_accepts_times_division_with_remainder() {
    assert_source_parses("fun calculateSpec(valueSpec: Int): Int = valueSpec * 6 / 3 % 2\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0183: kmp-lsp does not infer multiplicative expression result types"]
fn ks_expressions_0183_multiplicative_expression_uses_selected_operator_return_type() {
    let labels = inlay_hint_labels(
        "fun calculateSpec() {\n    val productSpec = 2 * 3\n    val quotientSpec = 6L / 3L\n    val remainderSpec = 7 % 4\n}\n",
    );
    assert_eq!(labels, vec![": Int", ": Long", ": Int"]);
}

#[test]
fn ks_expressions_0184_cast_expression_accepts_as_with_safe_as_operator() {
    assert_source_parses(
        "fun castSpec(valueSpec: Any) {\n    val uncheckedSpec = valueSpec as String\n    val checkedSpec = valueSpec as? String\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0188: kmp-lsp does not infer unchecked cast expression types"]
fn ks_expressions_0188_unchecked_cast_has_target_type() {
    let labels = inlay_hint_labels(
        "fun castSpec(valueSpec: Any) {\n    val uncheckedSpec = valueSpec as String\n}\n",
    );
    assert_eq!(labels, vec![": String"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0191: kmp-lsp does not warn about non-runtime-available cast targets"]
fn ks_expressions_0191_checked_cast_warns_for_non_runtime_available_target() {
    assert_source_parses("fun validSpec(valueSpec: Any) = valueSpec as? String\n");
    assert_source_has_syntax_error(
        "fun <TargetSpec> invalidSpec(valueSpec: Any) = valueSpec as? TargetSpec\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0193: kmp-lsp does not warn about unchecked generic casts"]
fn ks_expressions_0193_checked_cast_warns_for_unchecked_generic_arguments() {
    assert_source_parses("fun validSpec(valueSpec: Any) = valueSpec as? List<*>\n");
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Any) = valueSpec as? List<String>\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0195: kmp-lsp does not infer checked cast expression types"]
fn ks_expressions_0195_checked_cast_has_nullable_target_type() {
    let labels = inlay_hint_labels(
        "fun castSpec(valueSpec: Any) {\n    val checkedSpec = valueSpec as? String\n}\n",
    );
    assert_eq!(labels, vec![": String?"]);
}

#[test]
fn ks_expressions_0197_expression_accepts_multiple_prefix_annotations() {
    assert_source_parses(
        "@Target(AnnotationTarget.EXPRESSION) annotation class MarkerSpec\nfun annotateSpec(valueSpec: Int): Int = @MarkerSpec @MarkerSpec valueSpec\n",
    );
}

#[test]
fn ks_expressions_0199_prefix_increment_uses_prefix_operator() {
    assert_source_parses("fun incrementSpec() { var valueSpec = 1; ++valueSpec }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0202: kmp-lsp does not diagnose non-assignable prefix increment operands"]
fn ks_expressions_0202_prefix_increment_requires_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; ++valueSpec }\n");
    assert_source_has_syntax_error("fun invalidSpec() { ++1 }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0203: kmp-lsp does not validate prefix inc return types"]
fn ks_expressions_0203_prefix_increment_result_must_be_subtype_of_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun inc(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); ++valueSpec }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun inc(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); ++valueSpec }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0204: kmp-lsp does not infer prefix increment result types"]
fn ks_expressions_0204_prefix_increment_uses_inc_return_type() {
    let labels = inlay_hint_labels(
        "fun incrementSpec() {\n    var valueSpec: Int = 1\n    val resultSpec = ++valueSpec\n}\n",
    );
    assert_eq!(labels, vec![": Int"]);
}

#[test]
fn ks_expressions_0205_prefix_decrement_uses_prefix_operator() {
    assert_source_parses("fun decrementSpec() { var valueSpec = 1; --valueSpec }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0208: kmp-lsp does not diagnose non-assignable prefix decrement operands"]
fn ks_expressions_0208_prefix_decrement_requires_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; --valueSpec }\n");
    assert_source_has_syntax_error("fun invalidSpec() { --1 }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0209: kmp-lsp does not validate prefix dec return types"]
fn ks_expressions_0209_prefix_decrement_result_must_be_subtype_of_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun dec(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); --valueSpec }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun dec(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); --valueSpec }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0210: kmp-lsp does not infer prefix decrement result types"]
fn ks_expressions_0210_prefix_decrement_uses_dec_return_type() {
    let labels = inlay_hint_labels(
        "fun decrementSpec() {\n    var valueSpec: Int = 1\n    val resultSpec = --valueSpec\n}\n",
    );
    assert_eq!(labels, vec![": Int"]);
}

#[test]
fn ks_expressions_0211_unary_minus_accepts_prefix_operator() {
    assert_source_parses("fun negateSpec(numberSpec: Int) = -numberSpec\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0213: kmp-lsp does not infer unary minus expression types"]
fn ks_expressions_0213_unary_minus_reflects_operator_return_type() {
    let labels = inlay_hint_labels(
        "fun negateSpec(numberSpec: Int) {\n    val negativeSpec = -numberSpec\n}\n",
    );
    assert_eq!(labels, vec![": Int"]);
}

#[test]
fn ks_expressions_0215_unary_plus_accepts_prefix_operator() {
    assert_source_parses("fun preserveSpec(numberSpec: Int) = +numberSpec\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0217: kmp-lsp does not infer unary plus expression types"]
fn ks_expressions_0217_unary_plus_reflects_operator_return_type() {
    let labels = inlay_hint_labels(
        "fun preserveSpec(numberSpec: Int) {\n    val positiveSpec = +numberSpec\n}\n",
    );
    assert_eq!(labels, vec![": Int"]);
}

#[test]
fn ks_expressions_0219_logical_not_accepts_prefix_operator() {
    assert_source_parses("fun invertSpec(flagSpec: Boolean) = !flagSpec\n");
}

#[test]
fn ks_expressions_0221_logical_not_reflects_operator_return_type() {
    let labels = inlay_hint_labels(
        "fun invertSpec(flagSpec: Boolean) {\n    val invertedSpec = !flagSpec\n}\n",
    );
    assert_eq!(labels, vec![": Boolean"]);
}

#[test]
fn ks_expressions_0223_postfix_increment_uses_postfix_operator() {
    assert_source_parses("fun incrementSpec() { var valueSpec = 1; valueSpec++ }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0226: kmp-lsp does not diagnose non-assignable postfix increment operands"]
fn ks_expressions_0226_postfix_increment_requires_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; valueSpec++ }\n");
    assert_source_has_syntax_error("fun invalidSpec() { 1++ }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0227: kmp-lsp does not validate postfix inc return types"]
fn ks_expressions_0227_postfix_increment_result_must_be_subtype_of_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun inc(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); valueSpec++ }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun inc(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); valueSpec++ }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0228: kmp-lsp does not infer postfix increment result types"]
fn ks_expressions_0228_postfix_increment_has_operand_type() {
    let labels = inlay_hint_labels(
        "fun incrementSpec() {\n    var valueSpec: Int = 1\n    val resultSpec = valueSpec++\n}\n",
    );
    assert_eq!(labels, vec![": Int"]);
}

#[test]
fn ks_expressions_0229_postfix_decrement_uses_postfix_operator() {
    assert_source_parses("fun decrementSpec() { var valueSpec = 1; valueSpec-- }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0232: kmp-lsp does not diagnose non-assignable postfix decrement operands"]
fn ks_expressions_0232_postfix_decrement_requires_assignable_operand() {
    assert_source_parses("fun validSpec() { var valueSpec = 1; valueSpec-- }\n");
    assert_source_has_syntax_error("fun invalidSpec() { 1-- }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0233: kmp-lsp does not validate postfix dec return types"]
fn ks_expressions_0233_postfix_decrement_result_must_be_subtype_of_operand() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun dec(): ValidSpec = this\n}\nfun validSpec() { var valueSpec = ValidSpec(); valueSpec-- }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    operator fun dec(): String = \"invalid\"\n}\nfun invalidSpec() { var valueSpec = InvalidSpec(); valueSpec-- }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0234: kmp-lsp does not infer postfix decrement result types"]
fn ks_expressions_0234_postfix_decrement_has_operand_type() {
    let labels = inlay_hint_labels(
        "fun decrementSpec() {\n    var valueSpec: Int = 1\n    val resultSpec = valueSpec--\n}\n",
    );
    assert_eq!(labels, vec![": Int"]);
}

#[test]
fn ks_expressions_0235_not_null_assertion_accepts_nullable_operand() {
    assert_source_parses(
        "fun assertSpec(valueSpec: String?) {\n    val assertedSpec = valueSpec!!\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0239: kmp-lsp does not infer not-null assertion expression types"]
fn ks_expressions_0239_not_null_assertion_has_non_nullable_operand_type() {
    assert_source_parses(
        "fun assertSpec(valueSpec: String?) {\n    val assertedSpec = valueSpec!!\n}\n",
    );
    let labels = inlay_hint_labels(
        "fun assertSpec(valueSpec: String?) {\n    val assertedSpec = valueSpec!!\n}\n",
    );
    assert_eq!(labels, vec![": String"]);
}

#[test]
#[ignore = "KS-EXPRESSIONS-0241: tree-sitter-kotlin rejects trailing commas in indexing expressions"]
fn ks_expressions_0241_indexing_expression_accepts_multiple_indices_with_trailing_comma() {
    assert_source_parses(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) = gridSpec[0, 1]\n",
    );
    assert_source_parses(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) = gridSpec[\n    0,\n    1,\n]\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0244: kmp-lsp does not infer indexing expression types"]
fn ks_expressions_0244_indexing_expression_has_selected_get_return_type() {
    assert_source_parses(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) { val cellSpec = gridSpec[0, 1] }\n",
    );
    let labels = inlay_hint_labels(
        "class GridSpec {\n    operator fun get(rowSpec: Int, columnSpec: Int): String = \"cell\"\n}\nfun readSpec(gridSpec: GridSpec) { val cellSpec = gridSpec[0, 1] }\n",
    );
    assert_eq!(labels, vec![": String"]);
}

#[test]
fn ks_expressions_0245_indexing_expression_is_assignable() {
    assert_source_parses(
        "class GridSpec {\n    operator fun set(rowSpec: Int, columnSpec: Int, valueSpec: String) {}\n}\nfun writeSpec(gridSpec: GridSpec) { gridSpec[0, 1] = \"cell\" }\n",
    );
}

#[test]
fn ks_expressions_0246_navigation_accepts_direct_safe_with_reference_operators() {
    assert_source_parses(
        "class HolderSpec(val textSpec: String) {\n    fun lengthSpec(): Int = textSpec.length\n}\nfun navigateSpec(holderSpec: HolderSpec?) {\n    val directSpec = HolderSpec(\"value\").textSpec\n    val safeSpec = holderSpec?.lengthSpec()\n    val referenceSpec = HolderSpec::textSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0259: kmp-lsp drops nullability from safe-navigation result hints"]
fn ks_expressions_0259_safe_navigation_has_nullable_result_type() {
    assert_source_parses(
        "class HolderSpec(val textSpec: String)\nfun navigateSpec(holderSpec: HolderSpec?) { val safeSpec = holderSpec?.textSpec }\n",
    );
    let labels = inlay_hint_labels(
        "class HolderSpec(val textSpec: String)\nfun navigateSpec(holderSpec: HolderSpec?) { val safeSpec = holderSpec?.textSpec }\n",
    );
    assert_eq!(labels, vec![": String?"]);
}

#[test]
fn ks_expressions_0261_callable_reference_accepts_type_property() {
    assert_source_parses(
        "class CallableSpec(val valueSpec: Int)\nval referenceSpec = CallableSpec::valueSpec\n",
    );
}

#[test]
fn ks_expressions_0262_callable_reference_accepts_type_function() {
    assert_source_parses(
        "class CallableSpec {\n    fun renderSpec(): String = \"value\"\n}\nval referenceSpec = CallableSpec::renderSpec\n",
    );
}

#[test]
fn ks_expressions_0263_callable_reference_accepts_value_property() {
    assert_source_parses(
        "class CallableSpec(val valueSpec: Int)\nfun referenceSpec(callableSpec: CallableSpec) = callableSpec::valueSpec\n",
    );
}

#[test]
fn ks_expressions_0264_callable_reference_accepts_value_function() {
    assert_source_parses(
        "class CallableSpec {\n    fun renderSpec(): String = \"value\"\n}\nfun referenceSpec(callableSpec: CallableSpec) = callableSpec::renderSpec\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0266: kmp-lsp does not reject member-extension callable references"]
fn ks_expressions_0266_callable_reference_forbids_member_extension() {
    assert_source_parses(
        "class ValidSpec {\n    fun memberSpec(): Unit {}\n}\nfun validSpec() { val referenceSpec = ValidSpec::memberSpec }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun String.memberExtensionSpec(): Unit {}\n}\nfun invalidSpec() { val referenceSpec = InvalidSpec::memberExtensionSpec }\n",
    );
}

#[test]
fn ks_expressions_0276_class_literals_accept_type_with_value_receivers() {
    assert_source_parses(
        "fun classLiteralsSpec(valueSpec: Any) {\n    val typeLiteralSpec = String::class\n    val valueLiteralSpec = valueSpec::class\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0277: kmp-lsp does not reject parameterized class-literal types"]
fn ks_expressions_0277_parameterized_class_literal_must_omit_type_arguments() {
    assert_source_parses("val validSpec = List::class\n");
    assert_source_has_syntax_error("val invalidSpec = List<String>::class\n");
}

#[test]
fn ks_expressions_0281_type_class_literal_requires_non_nullable_runtime_available_type() {
    assert_source_parses("val validSpec = String::class\n");
    assert_source_has_syntax_error("val invalidSpec = String?::class\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0278: kmp-lsp does not infer class-literal KClass types"]
fn ks_expressions_0278_class_literal_has_kclass_type() {
    assert_source_parses("fun literalSpec() { val typeLiteralSpec = String::class }\n");
    let labels = inlay_hint_labels("fun literalSpec() { val typeLiteralSpec = String::class }\n");
    assert_eq!(labels, vec![": KClass<String>"]);
}

#[test]
fn ks_expressions_0285_call_access_expressions_accept_receiver_variants() {
    assert_source_parses(
        "class ReceiverSpec {\n    val propertySpec = 1\n    fun callSpec() {}\n    fun accessSpec() {\n        val localPropertySpec = 2\n        fun localCallSpec() {}\n        localCallSpec()\n        this.callSpec()\n        val firstSpec = localPropertySpec\n        val secondSpec = this.propertySpec\n    }\n}\n",
    );
}

#[test]
fn ks_expressions_0288_function_call_accepts_explicit_receiver_argument() {
    assert_source_parses(
        "class ReceiverSpec {\n    fun callSpec() {}\n}\nfun invokeSpec(receiverSpec: ReceiverSpec) { receiverSpec.callSpec() }\n",
    );
}

#[test]
fn ks_expressions_0289_function_call_accepts_normal_arguments() {
    assert_source_parses("fun callSpec(valueSpec: Int) {}\nfun invokeSpec() { callSpec(1) }\n");
}

#[test]
fn ks_expressions_0290_function_call_accepts_named_arguments() {
    assert_source_parses(
        "fun callSpec(valueSpec: Int) {}\nfun invokeSpec() { callSpec(valueSpec = 1) }\n",
    );
}

#[test]
fn ks_expressions_0291_function_call_accepts_vararg_arguments() {
    assert_source_parses(
        "fun callSpec(vararg valueSpec: Int) {}\nfun invokeSpec() { callSpec(1, 2, 3) }\n",
    );
}

#[test]
fn ks_expressions_0292_function_call_accepts_trailing_lambda_argument() {
    assert_source_parses(
        "fun callSpec(blockSpec: () -> Unit) {}\nfun invokeSpec() { callSpec { val valueSpec = 1 } }\n",
    );
}

#[test]
fn ks_expressions_0293_function_call_accepts_omitted_default_argument() {
    assert_source_parses("fun callSpec(valueSpec: Int = 1) {}\nfun invokeSpec() { callSpec() }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0303: kmp-lsp does not validate the vararg call context of spread expressions"]
fn ks_expressions_0303_spread_expression_requires_vararg_call_context() {
    assert_source_parses(
        "fun callSpec(vararg valueSpec: String) {}\nfun validSpec(valueSpec: Array<String>) { callSpec(*valueSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun callSpec(valueSpec: Array<String>) {}\nfun invalidSpec(valueSpec: Array<String>) { callSpec(*valueSpec) }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0304: kmp-lsp does not validate spread operand array types"]
fn ks_expressions_0304_spread_operand_requires_array_type() {
    assert_source_parses(
        "fun callSpec(vararg valueSpec: String) {}\nfun validSpec(valueSpec: Array<String>) { callSpec(*valueSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun callSpec(vararg valueSpec: String) {}\nfun invalidSpec(valueSpec: String) { callSpec(*valueSpec) }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0305: kmp-lsp does not restrict spread expressions to value arguments"]
fn ks_expressions_0305_spread_expression_requires_value_argument() {
    assert_source_parses(
        "fun callSpec(vararg valueSpec: String) {}\nfun validSpec(valueSpec: Array<String>) { callSpec(*valueSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(valueSpec: Array<String>) { val copiedSpec = *valueSpec }\n",
    );
}

#[test]
fn ks_expressions_0307_spread_arguments_mix_in_vararg_slot() {
    assert_source_parses(
        "fun consumeSpec(vararg valuesSpec: String) {}\nfun spreadSpec(valuesSpec: Array<String>) { consumeSpec(\"before\", *valuesSpec, \"after\") }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0309: kmp-lsp does not validate spread argument array subtypes"]
fn ks_expressions_0309_spread_argument_type_must_match_vararg_array_type() {
    assert_source_parses(
        "fun consumeSpec(vararg valuesSpec: String) {}\nfun validSpec(valuesSpec: Array<String>) { consumeSpec(*valuesSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun consumeSpec(vararg valuesSpec: String) {}\nfun invalidSpec(valuesSpec: IntArray) { consumeSpec(*valuesSpec) }\n",
    );
}

#[test]
fn ks_expressions_0310_named_function_reference_may_be_used_as_value() {
    assert_source_parses(
        "fun targetSpec(valueSpec: Int): Int = valueSpec\nval referenceSpec: (Int) -> Int = ::targetSpec\n",
    );
}

#[test]
fn ks_expressions_0311_function_literal_defines_function_in_place() {
    assert_source_parses("val functionSpec: (Int) -> Int = fun(valueSpec: Int): Int = valueSpec\n");
}

#[test]
fn ks_expressions_0312_function_literals_accept_both_declared_forms() {
    assert_source_parses(
        "val anonymousSpec: (Int) -> Int = fun(valueSpec: Int): Int = valueSpec\nval lambdaSpec: (Int) -> Int = { valueSpec -> valueSpec }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0313: tree-sitter-kotlin rejects suspend anonymous functions"]
fn ks_expressions_0313_anonymous_function_accepts_suspend_modifier() {
    assert_source_parses("val suspendSpec = suspend fun(valueSpec: Int): Int = valueSpec\n");
}

#[test]
fn ks_expressions_0315_anonymous_function_cannot_have_name() {
    assert_source_parses("val validSpec = fun(valueSpec: Int): Int = valueSpec\n");
    assert_source_has_syntax_error(
        "val invalidSpec = fun namedSpec(valueSpec: Int): Int = valueSpec\n",
    );
}

#[test]
fn ks_expressions_0316_anonymous_function_cannot_have_type_parameters() {
    assert_source_parses("val validSpec = fun(valueSpec: Int): Int = valueSpec\n");
    assert_source_has_syntax_error(
        "val invalidSpec = fun <ValueSpec>(valueSpec: ValueSpec): ValueSpec = valueSpec\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0317: kmp-lsp does not reject anonymous-function default parameters"]
fn ks_expressions_0317_anonymous_function_cannot_have_default_parameters() {
    assert_source_parses("val validSpec = fun(valueSpec: Int): Int = valueSpec\n");
    assert_source_has_syntax_error("val invalidSpec = fun(valueSpec: Int = 1): Int = valueSpec\n");
}

#[test]
fn ks_expressions_0318_anonymous_function_accepts_vararg_parameter() {
    assert_source_parses("val functionSpec = fun(vararg valueSpec: Int): Int = valueSpec.size\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0320: tree-sitter-kotlin rejects inferred anonymous-function parameter types"]
fn ks_expressions_0320_anonymous_function_may_omit_inferred_parameter_type() {
    assert_source_parses("val inferredSpec: (Int) -> Int = fun(valueSpec) = valueSpec\n");
}

#[test]
fn ks_expressions_0321_anonymous_function_may_omit_inferred_return_type() {
    assert_source_parses("val inferredSpec: (Int) -> Int = fun(valueSpec: Int) = valueSpec\n");
}

#[test]
fn ks_expressions_0322_anonymous_function_accepts_extension_receiver() {
    assert_source_parses("val extensionSpec = fun String.(): Int = length\n");
}

#[test]
fn ks_expressions_0323_anonymous_extension_rejects_parameterized_receiver() {
    assert_source_parses("val validSpec = fun String.(): Int = length\n");
    assert_source_has_syntax_error(
        "val invalidSpec = fun <ValueSpec> ValueSpec.(): ValueSpec = this\n",
    );
}

#[test]
fn ks_expressions_0325_lambda_literal_defines_unnamed_function() {
    assert_source_parses("val functionSpec: (Int) -> Int = { valueSpec -> valueSpec }\n");
}

#[test]
fn ks_expressions_0326_lambda_literal_accepts_parameter_list_variants() {
    assert_source_parses(
        "val explicitSpec: (Int) -> Int = { valueSpec -> valueSpec }\nval omittedSpec: (Int) -> Int = { it }\n",
    );
}

#[test]
fn ks_expressions_0327_lambda_body_accepts_statements_after_arrow() {
    assert_source_parses(
        "val functionSpec: (Int) -> Int = { valueSpec ->\n    val doubledSpec = valueSpec * 2\n    doubledSpec\n}\n",
    );
}

#[test]
fn ks_expressions_0329_lambda_literal_cannot_have_name() {
    assert_source_parses("val validSpec: (Int) -> Int = { valueSpec -> valueSpec }\n");
    assert_source_has_syntax_error(
        "val invalidSpec = { namedSpec(valueSpec: Int) -> valueSpec }\n",
    );
}

#[test]
fn ks_expressions_0330_lambda_literal_cannot_have_type_parameters() {
    assert_source_parses("val validSpec: (Int) -> Int = { valueSpec -> valueSpec }\n");
    assert_source_has_syntax_error(
        "val invalidSpec = { <ValueSpec> valueSpec: ValueSpec -> valueSpec }\n",
    );
}

#[test]
fn ks_expressions_0331_lambda_literal_cannot_have_default_parameters() {
    assert_source_parses("val validSpec: (Int) -> Int = { valueSpec -> valueSpec }\n");
    assert_source_has_syntax_error("val invalidSpec = { valueSpec: Int = 1 -> valueSpec }\n");
}

#[test]
fn ks_expressions_0332_lambda_literal_cannot_have_vararg_parameter() {
    assert_source_parses("val validSpec: (Int) -> Int = { valueSpec -> valueSpec }\n");
    assert_source_has_syntax_error(
        "val invalidSpec = { vararg valuesSpec: Int -> valuesSpec.size }\n",
    );
}

#[test]
fn ks_expressions_0333_lambda_literal_accepts_destructuring_parameter() {
    assert_source_parses(
        "val destructuredSpec: (Pair<Int, String>) -> String = { (numberSpec, textSpec) -> \"$numberSpec$textSpec\" }\n",
    );
}

#[test]
fn ks_expressions_0335_lambda_without_parameter_list_accepts_context_arities() {
    assert_source_parses(
        "val zeroSpec: () -> Int = { 1 }\nval oneSpec: (Int) -> Int = { it + 1 }\n",
    );
}

#[test]
fn ks_expressions_0338_lambda_parameter_list_forms_are_distinct() {
    assert_source_parses(
        "val omittedSpec: (Int) -> Int = { it }\nval explicitZeroSpec: () -> Int = { -> 1 }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0342: kmp-lsp does not diagnose non-local returns from non-inline lambdas"]
fn ks_expressions_0342_non_local_return_requires_inlined_lambda() {
    assert_source_parses(
        "inline fun validRunSpec(blockSpec: () -> Unit) = blockSpec()\nfun validSpec() { validRunSpec { return } }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidRunSpec(blockSpec: () -> Unit) = blockSpec()\nfun invalidSpec() { invalidRunSpec { return } }\n",
    );
}

#[test]
fn ks_expressions_0343_labeled_lambda_accepts_labeled_return() {
    assert_source_parses(
        "inline fun runSpec(blockSpec: () -> Unit) = blockSpec()\nfun labelsSpec() { runSpec explicitSpec@{ return@explicitSpec } }\n",
    );
}

#[test]
fn ks_expressions_0344_call_site_name_may_label_lambda_return() {
    assert_source_parses(
        "inline fun runSpec(blockSpec: () -> Unit) = blockSpec()\nfun labelsSpec() { runSpec { return@runSpec } }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0348: tree-sitter-kotlin rejects data object literals"]
fn ks_expressions_0348_object_literal_accepts_data_modifier() {
    assert_source_parses(
        "interface MarkerSpec\nval dataSpec = data object : MarkerSpec {\n    val valueSpec = 3\n}\n",
    );
}

#[test]
fn ks_expressions_0349_object_literals_accept_grammar_forms() {
    assert_source_parses(
        "open class BaseSpec\ninterface MarkerSpec\nfun objectsSpec() {\n    val plainSpec = object {\n        val valueSpec = 1\n    }\n    val inheritedSpec = object : BaseSpec(), MarkerSpec {\n        val valueSpec = 2\n    }\n}\n",
    );
}

#[test]
fn ks_expressions_0350_object_literal_cannot_have_name() {
    assert_source_parses("val validSpec = object {}\n");
    assert_source_has_syntax_error("val invalidSpec = object NamedSpec {}\n");
}

#[test]
fn ks_expressions_0351_object_literal_accepts_inner_class() {
    assert_source_parses("val validSpec = object {\n    inner class InnerSpec\n}\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0352: kmp-lsp does not reject nested classes in object literals"]
fn ks_expressions_0352_object_literal_forbids_nested_class() {
    assert_source_parses("val validSpec = object {\n    inner class InnerSpec\n}\n");
    assert_source_has_syntax_error("val invalidSpec = object {\n    class NestedSpec\n}\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0353: kmp-lsp does not reject nested interfaces in object literals"]
fn ks_expressions_0353_object_literal_forbids_nested_interface() {
    assert_source_parses("val validSpec = object {\n    inner class InnerSpec\n}\n");
    assert_source_has_syntax_error("val invalidSpec = object {\n    interface NestedSpec\n}\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0354: kmp-lsp does not reject nested objects in object literals"]
fn ks_expressions_0354_object_literal_forbids_nested_object() {
    assert_source_parses("val validSpec = object {\n    inner class InnerSpec\n}\n");
    assert_source_has_syntax_error("val invalidSpec = object {\n    object NestedSpec\n}\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0355: kmp-lsp does not validate object-literal base-class counts"]
fn ks_expressions_0355_object_literal_allows_at_most_one_base_class() {
    assert_source_parses(
        "open class FirstSpec\ninterface MarkerSpec\nval validSpec = object : FirstSpec(), MarkerSpec {}\n",
    );
    assert_source_has_syntax_error(
        "open class FirstSpec\nopen class SecondSpec\nval invalidSpec = object : FirstSpec(), SecondSpec() {}\n",
    );
}

#[test]
fn ks_expressions_0356_object_literal_accepts_base_interface_count() {
    assert_source_parses(
        "interface FirstSpec\ninterface SecondSpec\nval noneSpec = object {}\nval multipleSpec = object : FirstSpec, SecondSpec {}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0362: tree-sitter-kotlin rejects functional interface declarations"]
fn ks_expressions_0362_functional_interface_name_accepts_lambda_literal() {
    assert_source_parses(
        "fun interface RendererSpec { fun renderSpec(valueSpec: Int): String }\nval rendererSpec = RendererSpec { valueSpec -> valueSpec.toString() }\n",
    );
}

#[test]
fn ks_expressions_0367_unlabeled_this_expression_accepts_receiver_scope() {
    assert_source_parses("class ReceiverSpec {\n    fun valueSpec() = this\n}\n");
}

#[test]
fn ks_expressions_0370_classifier_labeled_this_accepts_declared_type() {
    assert_source_parses("class OuterSpec {\n    fun valueSpec() = this@OuterSpec\n}\n");
}

#[test]
fn ks_expressions_0372_extension_labeled_this_accepts_function_name() {
    assert_source_parses("fun String.extensionSpec(): String = this@extensionSpec\n");
}

#[test]
fn ks_expressions_0374_lambda_labeled_this_accepts_explicit_label() {
    assert_source_parses(
        "fun String.receiverSpec(blockSpec: String.() -> Unit) = blockSpec()\nfun valueSpec() { \"value\".receiverSpec explicitSpec@{ this@explicitSpec } }\n",
    );
}

#[test]
fn ks_expressions_0376_call_labeled_this_accepts_outer_function_name() {
    assert_source_parses(
        "fun String.receiverSpec(blockSpec: String.() -> Unit) = blockSpec()\nfun valueSpec() { \"value\".receiverSpec { this@receiverSpec } }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0378: kmp-lsp does not enforce explicit versus call-site this labels"]
fn ks_expressions_0378_explicit_lambda_label_disables_call_site_this_label() {
    assert_source_parses(
        "fun String.receiverSpec(blockSpec: String.() -> Unit) = blockSpec()\nfun validSpec() { \"value\".receiverSpec explicitSpec@{ this@explicitSpec } }\n",
    );
    assert_source_has_syntax_error(
        "fun String.receiverSpec(blockSpec: String.() -> Unit) = blockSpec()\nfun invalidSpec() { \"value\".receiverSpec explicitSpec@{ this@receiverSpec } }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0379: kmp-lsp does not restrict labeled this to extension-function lambdas"]
fn ks_expressions_0379_labeled_this_requires_extension_function_lambda() {
    assert_source_parses(
        "fun String.receiverSpec(blockSpec: String.() -> Unit) = blockSpec()\nfun validSpec() { \"value\".receiverSpec explicitSpec@{ this@explicitSpec } }\n",
    );
    assert_source_has_syntax_error(
        "fun normalSpec(blockSpec: () -> Unit) = blockSpec()\nfun invalidSpec() { normalSpec explicitSpec@{ this@explicitSpec } }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0380: kmp-lsp does not reject unknown this labels"]
fn ks_expressions_0380_this_expression_rejects_unknown_label() {
    assert_source_parses("class ValidSpec {\n    fun valueSpec() = this@ValidSpec\n}\n");
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun valueSpec() = this@MissingSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0382: kmp-lsp does not restrict super forms to receiver position"]
fn ks_expressions_0382_super_form_requires_call_or_property_receiver_position() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass ValidSpec : BaseSpec() {\n    override fun renderSpec() { super.renderSpec() }\n}\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec\nclass InvalidSpec : BaseSpec() {\n    fun valueSpec() { val copiedSpec = super }\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0384: kmp-lsp does not reject abstract super calls"]
fn ks_expressions_0384_super_form_cannot_access_unavailable_implementation() {
    assert_source_parses(
        "open class ConcreteSpec {\n    open fun renderSpec() {}\n}\nclass ValidSpec : ConcreteSpec() {\n    override fun renderSpec() { super.renderSpec() }\n}\n",
    );
    assert_source_has_syntax_error(
        "abstract class AbstractSpec {\n    abstract fun renderSpec()\n}\nclass InvalidSpec : AbstractSpec() {\n    override fun renderSpec() { super.renderSpec() }\n}\n",
    );
}

#[test]
fn ks_expressions_0385_basic_super_form_accepts_unqualified_receiver() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    override fun renderSpec() { super.renderSpec() }\n}\n",
    );
}

#[test]
fn ks_expressions_0387_extended_super_form_accepts_specific_supertype() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    override fun renderSpec() { super<BaseSpec>.renderSpec() }\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0388: kmp-lsp does not validate immediate supertype qualifiers"]
fn ks_expressions_0388_extended_super_form_requires_immediate_supertype() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass ValidSpec : BaseSpec() {\n    override fun renderSpec() { super<BaseSpec>.renderSpec() }\n}\n",
    );
    assert_source_has_syntax_error(
        "open class RootSpec {\n    open fun renderSpec() {}\n}\nopen class MiddleSpec : RootSpec()\nclass InvalidSpec : MiddleSpec() {\n    override fun renderSpec() { super<RootSpec>.renderSpec() }\n}\n",
    );
}

#[test]
fn ks_expressions_0390_outer_super_form_accepts_classifier_qualifier() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    inner class InnerSpec {\n        fun outerSpec() = super<BaseSpec>@DerivedSpec.renderSpec()\n    }\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0391: kmp-lsp does not validate outer super classifier labels"]
fn ks_expressions_0391_outer_super_form_requires_declared_classifier() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    inner class InnerSpec {\n        fun validSpec() = super<BaseSpec>@DerivedSpec.renderSpec()\n    }\n}\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    inner class InnerSpec {\n        fun invalidSpec() = super<BaseSpec>@MissingSpec.renderSpec()\n    }\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0392: kmp-lsp does not validate outer super immediate supertypes"]
fn ks_expressions_0392_outer_super_form_requires_immediate_supertype() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    inner class InnerSpec {\n        fun validSpec() = super<BaseSpec>@DerivedSpec.renderSpec()\n    }\n}\n",
    );
    assert_source_has_syntax_error(
        "open class RootSpec {\n    open fun renderSpec() {}\n}\nopen class MiddleSpec : RootSpec()\nclass DerivedSpec : MiddleSpec() {\n    inner class InnerSpec {\n        fun invalidSpec() = super<RootSpec>@DerivedSpec.renderSpec()\n    }\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0394: kmp-lsp does not restrict outer super forms to inner classes"]
fn ks_expressions_0394_outer_super_form_requires_inner_class() {
    assert_source_parses(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    inner class InnerSpec {\n        fun validSpec() = super<BaseSpec>@DerivedSpec.renderSpec()\n    }\n}\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec {\n    open fun renderSpec() {}\n}\nclass DerivedSpec : BaseSpec() {\n    class NestedSpec {\n        fun invalidSpec() = super<BaseSpec>@DerivedSpec.renderSpec()\n    }\n}\n",
    );
}

#[test]
fn ks_expressions_0395_jump_expression_grammar_accepts_declared_forms() {
    assert_source_parses(
        "fun jumpSpec(flagSpec: Boolean): Int {\n    loopSpec@ while (flagSpec) {\n        if (flagSpec) continue@loopSpec\n        break@loopSpec\n    }\n    if (flagSpec) throw IllegalStateException()\n    return 1\n}\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0397: kmp-lsp does not infer Nothing for jump expressions"]
fn ks_expressions_0397_jump_expression_has_nothing_type() {
    assert_source_parses("fun typeSpec() { val thrownSpec = throw IllegalStateException() }\n");
    let labels =
        inlay_hint_labels("fun typeSpec() { val thrownSpec = throw IllegalStateException() }\n");
    assert_eq!(labels, vec![": Nothing"]);
}

#[test]
fn ks_expressions_0399_throw_expression_accepts_operand_syntax() {
    assert_source_parses("fun throwSpec(errorSpec: Throwable): Nothing = throw errorSpec\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0401: kmp-lsp does not validate thrown exception types"]
fn ks_expressions_0401_throw_requires_exception_value() {
    assert_source_parses("fun validSpec(): Nothing = throw IllegalStateException()\n");
    assert_source_has_syntax_error("fun invalidSpec(): Nothing = throw \"not an exception\"\n");
}

#[test]
fn ks_expressions_0405_return_expression_accepts_omitted_value() {
    assert_source_parses("fun unitSpec() { return }\n");
}

#[test]
fn ks_expressions_0407_return_expression_accepts_simple_form() {
    assert_source_parses("fun returnSpec(): Int { return 1 }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0408: kmp-lsp does not reject return outside callable scopes"]
fn ks_expressions_0408_return_expression_requires_callable_target() {
    assert_source_parses("fun validSpec() { return }\n");
    assert_source_has_syntax_error("val invalidSpec = return\n");
}

#[test]
fn ks_expressions_0409_return_expression_accepts_labeled_form() {
    assert_source_parses("fun returnSpec(): Int { return@returnSpec 1 }\n");
}

#[test]
fn ks_expressions_0410_named_function_accepts_name_as_return_label() {
    assert_source_parses("fun returnSpec(): Int { return@returnSpec 1 }\n");
}

#[test]
fn ks_expressions_0412_call_site_name_may_label_return() {
    assert_source_parses(
        "inline fun runSpec(blockSpec: () -> Unit) = blockSpec()\nfun returnSpec() { runSpec { return@runSpec } }\n",
    );
}

#[test]
fn ks_expressions_0413_lambda_label_may_label_return() {
    assert_source_parses(
        "inline fun runSpec(blockSpec: () -> Unit) = blockSpec()\nfun returnSpec() { runSpec explicitSpec@{ return@explicitSpec } }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0414: kmp-lsp does not diagnose non-local returns from non-inline lambdas"]
fn ks_expressions_0414_non_local_return_requires_inlined_lambda() {
    assert_source_parses(
        "inline fun validRunSpec(blockSpec: () -> Unit) = blockSpec()\nfun validSpec() { validRunSpec { return } }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidRunSpec(blockSpec: () -> Unit) = blockSpec()\nfun invalidSpec() { invalidRunSpec { return } }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0416: kmp-lsp does not reject continue outside loops"]
fn ks_expressions_0416_continue_expression_requires_loop_body() {
    assert_source_parses("fun validSpec() { while (true) { continue } }\n");
    assert_source_has_syntax_error("fun invalidSpec() { continue }\n");
}

#[test]
fn ks_expressions_0418_continue_expression_accepts_simple_form() {
    assert_source_parses("fun continueSpec() { while (true) { continue } }\n");
}

#[test]
fn ks_expressions_0420_continue_expression_accepts_labeled_form() {
    assert_source_parses("fun continueSpec() { outerSpec@ while (true) { continue@outerSpec } }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0422: kmp-lsp does not reject continue across lambda boundaries"]
fn ks_expressions_0422_continue_cannot_cross_lambda_boundary() {
    assert_source_parses("fun validSpec() { while (true) { continue } }\n");
    assert_source_has_syntax_error(
        "fun invalidSpec() { while (true) { listOf(1).forEach { continue } } }\n",
    );
}

#[test]
#[ignore = "KS-EXPRESSIONS-0423: kmp-lsp does not reject break outside loops"]
fn ks_expressions_0423_break_expression_requires_loop_body() {
    assert_source_parses("fun validSpec() { while (true) { break } }\n");
    assert_source_has_syntax_error("fun invalidSpec() { break }\n");
}

#[test]
fn ks_expressions_0425_break_expression_accepts_simple_form() {
    assert_source_parses("fun breakSpec() { while (true) { break } }\n");
}

#[test]
fn ks_expressions_0427_break_expression_accepts_labeled_form() {
    assert_source_parses("fun breakSpec() { outerSpec@ while (true) { break@outerSpec } }\n");
}

#[test]
#[ignore = "KS-EXPRESSIONS-0429: kmp-lsp does not reject break across lambda boundaries"]
fn ks_expressions_0429_break_cannot_cross_lambda_boundary() {
    assert_source_parses("fun validSpec() { while (true) { break } }\n");
    assert_source_has_syntax_error(
        "fun invalidSpec() { while (true) { listOf(1).forEach { break } } }\n",
    );
}
