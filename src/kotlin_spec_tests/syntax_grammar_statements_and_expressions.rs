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

#[test]
fn ks_1_3_095_directly_assignable_expression_accepts_all_alternatives() {
    assert_source_parses(
        "fun update(values: MutableList<Int>) {\nvar count = 0\ncount = 1\nvalues[0] = count\n}\n",
    );
}

#[test]
#[ignore = "KS-1.3-096: tree-sitter-kotlin rejects parenthesized assignment targets"]
fn ks_1_3_096_parenthesized_directly_assignable_expression_allows_newlines() {
    assert_source_parses("fun update() {\nvar count = 0\n(\ncount\n) = 1\n}\n");
}

#[test]
fn ks_1_3_097_assignable_expression_accepts_prefix_or_parenthesized_forms() {
    assert_source_parses("fun update() {\nvar count = 0\n++count\n(count)++\n}\n");
}

#[test]
fn ks_1_3_098_parenthesized_assignable_expression_allows_newlines() {
    assert_source_parses("fun update() {\nvar count = 0\n(\ncount\n)++\n}\n");
}

#[test]
#[ignore = "KS-1.3-099: tree-sitter-kotlin rejects type arguments as an assignable suffix"]
fn ks_1_3_099_assignable_suffix_accepts_type_indexing_and_navigation_suffixes() {
    assert_source_parses(
        "class Holder(var count: Int)\nfun update(values: MutableList<Int>, holder: Holder) {\nvalues<Int>[0] = 1\nholder.count = 2\n}\n",
    );
}

#[test]
#[ignore = "KS-1.3-100: tree-sitter-kotlin rejects a trailing comma in an indexing suffix"]
fn ks_1_3_100_indexing_suffix_accepts_multiple_expressions_and_trailing_comma() {
    assert_source_parses(
        "class Grid { operator fun set(row: Int, column: Int, value: Int) {} }\nfun update(grid: Grid) { grid[0, 1,] = 2 }\n",
    );
}

#[test]
fn ks_1_3_101_navigation_suffix_accepts_member_safe_and_class_access() {
    assert_source_parses(
        "class Holder(val count: Int)\nfun inspect(holder: Holder?) {\nval direct = holder?.count\nval type = Holder::class\n}\n",
    );
}

#[test]
fn ks_1_3_102_call_suffix_accepts_arguments_type_arguments_and_lambda() {
    assert_source_parses(
        "fun <Element> consume(value: Element, block: () -> Unit) {}\nfun inspect() { consume<String>(\"item\") { println(\"done\") } }\n",
    );
}

#[test]
fn ks_1_3_103_annotated_lambda_accepts_annotations_label_and_newline() {
    assert_source_parses(
        "annotation class Marker\nfun consume(block: () -> Unit) {}\nfun inspect() { consume @Marker named@\n{ println(\"done\") } }\n",
    );
}

#[test]
#[ignore = "KS-1.3-104: tree-sitter-kotlin rejects a trailing comma in expression type arguments"]
fn ks_1_3_104_type_arguments_accept_projections_newlines_and_trailing_comma() {
    assert_source_parses(
        "fun <Element> create(): Element = TODO()\nfun inspect() = create<\nout String,\n>()\n",
    );
}

#[test]
fn ks_1_3_105_value_arguments_accept_empty_multiple_and_trailing_comma() {
    assert_source_parses(
        "fun consume(first: Int = 0, second: Int = 0) {}\nfun inspect() { consume(); consume(1, 2,) }\n",
    );
}

#[test]
fn ks_1_3_106_value_argument_accepts_annotation_name_and_spread() {
    assert_source_parses(
        "annotation class Marker\nfun consume(vararg values: Int) {}\nfun inspect(values: IntArray) { consume(@Marker values = *values) }\n",
    );
}

#[test]
fn ks_1_3_107_primary_expression_accepts_each_expression_family() {
    assert_source_parses(
        "class Item\nfun inspect() {\nval parenthesized = (1)\nval identifier = parenthesized\nval literal = 2\nval text = \"item\"\nval reference = ::Item\nval lambda = { 3 }\nval objectValue = object {}\nval collection = [1, 2]\nval current = this\nval parent = super.toString()\n}\n",
    );
}

#[test]
fn ks_1_3_108_parenthesized_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun calculate(first: Int, second: Int) = (\nfirst + second\n)\n");
}

#[test]
#[ignore = "KS-1.3-109: tree-sitter-kotlin rejects a trailing comma in a collection literal"]
fn ks_1_3_109_collection_literal_accepts_expressions_and_trailing_comma() {
    assert_source_parses(
        "annotation class Numbers(val values: IntArray)\n@Numbers([1, 2,]) class Sample\n",
    );
}

#[test]
#[ignore = "KS-1.3-110: tree-sitter-kotlin rejects a valid binary literal alternative"]
fn ks_1_3_110_literal_constant_accepts_all_literal_families() {
    assert_source_parses(
        "fun literals() {\nval boolean = true\nval integer = 42\nval hexadecimal = 0x2A\nval binary = 0b101010\nval character = 'x'\nval real = 4.2\nval nullValue = null\nval longValue = 42L\nval unsignedValue = 42U\n}\n",
    );
}
