//! Unit tests for `cli::path_meta`.

use super::*;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn write_file(dir: &TempDir, rel: &str) -> PathBuf {
    let path = dir.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "").unwrap();
    path
}

// ─── source_set ──────────────────────────────────────────────────────────────

#[test]
fn source_set_commonmain() {
    let p = PathBuf::from("/repo/features/play-domain/src/commonMain/kotlin/Foo.kt");
    assert_eq!(source_set(&p).as_deref(), Some("commonMain"));
}

#[test]
fn source_set_android_host_test() {
    let p = PathBuf::from("/repo/features/play-ui/src/androidHostTest/kotlin/FooTest.kt");
    assert_eq!(source_set(&p).as_deref(), Some("androidHostTest"));
}

#[test]
fn source_set_user_defined() {
    // Custom KMP source set names should round-trip — the helper is name-agnostic.
    let p = PathBuf::from("/repo/shared/src/jvmCommonMain/kotlin/Y.kt");
    assert_eq!(source_set(&p).as_deref(), Some("jvmCommonMain"));
}

#[test]
fn source_set_plain_gradle() {
    let p = PathBuf::from("/repo/app/src/main/java/A.java");
    assert_eq!(source_set(&p).as_deref(), Some("main"));
    let q = PathBuf::from("/repo/app/src/test/kotlin/B.kt");
    assert_eq!(source_set(&q).as_deref(), Some("test"));
}

#[test]
fn source_set_uses_deepest_src() {
    // Nested project where the workspace is itself under a `src/` directory
    // (rare but possible). The closest `src/` ancestor wins.
    let p = PathBuf::from("/work/src/checkout/app/src/commonMain/kotlin/X.kt");
    assert_eq!(source_set(&p).as_deref(), Some("commonMain"));
}

#[test]
fn source_set_returns_none_without_src() {
    let p = PathBuf::from("/repo/scripts/release.kt");
    assert!(source_set(&p).is_none());
}

// ─── module ──────────────────────────────────────────────────────────────────

#[test]
fn module_single_segment() {
    let dir = TempDir::new().unwrap();
    let file = write_file(&dir, "app/src/main/kotlin/A.kt");
    assert_eq!(module(&file, dir.path()).as_deref(), Some("app"));
}

#[test]
fn module_nested_path() {
    let dir = TempDir::new().unwrap();
    let file = write_file(&dir, "features/play-domain/src/commonMain/kotlin/X.kt");
    assert_eq!(
        module(&file, dir.path()).as_deref(),
        Some("features/play-domain")
    );
}

#[test]
fn module_returns_none_for_root_level_src() {
    let dir = TempDir::new().unwrap();
    let file = write_file(&dir, "src/main/kotlin/Root.kt");
    assert_eq!(module(&file, dir.path()), None);
}

#[test]
fn module_returns_none_when_path_outside_root() {
    let dir = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let file = write_file(&other, "app/src/main/kotlin/Foo.kt");
    assert_eq!(module(&file, dir.path()), None);
}

// ─── relative_path ───────────────────────────────────────────────────────────

#[test]
fn relative_path_under_root() {
    let dir = TempDir::new().unwrap();
    let file = write_file(&dir, "app/src/main/kotlin/Foo.kt");
    let rel = relative_path(&file, dir.path());
    assert_eq!(rel, "app/src/main/kotlin/Foo.kt");
}

#[test]
fn relative_path_falls_back_to_absolute_when_outside_root() {
    let dir = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let file = write_file(&other, "foo/Bar.kt");
    let rel = relative_path(&file, dir.path());
    // We don't lock down the exact form (canonicalization differs across
    // macOS `/private/var` vs `/var`); we just require the file basename to
    // survive somewhere in the output and we never want a partial strip.
    assert!(rel.ends_with("foo/Bar.kt"), "rel was {rel}");
    assert!(rel.contains('/'), "rel was {rel}");
}
