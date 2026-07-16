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
