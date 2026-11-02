use super::{lang_for_path, parse_live, utf16_col_to_byte};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::Url;

const KOTLIN_SRC: &str = "package com.example\nfun main() {}";
const JAVA_SRC: &str = "package com.example;\npublic class Foo {}";
const SWIFT_SRC: &str = "import Foundation\nfunc greet() {}";

fn kt_uri() -> Url {
    Url::parse("file:///tmp/Foo.kt").unwrap()
}
fn txt_uri() -> Url {
    Url::parse("file:///tmp/README.md").unwrap()
}

#[test]
fn lang_for_kotlin() {
    assert!(lang_for_path("Foo.kt").is_some());
    assert!(lang_for_path("build.gradle.kts").is_some());
}

#[test]
fn lang_for_java() {
    assert!(lang_for_path("Foo.java").is_some());
}

#[test]
fn lang_for_swift() {
    assert!(lang_for_path("Foo.swift").is_some());
}

#[test]
fn lang_for_unknown() {
    assert!(lang_for_path("README.md").is_none());
    assert!(lang_for_path("script.py").is_none());
    assert!(lang_for_path("").is_none());
}

#[test]
fn parse_kotlin_returns_tree() {
    let lang = lang_for_path("Foo.kt").unwrap();
    let doc = parse_live(KOTLIN_SRC, lang).expect("parse failed");
    assert_eq!(doc.tree.root_node().kind(), "source_file");
    assert_eq!(doc.bytes, KOTLIN_SRC.as_bytes());
}

#[test]
fn parse_java_returns_tree() {
    let lang = lang_for_path("Foo.java").unwrap();
    let doc = parse_live(JAVA_SRC, lang).expect("parse failed");
    assert_eq!(doc.tree.root_node().kind(), "program");
}

#[test]
fn parse_swift_returns_tree() {
    let lang = lang_for_path("Foo.swift").unwrap();
    let doc = parse_live(SWIFT_SRC, lang).expect("parse failed");
    assert!(!doc.tree.root_node().kind().is_empty());
}

#[test]
fn parse_empty_content() {
    let lang = lang_for_path("Foo.kt").unwrap();
    let doc = parse_live("", lang).expect("parse should succeed on empty input");
    assert_eq!(doc.tree.root_node().kind(), "source_file");
}

#[test]
fn store_then_live_doc_returns_some() {
    let idx = Indexer::new();
    let uri = kt_uri();
    idx.store_live_tree(&uri, KOTLIN_SRC);
    assert!(idx.live_doc(&uri).is_some());
}

#[test]
fn update_reflects_new_content() {
    let idx = Indexer::new();
    let uri = kt_uri();
    idx.store_live_tree(&uri, KOTLIN_SRC);
    let v1_bytes = idx.live_doc(&uri).unwrap().bytes.clone();

    let new_src = "package com.example\nfun other() {}";
    idx.store_live_tree(&uri, new_src);
    let v2_bytes = idx.live_doc(&uri).unwrap().bytes.clone();

    assert_ne!(v1_bytes, v2_bytes);
    assert_eq!(v2_bytes, new_src.as_bytes());
}

#[test]
fn remove_clears_live_doc() {
    let idx = Indexer::new();
    let uri = kt_uri();
    idx.store_live_tree(&uri, KOTLIN_SRC);
    idx.remove_live_tree(&uri);
    assert!(idx.live_doc(&uri).is_none());
}

#[test]
fn unknown_extension_never_stores() {
    let idx = Indexer::new();
    let uri = txt_uri();
    idx.store_live_tree(&uri, "hello world");
    assert!(idx.live_doc(&uri).is_none());
}

#[test]
fn unknown_extension_stale_eviction() {
    // If a URI previously had a live tree and is then stored again with an
    // unsupported extension, the stale entry must be evicted.
    let idx = Indexer::new();
    let txt = txt_uri();
    // Manually insert a stale entry for the txt URI by abusing the DashMap.
    let lang = lang_for_path("Foo.kt").unwrap();
    let doc = parse_live(KOTLIN_SRC, lang).unwrap();
    idx.live_trees
        .insert(txt.to_string(), std::sync::Arc::new(doc));
    assert!(
        idx.live_doc(&txt).is_some(),
        "pre-condition: stale entry exists"
    );

    // Now call store_live_tree — unsupported extension must evict the stale entry.
    idx.store_live_tree(&txt, "content");
    assert!(idx.live_doc(&txt).is_none(), "stale entry must be removed");
}

#[test]
fn unknown_extension_does_not_affect_other_entries() {
    // Storing an unsupported URI should be a no-op: it must not create a live
    // entry for that URI, and it must not disturb an existing supported entry
    // stored under a different URI.
    let idx = Indexer::new();
    let uri = kt_uri();
    idx.store_live_tree(&uri, KOTLIN_SRC);
    assert!(idx.live_doc(&uri).is_some());

    let txt = txt_uri();
    idx.store_live_tree(&txt, "content");
    assert!(idx.live_doc(&txt).is_none());
    // Original kt entry untouched.
    assert!(idx.live_doc(&uri).is_some());
}

#[test]
fn live_trees_survive_reset_index_state() {
    let idx = Indexer::new();
    let uri = kt_uri();
    idx.store_live_tree(&uri, KOTLIN_SRC);
    idx.reset_index_state();
    // Live trees must NOT be cleared by a workspace reindex.
    assert!(idx.live_doc(&uri).is_some());
}

// ── utf16_col_to_byte ────────────────────────────────────────────────────────

#[test]
fn utf16_col_ascii_identity() {
    // For ASCII text, UTF-16 offset == byte offset.
    assert_eq!(utf16_col_to_byte("hello world", 6), 6);
    assert_eq!(utf16_col_to_byte("hello world", 0), 0);
    assert_eq!(utf16_col_to_byte("hello world", 11), 11);
}

#[test]
fn utf16_col_multibyte_bmp() {
    // "é" is U+00E9: 2 UTF-8 bytes, 1 UTF-16 unit.
    // "résumé" bytes: r(1) é(2) s(1) u(1) m(1) é(2) = 8 bytes, but 6 UTF-16 units.
    let s = "résumé";
    // UTF-16 col 2 = after "ré" = byte offset 3 (r=1, é=2)
    assert_eq!(utf16_col_to_byte(s, 2), 3);
    // UTF-16 col 5 = after "résum" = byte offset 6 (r=1, é=2, s=1, u=1, m=1)
    assert_eq!(utf16_col_to_byte(s, 5), 6);
}

#[test]
fn utf16_col_surrogate_pair() {
    // U+1F600 (emoji) encodes to 2 UTF-16 units and 4 UTF-8 bytes.
    let s = "a😀b";
    // UTF-16 col 0 → byte 0 (before 'a')
    assert_eq!(utf16_col_to_byte(s, 0), 0);
    // UTF-16 col 1 → byte 1 (after 'a', before emoji)
    assert_eq!(utf16_col_to_byte(s, 1), 1);
    // UTF-16 col 3 → byte 5 (after 'a' + emoji (4 bytes), at 'b')
    assert_eq!(utf16_col_to_byte(s, 3), 5);
    // UTF-16 col 4 → byte 6 (after 'b')
    assert_eq!(utf16_col_to_byte(s, 4), 6);
}

#[test]
fn utf16_col_past_end_returns_len() {
    let s = "abc";
    assert_eq!(utf16_col_to_byte(s, 100), s.len());
}

/// An indexed file nobody has open is parsed once, not once per query.
#[test]
fn an_indexed_unopened_file_is_parsed_once_for_repeated_queries() {
    let indexer = Indexer::new();
    let uri = kt_uri();
    indexer.index_content(&uri, "class First");

    let first = indexer.live_doc_or_parse(&uri).unwrap();
    let second = indexer.live_doc_or_parse(&uri).unwrap();

    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

/// Re-indexing the file with new content must not keep answering with the
/// old tree, and an unrelated file must not borrow it.
#[test]
fn a_reindexed_file_is_parsed_again_and_files_do_not_share_trees() {
    let indexer = Indexer::new();
    let uri = kt_uri();
    let other = Url::parse("file:///tmp/Other.kt").unwrap();
    indexer.index_content(&uri, "class First");
    indexer.index_content(&other, "class Other");
    let before = indexer.live_doc_or_parse(&uri).unwrap();

    indexer.index_content(&uri, "class Second");
    let after = indexer.live_doc_or_parse(&uri).unwrap();
    let other_doc = indexer.live_doc_or_parse(&other).unwrap();

    assert_eq!(before.bytes, b"class First");
    assert_eq!(after.bytes, b"class Second");
    assert_eq!(other_doc.bytes, b"class Other");
}

/// Re-classifying a file's source set rebuilds its `FileData` but keeps the
/// same source lines: the remembered tree is still right and must be served.
/// New lines under the same URI must never be answered with the old tree.
#[test]
fn a_tree_survives_a_source_set_rebuild_but_not_new_lines() {
    let indexer = Indexer::new();
    let uri = kt_uri();
    indexer.index_content(&uri, "class First");
    let before = indexer.live_doc_or_parse(&uri).unwrap();

    let indexed = indexer.files.get(uri.as_str()).map(|file| file.clone());
    let mut reclassified = (*indexed.unwrap()).clone();
    reclassified.source_set = crate::types::SourceSet::Test;
    indexer
        .files
        .insert(uri.to_string(), std::sync::Arc::new(reclassified));
    let after_rebuild = indexer.live_doc_or_parse(&uri).unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &after_rebuild));

    let competing = indexer.files.get(uri.as_str()).map(|file| file.clone());
    let mut rewritten = (*competing.unwrap()).clone();
    rewritten.lines = std::sync::Arc::new(vec!["class Second".to_owned()]);
    indexer
        .files
        .insert(uri.to_string(), std::sync::Arc::new(rewritten));
    let after_new_lines = indexer.live_doc_or_parse(&uri).unwrap();
    assert_eq!(after_new_lines.bytes, b"class Second");
}

/// A poisoned cache lock costs a re-parse, never the answer.
#[test]
fn a_poisoned_parse_cache_still_answers() {
    let indexer = std::sync::Arc::new(Indexer::new());
    let uri = kt_uri();
    indexer.index_content(&uri, "class First");
    let poisoner = std::sync::Arc::clone(&indexer);
    let _ = std::thread::spawn(move || {
        let _guard = poisoner.unopened_parses.lock().unwrap();
        panic!("poison the parse cache");
    })
    .join();

    assert_eq!(
        indexer.live_doc_or_parse(&uri).unwrap().bytes,
        b"class First"
    );
}

/// The cache evicts the least recently used file: a file queried between
/// many other files stays remembered.
#[test]
fn a_hot_file_survives_a_stream_of_cold_files() {
    let indexer = Indexer::new();
    let hot = kt_uri();
    indexer.index_content(&hot, "class Hot");
    let hot_doc = indexer.live_doc_or_parse(&hot).unwrap();

    for number in 0..40 {
        let cold = Url::parse(&format!("file:///tmp/Cold{number}.kt")).unwrap();
        indexer.index_content(&cold, "class Cold");
        indexer.live_doc_or_parse(&cold).unwrap();
        let hot_again = indexer.live_doc_or_parse(&hot).unwrap();
        assert!(
            std::sync::Arc::ptr_eq(&hot_doc, &hot_again),
            "hot file re-parsed after {number} cold files"
        );
    }
}
