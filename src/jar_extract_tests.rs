use super::*;
use std::io::Write;
use zip::write::SimpleFileOptions;

fn make_jar(dir: &Path, name: &str, entries: &[(&str, &str)]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    for (entry, content) in entries {
        zip.start_file(*entry, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn parse_jar_entry_uri_splits_entry() {
    // Build the jar URI from a real path via Url::from_file_path so it round-trips
    // through Url::to_file_path cross-platform (Windows needs a drive letter).
    let tmp = tempfile::tempdir().unwrap();
    let jar_path = tmp.path().join("foo-sources.jar");
    let jar_url = Url::from_file_path(&jar_path).unwrap();
    let (jar, entry) = parse_jar_entry_uri(&format!("jar:{jar_url}!/a/B.kt")).unwrap();
    assert_eq!(jar, jar_path);
    assert_eq!(entry, "a/B.kt");
    // Compiled-only JAR URI (no `!/entry`) → nothing to extract.
    assert!(parse_jar_entry_uri("jar:file:///x/foo.aar").is_none());
    // Not a jar: URI.
    assert!(parse_jar_entry_uri("file:///x.kt").is_none());
    // Empty entry and path-traversal / absolute entries are rejected (extraction safety).
    assert!(parse_jar_entry_uri("jar:file:///x/foo.jar!/").is_none());
    assert!(parse_jar_entry_uri("jar:file:///x/foo.jar!/../etc/passwd").is_none());
    assert!(parse_jar_entry_uri("jar:file:///x/foo.jar!//abs.kt").is_none());
}

#[test]
fn extract_round_trips_entry_to_file() {
    let _lock = crate::indexer::test_helpers::XDG_CACHE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _xdg = crate::indexer::test_helpers::EnvVarGuard::set("XDG_CACHE_HOME", tmp.path());

    let jar = make_jar(
        tmp.path(),
        "lib-sources.jar",
        &[("com/example/Foo.kt", "package com.example\nclass Foo")],
    );

    let url = extract_jar_entry_to_disk(&jar, "com/example/Foo.kt").unwrap();
    assert_eq!(url.scheme(), "file");
    let content = std::fs::read_to_string(url.to_file_path().unwrap()).unwrap();
    assert!(
        content.contains("class Foo"),
        "extracted content: {content:?}"
    );

    // Idempotent — second call returns the same path.
    let url_again = extract_jar_entry_to_disk(&jar, "com/example/Foo.kt").unwrap();
    assert_eq!(url, url_again);

    // Absent entry → None.
    assert!(extract_jar_entry_to_disk(&jar, "no/such.kt").is_none());
}

#[test]
fn rewrite_converts_jar_definition_to_file() {
    let _lock = crate::indexer::test_helpers::XDG_CACHE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let _xdg = crate::indexer::test_helpers::EnvVarGuard::set("XDG_CACHE_HOME", tmp.path());

    let jar = make_jar(tmp.path(), "ui-sources.jar", &[("a/B.kt", "class B")]);
    let indexer = Indexer::new();
    let range = tower_lsp::lsp_types::Range::default();

    // Build the jar: URI via Url::from_file_path so it's well-formed cross-platform
    // (Windows drive letters / backslashes).
    let jar_file_url = Url::from_file_path(&jar).unwrap();
    let jar_uri = Url::parse(&format!("jar:{jar_file_url}!/a/B.kt")).unwrap();
    let rewritten = rewrite_jar_definitions(
        &indexer,
        GotoDefinitionResponse::Scalar(Location {
            uri: jar_uri,
            range,
        }),
    );
    let GotoDefinitionResponse::Scalar(loc) = rewritten else {
        panic!("expected scalar");
    };
    assert_eq!(loc.uri.scheme(), "file", "jar target must become file://");
    assert!(
        indexer.library_uris.contains(loc.uri.as_str()),
        "extracted source must be registered as a library URI"
    );

    // A compiled-only jar URI (no source entry) is left unchanged.
    let aar = Url::parse("jar:file:///x/foo.aar").unwrap();
    let passthrough = rewrite_jar_definitions(
        &indexer,
        GotoDefinitionResponse::Scalar(Location {
            uri: aar.clone(),
            range,
        }),
    );
    let GotoDefinitionResponse::Scalar(loc2) = passthrough else {
        panic!("expected scalar");
    };
    assert_eq!(loc2.uri, aar, "compiled-only jar URI must pass through");
}
