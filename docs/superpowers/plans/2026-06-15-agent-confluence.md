# Agent Confluence Quick Wins — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port five targeted improvements that tighten the loop between agents (Serena, Copilot CLI) and kmp-lsp — faster syntax feedback, less noisy refs, two parser false-positive fixes, and a trivial clippy fix.

**Architecture:** All changes are additive or surgical. `check` is a new CLI subcommand wrapping `parse_by_extension`. `--exclude-imports` is a post-filter on the existing refs pipeline. The two parser fixes add helper functions to `collect_syntax_errors`. The `rfind` fix is a single-line swap in `completion_context.rs`. Diagnose gains syntax errors as an extra output section.

**Tech Stack:** Rust, tree-sitter, lexopt (CLI arg parsing), serde_json, tempfile (integration tests)

---

## File Map

| File | Action | What changes |
|---|---|---|
| `src/features/completion_context.rs` | Modify | `rfind` clippy fix (1 line) |
| `src/parser.rs` | Modify | Add `is_file_annotation_comma_error`, use it in `collect_syntax_errors` |
| `src/parser_tests.rs` | Modify | Add 2 tests for `@file:[...]` comma false positive |
| `src/cli/args.rs` | Modify | Add `Check` variant; add `exclude_imports: bool` to `Refs` |
| `src/cli/check.rs` | Create | `run_check` + `expand_file_list` |
| `src/cli/mod.rs` | Modify | Declare `pub(crate) mod check` |
| `src/cli/run.rs` | Modify | Wire `check` subcommand; add `--exclude-imports` filter to `run_refs`; emit syntax errors in `run_diagnose` |
| `tests/cli_check.rs` | Create | Integration tests for `kmp-lsp check` |

---

## Task 1: `rfind` clippy fix

**Files:**
- Modify: `src/features/completion_context.rs` line ~101

- [ ] **Step 1: Apply the fix**

In `src/features/completion_context.rs`, find the `param_type_at` function (~line 97). Change:
```rust
        .filter(|s| !s.is_empty())
        .nth(idx)?;
```
to:
```rust
        .nth(idx)?;
```

Wait — `nth` doesn't filter. The actual fix is on the `find_lambda_label` function, not `param_type_at`. Find the function `find_lambda_label` which contains `.filter(|s| !s.is_empty()).next_back()` and change it to `.rfind(|s: &&str| !s.is_empty())`.

Search for the exact site:
```bash
grep -n "next_back\|filter.*is_empty.*next_back" src/features/completion_context.rs
```

Change `.filter(|s| !s.is_empty()).next_back()` → `.rfind(|s: &&str| !s.is_empty())`.

- [ ] **Step 2: Verify clippy passes**

```bash
cargo clippy -- -D warnings 2>&1 | grep completion_context
```
Expected: no output (no warnings for that file).

- [ ] **Step 3: Commit**

```bash
git add src/features/completion_context.rs
git commit -m "fix: rfind replaces filter+next_back (clippy 1.96)"
```

---

## Task 2: `@file:[...]` annotation comma fix

**Files:**
- Modify: `src/parser.rs` — add `is_file_annotation_comma_error`, use in `collect_syntax_errors`
- Modify: `src/parser_tests.rs` — 2 new tests

- [ ] **Step 1: Write failing tests first**

In `src/parser_tests.rs`, add after the existing `no_errors_on_valid_kotlin` test:

```rust
#[test]
fn no_false_positive_file_annotation_single() {
    // @file:[Ann] with a single annotation must not produce errors.
    let data = parse_kotlin("@file:[JvmName(\"Foo\")]\npackage com.example\n");
    assert!(
        data.syntax_errors.is_empty(),
        "unexpected errors: {:?}",
        data.syntax_errors
    );
}

#[test]
fn no_false_positive_file_annotation_comma() {
    // @file:[Ann1, Ann2] comma separator triggers a tree-sitter-kotlin 0.3 bug;
    // the lone `,` must be suppressed, not reported as a syntax error.
    let data = parse_kotlin("@file:[JvmName(\"Foo\"), Suppress(\"unused\")]\npackage com.example\n");
    assert!(
        data.syntax_errors.is_empty(),
        "unexpected errors: {:?}",
        data.syntax_errors
    );
}
```

- [ ] **Step 2: Run to verify the comma test fails**

```bash
cargo test --lib no_false_positive_file_annotation_comma 2>&1 | tail -20
```
Expected: FAIL — the comma produces an ERROR node today.

- [ ] **Step 3: Add `is_file_annotation_comma_error` to `src/parser.rs`**

Find `fn is_nullable_function_type_error` in `src/parser.rs` and insert the new helper **before** it:

```rust
/// Returns true if this ERROR node is a lone `,` inside a `@file:[...]` annotation.
/// tree-sitter-kotlin 0.3 uses `repeat1` without comma separators inside the bracket
/// syntax, so each comma becomes a spurious ERROR node.
fn is_file_annotation_comma_error(node: &Node, bytes: &[u8]) -> bool {
    if !node.is_error() {
        return false;
    }
    if node.utf8_text(bytes).unwrap_or("").trim() != "," {
        return false;
    }
    let start_byte = node.start_byte();
    if start_byte < 5 {
        return false;
    }
    let before = std::str::from_utf8(&bytes[..start_byte]).unwrap_or("");
    if let Some(file_pos) = before.rfind("@file:") {
        let after_file = &before[file_pos + 6..];
        if after_file.trim_start().starts_with('[') {
            return true;
        }
    }
    false
}
```

- [ ] **Step 4: Use the helper in `collect_syntax_errors`**

In `collect_syntax_errors`, find the block that checks `is_chained_call_assignment_error` and `is_nullable_function_type_error`. Add the new check immediately after `is_chained_call_assignment_error`:

```rust
            if is_chained_call_assignment_error(&node, bytes) {
                continue;
            }
            // Skip lone `,` inside @file:[...] bracket syntax (tree-sitter-kotlin 0.3 bug).
            if is_file_annotation_comma_error(&node, bytes) {
                continue;
            }
```

- [ ] **Step 5: Run both tests**

```bash
cargo test --lib no_false_positive_file_annotation 2>&1 | tail -15
```
Expected: both PASS.

- [ ] **Step 6: Run full suite to check no regressions**

```bash
cargo test --lib 2>&1 | tail -5
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/parser.rs src/parser_tests.rs
git commit -m "fix(parser): suppress @file:[...] comma false positive (tree-sitter-kotlin 0.3 bug)"
```

---

## Task 3: `check` subcommand

**Files:**
- Create: `src/cli/check.rs`
- Modify: `src/cli/mod.rs` — add `pub(crate) mod check`
- Modify: `src/cli/args.rs` — add `Check` variant
- Modify: `src/cli/run.rs` — wire the subcommand
- Create: `tests/cli_check.rs`

- [ ] **Step 1: Write integration tests first**

Create `tests/cli_check.rs`:

```rust
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
    assert!(!stderr.is_empty(), "expected error count on stderr: {stderr}");
}

#[test]
fn json_output_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "Foo.kt", "fun greet() = println(\"hi\")\n");
    let file = dir.path().join("Foo.kt");
    let (ok, stdout, _) = check(&["--json", file.to_str().unwrap()]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["files_ok"], 1);
    assert_eq!(v["files_with_errors"], 0);
    assert!(v["errors"].as_array().unwrap().is_empty());
}

#[test]
fn json_output_error_file() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "Bad.kt", "fun broken( {\n");
    let file = dir.path().join("Bad.kt");
    let (ok, stdout, _) = check(&["--json", file.to_str().unwrap()]);
    assert!(!ok);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["files_with_errors"], 1);
    assert!(!v["errors"].as_array().unwrap().is_empty());
}

#[test]
fn directory_arg_walks_kt_files() {
    let dir = tempfile::tempdir().unwrap();
    write_fixture(dir.path(), "src/A.kt", "class A\n");
    write_fixture(dir.path(), "src/B.kt", "class B\n");
    write_fixture(dir.path(), "src/README.md", "# docs\n");
    let (ok, stdout, _) = check(&[dir.path().join("src").to_str().unwrap()]);
    assert!(ok);
    assert!(stdout.contains("OK"));
}

#[test]
fn missing_file_exits_nonzero() {
    let (ok, _, stderr) = check(&["/nonexistent/path/Foo.kt"]);
    assert!(!ok);
    _ = stderr; // warning printed, not assertion
}
```

- [ ] **Step 2: Verify tests fail to compile (subcommand not yet wired)**

```bash
cargo test --test cli_check 2>&1 | head -20
```
Expected: compile error about unknown subcommand `check`.

- [ ] **Step 3: Create `src/cli/check.rs`**

```rust
//! `kmp-lsp check` — syntax validation without an LSP session or index.

use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Serialize)]
struct CheckError {
    file: String,
    line: u32,
    col: u32,
    message: String,
}

pub(crate) fn run_check(files: &[PathBuf], json: bool) {
    use crate::parser::parse_by_extension;

    let mut errors: Vec<CheckError> = Vec::new();
    let mut files_ok: u32 = 0;
    let mut files_err: u32 = 0;

    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(content) => content,
            Err(error) => {
                let message = format!("read error: {error}");
                if !json {
                    eprintln!("{}: {message}", file.display());
                }
                errors.push(CheckError {
                    file: file.to_string_lossy().into_owned(),
                    line: 0,
                    col: 0,
                    message,
                });
                files_err += 1;
                continue;
            }
        };

        let data = parse_by_extension(&file.to_string_lossy(), &content);

        if data.syntax_errors.is_empty() {
            files_ok += 1;
            continue;
        }

        files_err += 1;
        for syntax_error in &data.syntax_errors {
            errors.push(CheckError {
                file: file.to_string_lossy().into_owned(),
                line: syntax_error.range.start.line + 1,
                col: syntax_error.range.start.character + 1,
                message: syntax_error.message.clone(),
            });
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "files_ok": files_ok,
                "files_with_errors": files_err,
                "errors": errors,
            }))
            .expect("serialize JSON")
        );
    } else {
        for error in &errors {
            println!("{}:{}:{}: {}", error.file, error.line, error.col, error.message);
        }
        if errors.is_empty() {
            println!("All {} files OK.", files_ok);
        } else {
            eprintln!("{} error(s) in {} file(s).", errors.len(), files_err);
        }
    }

    if !errors.is_empty() {
        std::process::exit(1);
    }
}

/// Expand a list of file/directory paths to individual source files.
/// Directories are walked recursively; only `.kt`, `.kts`, `.java`, `.swift` are included.
pub(crate) fn expand_file_list(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_file() {
                    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                        if matches!(ext, "kt" | "kts" | "java" | "swift") {
                            result.push(p.to_path_buf());
                        }
                    }
                }
            }
        } else {
            if !path.exists() {
                eprintln!("warning: {}: no such file or directory", path.display());
            }
            result.push(path.clone());
        }
    }
    result
}
```

- [ ] **Step 4: Declare the module in `src/cli/mod.rs`**

Add `pub(crate) mod check;` alongside the other module declarations.

- [ ] **Step 5: Add `Check` variant to `src/cli/args.rs`**

In the `Subcommand` enum, add:

```rust
    /// Syntax-check files with tree-sitter (no index needed). Exit 1 if errors found.
    Check {
        files: Vec<PathBuf>,
    },
```

In `build_subcommand`, add the match arm alongside the others:

```rust
        "check" => Ok(Subcommand::Check {
            files: positionals.iter().map(PathBuf::from).collect(),
        }),
```

In the `is_subcommand` list, add `"check"`.

- [ ] **Step 6: Wire in `src/cli/run.rs`**

In the `match args.subcommand` block, add:

```rust
        Subcommand::Check { files } => {
            if files.is_empty() {
                eprintln!("check requires at least one FILE or DIR argument");
                std::process::exit(1);
            }
            let expanded = super::check::expand_file_list(&files);
            super::check::run_check(&expanded, json);
        }
```

Note: `run` is `async fn`, but `run_check` is sync — the match arm does not need `.await`.

- [ ] **Step 7: Run integration tests**

```bash
cargo test --test cli_check 2>&1 | tail -20
```
Expected: all 6 tests pass.

- [ ] **Step 8: Verify help text includes check**

```bash
cargo run --quiet -- --help 2>&1 | grep check
```
Expected: `check` listed as a subcommand.

- [ ] **Step 9: Commit**

```bash
git add src/cli/check.rs src/cli/mod.rs src/cli/args.rs src/cli/run.rs tests/cli_check.rs
git commit -m "feat(cli): add check subcommand — syntax validation without index"
```

---

## Task 4: Syntax errors in `diagnose`

**Files:**
- Modify: `src/cli/run.rs` — `run_diagnose` function

- [ ] **Step 1: Write a failing integration test**

In `tests/cli_diagnose.rs`, add at the bottom:

```rust
/// diagnose must report tree-sitter syntax errors alongside call-arg diagnostics.
#[test]
fn syntax_error_reported_by_diagnose() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_fixture(root, "workspace.json", r#"{"sourcePaths":[]}"#);
    write_fixture(root, "src/Bad.kt", "class Foo {\n    fun bar() {\n");
    let file = root.join("src/Bad.kt");

    let out = Command::new(BIN)
        .args(["diagnose", "--root"])
        .arg(root)
        .arg(&file)
        .output()
        .expect("failed to spawn kmp-lsp");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|l| l.contains("syntax error") || l.contains("missing")),
        "expected syntax error in diagnose output:\n{stdout}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --test cli_diagnose syntax_error_reported_by_diagnose 2>&1 | tail -15
```
Expected: FAIL — diagnose currently emits "No diagnostics." for syntax-broken files.

- [ ] **Step 3: Extend `run_diagnose` in `src/cli/run.rs`**

In `run_diagnose`, after `index.store_live_tree(&uri, &source)`, add a parse-only syntax check:

```rust
    // Emit syntax errors (tree-sitter, no index required).
    let syntax_data = crate::parser::parse_by_extension(&path_str, &source);
    for syntax_error in &syntax_data.syntax_errors {
        let line = syntax_error.range.start.line + 1;
        let col = syntax_error.range.start.character + 1;
        println!("{}:{} [error]: {}", line, col, syntax_error.message);
    }
```

Place this block **before** the `call_arg_diagnostics` call so syntax errors appear first in output.

- [ ] **Step 4: Run the new test**

```bash
cargo test --test cli_diagnose syntax_error_reported_by_diagnose 2>&1 | tail -10
```
Expected: PASS.

- [ ] **Step 5: Run full diagnose test suite**

```bash
cargo test --test cli_diagnose 2>&1 | tail -5
```
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src/cli/run.rs tests/cli_diagnose.rs
git commit -m "fix(cli): diagnose now reports tree-sitter syntax errors"
```

---

## Task 5: `--exclude-imports` on `refs`

**Files:**
- Modify: `src/cli/args.rs` — add `exclude_imports: bool` to `Refs`
- Modify: `src/cli/run.rs` — add flag parsing and post-filter in `run_refs`

- [ ] **Step 1: Write a failing integration test**

Add to `tests/cli_complete.rs` or create `tests/cli_refs.rs`:

```rust
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
    // File A imports Foo; File B uses Foo as a real reference.
    write_fixture(
        root,
        "src/A.kt",
        "import com.example.Foo\n\nfun useIt(f: Foo) {}\n",
    );
    write_fixture(root, "src/B.kt", "class Foo\n");

    let out_with = Command::new(BIN)
        .args(["refs", "Foo", "--root"])
        .arg(root)
        .output()
        .expect("spawn");
    let with_imports = String::from_utf8_lossy(&out_with.stdout);

    let out_without = Command::new(BIN)
        .args(["refs", "Foo", "--exclude-imports", "--root"])
        .arg(root)
        .output()
        .expect("spawn");
    let without_imports = String::from_utf8_lossy(&out_without.stdout);

    // Default output includes import lines.
    assert!(
        with_imports.contains("import"),
        "expected import line in default refs output:\n{with_imports}"
    );
    // With --exclude-imports, the import line is gone.
    assert!(
        !without_imports.contains("import"),
        "expected no import lines with --exclude-imports:\n{without_imports}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --test cli_refs exclude_imports 2>&1 | tail -15
```
Expected: compile error or FAIL — `--exclude-imports` not recognised.

- [ ] **Step 3: Add field to `Refs` variant in `src/cli/args.rs`**

Change:
```rust
    Refs {
        name: String,
    },
```
to:
```rust
    Refs {
        name: String,
        /// Strip import-statement matches from results.
        exclude_imports: bool,
    },
```

In `build_subcommand` for `"refs"`:
```rust
        "refs" => Ok(Subcommand::Refs {
            name: positionals.into_iter().next().ok_or("refs requires a NAME argument")?,
            exclude_imports: parsed.exclude_imports,
        }),
```

In the `ParsedCliFlags` struct, add:
```rust
    pub exclude_imports: bool,
```

In the flag-parsing loop, add a case for `"exclude-imports"`:
```rust
                "exclude-imports" => parsed.exclude_imports = true,
```

- [ ] **Step 4: Thread the flag through `run_refs` in `src/cli/run.rs`**

Change the `Subcommand::Refs` match arm to:
```rust
        Subcommand::Refs { name, exclude_imports } => {
            let root = resolve_root(args.root.as_deref());
            run_refs(&root, args.mode, json, verbose, &name, exclude_imports).await
        }
```

Change `run_refs` signature and body:
```rust
async fn run_refs(root: &Path, mode: Mode, json: bool, verbose: bool, name: &str, exclude_imports: bool) {
    let mut results = match effective_mode(mode, root, "refs", verbose) {
        Mode::Fast => fast_refs(name, root),
        _ => {
            let index = build_index(root, false).await;
            smart_refs(&index, name, root)
        }
    };

    if exclude_imports {
        results.retain(|result| {
            // Smart-mode results may carry kind="import" directly.
            if result.kind == "import" {
                return false;
            }
            // For rg-based results, read the line from disk and check the prefix.
            if !result.kind.is_empty() {
                return true;
            }
            std::fs::read_to_string(&result.file)
                .ok()
                .and_then(|source| {
                    source
                        .lines()
                        .nth(result.line.saturating_sub(1) as usize)
                        .map(|line| !line.trim_start().starts_with("import "))
                })
                .unwrap_or(true)
        });
    }

    exit_if_empty(&results, json, &format!("No references found for '{name}'"));
    print_results(&results, json);
}
```

- [ ] **Step 5: Run the test**

```bash
cargo test --test cli_refs exclude_imports 2>&1 | tail -15
```
Expected: PASS.

- [ ] **Step 6: Run full suite**

```bash
cargo test 2>&1 | tail -10
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/cli/args.rs src/cli/run.rs tests/cli_refs.rs
git commit -m "feat(cli): add --exclude-imports flag to refs — filters import-statement matches"
```

---

## Final Verification

- [ ] **Run full test suite**

```bash
cargo test 2>&1 | tail -10
```
Expected: all pass, no failures.

- [ ] **Run clippy**

```bash
cargo clippy -- -D warnings 2>&1 | tail -10
```
Expected: clean.

- [ ] **Smoke-test new commands manually**

```bash
# check on a real file
kmp-lsp check src/parser.rs

# check with JSON
kmp-lsp check --json src/parser.rs

# refs with exclude-imports
kmp-lsp refs --exclude-imports Indexer --root .
```

---

## Self-Review

**Spec coverage:**
- `rfind` fix ✅ Task 1
- `@file:[...]` comma fix ✅ Task 2
- `check` subcommand (no index, multi-file, JSON, exit code 1) ✅ Task 3
- Syntax errors in `diagnose` ✅ Task 4
- `--exclude-imports` on refs ✅ Task 5

**Potential issues caught:**
- `run` in `src/cli/run.rs` is `async` but `run_check` is sync — the match arm must not `.await` it. Noted in Task 3 Step 6.
- `walkdir` must be in `Cargo.toml`. Check with `grep walkdir Cargo.toml`. If missing, add `walkdir = "2"` to `[dependencies]`.
- `result.line.saturating_sub(1)` in `retain` — `line` is 1-based, `nth` is 0-based.
- The `ParsedCliFlags` struct location in `args.rs` must be verified before adding `exclude_imports`; search for `struct ParsedCliFlags`.
