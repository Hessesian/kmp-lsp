use super::{assert_source_contains_node_kind, assert_source_parses};

#[test]
fn ks_1_3_111_string_literal_accepts_line_and_multiline_forms() {
    assert_source_parses("val line = \"status\"\nval multiline = \"\"\"status\"\"\"\n");
}

#[test]
fn ks_1_3_112_line_string_literal_accepts_content_and_expressions() {
    assert_source_parses(
        "fun render(name: String, count: Int) = \"Name: $name, count: ${count + 1}, newline: \\n\"\n",
    );
}

#[test]
fn ks_1_3_113_multiline_string_literal_accepts_content_expressions_and_quotes() {
    assert_source_parses(
        "fun render(name: String) = \"\"\"Name: $name; expression: ${name.length}; quote: \"\"\"\n",
    );
}

#[test]
fn ks_1_3_114_line_string_content_accepts_text_escape_and_reference() {
    assert_source_parses("fun render(name: String) = \"text \\t $name\"\n");
}

#[test]
fn ks_1_3_115_line_string_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun render(count: Int) = \"count=${\ncount + 1\n}\"\n");
}

#[test]
fn ks_1_3_116_multiline_string_content_accepts_text_quote_and_reference() {
    assert_source_parses("fun render(name: String) = \"\"\"text \" $name\"\"\"\n");
}

#[test]
fn ks_1_3_117_multiline_string_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun render(count: Int) = \"\"\"count=${\ncount + 1\n}\"\"\"\n");
}

#[test]
fn ks_1_3_118_lambda_literal_accepts_parameters_arrow_and_statements() {
    assert_source_contains_node_kind(
        "val transform: (Int) -> Int = { count ->\nval offset = 1\ncount + offset\n}\nval action = { println(\"done\") }\n",
        crate::queries::KIND_LAMBDA_LIT,
    );
}

#[test]
#[ignore = "KS-1.3-119: tree-sitter-kotlin rejects a trailing comma in lambda parameters"]
fn ks_1_3_119_lambda_parameters_accept_multiple_and_trailing_comma() {
    assert_source_parses("val combine = { first: Int, second: Int, -> first + second }\n");
}

#[test]
#[ignore = "KS-1.3-120: tree-sitter-kotlin rejects a typed destructuring lambda parameter"]
fn ks_1_3_120_lambda_parameter_accepts_variable_and_typed_destructuring() {
    assert_source_parses(
        "val single = { count: Int -> count }\nval pair = { (count, title): Pair<Int, String> -> title + count }\n",
    );
}

#[test]
#[ignore = "KS-1.3-121: tree-sitter-kotlin rejects a suspend anonymous receiver function with constraints"]
fn ks_1_3_121_anonymous_function_accepts_suspend_receiver_constraints_and_body() {
    assert_source_parses(
        "fun <Element> build() = suspend fun List<Element>.(value: Element): Element where Element : Any = value\n",
    );
}

#[test]
fn ks_1_3_122_function_literal_accepts_lambda_and_anonymous_function() {
    assert_source_parses(
        "val lambda = { count: Int -> count + 1 }\nval anonymous = fun(count: Int): Int { return count + 1 }\n",
    );
}

#[test]
#[ignore = "KS-1.3-123: tree-sitter-kotlin rejects data on an object literal"]
fn ks_1_3_123_object_literal_accepts_data_supertypes_and_body() {
    assert_source_parses(
        "interface Item { fun title(): String }\nval item = data object : Item { override fun title() = \"item\" }\n",
    );
}

#[test]
fn ks_1_3_124_this_expression_accepts_plain_and_labeled_forms() {
    assert_source_parses(
        "class Holder {\nfun inspect() {\nval plain = this\nwith(this) named@ { val labeled = this@named }\n}\n}\n",
    );
}

#[test]
fn ks_1_3_125_super_expression_accepts_type_and_label_qualifiers() {
    assert_source_parses(
        "interface Named {\nfun title(): String { return \"named\" }\n}\nopen class Base {\nopen fun title(): String { return \"base\" }\n}\nclass Child : Base(), Named {\noverride fun title(): String {\nval plain = super.title()\nreturn super<Base>.title() + super<Named>.title()\n}\n}\n",
    );
}

#[test]
fn ks_1_3_126_if_expression_accepts_body_else_and_empty_forms() {
    assert_source_parses(
        "fun inspect(flag: Boolean) {\nif (flag) println(1)\nif (flag) { println(2) } else println(3)\nif (flag);\n}\n",
    );
}

#[test]
fn ks_1_3_127_when_subject_accepts_expression_or_bound_variable() {
    assert_source_parses(
        "annotation class Marker\nfun inspect(value: Any) {\nwhen (value) { else -> println(value) }\nwhen (@Marker val subject = value) { else -> println(subject) }\n}\n",
    );
}
