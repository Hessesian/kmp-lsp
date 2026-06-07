//! Integration tests for new CLI commands: check, context, organize-imports.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_kotlin-lsp");

fn write_fixture(dir: &Path, rel_path: &str, content: &str) {
    let full = dir.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, content).unwrap();
}

fn index(dir: &Path) {
    let output = Command::new(BIN)
        .args(["index", "--root"])
        .arg(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "index failed: {:?}", output);
}

// ── check ────────────────────────────────────────────────────────────────────

#[test]
fn check_valid_file_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "src/Ok.kt", "class Ok(val x: Int)");
    let output = Command::new(BIN)
        .args(["check", &dir.path().join("src/Ok.kt").to_string_lossy()])
        .output()
        .unwrap();
    assert!(output.status.success(), "check ok file: {:?}", output);
}

#[test]
fn check_syntax_error_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "src/Bad.kt", "class Bad {");
    let output = Command::new(BIN)
        .args(["check", &dir.path().join("src/Bad.kt").to_string_lossy()])
        .output()
        .unwrap();
    assert!(!output.status.success(), "check bad file should exit 1");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error"),
        "stderr should mention error: {}",
        stderr
    );
}

#[test]
fn check_json_output() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "src/Bad.kt", "class Bad {");
    let output = Command::new(BIN)
        .args([
            "check",
            "--json",
            &dir.path().join("src/Bad.kt").to_string_lossy(),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["files_ok"], 0);
    assert_eq!(v["files_with_errors"], 1);
}

// ── organize-imports ─────────────────────────────────────────────────────────

#[test]
fn organize_imports_removes_unused() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(
        dir.path(),
        "src/Main.kt",
        "package com.example\n\nimport java.util.List\nimport java.util.Map\n\nclass Main(val list: List<String>)",
    );
    let output = Command::new(BIN)
        .args([
            "organize-imports",
            &dir.path().join("src/Main.kt").to_string_lossy(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Map is unused, should be removed
    assert!(
        stdout.contains("- import java.util.Map"),
        "unused Map should be removed: {stdout}"
    );
}

// ── context ──────────────────────────────────────────────────────────────────

#[ignore]
#[test]
fn inject_sorts_by_frequency() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(
        dir.path(),
        "src/Main.kt",
        "class User\nclass UserRepository\nclass App {\n    val repo: UserRepository = UserRepository()\n    val u1: User = User()\n    val u2: User = User()\n}",
    );
    index(dir.path());
    let output = Command::new(BIN)
        .args([
            "inject",
            &dir.path().join("src/Main.kt").to_string_lossy(),
            "--root",
            &dir.path().to_string_lossy(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // User should appear before UserRepository (referenced more often)
    let user_pos = stdout.find("User:").unwrap_or(usize::MAX);
    let repo_pos = stdout.find("UserRepository:").unwrap_or(usize::MAX);
    assert!(
        user_pos < repo_pos,
        "User (3 refs) should come before UserRepository (2 refs): {stdout}"
    );
}

#[test]
fn insert_writes_content_in_place() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "T.kt", "line1\nline2");
    let output = Command::new(BIN)
        .args([
            "insert",
            &dir.path().join("T.kt").to_string_lossy(),
            "1",
            "--after",
            "--content",
            "INSERTED",
            "--in-place",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "insert failed: {:?}", output);
    let file = std::fs::read_to_string(dir.path().join("T.kt")).unwrap();
    assert!(
        file.contains("line1\nINSERTED\nline2"),
        "should insert content after line 1: {file}"
    );
}

#[test]
fn batch_dry_run_reports_changes_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("T.kt");
    write_fixture(dir.path(), "T.kt", "fun oldName() {}\n");

    let rule = serde_json::json!({
        "files": {
            target.to_string_lossy().to_string(): [
                {
                    "action": "replace",
                    "old": "oldName",
                    "new": "newName"
                }
            ]
        }
    });
    let rule_file = dir.path().join("rules.json");
    std::fs::write(&rule_file, serde_json::to_string(&rule).unwrap()).unwrap();

    let output = Command::new(BIN)
        .args(["batch", &rule_file.to_string_lossy(), "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success(), "batch failed: {:?}", output);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry-run"),
        "should report dry-run: {stdout}"
    );
    assert!(
        stdout.contains("newName"),
        "should preview replacement: {stdout}"
    );
    let file = std::fs::read_to_string(target).unwrap();
    assert_eq!(file, "fun oldName() {}\n");
}

#[test]
fn cache_stats_subcommand_runs() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(BIN)
        .args(["cache", "stats", "--root", &dir.path().to_string_lossy()])
        .output()
        .unwrap();
    assert!(output.status.success(), "cache stats failed: {:?}", output);

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Cache path:") && stdout.contains("Status:"),
        "cache stats should print status: {stdout}"
    );
}
