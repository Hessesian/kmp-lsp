use super::{assert_source_contains_node_kind, assert_source_parses, count_nodes_of_kind};

#[test]
fn ks_syntax_0251_statements_allow_separators_with_trailing_semis() {
    assert_source_contains_node_kind(
        "fun render() {\nval count = 1; println(count)\n;\n}\n",
        crate::queries::KIND_STATEMENTS,
    );
}

#[test]
fn ks_syntax_0252_statement_accepts_labels_annotations_with_all_statement_families() {
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
fn ks_syntax_0253_label_combines_identifier_at_token_with_newlines() {
    assert_source_parses(
        "fun render(items: List<Int>) {\nnamed@\nfor (item in items) { continue@named }\n}\n",
    );
}

#[test]
fn ks_syntax_0254_control_structure_body_accepts_block_or_single_statement() {
    for source in [
        "fun blockBody(flag: Boolean) { if (flag) { println(flag) } }\n",
        "fun statementBody(flag: Boolean) { if (flag) println(flag) }\n",
    ] {
        assert_source_contains_node_kind(source, crate::queries::KIND_CONTROL_STRUCTURE_BODY);
    }
}

#[test]
fn ks_syntax_0255_block_wraps_statements_in_braces() {
    let tree = super::parse_kotlin_source(
        "fun render(flag: Boolean) {\nif (flag) {\nval count = 1\nprintln(count)\n}\n}\n",
    );

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CONTROL_STRUCTURE_BODY) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_STATEMENTS) > 0);
}

#[test]
fn ks_syntax_0256_loop_statement_accepts_for_while_with_do_while() {
    assert_source_parses(
        "fun render(items: List<Int>) {\nfor (item in items) println(item)\nwhile (false) println(0)\ndo println(1) while (false)\n}\n",
    );
}

#[test]
fn ks_syntax_0257_for_statement_accepts_annotation_variable_destructuring_with_body() {
    assert_source_parses(
        "annotation class Marker\nfun render(items: List<Pair<Int, String>>) {\nfor (@Marker item in items) println(item)\nfor ((count, title) in items) println(title + count)\n}\n",
    );
}

#[test]
fn ks_syntax_0258_while_statement_accepts_body_or_semicolon() {
    assert_source_parses("fun render() {\nwhile (false) { println(1) }\nwhile (false);\n}\n");
}

#[test]
fn ks_syntax_0259_do_while_statement_accepts_optional_body() {
    assert_source_parses(
        "fun render() {\ndo { println(1) } while (false)\ndo println(2) while (false)\ndo while (false)\n}\n",
    );
}

#[test]
fn ks_syntax_0260_assignment_accepts_simple_with_operator_forms() {
    assert_source_parses(
        "fun render() {\nvar count = 0\ncount = 1\ncount += 2\nval values = mutableListOf(0)\nvalues[0] = count\n}\n",
    );
}

#[test]
fn ks_syntax_0261_semi_accepts_semicolon_or_newline_with_following_newlines() {
    assert_source_parses("val first = 1; val second = 2\n\nval third = 3\n");
}

#[test]
#[ignore = "KS-SYNTAX-0262: tree-sitter-kotlin rejects repeated semicolon and newline separators"]
fn ks_syntax_0262_semis_accept_multiple_semicolons_with_newlines() {
    assert_source_parses("fun render() {\nval first = 1;;;\n\n;;val second = 2\n}\n");
}

#[test]
fn ks_syntax_0263_expression_is_a_disjunction() {
    assert_source_contains_node_kind(
        "fun enabled(first: Boolean, second: Boolean) = first || second\n",
        crate::queries::KIND_DISJUNCTION_EXPR,
    );
}

#[test]
fn ks_syntax_0264_disjunction_accepts_newlines_around_operators() {
    assert_source_contains_node_kind(
        "fun enabled(first: Boolean, second: Boolean) = first\n||\nsecond\n",
        crate::queries::KIND_DISJUNCTION_EXPR,
    );
}

#[test]
fn ks_syntax_0265_conjunction_accepts_newlines_around_operators() {
    assert_source_contains_node_kind(
        "fun enabled(first: Boolean, second: Boolean) = first\n&&\nsecond\n",
        crate::queries::KIND_CONJUNCTION_EXPR,
    );
}

#[test]
fn ks_syntax_0266_equality_accepts_chained_equality_operators() {
    assert_source_parses(
        "fun matches(first: Int, second: Int, third: Int) = first == second != third\n",
    );
}

#[test]
fn ks_syntax_0267_comparison_accepts_chained_comparison_operators() {
    assert_source_contains_node_kind(
        "fun ordered(first: Int, second: Int, third: Int) = first < second >= third\n",
        crate::queries::KIND_COMPARISON_EXPR,
    );
}

#[test]
fn ks_syntax_0268_generic_call_like_comparison_accepts_call_suffixes() {
    assert_source_contains_node_kind(
        "fun <Element> create(factory: () -> Element) = factory<Element>()\n",
        crate::queries::KIND_CALL_SUFFIX,
    );
}

#[test]
fn ks_syntax_0269_infix_operation_accepts_membership_with_type_checks() {
    assert_source_parses(
        "fun inspect(item: Any, items: List<Any>) {\nval present = item in items\nval absent = item !in items\nval text = item is String\nval other = item !is String\n}\n",
    );
}

#[test]
fn ks_syntax_0270_elvis_expression_accepts_newlines_around_elvis() {
    assert_source_parses(
        "fun choose(first: String?, second: String?, fallback: String) = first\n?:\nsecond\n?:\nfallback\n",
    );
}

#[test]
fn ks_syntax_0271_elvis_token_requires_question_mark_without_whitespace_before_colon() {
    assert_source_parses("fun choose(value: String?, fallback: String) = value ?: fallback\n");
    super::assert_source_has_syntax_error(
        "fun choose(value: String?, fallback: String) = value ? : fallback\n",
    );
}

#[test]
fn ks_syntax_0272_infix_function_call_accepts_identifier_with_newline() {
    assert_source_contains_node_kind(
        "infix fun String.merge(other: String) = this + other\nfun combine(first: String, second: String) = first merge\nsecond\n",
        crate::queries::KIND_INFIX_EXPR,
    );
}

#[test]
#[ignore = "KS-SYNTAX-0273: tree-sitter-kotlin rejects the open-ended range operator"]
fn ks_syntax_0273_range_expression_accepts_closed_with_open_end_operators() {
    assert_source_parses(
        "fun ranges(start: Int, finish: Int) {\nval closed = start..finish\nval open = start..<finish\n}\n",
    );
}

#[test]
fn ks_syntax_0274_additive_expression_accepts_plus_minus_with_newlines() {
    assert_source_parses(
        "fun adjust(first: Int, second: Int, third: Int) = first +\nsecond -\nthird\n",
    );
}

#[test]
fn ks_syntax_0275_multiplicative_expression_accepts_all_operators() {
    assert_source_parses(
        "fun scale(first: Int, second: Int, third: Int, fourth: Int) = first * second / third % fourth\n",
    );
}

#[test]
fn ks_syntax_0276_as_expression_accepts_unsafe_with_safe_casts() {
    assert_source_parses(
        "fun cast(value: Any) {\nval definite = value as String\nval optional = value as? String\n}\n",
    );
}

#[test]
fn ks_syntax_0277_prefix_unary_expression_accepts_repeated_prefixes() {
    assert_source_contains_node_kind(
        "fun invert(flag: Boolean) = !!flag\nfun offset(count: Int) = -+count\n",
        crate::queries::KIND_PREFIX_EXPR,
    );
}

#[test]
fn ks_syntax_0278_unary_prefix_accepts_annotation_label_with_operator() {
    assert_source_parses(
        "annotation class Marker\nfun inspect(value: Int, flag: Boolean) {\nval annotated = @Marker value\nval labeled = named@ value\nval inverted = !flag\n}\n",
    );
}

#[test]
fn ks_syntax_0279_postfix_unary_expression_accepts_repeated_suffixes() {
    assert_source_parses(
        "fun inspect(values: List<String?>) = values[0]!!.length\nfun increment(count: Int) { var current = count; current++ }\n",
    );
}

#[test]
fn ks_syntax_0280_postfix_unary_suffix_accepts_every_alternative() {
    assert_source_parses(
        "fun <Element> inspect(factory: () -> List<Element?>) = factory<Element>()[0]!!.hashCode()\n",
    );
}

#[test]
fn ks_syntax_0281_directly_assignable_expression_accepts_all_alternatives() {
    assert_source_parses(
        "fun update(values: MutableList<Int>) {\nvar count = 0\ncount = 1\nvalues[0] = count\n}\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0282: tree-sitter-kotlin rejects parenthesized assignment targets"]
fn ks_syntax_0282_parenthesized_directly_assignable_expression_allows_newlines() {
    assert_source_parses("fun update() {\nvar count = 0\n(\ncount\n) = 1\n}\n");
}

#[test]
fn ks_syntax_0283_assignable_expression_accepts_prefix_or_parenthesized_forms() {
    assert_source_parses("fun update() {\nvar count = 0\n++count\n(count)++\n}\n");
}

#[test]
fn ks_syntax_0284_parenthesized_assignable_expression_allows_newlines() {
    assert_source_parses("fun update() {\nvar count = 0\n(\ncount\n)++\n}\n");
}

#[test]
#[ignore = "KS-SYNTAX-0285: tree-sitter-kotlin rejects type arguments as an assignable suffix"]
fn ks_syntax_0285_assignable_suffix_accepts_type_indexing_with_navigation_suffixes() {
    assert_source_parses(
        "class Holder(var count: Int)\nfun update(values: MutableList<Int>, holder: Holder) {\nvalues<Int>[0] = 1\nholder.count = 2\n}\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0286: tree-sitter-kotlin rejects a trailing comma in an indexing suffix"]
fn ks_syntax_0286_indexing_suffix_accepts_multiple_expressions_with_trailing_comma() {
    assert_source_parses(
        "class Grid { operator fun set(row: Int, column: Int, value: Int) {} }\nfun update(grid: Grid) { grid[0, 1,] = 2 }\n",
    );
}

#[test]
fn ks_syntax_0287_navigation_suffix_accepts_member_safe_with_class_access() {
    assert_source_parses(
        "class Holder(val count: Int)\nfun inspect(holder: Holder?) {\nval direct = holder?.count\nval type = Holder::class\n}\n",
    );
}

#[test]
fn ks_syntax_0288_call_suffix_accepts_arguments_type_arguments_with_lambda() {
    assert_source_parses(
        "fun <Element> consume(value: Element, block: () -> Unit) {}\nfun inspect() { consume<String>(\"item\") { println(\"done\") } }\n",
    );
}

#[test]
fn ks_syntax_0289_annotated_lambda_accepts_annotations_label_with_newline() {
    assert_source_parses(
        "annotation class Marker\nfun consume(block: () -> Unit) {}\nfun inspect() { consume @Marker named@\n{ println(\"done\") } }\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0290: tree-sitter-kotlin rejects a trailing comma in expression type arguments"]
fn ks_syntax_0290_type_arguments_accept_projections_newlines_with_trailing_comma() {
    assert_source_parses(
        "fun <Element> create(): Element = TODO()\nfun inspect() = create<\nout String,\n>()\n",
    );
}

#[test]
fn ks_syntax_0291_value_arguments_accept_empty_multiple_with_trailing_comma() {
    assert_source_parses(
        "fun consume(first: Int = 0, second: Int = 0) {}\nfun inspect() { consume(); consume(1, 2,) }\n",
    );
}

#[test]
fn ks_syntax_0292_value_argument_accepts_annotation_name_with_spread() {
    assert_source_parses(
        "annotation class Marker\nfun consume(vararg values: Int) {}\nfun inspect(values: IntArray) { consume(@Marker values = *values) }\n",
    );
}

#[test]
fn ks_syntax_0293_primary_expression_accepts_each_expression_family() {
    assert_source_parses(
        "class Item\nfun inspect() {\nval parenthesized = (1)\nval identifier = parenthesized\nval literal = 2\nval text = \"item\"\nval reference = ::Item\nval lambda = { 3 }\nval objectValue = object {}\nval collection = [1, 2]\nval current = this\nval parent = super.toString()\n}\n",
    );
}

#[test]
fn ks_syntax_0294_parenthesized_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun calculate(first: Int, second: Int) = (\nfirst + second\n)\n");
}

#[test]
#[ignore = "KS-SYNTAX-0295: tree-sitter-kotlin rejects a trailing comma in a collection literal"]
fn ks_syntax_0295_collection_literal_accepts_expressions_with_trailing_comma() {
    assert_source_parses(
        "annotation class Numbers(val values: IntArray)\n@Numbers([1, 2,]) class Sample\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0296: tree-sitter-kotlin rejects a valid binary literal alternative"]
fn ks_syntax_0296_literal_constant_accepts_all_literal_families() {
    assert_source_parses(
        "fun literals() {\nval boolean = true\nval integer = 42\nval hexadecimal = 0x2A\nval binary = 0b101010\nval character = 'x'\nval real = 4.2\nval nullValue = null\nval longValue = 42L\nval unsignedValue = 42U\n}\n",
    );
}
