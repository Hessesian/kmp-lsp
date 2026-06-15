//! Integration tests for `kmp-lsp refs`.

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

#[test]
fn exclude_imports_removes_import_lines() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    // src/A.kt imports Foo and uses it as a real reference.
    write_fixture(
        root,
        "src/A.kt",
        "import com.example.Foo\n\nfun useIt(f: Foo) {}\n",
    );
    write_fixture(root, "src/B.kt", "class Foo\n");

    let out_default = Command::new(BIN)
        .args(["refs", "Foo", "--fast", "--root"])
        .arg(root)
        .output()
        .expect("spawn");
    let default_output = String::from_utf8_lossy(&out_default.stdout);

    let out_excluded = Command::new(BIN)
        .args(["refs", "Foo", "--fast", "--exclude-imports", "--root"])
        .arg(root)
        .output()
        .expect("spawn");
    let excluded_output = String::from_utf8_lossy(&out_excluded.stdout);

    // Default output includes the import on line 1 of A.kt.
    let default_lines: Vec<&str> = default_output.lines().collect();
    let excluded_lines: Vec<&str> = excluded_output.lines().collect();

    // The import is on line 1; check default output has an A.kt line-1 entry.
    assert!(
        default_lines
            .iter()
            .any(|line| line.contains("A.kt") && line.contains(":1:")),
        "expected import (A.kt:1:...) in default refs output:\n{default_output}"
    );
    // With --exclude-imports the A.kt:1 entry must be gone.
    assert!(
        !excluded_lines
            .iter()
            .any(|line| line.contains("A.kt") && line.contains(":1:")),
        "expected no import line (A.kt:1:...) with --exclude-imports:\n{excluded_output}"
    );
    // But the real usage on line 3 must still appear.
    assert!(
        excluded_lines
            .iter()
            .any(|line| line.contains("A.kt") && line.contains(":3:")),
        "expected parameter usage (A.kt:3:...) to survive --exclude-imports:\n{excluded_output}"
    );
}
