use super::{assert_source_contains_node_kind, assert_source_parses, count_nodes_of_kind};

#[test]
fn ks_1_3_065_statements_allow_separators_and_trailing_semis() {
    assert_source_contains_node_kind(
        "fun render() {\nval count = 1; println(count)\n;\n}\n",
        crate::queries::KIND_STATEMENTS,
    );
}

#[test]
fn ks_1_3_066_statement_accepts_labels_annotations_and_all_statement_families() {
    assert_source_parses(
        r#"
annotation class Marker
fun render(items: List<Int>) {
    @Marker val count = 1
    var result = 0
    result = count
    named@ for (item in items) result += item
    println(result)
}
"#,
    );
}

#[test]
fn ks_1_3_067_label_combines_identifier_at_token_and_newlines() {
    assert_source_parses(
        "fun render(items: List<Int>) {\nnamed@\nfor (item in items) { continue@named }\n}\n",
    );
}

#[test]
fn ks_1_3_068_control_structure_body_accepts_block_or_single_statement() {
    for source in [
        "fun blockBody(flag: Boolean) { if (flag) { println(flag) } }\n",
        "fun statementBody(flag: Boolean) { if (flag) println(flag) }\n",
    ] {
        assert_source_contains_node_kind(source, crate::queries::KIND_CONTROL_STRUCTURE_BODY);
    }
}

#[test]
fn ks_1_3_069_block_wraps_statements_in_braces() {
    let tree = super::parse_kotlin_source(
        "fun render(flag: Boolean) {\nif (flag) {\nval count = 1\nprintln(count)\n}\n}\n",
    );

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CONTROL_STRUCTURE_BODY) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_STATEMENTS) > 0);
}

#[test]
fn ks_1_3_070_loop_statement_accepts_for_while_and_do_while() {
    assert_source_parses(
        "fun render(items: List<Int>) {\nfor (item in items) println(item)\nwhile (false) println(0)\ndo println(1) while (false)\n}\n",
    );
}

#[test]
fn ks_1_3_071_for_statement_accepts_annotation_variable_destructuring_and_body() {
    assert_source_parses(
        "annotation class Marker\nfun render(items: List<Pair<Int, String>>) {\nfor (@Marker item in items) println(item)\nfor ((count, title) in items) println(title + count)\n}\n",
    );
}

#[test]
fn ks_1_3_072_while_statement_accepts_body_or_semicolon() {
    assert_source_parses("fun render() {\nwhile (false) { println(1) }\nwhile (false);\n}\n");
}

#[test]
fn ks_1_3_073_do_while_statement_accepts_optional_body() {
    assert_source_parses(
        "fun render() {\ndo { println(1) } while (false)\ndo println(2) while (false)\ndo while (false)\n}\n",
    );
}

#[test]
fn ks_1_3_074_assignment_accepts_simple_and_operator_forms() {
    assert_source_parses(
        "fun render() {\nvar count = 0\ncount = 1\ncount += 2\nval values = mutableListOf(0)\nvalues[0] = count\n}\n",
    );
}

#[test]
fn ks_1_3_075_semi_accepts_semicolon_or_newline_with_following_newlines() {
    assert_source_parses("val first = 1; val second = 2\n\nval third = 3\n");
}

#[test]
#[ignore = "KS-1.3-076: tree-sitter-kotlin rejects repeated semicolon and newline separators"]
fn ks_1_3_076_semis_accept_multiple_semicolons_and_newlines() {
    assert_source_parses("fun render() {\nval first = 1;;;\n\n;;val second = 2\n}\n");
}

#[test]
fn ks_1_3_077_expression_is_a_disjunction() {
    assert_source_contains_node_kind(
        "fun enabled(first: Boolean, second: Boolean) = first || second\n",
        crate::queries::KIND_DISJUNCTION_EXPR,
    );
}

#[test]
fn ks_1_3_078_disjunction_accepts_newlines_around_operators() {
    assert_source_contains_node_kind(
        "fun enabled(first: Boolean, second: Boolean) = first\n||\nsecond\n",
        crate::queries::KIND_DISJUNCTION_EXPR,
    );
}

#[test]
fn ks_1_3_079_conjunction_accepts_newlines_around_operators() {
    assert_source_contains_node_kind(
        "fun enabled(first: Boolean, second: Boolean) = first\n&&\nsecond\n",
        crate::queries::KIND_CONJUNCTION_EXPR,
    );
}

#[test]
fn ks_1_3_080_equality_accepts_chained_equality_operators() {
    assert_source_parses(
        "fun matches(first: Int, second: Int, third: Int) = first == second != third\n",
    );
}

#[test]
fn ks_1_3_081_comparison_accepts_chained_comparison_operators() {
    assert_source_contains_node_kind(
        "fun ordered(first: Int, second: Int, third: Int) = first < second >= third\n",
        crate::queries::KIND_COMPARISON_EXPR,
    );
}

#[test]
fn ks_1_3_082_generic_call_like_comparison_accepts_call_suffixes() {
    assert_source_contains_node_kind(
        "fun <Element> create(factory: () -> Element) = factory<Element>()\n",
        crate::queries::KIND_CALL_SUFFIX,
    );
}

#[test]
fn ks_1_3_083_infix_operation_accepts_membership_and_type_checks() {
    assert_source_parses(
        "fun inspect(item: Any, items: List<Any>) {\nval present = item in items\nval absent = item !in items\nval text = item is String\nval other = item !is String\n}\n",
    );
}

#[test]
fn ks_1_3_084_elvis_expression_accepts_newlines_around_elvis() {
    assert_source_parses(
        "fun choose(first: String?, second: String?, fallback: String) = first\n?:\nsecond\n?:\nfallback\n",
    );
}

#[test]
fn ks_1_3_085_elvis_token_requires_question_mark_without_whitespace_before_colon() {
    assert_source_parses("fun choose(value: String?, fallback: String) = value ?: fallback\n");
    super::assert_source_has_syntax_error(
        "fun choose(value: String?, fallback: String) = value ? : fallback\n",
    );
}

#[test]
fn ks_1_3_086_infix_function_call_accepts_identifier_and_newline() {
    assert_source_contains_node_kind(
        "infix fun String.merge(other: String) = this + other\nfun combine(first: String, second: String) = first merge\nsecond\n",
        crate::queries::KIND_INFIX_EXPR,
    );
}

#[test]
#[ignore = "KS-1.3-087: tree-sitter-kotlin rejects the open-ended range operator"]
fn ks_1_3_087_range_expression_accepts_closed_and_open_end_operators() {
    assert_source_parses(
        "fun ranges(start: Int, finish: Int) {\nval closed = start..finish\nval open = start..<finish\n}\n",
    );
}

#[test]
fn ks_1_3_088_additive_expression_accepts_plus_minus_and_newlines() {
    assert_source_parses(
        "fun adjust(first: Int, second: Int, third: Int) = first +\nsecond -\nthird\n",
    );
}

#[test]
fn ks_1_3_089_multiplicative_expression_accepts_all_operators() {
    assert_source_parses(
        "fun scale(first: Int, second: Int, third: Int, fourth: Int) = first * second / third % fourth\n",
    );
}

#[test]
fn ks_1_3_090_as_expression_accepts_unsafe_and_safe_casts() {
    assert_source_parses(
        "fun cast(value: Any) {\nval definite = value as String\nval optional = value as? String\n}\n",
    );
}

#[test]
fn ks_1_3_091_prefix_unary_expression_accepts_repeated_prefixes() {
    assert_source_contains_node_kind(
        "fun invert(flag: Boolean) = !!flag\nfun offset(count: Int) = -+count\n",
        crate::queries::KIND_PREFIX_EXPR,
    );
}

#[test]
fn ks_1_3_092_unary_prefix_accepts_annotation_label_and_operator() {
    assert_source_parses(
        "annotation class Marker\nfun inspect(value: Int, flag: Boolean) {\nval annotated = @Marker value\nval labeled = named@ value\nval inverted = !flag\n}\n",
    );
}

#[test]
fn ks_1_3_093_postfix_unary_expression_accepts_repeated_suffixes() {
    assert_source_parses(
        "fun inspect(values: List<String?>) = values[0]!!.length\nfun increment(count: Int) { var current = count; current++ }\n",
    );
}

#[test]
fn ks_1_3_094_postfix_unary_suffix_accepts_every_alternative() {
    assert_source_parses(
        "fun <Element> inspect(factory: () -> List<Element?>) = factory<Element>()[0]!!.hashCode()\n",
    );
}
