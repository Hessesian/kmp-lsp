//! Tests for `path_util`. The helpers are most interesting on Windows but
//! must also behave correctly on Unix (where most local development happens).

use super::*;

#[test]
fn forward_slash_unix_path_unchanged() {
    let p = Path::new("/foo/bar/baz.kt");
    assert_eq!(to_forward_slash(p), "/foo/bar/baz.kt");
}

#[test]
fn forward_slash_relative_path() {
    let p = Path::new("src/main/Foo.kt");
    let s = to_forward_slash(p);
    // On all platforms the result must use `/`.
    assert!(!s.contains('\\'), "contains backslash: {s}");
    assert!(s.ends_with("Foo.kt"));
    assert!(s.contains("src") && s.contains("main"));
}

#[test]
fn forward_slash_empty_path() {
    assert_eq!(to_forward_slash(Path::new("")), "");
}

#[test]
fn strip_unc_no_op_on_path_without_prefix() {
    let p = PathBuf::from("/usr/local/bin");
    assert_eq!(strip_unc_prefix(p.clone()), p);
}

#[cfg(windows)]
#[test]
fn strip_unc_drive_letter_path() {
    let p = PathBuf::from(r"\\?\C:\Users\foo\bar.kt");
    assert_eq!(strip_unc_prefix(p), PathBuf::from(r"C:\Users\foo\bar.kt"));
}

#[cfg(windows)]
#[test]
fn strip_unc_leaves_server_paths_alone() {
    // \\?\UNC\server\share is structurally different; we don't try to rewrite it.
    let p = PathBuf::from(r"\\?\UNC\server\share\file.kt");
    assert_eq!(strip_unc_prefix(p.clone()), p);
}

#[test]
fn stem_basic() {
    let u = Url::parse("file:///pkg/Foo.kt").unwrap();
    assert_eq!(file_stem_from_uri(&u).as_deref(), Some("Foo"));
}

#[test]
fn stem_no_extension() {
    let u = Url::parse("file:///pkg/README").unwrap();
    assert_eq!(file_stem_from_uri(&u).as_deref(), Some("README"));
}

#[test]
fn stem_dotfile_keeps_full_name() {
    // `.gitignore` has no "extension" — the whole name is the stem.
    let u = Url::parse("file:///root/.gitignore").unwrap();
    assert_eq!(file_stem_from_uri(&u).as_deref(), Some(".gitignore"));
}

#[test]
fn stem_windows_style_uri() {
    // Drive-letter URI — should work on all platforms.
    let u = Url::parse("file:///C:/pkg/Foo.kt").unwrap();
    assert_eq!(file_stem_from_uri(&u).as_deref(), Some("Foo"));
}

#[test]
fn stem_multiple_dots() {
    let u = Url::parse("file:///pkg/Foo.bar.kt").unwrap();
    // `rfind('.')` so only the last extension is stripped.
    assert_eq!(file_stem_from_uri(&u).as_deref(), Some("Foo.bar"));
}
