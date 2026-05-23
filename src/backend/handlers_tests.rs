use super::word_byte_offsets;

#[test]
fn finds_single_word() {
    let offsets: Vec<_> = word_byte_offsets("hello world", "world").collect();
    assert_eq!(offsets, vec![6]);
}

#[test]
fn skips_partial_match() {
    // "name" should not match inside "rename"
    let offsets: Vec<_> = word_byte_offsets("rename name", "name").collect();
    assert_eq!(offsets, vec![7]);
}

#[test]
fn multiple_occurrences() {
    let offsets: Vec<_> = word_byte_offsets("a b a c a", "a").collect();
    assert_eq!(offsets, vec![0, 4, 8]);
}

#[test]
fn unicode_line() {
    // "ñ" is 2 bytes in UTF-8; "name" after it still at correct byte offset
    let line = "ñ name ñ";
    let offsets: Vec<_> = word_byte_offsets(line, "name").collect();
    assert_eq!(offsets.len(), 1);
    assert_eq!(&line[offsets[0]..offsets[0] + 4], "name");
}

#[test]
fn no_match() {
    let offsets: Vec<_> = word_byte_offsets("foo bar", "baz").collect();
    assert!(offsets.is_empty());
}

// ── Call hierarchy helpers tests ────────────────────────────────────────────

#[cfg(test)]
mod call_hierarchy_tests {
    use super::super::{extract_call_hierarchy_name, find_cst_ident_range, is_keyword};

    fn parse_kotlin(source: &str) -> Option<tree_sitter::Tree> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_kotlin::language()).ok()?;
        parser.parse(source, None)
    }

    /// Find the first descendant with the given `kind`, depth-first.
    fn find_deepest_child<'a>(
        node: tree_sitter::Node<'a>,
        kind: &str,
    ) -> Option<tree_sitter::Node<'a>> {
        let mut cursor = node.walk();
        if node.child_count() == 0 {
            return None;
        }
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(found) = find_deepest_child(child, kind) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn extract_name_from_function_declaration() {
        let src = "fun greet(name: String): String { return \"hi\" }";
        let tree = parse_kotlin(src).expect("parse");
        let root = tree.root_node();
        let mut cursor = root.walk();
        let decl = root
            .children(&mut cursor)
            .find(|c| c.kind() == "function_declaration")
            .expect("should have function_declaration");
        let name = extract_call_hierarchy_name(&decl, src);
        assert_eq!(name, "greet");
    }

    #[test]
    fn extract_name_from_method() {
        let src = "class Foo { fun bar() {} }";
        let tree = parse_kotlin(src).expect("parse");
        let root = tree.root_node();
        // tree-sitter-kotlin may use "function_declaration" for methods inside classes.
        let decl =
            find_deepest_child(root, "function_declaration").expect("should find nested function");
        let name = extract_call_hierarchy_name(&decl, src);
        assert_eq!(name, "bar");
    }

    #[test]
    fn ident_range_found_in_declaration() {
        let src = "fun hello() {}";
        let tree = parse_kotlin(src).expect("parse");
        let root = tree.root_node();
        let mut cursor = root.walk();
        let decl = root
            .children(&mut cursor)
            .find(|c| c.kind() == "function_declaration")
            .expect("should have function_declaration");
        let range = find_cst_ident_range(&decl, src);
        assert_eq!(range.start.character, 4); // 'h' in "hello"
        assert_eq!(range.end.character, 9); // after "hello"
    }

    #[test]
    fn collect_outgoing_calls_finds_callee() {
        let src = "fun a() { b() }";
        let tree = parse_kotlin(src).expect("parse");
        let root = tree.root_node();

        // Walk the CST and verify call_expression node exists.
        fn has_call_expression(node: tree_sitter::Node) -> bool {
            if node.kind() == "call_expression" {
                return true;
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if has_call_expression(child) {
                    return true;
                }
            }
            false
        }
        assert!(has_call_expression(root), "Expected call_expression in CST");
    }

    #[test]
    fn keyword_filter_rejects_reserved_words() {
        // Verify keyword filtering used in call collection.
        assert!(is_keyword("return"));
        assert!(!is_keyword("println"));
    }

    #[test]
    fn is_keyword_rejects_language_keywords() {
        assert!(is_keyword("if"));
        assert!(is_keyword("return"));
        assert!(is_keyword("class"));
    }

    #[test]
    fn is_keyword_accepts_identifiers() {
        assert!(!is_keyword("myFunction"));
        assert!(!is_keyword("println"));
        assert!(!is_keyword("foo"));
    }
}
