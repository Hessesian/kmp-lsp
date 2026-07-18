//! Tests for shared ViewBinding receiver resolution.

use tower_lsp::lsp_types::Url;

use crate::indexer::live_tree::RequestParseCache;
use crate::indexer::Indexer;
use crate::queries::KIND_SIMPLE_IDENT;
use crate::resolver::ReceiverType;
use crate::viewbinding::receiver::{
    binding_class_for_bare_field_access, binding_class_from_receiver_type,
    implicit_receiver_type_for_bare_member_at, is_view_binding_class_name,
};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn live_indexed(path: &str, source: &str) -> (Url, Indexer) {
    let file_uri = uri(path);
    let indexer = Indexer::new();
    indexer.index_content(&file_uri, source);
    indexer.set_live_lines(&file_uri, source);
    indexer.store_live_tree(&file_uri, source);
    (file_uri, indexer)
}

fn utf16_position_in(source: &str, needle: &str) -> (usize, usize) {
    let byte_offset = source.find(needle).expect("needle in source");
    let mut line = 0_usize;
    let mut utf16_column = 0_usize;
    for (index, character) in source.char_indices() {
        if index == byte_offset {
            return (line, utf16_column);
        }
        if character == '\n' {
            line += 1;
            utf16_column = 0;
        } else {
            utf16_column += character.len_utf16();
        }
    }
    panic!("needle not found");
}

#[test]
fn is_view_binding_class_name_accepts_agp_convention() {
    assert!(is_view_binding_class_name("FooBarBinding"));
    assert!(!is_view_binding_class_name("Binding"));
    assert!(!is_view_binding_class_name("NotABindingClass"));
}

#[test]
fn binding_class_from_receiver_type_uses_leaf() {
    let receiver_type = ReceiverType::from_raw("com.example.FooBarBinding".to_string());
    assert_eq!(
        binding_class_from_receiver_type(&receiver_type),
        Some("FooBarBinding".into())
    );
}

#[test]
fn bare_field_access_resolves_binding_class_inside_with_block() {
    let source = r#"fun demo(binding: FooBarBinding) {
    with(binding) {
        title
    }
}
"#;
    let (file_uri, indexer) = live_indexed("/with_block.kt", source);
    let document = indexer.live_doc(&file_uri).expect("live doc");
    let bytes = document.bytes.as_slice();
    let (line, utf16_column) = utf16_position_in(source, "title");
    let byte_column = crate::indexer::live_tree::utf16_col_to_byte(
        source.lines().nth(line).unwrap(),
        utf16_column,
    );
    let point = tree_sitter::Point {
        row: line,
        column: byte_column,
    };
    let identifier_node = document
        .tree
        .root_node()
        .descendant_for_point_range(point, point)
        .expect("identifier node");
    assert_eq!(identifier_node.kind(), KIND_SIMPLE_IDENT);

    let binding_class = binding_class_for_bare_field_access(
        &indexer,
        &identifier_node,
        "title",
        bytes,
        &file_uri,
        None,
    );
    assert_eq!(binding_class, Some("FooBarBinding".into()));
}

#[test]
fn implicit_receiver_type_returns_none_when_local_shadows() {
    let source = r#"fun demo(binding: FooBarBinding) {
    with(binding) {
        val title = "local"
        title
    }
}
"#;
    let (file_uri, indexer) = live_indexed("/shadowed.kt", source);
    let (line, utf16_column) = utf16_position_in(source, "        title\n");
    assert!(implicit_receiver_type_for_bare_member_at(
        &indexer,
        &file_uri,
        line,
        utf16_column,
        "title",
        None,
    )
    .is_none());
}

#[test]
fn implicit_receiver_type_resolves_inside_apply_block() {
    let source = r#"fun demo(binding: FooBarBinding) {
    binding.apply {
        title
    }
}
"#;
    let (file_uri, indexer) = live_indexed("/apply_block.kt", source);
    let (line, utf16_column) = utf16_position_in(source, "        title\n");
    let receiver_type = implicit_receiver_type_for_bare_member_at(
        &indexer,
        &file_uri,
        line,
        utf16_column,
        "title",
        None,
    )
    .expect("implicit receiver");
    assert_eq!(receiver_type.leaf, "FooBarBinding");
}

#[test]
fn binding_class_for_bare_field_access_uses_request_parse_cache() {
    let source = r#"fun demo(binding: FooBarBinding) {
    with(binding) { title }
}
"#;
    let (file_uri, indexer) = live_indexed("/cache.kt", source);
    let document = indexer.live_doc(&file_uri).expect("live doc");
    let bytes = document.bytes.as_slice();
    let (line, utf16_column) = utf16_position_in(source, "title");
    let byte_column = crate::indexer::live_tree::utf16_col_to_byte(
        source.lines().nth(line).unwrap(),
        utf16_column,
    );
    let point = tree_sitter::Point {
        row: line,
        column: byte_column,
    };
    let identifier_node = document
        .tree
        .root_node()
        .descendant_for_point_range(point, point)
        .expect("identifier node");

    let mut parse_cache = RequestParseCache::new();
    let binding_class = binding_class_for_bare_field_access(
        &indexer,
        &identifier_node,
        "title",
        bytes,
        &file_uri,
        Some(&mut parse_cache),
    );
    assert_eq!(binding_class, Some("FooBarBinding".into()));
}
