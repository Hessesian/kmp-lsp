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
            println!(
                "{}:{}:{}: {}",
                error.file, error.line, error.col, error.message
            );
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

/// Expand file/directory paths to individual source files.
/// Directories are walked recursively; only `.kt`, `.kts`, `.java`, `.swift` are included.
pub(crate) fn expand_file_list(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        if path.is_dir() {
            for entry in walkdir::WalkDir::new(path)
                .into_iter()
                .filter_map(|entry| entry.ok())
            {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
                        if matches!(ext, "kt" | "kts" | "java" | "swift") {
                            result.push(path.to_path_buf());
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
