//! Tests for the sources-JAR parse cache (disk roundtrip, freshness, pruning).

use std::sync::Arc;

use super::sources_jar_cache::{
    entry_is_fresh, jar_fingerprint, load_sources_jar_cache, prune_deleted_jars,
    save_sources_jar_cache, JarFingerprint, SourcesFileEntry, SourcesJarEntry,
};

/// Parse a tiny Kotlin snippet to get a realistic FileData for fixtures.
fn parsed_file_entry(uri_text: &str, source: &str) -> SourcesFileEntry {
    let uri = tower_lsp::lsp_types::Url::parse(uri_text).expect("test uri");
    let result = crate::indexer::Indexer::parse_file(&uri, source);
    SourcesFileEntry {
        uri: uri_text.to_owned(),
        content_hash: result.content_hash,
        file_data: Arc::new(result.data),
    }
}

fn entry_for(fingerprint: &JarFingerprint, files: Vec<SourcesFileEntry>) -> SourcesJarEntry {
    SourcesJarEntry {
        mtime_secs: fingerprint.mtime_secs,
        mtime_nanos: fingerprint.mtime_nanos,
        file_size: fingerprint.file_size,
        files,
    }
}

#[test]
fn roundtrip_preserves_entries() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let jar_path = tmpdir.path().join("lib-1.0-sources.jar");
    std::fs::write(&jar_path, b"not a real zip, fingerprint only").expect("write jar");
    let fingerprint = jar_fingerprint(&jar_path).expect("fingerprint");

    let file_entry = parsed_file_entry(
        "jar:file:///fake/lib-1.0-sources.jar!/com/example/Core.kt",
        "package com.example\n\nclass Core\n",
    );
    let mut entries = std::collections::HashMap::new();
    entries.insert(
        jar_path.to_string_lossy().to_string(),
        entry_for(&fingerprint, vec![file_entry]),
    );

    save_sources_jar_cache(Some(tmpdir.path()), &entries);
    let loaded = load_sources_jar_cache(Some(tmpdir.path()));

    let entry = loaded
        .get(jar_path.to_string_lossy().as_ref())
        .expect("entry survives roundtrip");
    assert_eq!(entry.files.len(), 1);
    assert_eq!(entry.files[0].file_data.symbols.len(), 1);
    assert_eq!(entry.files[0].file_data.symbols[0].name, "Core");
    assert_eq!(
        entry.files[0].file_data.package.as_deref(),
        Some("com.example")
    );
}

#[test]
fn load_from_missing_dir_returns_empty() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let loaded = load_sources_jar_cache(Some(&tmpdir.path().join("does-not-exist")));
    assert!(loaded.is_empty());
}

#[test]
fn entry_is_fresh_matches_unchanged_file() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let jar_path = tmpdir.path().join("lib-1.0-sources.jar");
    std::fs::write(&jar_path, b"content").expect("write jar");
    let fingerprint = jar_fingerprint(&jar_path).expect("fingerprint");
    let entry = entry_for(&fingerprint, Vec::new());
    let current = jar_fingerprint(&jar_path).expect("fingerprint again");
    assert!(entry_is_fresh(&entry, &current));
}

#[test]
fn entry_is_stale_after_size_change() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let jar_path = tmpdir.path().join("lib-1.0-sources.jar");
    std::fs::write(&jar_path, b"content").expect("write jar");
    let fingerprint = jar_fingerprint(&jar_path).expect("fingerprint");
    let entry = entry_for(&fingerprint, Vec::new());
    std::fs::write(&jar_path, b"content grew larger").expect("rewrite jar");
    let current = jar_fingerprint(&jar_path).expect("fingerprint after change");
    assert!(!entry_is_fresh(&entry, &current));
}

#[test]
fn prune_drops_entries_for_deleted_jars() {
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let live_jar = tmpdir.path().join("live-1.0-sources.jar");
    std::fs::write(&live_jar, b"content").expect("write jar");
    let fingerprint = jar_fingerprint(&live_jar).expect("fingerprint");

    let mut entries = std::collections::HashMap::new();
    entries.insert(
        live_jar.to_string_lossy().to_string(),
        entry_for(&fingerprint, Vec::new()),
    );
    entries.insert(
        tmpdir
            .path()
            .join("deleted-0.9-sources.jar")
            .to_string_lossy()
            .to_string(),
        entry_for(&fingerprint, Vec::new()),
    );

    let pruned = prune_deleted_jars(&mut entries);
    assert!(pruned, "pruning removed something");
    assert_eq!(entries.len(), 1);
    assert!(entries.contains_key(live_jar.to_string_lossy().as_ref()));

    let pruned_again = prune_deleted_jars(&mut entries);
    assert!(!pruned_again, "second prune is a no-op");
}
