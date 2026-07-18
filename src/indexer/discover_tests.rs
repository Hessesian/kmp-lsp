//! Unit tests for `indexer::discover`.

use super::{
    find_layout_files, find_source_files, find_source_files_unconstrained, warm_discover_files,
};
use crate::indexer::cache::workspace_cache_path;
use crate::indexer::test_helpers::with_xdg_cache;
use crate::rg::IgnoreMatcher;

/// `find_source_files` on a directory with no source files returns an empty vec.
#[test]
fn find_source_files_empty_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = find_source_files(tmp.path(), None);
    assert!(
        paths.is_empty(),
        "expected no files in empty dir, got: {paths:?}"
    );
}

/// `find_source_files` discovers .kt files.
#[test]
fn find_source_files_finds_kt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("Foo.kt"), "class Foo").expect("write");
    std::fs::write(tmp.path().join("Bar.txt"), "text").expect("write");

    let paths = find_source_files(tmp.path(), None);
    let names: Vec<_> = paths
        .iter()
        .filter_map(|p| p.file_name()?.to_str())
        .collect();
    assert!(names.contains(&"Foo.kt"), "Foo.kt missing: {names:?}");
    assert!(
        !names.contains(&"Bar.txt"),
        "Bar.txt should not be included"
    );
}

/// `find_source_files` discovers .java files.
#[test]
fn find_source_files_finds_java() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("Hello.java"), "class Hello {}").expect("write");

    let paths = find_source_files(tmp.path(), None);
    let names: Vec<_> = paths
        .iter()
        .filter_map(|p| p.file_name()?.to_str())
        .collect();
    assert!(
        names.contains(&"Hello.java"),
        "Hello.java missing: {names:?}"
    );
}

/// `find_source_files` with an IgnoreMatcher that matches the file should exclude it.
#[test]
fn find_source_files_respects_ignore_matcher() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sub = tmp.path().join("generated");
    std::fs::create_dir(&sub).expect("mkdir");
    std::fs::write(sub.join("Gen.kt"), "class Gen").expect("write");
    std::fs::write(tmp.path().join("Keep.kt"), "class Keep").expect("write");

    let matcher = IgnoreMatcher::new(vec!["generated/**".to_owned()], tmp.path());
    let paths = find_source_files(tmp.path(), Some(&matcher));
    let names: Vec<_> = paths
        .iter()
        .filter_map(|p| p.file_name()?.to_str())
        .collect();
    assert!(names.contains(&"Keep.kt"), "Keep.kt should be found");
    assert!(
        !names.contains(&"Gen.kt"),
        "Gen.kt inside 'generated/' should be excluded"
    );
}

/// `find_source_files_unconstrained` finds .kt files without skipping `build` dirs.
#[test]
fn find_source_files_unconstrained_includes_build_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let build = tmp.path().join("build");
    std::fs::create_dir(&build).expect("mkdir build");
    std::fs::write(build.join("Generated.kt"), "class Generated").expect("write");

    let paths = find_source_files_unconstrained(tmp.path());
    let names: Vec<_> = paths
        .iter()
        .filter_map(|p| p.file_name()?.to_str())
        .collect();
    assert!(
        names.contains(&"Generated.kt"),
        "Generated.kt in build/ should be found by unconstrained scan"
    );
}

/// `warm_discover_files` on a fresh cache with a real file returns that file.
#[test]
fn warm_discover_files_returns_cached_existing_files() {
    use crate::indexer::cache::{FileCacheEntry, IndexCache, CACHE_VERSION};
    use crate::types::FileData;
    use std::collections::HashMap;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("workspace");
    std::fs::create_dir(&root).expect("mkdir workspace");
    let kt = root.join("Main.kt");
    std::fs::write(&kt, "class Main").expect("write");

    let mut entries = HashMap::new();
    entries.insert(
        kt.to_string_lossy().to_string(),
        FileCacheEntry {
            mtime_secs: 0,
            file_size: 0,
            content_hash: 0,
            file_data: std::sync::Arc::new(FileData::default()),
            qualified_keys: vec![],
        },
    );
    let cache = IndexCache {
        version: CACHE_VERSION,
        complete_scan: true,
        entries,
        layouts: HashMap::new(),
        generated_bindings: HashMap::new(),
    };

    with_xdg_cache(tmp.path(), || {
        // Create the on-disk cache file so warm_discover_files can stat it.
        let cache_path = workspace_cache_path(&root);
        std::fs::create_dir_all(cache_path.parent().unwrap()).expect("mkdir cache dir");
        std::fs::write(&cache_path, b"").expect("touch cache file");

        let paths = warm_discover_files(&root, &cache, None);
        let names: Vec<_> = paths
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert!(
            names.contains(&"Main.kt"),
            "Main.kt should be returned by warm_discover_files: {names:?}"
        );
    });
}

/// `warm_discover_files` excludes cached files that no longer exist on disk.
#[test]
fn warm_discover_files_skips_deleted_files() {
    use crate::indexer::cache::{FileCacheEntry, IndexCache, CACHE_VERSION};
    use crate::types::FileData;
    use std::collections::HashMap;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("workspace");
    std::fs::create_dir(&root).expect("mkdir workspace");
    let ghost = root.join("Deleted.kt");
    // Do NOT create the file — it's "in the cache" but deleted on disk.

    let mut entries = HashMap::new();
    entries.insert(
        ghost.to_string_lossy().to_string(),
        FileCacheEntry {
            mtime_secs: 0,
            file_size: 0,
            content_hash: 0,
            file_data: std::sync::Arc::new(FileData::default()),
            qualified_keys: vec![],
        },
    );
    let cache = IndexCache {
        version: CACHE_VERSION,
        complete_scan: true,
        entries,
        layouts: HashMap::new(),
        generated_bindings: HashMap::new(),
    };

    with_xdg_cache(tmp.path(), || {
        let cache_path = workspace_cache_path(&root);
        std::fs::create_dir_all(cache_path.parent().unwrap()).expect("mkdir cache dir");
        std::fs::write(&cache_path, b"").expect("touch cache file");

        let paths = warm_discover_files(&root, &cache, None);
        assert!(
            !paths.iter().any(|p| p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "Deleted.kt")
                .unwrap_or(false)),
            "deleted file should not appear in warm_discover_files result"
        );
    });
}

/// `find_layout_files` discovers layout XML under `res/layout*` but not backups.
#[test]
fn find_layout_files_discovers_layout_variants() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let layout_dir = tmp.path().join("app/src/main/res/layout");
    let land_dir = tmp.path().join("app/src/main/res/layout-land");
    let backup_dir = tmp.path().join("app/src/main/res/layouts_backup");
    std::fs::create_dir_all(&layout_dir).expect("mkdir layout");
    std::fs::create_dir_all(&land_dir).expect("mkdir layout-land");
    std::fs::create_dir_all(&backup_dir).expect("mkdir backup");
    std::fs::write(layout_dir.join("foo_bar.xml"), "<LinearLayout/>").expect("write default");
    std::fs::write(land_dir.join("foo_bar.xml"), "<LinearLayout/>").expect("write land");
    std::fs::write(backup_dir.join("ignored.xml"), "<LinearLayout/>").expect("write backup");
    std::fs::write(tmp.path().join("notes.xml"), "<LinearLayout/>").expect("write non-layout");

    let paths = find_layout_files(tmp.path(), None);
    let relative: Vec<_> = paths
        .iter()
        .map(|path| {
            path.strip_prefix(tmp.path())
                .unwrap_or(path)
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert!(
        relative
            .iter()
            .any(|path| path.contains("res/layout/foo_bar.xml")),
        "default layout missing: {relative:?}"
    );
    assert!(
        relative
            .iter()
            .any(|path| path.contains("res/layout-land/foo_bar.xml")),
        "land layout missing: {relative:?}"
    );
    assert!(
        !relative.iter().any(|path| path.contains("layouts_backup")),
        "backup layout should be excluded: {relative:?}"
    );
    assert!(
        !relative.iter().any(|path| path.ends_with("notes.xml")),
        "non-layout xml should be excluded: {relative:?}"
    );
}

/// `find_source_files` still excludes `build/` while layout discovery is separate.
#[test]
fn find_source_files_still_excludes_build_dir_with_layout_discovery() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let build_dir = tmp.path().join("build/generated/layout/foo.xml");
    std::fs::create_dir_all(build_dir.parent().unwrap()).expect("mkdir build layout");
    std::fs::write(build_dir, "<LinearLayout/>").expect("write build xml");

    let source_paths = find_source_files(tmp.path(), None);
    assert!(
        source_paths.is_empty(),
        "build xml must not be a source file"
    );

    let layout_paths = find_layout_files(tmp.path(), None);
    assert!(
        layout_paths.is_empty(),
        "build xml must not be discovered as a layout file: {layout_paths:?}"
    );
}
