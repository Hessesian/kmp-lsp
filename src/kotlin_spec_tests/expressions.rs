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
