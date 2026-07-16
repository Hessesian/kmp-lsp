use std::sync::Arc;

use super::{assert_source_has_syntax_error, assert_source_parses};
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
