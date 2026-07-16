use super::{
    assert_source_contains_node_kind, assert_source_has_syntax_error, assert_source_parses,
};

#[test]
fn ks_7_001_expressions_and_declarations_are_valid_statements() {
    assert_source_parses(
        "fun renderSpec(flagSpec: Boolean) {\n    val valueSpec: Int = 1\n    println(valueSpec)\n    if (flagSpec) valueSpec else 0\n}\n",
    );
}

#[test]
fn ks_7_1_001_assignment_accepts_mutable_identifier_navigation_and_indexing_lhs() {
    assert_source_parses(
        "class StateSpec { var valueSpec: Int = 0; }\nfun updateSpec(stateSpec: StateSpec, valuesSpec: IntArray) {\n    var localSpec: Int = 0\n    localSpec = 1\n    stateSpec.valueSpec = localSpec\n    valuesSpec[0] = stateSpec.valueSpec\n}\n",
    );
}

#[test]
fn ks_7_1_002_non_assignable_expression_cannot_be_assignment_lhs() {
    assert_source_parses("fun validSpec() { var valueSpec = 0; valueSpec = 1 }\n");
    assert_source_has_syntax_error("fun invalidSpec() { 1 + 2 = 3 }\n");
}

#[test]
#[ignore = "KS-7.1-003: kmp-lsp does not diagnose assignments to read-only properties"]
fn ks_7_1_003_read_only_property_cannot_be_assignment_lhs() {
    assert_source_parses("fun validSpec() { var valueSpec = 0; valueSpec = 1 }\n");
    assert_source_has_syntax_error("fun invalidSpec() { val valueSpec = 0; valueSpec = 1 }\n");
}

#[test]
fn ks_7_1_004_assignment_is_not_an_expression() {
    assert_source_parses("fun validSpec() { var valueSpec = 0; valueSpec = 1 }\n");
    assert_source_has_syntax_error(
        "fun invalidSpec(): Int { var valueSpec = 0; return (valueSpec = 1) }\n",
    );
}

#[test]
fn ks_7_1_2_001_operator_assignment_accepts_all_five_combined_forms() {
    assert_source_parses(
        "fun updateSpec() {\n    var valueSpec = 120\n    valueSpec += 2\n    valueSpec -= 3\n    valueSpec *= 4\n    valueSpec /= 5\n    valueSpec %= 6\n}\n",
    );
}

#[test]
fn ks_7_1_3_001_safe_navigation_may_appear_on_assignment_lhs() {
    assert_source_parses(
        "class StateSpec { var valueSpec: Int = 0; }\nfun updateSpec(stateSpec: StateSpec?) { stateSpec?.valueSpec = 1 }\n",
    );
}

#[test]
fn ks_7_2_001_loop_statement_has_for_while_and_do_while_forms() {
    assert_source_parses(
        "fun iterateSpec(valuesSpec: List<Int>) {\n    for (valueSpec in valuesSpec) println(valueSpec)\n    while (false) println(0)\n    do println(1) while (false)\n}\n",
    );
}

#[test]
#[ignore = "KS-7.2-002: kmp-lsp does not diagnose break or continue outside loops"]
fn ks_7_2_002_break_and_continue_are_allowed_only_in_loop_bodies() {
    assert_source_parses("fun validSpec() { while (true) { if (false) continue; break } }\n");
    assert_source_has_syntax_error("fun invalidBreakSpec() { break }\n");
    assert_source_has_syntax_error("fun invalidContinueSpec() { continue }\n");
}

#[test]
fn ks_7_2_1_001_while_loop_accepts_body_or_empty_semicolon_body() {
    assert_source_parses(
        "fun iterateSpec() {\n    while (false) { println(1) }\n    while (false);\n}\n",
    );
}

#[test]
#[ignore = "KS-7.2.1-002: kmp-lsp does not type-check while conditions"]
fn ks_7_2_1_002_while_loop_condition_must_be_boolean() {
    assert_source_parses("fun validSpec() { while (false); }\n");
    assert_source_has_syntax_error("fun invalidSpec() { while (1); }\n");
}

#[test]
fn ks_7_2_2_001_do_while_loop_accepts_block_single_or_missing_body() {
    assert_source_parses(
        "fun iterateSpec() {\n    do { println(1) } while (false)\n    do println(2) while (false)\n    do while (false)\n}\n",
    );
}

#[test]
#[ignore = "KS-7.2.2-002: kmp-lsp does not type-check do-while conditions"]
fn ks_7_2_2_002_do_while_loop_condition_must_be_boolean() {
    assert_source_parses("fun validSpec() { do while (false) }\n");
    assert_source_has_syntax_error("fun invalidSpec() { do while (1) }\n");
}

#[test]
fn ks_7_2_3_001_for_loop_has_only_foreach_form() {
    assert_source_parses(
        "fun validSpec(valuesSpec: List<Int>) { for (valueSpec in valuesSpec) println(valueSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec() { for (valueSpec = 0; valueSpec < 3; valueSpec++) println(valueSpec) }\n",
    );
}

#[test]
fn ks_7_2_3_002_for_loop_accepts_annotated_variable_or_destructuring_declaration() {
    assert_source_parses(
        "annotation class MarkerSpec\nfun iterateSpec(valuesSpec: List<Pair<Int, String>>) {\n    for (@MarkerSpec valueSpec in valuesSpec) println(valueSpec)\n    for ((countSpec, textSpec) in valuesSpec) println(textSpec + countSpec)\n}\n",
    );
}

#[test]
fn ks_7_3_001_code_block_accepts_empty_newline_and_semicolon_separated_statements() {
    assert_source_parses(
        "fun emptySpec() {}\nfun populatedSpec() {\n    val firstSpec = 1; val secondSpec = 2\n    println(firstSpec + secondSpec);\n}\n",
    );
}

#[test]
fn ks_7_3_002_bare_braces_in_statement_position_are_lambda_literal() {
    assert_source_contains_node_kind(
        "fun buildSpec() { { println(\"lambda\") } }\n",
        crate::queries::KIND_LAMBDA_LIT,
    );
}

#[test]
fn ks_7_3_003_control_structure_body_accepts_block_or_single_statement() {
    assert_source_parses(
        "fun renderSpec(flagSpec: Boolean) {\n    if (flagSpec) { println(1) }\n    if (!flagSpec) println(2)\n}\n",
    );
}
