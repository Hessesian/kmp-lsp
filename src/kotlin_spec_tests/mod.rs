//! Contract tests traced directly to Kotlin language specification clauses.

mod coverage_matrix;
mod syntax_and_grammar;

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
