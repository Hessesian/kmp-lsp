use super::{assert_source_parses, parse_kotlin_source};

#[test]
fn ks_1_2_1_line_feed_terminates_line_comment() {
    let source = "// first line\nval visible = 1\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(tree.root_node().to_sexp().contains("property_declaration"));
}

#[test]
fn ks_1_2_1_crlf_terminates_line_comment() {
    let source = "// first line\r\nval visible = 1\r\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(tree.root_node().to_sexp().contains("property_declaration"));
}

#[test]
fn ks_1_2_1_delimited_comments_may_be_nested() {
    assert_source_parses("/* outer /* nested */ outer */\nval visible = 1\n");
}

#[test]
fn ks_1_2_1_spaces_tabs_and_form_feed_are_whitespace() {
    assert_source_parses("val\tanswer\u{000c}=\t42\n");
}

#[test]
fn ks_1_2_1_shebang_extends_to_the_line_terminator() {
    let source = "#!/usr/bin/env kotlin\nval visible = 1\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(tree.root_node().to_sexp().contains("shebang_line"));
    assert!(tree.root_node().to_sexp().contains("property_declaration"));
}
