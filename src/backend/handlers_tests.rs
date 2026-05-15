use crate::features::text_utils::{whole_word_replace_file, word_byte_offsets};

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

#[test]
fn whole_word_replace_file_empty_word_returns_input() {
    let lines = vec!["import foo.Bar".to_string(), "val name = Bar".to_string()];
    let text = whole_word_replace_file(&lines, "", "Baz");
    assert_eq!(text, "import foo.Bar\nval name = Bar");
}

#[tokio::test]
async fn panic_safe_catches_panic_returns_internal_error() {
    use crate::backend::panic_safe;

    let result: tower_lsp::jsonrpc::Result<Option<()>> =
        panic_safe("test_handler", async { panic!("intentional test panic") }).await;

    let err = result.unwrap_err();
    assert_eq!(err.code, tower_lsp::jsonrpc::ErrorCode::InternalError);
    assert!(err.message.contains("test_handler"));
}

#[tokio::test]
async fn panic_safe_passes_through_ok() {
    use crate::backend::panic_safe;

    let result = panic_safe("test_handler", async { Ok(Some(42)) }).await;
    assert_eq!(result.unwrap(), Some(42));
}
