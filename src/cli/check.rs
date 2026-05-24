//! CLI `check` subcommand — syntax error diagnostics without an LSP session.

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
    let mut files_ok = 0u32;
    let mut files_err = 0u32;

    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => {
                if !json {
                    eprintln!("{}: read error: {e}", file.display());
                }
                errors.push(CheckError {
                    file: file.to_string_lossy().into_owned(),
                    line: 0,
                    col: 0,
                    message: format!("read error: {e}"),
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
        for se in &data.syntax_errors {
            errors.push(CheckError {
                file: file.to_string_lossy().into_owned(),
                line: se.range.start.line + 1,
                col: se.range.start.character + 1,
                message: se.message.clone(),
            });
        }
    }

    if json {
        let output = serde_json::json!({
            "files_ok": files_ok,
            "files_with_errors": files_err,
            "errors": errors,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        for e in &errors {
            println!("{}:{}:{}: {}", e.file, e.line, e.col, e.message);
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

pub(crate) fn expand_file_list(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        if path.is_dir() {
            let walker = walkdir::WalkDir::new(path)
                .into_iter()
                .filter_map(|e| e.ok());
            for entry in walker {
                let p = entry.path();
                if p.is_file() {
                    if let Some(ext) = p.extension() {
                        if matches!(ext.to_str(), Some("kt" | "kts" | "java" | "swift")) {
                            result.push(p.to_path_buf());
                        }
                    }
                }
            }
        } else if path.is_file() {
            result.push(path.clone());
        } else {
            eprintln!("warning: {}: no such file or directory", path.display());
        }
    }
    result
}
