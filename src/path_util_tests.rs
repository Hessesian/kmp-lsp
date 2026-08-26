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

#[test]
fn path_from_uri_drive_letter_uri_resolves_the_full_path() {
    let uri = Url::parse("file:///C:/pkg/Foo.kt").unwrap();
    let path = path_from_uri(&uri).unwrap();
    assert_eq!(path.file_name().unwrap(), "Foo.kt");
}

#[test]
fn path_from_uri_no_drive_letter_uri_still_resolves_a_matching_path() {
    // No drive letter — `Url::to_file_path()` rejects this on Windows (see
    // this module's own doc comment), so `path_from_uri` must fall back to
    // building the `PathBuf` from the URL's raw path string instead.
    let uri = Url::parse("file:///t/app/Bar.kt").unwrap();
    let path = path_from_uri(&uri).unwrap();
    let content_root = PathBuf::from("/t/app");
    assert!(
        path.starts_with(&content_root),
        "expected {path:?} to start with {content_root:?}"
    );
}

#[test]
fn path_from_url_path_string_builds_a_path_that_starts_with_its_own_prefix() {
    // Exercises the Windows-only fallback branch directly, since
    // `Url::to_file_path()` never fails for an absolute-looking URI on Unix
    // (there is no drive-letter requirement there), so `path_from_uri`
    // itself can't reach this branch in a Linux test run.
    let path = path_from_url_path_string("/t/app/Bar.kt").unwrap();
    assert!(path.starts_with(PathBuf::from("/t/app")));
    assert_eq!(path.file_name().unwrap(), "Bar.kt");
}

#[test]
fn path_from_url_path_string_rejects_an_empty_path() {
    assert_eq!(path_from_url_path_string(""), None);
}

#[test]
fn path_from_uri_rejects_a_jar_scheme_uri() {
    // A `jar:file://...` URI is opaque (cannot-be-a-base) and has no
    // filesystem path of its own; the drive-letter fallback must not
    // mistake its opaque data for a real path.
    let uri = Url::parse("jar:file:///home/user/x.jar").unwrap();
    assert_eq!(path_from_uri(&uri), None);
}
