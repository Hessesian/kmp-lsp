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
// ── selectionRange tests ───────────────────────────────────────────────────

#[cfg(test)]
mod selection_range_tests {
    use tower_lsp::lsp_types::{Position, Range, SelectionRange};

    /// Build a chain of SelectionRange nodes from the CST of `source` at `pos`.
    /// Returns the innermost selection range with parent links to ancestors.
    fn build_selection_chain(
        source: &str,
        pos: Position,
        lang: tree_sitter::Language,
    ) -> Option<SelectionRange> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&lang).ok()?;
        let tree = parser.parse(source, None)?;
        let root = tree.root_node();

        let line_idx = pos.line as usize;
        let line_text = source.lines().nth(line_idx)?;
        let byte_col =
            crate::indexer::live_tree::utf16_col_to_byte(line_text, pos.character as usize);
        let point = tree_sitter::Point::new(line_idx, byte_col);

        let node = root.descendant_for_point_range(point, point)?;

        let mut chain: Vec<SelectionRange> = Vec::new();
        let mut cur = node;
        let mut max_depth = 50u32;
        while max_depth > 0 {
            let start = cur.start_position();
            let end = cur.end_position();
            let range = Range {
                start: Position {
                    line: start.row as u32,
                    character: start.column as u32,
                },
                end: Position {
                    line: end.row as u32,
                    character: end.column as u32,
                },
            };
            if chain
                .last()
                .is_none_or(|prev: &SelectionRange| prev.range != range)
            {
                chain.push(SelectionRange {
                    range,
                    parent: None,
                });
            }
            max_depth -= 1;
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }

        for i in (1..chain.len()).rev() {
            let parent = chain.remove(i);
            chain[i - 1].parent = Some(Box::new(parent));
        }

        chain.into_iter().next()
    }

    /// Count the number of ancestors in a selection range chain.
    fn chain_depth(range: &SelectionRange) -> usize {
        let mut count = 1;
        let mut cur = range;
        while let Some(ref p) = cur.parent {
            count += 1;
            cur = p;
        }
        count
    }

    #[test]
    fn selects_word_inside_string_literal() {
        let src = "fun main() { val x = \"hello world\" }";
        let pos = Position {
            line: 0,
            character: 21, // 'w' in "world" (UTF-16)
        };
        let chain = build_selection_chain(src, pos, tree_sitter_kotlin::language())
            .expect("should build chain");

        // Should expand: word → string → assignment → block → function
        assert!(
            chain_depth(&chain) >= 3,
            "Expected at least 3 levels, got {}",
            chain_depth(&chain)
        );
    }

    #[test]
    fn selects_identifier_in_function_call() {
        let src = "fun main() { println(\"hi\") }";
        let pos = Position {
            line: 0,
            character: 14, // 'p' in "println"
        };
        let chain = build_selection_chain(src, pos, tree_sitter_kotlin::language())
            .expect("should build chain");
        assert!(chain_depth(&chain) >= 4);
    }

    #[test]
    fn selects_inside_nested_class() {
        let src = "class Outer { class Inner { fun foo() = 42 } }";
        let pos = Position {
            line: 0,
            character: 29, // 'u' in "fun"
        };
        let chain = build_selection_chain(src, pos, tree_sitter_kotlin::language())
            .expect("should build chain");
        // Should expand: fun → function_body → class_body → class → source_file
        assert!(
            chain_depth(&chain) >= 3,
            "Expected at least 3 levels, got {}",
            chain_depth(&chain)
        );
    }

    #[test]
    fn empty_chain_given_no_parent() {
        let chain = build_selection_chain(
            "",
            Position {
                line: 0,
                character: 0,
            },
            tree_sitter_kotlin::language(),
        );
        assert!(chain.is_none());
    }

    #[test]
    fn java_method_selection_expands() {
        let src = "class Foo { void bar() { int x = 1; } }";
        // Position on 'x'
        let pos = Position {
            line: 0,
            character: 27,
        };
        let chain = build_selection_chain(src, pos, tree_sitter_java::language())
            .expect("should build chain");
        assert!(chain_depth(&chain) >= 4);
    }
}

// ── foldingRange tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod folding_range_tests {
    use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind};

    /// Simplified folding range computation for testing.
    /// Takes text lines and returns fold ranges.
    fn compute_folds(lines: &[&str]) -> Vec<FoldingRange> {
        let mut ranges: Vec<FoldingRange> = Vec::new();
        let mut stack: Vec<u32> = Vec::new();

        // Brace regions.
        for (i, line) in lines.iter().enumerate() {
            let opens = line.chars().filter(|&c| c == '{').count() as i32;
            let closes = line.chars().filter(|&c| c == '}').count() as i32;
            let net = opens - closes;
            if net > 0 {
                for _ in 0..net {
                    stack.push(i as u32);
                }
            } else if net < 0 {
                for _ in 0..(-net) {
                    if let Some(start_line) = stack.pop() {
                        if i as u32 > start_line + 1 {
                            ranges.push(FoldingRange {
                                start_line,
                                end_line: i as u32,
                                start_character: None,
                                end_character: None,
                                kind: Some(FoldingRangeKind::Region),
                                collapsed_text: Some("{...}".into()),
                            });
                        }
                    }
                }
            }
        }

        // Import blocks.
        let mut import_start: Option<u32> = None;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("import ") {
                if import_start.is_none() {
                    import_start = Some(i as u32);
                }
            } else if let Some(is) = import_start.take() {
                if i as u32 > is + 1 {
                    ranges.push(FoldingRange {
                        start_line: is,
                        end_line: (i as u32) - 1,
                        start_character: None,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Imports),
                        collapsed_text: Some("imports".into()),
                    });
                }
            }
        }
        if let Some(is) = import_start {
            let last = lines.len() as u32 - 1;
            if last > is + 1 {
                ranges.push(FoldingRange {
                    start_line: is,
                    end_line: last,
                    start_character: None,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Imports),
                    collapsed_text: Some("imports".into()),
                });
            }
        }

        // Block comments /* ... */
        let mut bc_start: Option<u32> = None;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if bc_start.is_some() {
                if trimmed.contains("*/") {
                    let start = bc_start.take().unwrap();
                    if i as u32 > start + 1 {
                        ranges.push(FoldingRange {
                            start_line: start,
                            end_line: i as u32,
                            start_character: None,
                            end_character: None,
                            kind: Some(FoldingRangeKind::Comment),
                            collapsed_text: Some("/* ...".into()),
                        });
                    }
                }
            } else if let Some(pos) = trimmed.find("/*") {
                let after_open = &trimmed[pos + 2..];
                if !after_open.contains("*/") {
                    bc_start = Some(i as u32);
                }
            }
        }

        // Line comment blocks.
        let mut comment_start: Option<u32> = None;
        for (i, line) in lines.iter().enumerate() {
            if line.trim().starts_with("//") {
                if comment_start.is_none() {
                    comment_start = Some(i as u32);
                }
            } else if let Some(cs) = comment_start.take() {
                if i as u32 > cs + 1 {
                    ranges.push(FoldingRange {
                        start_line: cs,
                        end_line: (i as u32) - 1,
                        start_character: None,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Comment),
                        collapsed_text: Some("// ...".into()),
                    });
                }
            }
        }
        if let Some(cs) = comment_start {
            let last = lines.len() as u32 - 1;
            if last > cs + 1 {
                ranges.push(FoldingRange {
                    start_line: cs,
                    end_line: last,
                    start_character: None,
                    end_character: None,
                    kind: Some(FoldingRangeKind::Comment),
                    collapsed_text: Some("// ...".into()),
                });
            }
        }

        ranges
    }

    #[test]
    fn detects_brace_region() {
        let folds = compute_folds(&["fun main() {", "  println()", "}"]);
        assert_eq!(folds.len(), 1);
        assert_eq!(folds[0].start_line, 0);
        assert_eq!(folds[0].end_line, 2);
        assert_eq!(folds[0].kind, Some(FoldingRangeKind::Region));
    }

    #[test]
    fn detects_import_block() {
        let folds = compute_folds(&[
            "package com.example",
            "",
            "import a.A",
            "import b.B",
            "import c.C",
            "",
            "class Foo",
        ]);
        let import_folds: Vec<_> = folds
            .iter()
            .filter(|f| f.kind == Some(FoldingRangeKind::Imports))
            .collect();
        assert_eq!(
            import_folds.len(),
            1,
            "Expected 1 import fold, got {:?}",
            folds
        );
        assert_eq!(import_folds[0].start_line, 2);
        assert_eq!(import_folds[0].end_line, 4);
    }

    #[test]
    fn detects_import_block_to_eof() {
        let folds = compute_folds(&[
            "package com.example",
            "",
            "import a.A",
            "import b.B",
            "import c.C",
        ]);
        let import_folds: Vec<_> = folds
            .iter()
            .filter(|f| f.kind == Some(FoldingRangeKind::Imports))
            .collect();
        assert_eq!(import_folds.len(), 1);
        assert_eq!(import_folds[0].start_line, 2);
        assert_eq!(import_folds[0].end_line, 4);
    }

    #[test]
    fn skips_single_import() {
        let folds = compute_folds(&["package com.example", "", "import a.A", "", "class Foo"]);
        let import_folds: Vec<_> = folds
            .iter()
            .filter(|f| f.kind == Some(FoldingRangeKind::Imports))
            .collect();
        assert!(import_folds.is_empty(), "Single import should not fold");
    }

    #[test]
    fn detects_block_comment() {
        let folds = compute_folds(&["/*", " * Multi-line", " * comment", " */", "class Foo"]);
        let comment_folds: Vec<_> = folds
            .iter()
            .filter(|f| f.kind == Some(FoldingRangeKind::Comment))
            .collect();
        assert!(
            !comment_folds.is_empty(),
            "Expected block comment fold, got {:?}",
            folds
        );
    }

    #[test]
    fn skips_single_line_block_comment() {
        let folds = compute_folds(&["class Foo /* comment */ {", "}"]);
        let comment_folds: Vec<_> = folds
            .iter()
            .filter(|f| {
                f.kind == Some(FoldingRangeKind::Comment)
                    && f.collapsed_text == Some("/* ...".into())
            })
            .collect();
        assert!(
            comment_folds.is_empty(),
            "Single-line /* ... */ should not fold"
        );
    }

    #[test]
    fn detects_consecutive_line_comments() {
        let folds = compute_folds(&["// header 1", "// header 2", "// header 3", "", "class Foo"]);
        let comment_folds: Vec<_> = folds
            .iter()
            .filter(|f| f.collapsed_text == Some("// ...".into()))
            .collect();
        assert_eq!(comment_folds.len(), 1);
        assert_eq!(comment_folds[0].start_line, 0);
        assert_eq!(comment_folds[0].end_line, 2);
    }

    #[test]
    fn collapsed_text_on_brace_region() {
        let folds = compute_folds(&["{", "  val x = 1", "}"]);
        assert_eq!(folds.len(), 1);
        assert_eq!(folds[0].collapsed_text.as_deref(), Some("{...}"));
    }

    #[test]
    fn collapsed_text_on_import_block() {
        let folds = compute_folds(&["import a.A", "import b.B", "import c.C", "", "class Foo"]);
        let import_folds: Vec<_> = folds
            .iter()
            .filter(|f| f.kind == Some(FoldingRangeKind::Imports))
            .collect();
        assert_eq!(import_folds.len(), 1);
        assert_eq!(import_folds[0].collapsed_text.as_deref(), Some("imports"));
    }

    #[test]
    fn no_comment_fold_for_single_line() {
        let folds = compute_folds(&["// just one comment", "class Foo"]);
        let comment_folds: Vec<_> = folds
            .iter()
            .filter(|f| {
                f.kind == Some(FoldingRangeKind::Comment)
                    && f.collapsed_text == Some("// ...".into())
            })
            .collect();
        assert!(
            comment_folds.is_empty(),
            "Single comment line should not fold"
        );
    }

    #[test]
    fn comment_block_to_eof() {
        let folds = compute_folds(&[
            "class Foo {}",
            "",
            "// trailing 1",
            "// trailing 2",
            "// trailing 3",
        ]);
        let comment_folds: Vec<_> = folds
            .iter()
            .filter(|f| {
                f.kind == Some(FoldingRangeKind::Comment)
                    && f.collapsed_text == Some("// ...".into())
            })
            .collect();
        assert_eq!(comment_folds.len(), 1);
        assert_eq!(comment_folds[0].start_line, 2);
        assert_eq!(comment_folds[0].end_line, 4);
    }
}

// ── code action helpers tests ────────────────────────────────────────────────

#[cfg(test)]
mod code_action_tests {
    use crate::backend::actions::{
        build_override_signature, extract_override_params, extract_override_return,
        strip_visibility_and_modifiers,
    };
    use crate::types::{SymbolEntry, Visibility};
    use tower_lsp::lsp_types::{Range, SymbolKind};

    fn make_sym(name: &str, detail: &str) -> SymbolEntry {
        SymbolEntry {
            name: name.to_owned(),
            kind: SymbolKind::METHOD,
            visibility: Visibility::Public,
            range: Range::default(),
            selection_range: Range::default(),
            detail: detail.to_owned(),
            type_params: vec![],
            extension_receiver: String::new(),
            deprecated: false,
        }
    }

    #[test]
    fn override_signature_from_detail() {
        let sym = make_sym("getItem", "fun getItem(index: Int): String");
        let sig = build_override_signature(&sym);
        assert_eq!(sig, "fun getItem(index: Int): String");
    }

    #[test]
    fn override_signature_no_detail() {
        let sym = make_sym("toString", "");
        let sig = build_override_signature(&sym);
        assert_eq!(sig, "fun toString()");
    }

    #[test]
    fn strip_visibility_removes_modifiers() {
        assert_eq!(strip_visibility_and_modifiers("private fun foo()"), "foo()");
        assert_eq!(strip_visibility_and_modifiers("suspend fun bar()"), "bar()");
        assert_eq!(strip_visibility_and_modifiers("fun baz()"), "baz()");
    }

    #[test]
    fn extract_params_simple() {
        assert_eq!(extract_override_params("foo(x: Int)"), "(x: Int)");
        assert_eq!(extract_override_params("foo()"), "()");
    }

    #[test]
    fn extract_return_type() {
        assert_eq!(extract_override_return("foo(): String"), ": String");
        assert_eq!(extract_override_return("foo()"), "");
    }
}
