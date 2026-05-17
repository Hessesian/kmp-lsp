//! Unit tests for `indexer::discover`.

use super::{find_source_files, find_source_files_unconstrained, warm_discover_files};
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

/// `find_source_files` skips well-known build-cache and IDE dirs by default.
#[test]
fn find_source_files_skips_default_excluded_dir_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // One file inside each default-excluded directory + one keeper.
    for excluded in [
        ".git",
        "build",
        "target",
        ".gradle",
        ".build",
        "DerivedData",
        "Generated",
        ".kotlin",
        ".idea",
        ".fleet",
        ".vscode",
        "node_modules",
        ".cache",
        "captures",
        ".externalNativeBuild",
        ".cxx",
        "xcuserdata",
        "Pods",
    ] {
        let dir = tmp.path().join(excluded);
        std::fs::create_dir_all(&dir).expect("mkdir excluded");
        std::fs::write(dir.join("Skip.kt"), "class Skip").expect("write");
    }
    std::fs::write(tmp.path().join("Keep.kt"), "class Keep").expect("write");

    let paths = find_source_files(tmp.path(), None);
    let names: Vec<_> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|p| p.ends_with("Keep.kt")),
        "Keep.kt missing: {names:?}"
    );
    assert!(
        !names.iter().any(|p| p.ends_with("Skip.kt")),
        "Skip.kt inside excluded dir should not be indexed: {names:?}"
    );
}

/// Nested `.claude/worktrees/**` files are skipped even though `.claude` itself is kept.
#[test]
fn find_source_files_skips_claude_worktrees_path_glob() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dotclaude = tmp.path().join(".claude");
    let worktrees = dotclaude.join("worktrees/branch-x/src");
    let projects = dotclaude.join("projects/some-proj");
    let plans = dotclaude.join("plans");
    let commands = dotclaude.join("commands");
    std::fs::create_dir_all(&worktrees).expect("mkdir worktrees");
    std::fs::create_dir_all(&projects).expect("mkdir projects");
    std::fs::create_dir_all(&plans).expect("mkdir plans");
    std::fs::create_dir_all(&commands).expect("mkdir commands");
    std::fs::write(worktrees.join("Agent.kt"), "class Agent").expect("write");
    std::fs::write(projects.join("Proj.kt"), "class Proj").expect("write");
    std::fs::write(plans.join("Plan.kt"), "class Plan").expect("write");
    // Sibling `.claude/commands` should NOT be excluded — only the listed subdirs are.
    std::fs::write(commands.join("Cmd.kt"), "class Cmd").expect("write");

    let paths = find_source_files(tmp.path(), None);
    let names: Vec<_> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|p| p.ends_with("Agent.kt")),
        ".claude/worktrees should be excluded: {names:?}"
    );
    assert!(
        !names.iter().any(|p| p.ends_with("Proj.kt")),
        ".claude/projects should be excluded: {names:?}"
    );
    assert!(
        !names.iter().any(|p| p.ends_with("Plan.kt")),
        ".claude/plans should be excluded: {names:?}"
    );
    assert!(
        names.iter().any(|p| p.ends_with("Cmd.kt")),
        ".claude/commands should NOT be excluded: {names:?}"
    );
}

/// User-provided ignorePatterns still compose with the default excludes.
#[test]
fn find_source_files_user_patterns_compose_with_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let build = tmp.path().join("build");
    let custom = tmp.path().join("custom-gen");
    std::fs::create_dir_all(&build).expect("mkdir build");
    std::fs::create_dir_all(&custom).expect("mkdir custom");
    std::fs::write(build.join("Built.kt"), "class Built").expect("write");
    std::fs::write(custom.join("Gen.kt"), "class Gen").expect("write");
    std::fs::write(tmp.path().join("Keep.kt"), "class Keep").expect("write");

    let matcher = IgnoreMatcher::new(vec!["custom-gen/**".to_owned()], tmp.path());
    let paths = find_source_files(tmp.path(), Some(&matcher));
    let names: Vec<_> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().any(|p| p.ends_with("Keep.kt")),
        "Keep.kt missing: {names:?}"
    );
    assert!(
        !names.iter().any(|p| p.ends_with("Built.kt")),
        "default build/ exclusion should still apply: {names:?}"
    );
    assert!(
        !names.iter().any(|p| p.ends_with("Gen.kt")),
        "user pattern should still apply: {names:?}"
    );
}

/// `find_source_files_unconstrained` deliberately ignores defaults — user
/// explicitly asked for these directories.
#[test]
fn find_source_files_unconstrained_keeps_default_excluded_dirs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for excluded in ["build", ".gradle", "node_modules", ".kotlin"] {
        let dir = tmp.path().join(excluded);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("Inside.kt"), "class Inside").expect("write");
    }
    let paths = find_source_files_unconstrained(tmp.path());
    let count = paths
        .iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some("Inside.kt"))
        .count();
    assert_eq!(
        count, 4,
        "unconstrained scan should find Inside.kt in every excluded-by-default dir: {paths:?}"
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
            file_data: FileData::default(),
        },
    );
    let cache = IndexCache {
        version: CACHE_VERSION,
        complete_scan: true,
        entries,
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
            file_data: FileData::default(),
        },
    );
    let cache = IndexCache {
        version: CACHE_VERSION,
        complete_scan: true,
        entries,
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
