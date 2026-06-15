//! Integration tests for `kmp-lsp check`.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_kmp-lsp");

fn write_fixture(dir: &Path, rel_path: &str, content: &str) {
    let full = dir.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, content).unwrap();
}

fn check(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(BIN)
        .arg("check")
        .args(args)
        .output()
        .expect("failed to spawn kmp-lsp");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn valid_file_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "Foo.kt", "class Foo { fun bar(): Int = 42 }\n");
    let file = dir.path().join("Foo.kt");
    let (ok, stdout, _) = check(&[file.to_str().unwrap()]);
    assert!(ok, "expected exit 0 for valid file");
    assert!(stdout.contains("OK"), "expected OK message: {stdout}");
}

#[test]
fn syntax_error_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "Bad.kt", "class Foo {\n    fun bar() {\n");
    let file = dir.path().join("Bad.kt");
    let (ok, _, stderr) = check(&[file.to_str().unwrap()]);
    assert!(!ok, "expected exit 1 for file with syntax error");
    assert!(
        !stderr.is_empty(),
        "expected error count on stderr: {stderr}"
    );
}

#[test]
fn json_output_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "Foo.kt", "fun greet() = println(\"hi\")\n");
    let file = dir.path().join("Foo.kt");
    let (ok, stdout, _) = check(&["--json", file.to_str().unwrap()]);
    assert!(ok, "expected exit 0 for valid file in JSON mode");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(json["files_ok"], 1);
    assert_eq!(json["files_with_errors"], 0);
    assert!(json["errors"].as_array().unwrap().is_empty());
}

#[test]
fn json_output_error_file() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "Bad.kt", "fun broken( {\n");
    let file = dir.path().join("Bad.kt");
    let (ok, stdout, _) = check(&["--json", file.to_str().unwrap()]);
    assert!(!ok, "expected exit 1 for invalid file in JSON mode");
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(json["files_with_errors"], 1);
    assert!(!json["errors"].as_array().unwrap().is_empty());
}

#[test]
fn directory_arg_walks_kt_files() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "src/A.kt", "class A\n");
    write_fixture(dir.path(), "src/B.kt", "class B\n");
    write_fixture(dir.path(), "src/README.md", "# docs\n");
    let (ok, stdout, _) = check(&[dir.path().join("src").to_str().unwrap()]);
    assert!(ok, "expected exit 0 for valid directory");
    assert!(stdout.contains("OK"), "expected OK message: {stdout}");
}

#[test]
fn multiple_files_reports_all_errors() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "Good.kt", "class Good\n");
    write_fixture(dir.path(), "Bad.kt", "class Bad {\n");
    let good = dir.path().join("Good.kt");
    let bad = dir.path().join("Bad.kt");
    let (ok, _, stderr) = check(&[good.to_str().unwrap(), bad.to_str().unwrap()]);
    assert!(!ok, "expected exit 1 when any file has errors");
    assert!(stderr.contains("error"), "expected error summary: {stderr}");
}
