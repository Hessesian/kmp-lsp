use super::{
    assert_source_contains_node_kind, assert_source_has_syntax_error, assert_source_parses,
};

#[test]
fn ks_statements_0001_expressions_and_declarations_are_valid_statements() {
    assert_source_parses(
        "fun renderSpec(flagSpec: Boolean) {\n    val valueSpec: Int = 1\n    println(valueSpec)\n    if (flagSpec) valueSpec else 0\n}\n",
    );
}

#[test]
fn ks_statements_0003_assignment_requires_expression_operands() {
    assert_source_parses(
        "fun updateSpec(sourceSpec: Int) { var targetSpec = 0; targetSpec = sourceSpec + 1 }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec() { var targetSpec = 0; targetSpec = val sourceSpec = 1 }\n",
    );
}

#[test]
fn ks_statements_0004_assignment_accepts_mutable_identifier_navigation_and_indexing_left_hand_side()
{
    assert_source_parses(
        "class StateSpec { var valueSpec: Int = 0; }\nfun updateSpec(stateSpec: StateSpec, valuesSpec: IntArray) {\n    var localSpec: Int = 0\n    localSpec = 1\n    stateSpec.valueSpec = localSpec\n    valuesSpec[0] = stateSpec.valueSpec\n}\n",
    );
}

#[test]
fn ks_statements_0004_non_assignable_expression_cannot_be_assignment_left_hand_side() {
    assert_source_parses("fun validSpec() { var valueSpec = 0; valueSpec = 1 }\n");
    assert_source_has_syntax_error("fun invalidSpec() { 1 + 2 = 3 }\n");
}

#[test]
#[ignore = "KS-STATEMENTS-0005: kmp-lsp does not diagnose assignments to read-only local properties"]
fn ks_statements_0005_read_only_local_property_cannot_be_assignment_left_hand_side() {
    assert_source_parses("fun validSpec() { var valueSpec = 0; valueSpec = 1 }\n");
    assert_source_has_syntax_error("fun invalidSpec() { val valueSpec = 0; valueSpec = 1 }\n");
}

#[test]
#[ignore = "KS-STATEMENTS-0005: kmp-lsp does not diagnose assignments to read-only member properties"]
fn ks_statements_0005_read_only_navigation_property_cannot_be_assignment_left_hand_side() {
    assert_source_parses(
        "class MutableStateSpec { var valueSpec = 0; }\nfun validSpec(stateSpec: MutableStateSpec) { stateSpec.valueSpec = 1 }\n",
    );
    assert_source_has_syntax_error(
        "class ReadOnlyStateSpec { val valueSpec = 0; }\nfun invalidSpec(stateSpec: ReadOnlyStateSpec) { stateSpec.valueSpec = 1 }\n",
    );
}

#[test]
fn ks_statements_0006_assignment_is_not_an_expression() {
    assert_source_parses("fun validSpec() { var valueSpec = 0; valueSpec = 1 }\n");
    assert_source_has_syntax_error(
        "fun invalidSpec(): Int { var valueSpec = 0; return (valueSpec = 1) }\n",
    );
}

#[test]
fn ks_statements_0007_simple_assignment_uses_assign_operator() {
    assert_source_parses("fun updateSpec() { var valueSpec = 0; valueSpec = 1 }\n");
}

#[test]
fn ks_statements_0011_operator_assignment_accepts_all_five_combined_forms() {
    assert_source_parses(
        "fun updateSpec() {\n    var valueSpec = 120\n    valueSpec += 2\n    valueSpec -= 3\n    valueSpec *= 4\n    valueSpec /= 5\n    valueSpec %= 6\n}\n",
    );
}

#[test]
fn ks_statements_0018_increment_and_decrement_operators_are_expressions() {
    assert_source_parses(
        "fun updateSpec() { var valueSpec = 1; val previousSpec = valueSpec++; val nextSpec = ++valueSpec; println(previousSpec + nextSpec) }\n",
    );
}

#[test]
fn ks_statements_0019_safe_navigation_may_appear_on_assignment_left_hand_side() {
    assert_source_parses(
        "class StateSpec { var valueSpec: Int = 0; }\nfun updateSpec(stateSpec: StateSpec?) { stateSpec?.valueSpec = 1 }\n",
    );
}

#[test]
fn ks_statements_0023_loop_statement_has_for_while_and_do_while_forms() {
    assert_source_parses(
        "fun iterateSpec(valuesSpec: List<Int>) {\n    for (valueSpec in valuesSpec) println(valueSpec)\n    while (false) println(0)\n    do println(1) while (false)\n}\n",
    );
}

#[test]
#[ignore = "KS-STATEMENTS-0024: kmp-lsp does not diagnose break outside loops"]
fn ks_statements_0024_break_is_allowed_only_in_loop_bodies() {
    assert_source_parses("fun validSpec() { while (true) { break } }\n");
    assert_source_has_syntax_error("fun invalidBreakSpec() { break }\n");
}

#[test]
#[ignore = "KS-STATEMENTS-0024: kmp-lsp does not diagnose continue outside loops"]
fn ks_statements_0024_continue_is_allowed_only_in_loop_bodies() {
    assert_source_parses("fun validSpec() { while (true) { continue } }\n");
    assert_source_has_syntax_error("fun invalidContinueSpec() { continue }\n");
}

#[test]
fn ks_statements_0025_while_loop_accepts_body_or_empty_semicolon_body() {
    assert_source_parses(
        "fun iterateSpec() {\n    while (false) { println(1) }\n    while (false);\n}\n",
    );
}

#[test]
#[ignore = "KS-STATEMENTS-0028: kmp-lsp does not type-check while conditions"]
fn ks_statements_0028_while_loop_condition_must_be_boolean() {
    assert_source_parses("fun validSpec() { while (false); }\n");
    assert_source_has_syntax_error("fun invalidSpec() { while (1); }\n");
}

#[test]
fn ks_statements_0029_do_while_loop_accepts_block_single_or_missing_body() {
    assert_source_parses(
        "fun iterateSpec() {\n    do { println(1) } while (false)\n    do println(2) while (false)\n    do while (false)\n}\n",
    );
}

#[test]
#[ignore = "KS-STATEMENTS-0032: kmp-lsp does not type-check do-while conditions"]
fn ks_statements_0032_do_while_loop_condition_must_be_boolean() {
    assert_source_parses("fun validSpec() { do while (false) }\n");
    assert_source_has_syntax_error("fun invalidSpec() { do while (1) }\n");
}

#[test]
fn ks_statements_0033_for_loop_has_only_foreach_form() {
    assert_source_parses(
        "fun validSpec(valuesSpec: List<Int>) { for (valueSpec in valuesSpec) println(valueSpec) }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec() { for (valueSpec = 0; valueSpec < 3; valueSpec++) println(valueSpec) }\n",
    );
}

#[test]
fn ks_statements_0034_for_loop_has_body_container_and_iteration_variable() {
    assert_source_parses(
        "fun iterateSpec(valuesSpec: List<Int>) { for (valueSpec in valuesSpec.drop(1)) { println(valueSpec) } }\n",
    );
}

#[test]
fn ks_statements_0036_for_loop_accepts_annotated_variable_or_destructuring_declaration() {
    assert_source_parses(
        "annotation class MarkerSpec\nfun iterateSpec(valuesSpec: List<Pair<Int, String>>) {\n    for (@MarkerSpec valueSpec in valuesSpec) println(valueSpec)\n    for ((countSpec, textSpec) in valuesSpec) println(textSpec + countSpec)\n}\n",
    );
}

#[test]
fn ks_statements_0038_code_block_accepts_empty_newline_and_semicolon_separated_statements() {
    assert_source_parses(
        "fun emptySpec() {}\nfun populatedSpec() {\n    val firstSpec = 1; val secondSpec = 2\n    println(firstSpec + secondSpec);\n}\n",
    );
}

#[test]
fn ks_statements_0040_bare_braces_in_statement_position_are_lambda_literal() {
    assert_source_contains_node_kind(
        "fun buildSpec() { { println(\"lambda\") } }\n",
        crate::queries::KIND_LAMBDA_LIT,
    );
}

#[test]
fn ks_statements_0042_control_structure_body_accepts_block_or_single_statement() {
    assert_source_parses(
        "fun renderSpec(flagSpec: Boolean) {\n    if (flagSpec) { println(1) }\n    if (!flagSpec) println(2)\n}\n",
    );
}
