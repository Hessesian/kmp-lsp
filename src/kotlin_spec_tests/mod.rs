//! Contract tests traced directly to Kotlin language specification clauses.

mod coverage_matrix;
mod syntax_and_grammar;
mod syntax_grammar_files_and_declarations;
mod syntax_grammar_literals_and_control;
mod syntax_grammar_statements_and_expressions;
mod syntax_grammar_types;

use tree_sitter::{Parser, Tree};

fn parse_kotlin_source(source: &str) -> Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_kotlin::language())
        .expect("tree-sitter-kotlin language must load");
    parser
        .parse(source, None)
        .expect("tree-sitter must return a Kotlin CST")
}

fn assert_source_parses(source: &str) {
    let tree = parse_kotlin_source(source);
    assert!(
        !tree.root_node().has_error(),
        "expected a clean Kotlin CST, got: {}",
        tree.root_node().to_sexp()
    );
}

fn assert_source_has_syntax_error(source: &str) {
    let tree = parse_kotlin_source(source);
    assert!(
        tree.root_node().has_error(),
        "expected a Kotlin CST error, got: {}",
        tree.root_node().to_sexp()
    );
}

fn assert_source_contains_node_kind(source: &str, expected_kind: &str) {
    let tree = parse_kotlin_source(source);
    assert!(
        !tree.root_node().has_error(),
        "expected a clean Kotlin CST, got: {}",
        tree.root_node().to_sexp()
    );
    assert!(
        count_nodes_of_kind(&tree, expected_kind) > 0,
        "expected CST node kind {expected_kind}, got: {}",
        tree.root_node().to_sexp()
    );
}

fn count_nodes_of_kind(tree: &Tree, expected_kind: &str) -> usize {
    let mut count = 0;
    let mut cursor = tree.root_node().walk();

    loop {
        if cursor.node().kind() == expected_kind {
            count += 1;
        }

        if cursor.goto_first_child() {
            continue;
        }

        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return count;
            }
        }
    }
}
