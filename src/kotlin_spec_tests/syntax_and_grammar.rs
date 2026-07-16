use super::{
    assert_source_contains_node_kind, assert_source_has_syntax_error, assert_source_parses,
    count_nodes_of_kind, parse_kotlin_source,
};

#[test]
fn ks_1_2_1_line_feed_terminates_line_comment() {
    let source = "// first line\nval visible = 1\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        1
    );
}

#[test]
fn ks_1_2_1_crlf_terminates_line_comment() {
    let source = "// first line\r\nval visible = 1\r\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        1
    );
}

#[test]
fn ks_1_2_1_delimited_comments_may_be_nested() {
    assert_source_parses("/* outer /* nested */ outer */\nval visible = 1\n");
}

#[test]
fn ks_1_2_1_spaces_tabs_and_form_feed_are_whitespace() {
    assert_source_parses("val\tanswer\u{000c}=\t42\n");
}

#[test]
fn ks_1_2_1_shebang_extends_to_the_line_terminator() {
    let source = "#!/usr/bin/env kotlin\nval visible = 1\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        1
    );
}

#[test]
#[ignore = "KS-1.2.2-01: tree-sitter-kotlin accepts the unescaped hard keyword `if` as a simple identifier"]
fn ks_1_2_2_hard_keywords_require_escaping_as_identifiers() {
    assert_source_has_syntax_error("val if = 1\n");
    assert_source_parses("val `if` = 1\n");
}

#[test]
#[ignore = "KS-1.2.2-02: tree-sitter-kotlin rejects the specification-listed soft keyword `dynamic` as a property name"]
fn ks_1_2_2_soft_keywords_may_be_unescaped_identifiers() {
    let soft_keywords = [
        "abstract",
        "annotation",
        "by",
        "catch",
        "companion",
        "constructor",
        "crossinline",
        "data",
        "dynamic",
        "enum",
        "external",
        "final",
        "finally",
        "import",
        "infix",
        "init",
        "inline",
        "inner",
        "internal",
        "lateinit",
        "noinline",
        "open",
        "operator",
        "out",
        "override",
        "private",
        "protected",
        "public",
        "reified",
        "sealed",
        "tailrec",
        "vararg",
        "where",
        "get",
        "set",
        "field",
        "property",
        "receiver",
        "param",
        "setparam",
        "delegate",
        "file",
        "expect",
        "actual",
        "const",
        "suspend",
    ];

    for soft_keyword in soft_keywords {
        let source = format!("val {soft_keyword} = 1\n");
        let tree = parse_kotlin_source(&source);
        assert!(
            !tree.root_node().has_error(),
            "soft keyword {soft_keyword} should parse as an identifier, got: {}",
            tree.root_node().to_sexp()
        );
    }
}

#[test]
fn ks_1_2_2_operator_lexemes_form_valid_expressions() {
    assert_source_parses(
        r#"
class Box
fun operators(left: Int, right: Int, value: Any?, values: List<Int>) {
    var result = left + right - left * right / 2 % 2
    result++
    result--
    result += 1
    result -= 1
    result *= 2
    result /= 2
    result %= 2
    val logic = left <= right && left != right || left >= right
    val identity = value === null || value !== null
    val range = left..right
    val membership = left in values && right !in values
    val typeChecks = value is Box || value !is Box
    val safeCast = value as? Box
    val callable = Box::class
}
"#,
    );
}

#[test]
fn ks_1_2_3_decimal_integer_literals_allow_internal_separators() {
    assert_source_parses("val count = 1_000_000\n");
    assert_source_has_syntax_error("val leading = _100\nval trailing = 100_\n");
}

#[test]
fn ks_1_2_3_real_literals_support_fraction_exponent_and_float_suffix() {
    for literal in ["0.5", "1e9", "1E-9", "2.5e+3", "3.0f", "4F"] {
        assert_source_parses(&format!("val measurement = {literal}\n"));
    }
}

#[test]
fn ks_1_2_3_hexadecimal_literals_require_hexadecimal_digits() {
    assert_source_parses("val mask = 0xCA_FE\nval upper = 0X10\n");
    assert_source_has_syntax_error("val missing = 0x\n");
}

#[test]
#[ignore = "KS-1.2.3-04: tree-sitter-kotlin rejects binary literals with separators or an uppercase B prefix"]
fn ks_1_2_3_binary_literals_require_binary_digits() {
    assert_source_parses("val flags = 0b1010_0011\nval upper = 0B10\n");
    assert_source_has_syntax_error("val invalid = 0b102\n");
}

#[test]
#[ignore = "KS-1.2.3-05: tree-sitter-kotlin misparses the valid binary unsigned literal 0b10U"]
fn ks_1_2_3_unsigned_literals_accept_unsigned_and_unsigned_long_suffixes() {
    assert_source_parses("val count = 42u\nval large = 0xFFUL\nval flags = 0b10U\n");
}

#[test]
#[ignore = "KS-1.2.3-06: tree-sitter-kotlin misparses the valid binary long literal 0b10L"]
fn ks_1_2_3_long_literals_accept_uppercase_long_suffix() {
    assert_source_parses("val decimal = 42L\nval hex = 0xFFL\nval binary = 0b10L\n");
    assert_source_has_syntax_error("val lowercase = 42l\n");
}

#[test]
fn ks_1_2_3_boolean_literals_are_distinct_tokens() {
    assert_source_contains_node_kind(
        "val enabled = true\nval disabled = false\n",
        crate::queries::KIND_BOOLEAN_LITERAL,
    );
}

#[test]
fn ks_1_2_3_null_literal_is_a_distinct_token() {
    assert_source_contains_node_kind("val absent = null\n", crate::queries::KIND_NULL_LITERAL);
}

#[test]
fn ks_1_2_3_character_literals_accept_escape_and_unicode_sequences() {
    assert_source_parses("val tab = '\\t'\nval letter = 'K'\nval unicode = '\\u004b'\n");
    assert_source_has_syntax_error("val tooMany = 'KT'\n");
}

#[test]
fn ks_1_2_4_identifiers_accept_unicode_letters_underscores_and_digits() {
    assert_source_parses("val _count2 = 2\nval Δelta3 = 3\nval данные4 = 4\n");
    assert_source_has_syntax_error("val 2count = 2\n");
}

#[test]
fn ks_1_2_4_backticks_escape_keywords_and_non_alphanumeric_names() {
    assert_source_parses("val `when` = 1\nfun `render-screen`() = `when`\n");
}

#[test]
fn ks_1_2_5_line_strings_support_references_expressions_and_escapes() {
    assert_source_parses(
        r#"val name = "sample"
val message = "Hello, $name: ${name.length}\n"
"#,
    );
}

#[test]
fn ks_1_2_5_multiline_strings_treat_backslashes_as_text() {
    assert_source_parses(
        r##"val name = "sample"
val message = """path\segment
Hello, $name: ${name.length}"""
"##,
    );
}

#[test]
fn ks_1_2_6_comments_and_whitespace_do_not_change_syntax_parsing() {
    let compact = parse_kotlin_source("val answer=42\n");
    let separated = parse_kotlin_source("/* lead */ val\tanswer /* type */ = // value\n42\n");

    assert!(!compact.root_node().has_error());
    assert!(!separated.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&compact, crate::queries::KIND_PROP_DECL),
        count_nodes_of_kind(&separated, crate::queries::KIND_PROP_DECL)
    );
}
