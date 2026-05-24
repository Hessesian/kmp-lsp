//! CLI `organize-imports` subcommand — sort, dedup, remove unused imports.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Serialize)]
struct ImportChange {
    file: String,
    removed: Vec<String>,
    added: Vec<String>,
    sorted: Vec<String>,
}

pub(crate) fn run_organize_imports(files: &[PathBuf], json: bool) {
    use crate::parser::parse_imports_from_lines;

    let mut changes: Vec<ImportChange> = Vec::new();
    let mut had_changes = false;

    for file in files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: read error: {e}", file.display());
                continue;
            }
        };

        let lines: Vec<String> = content.lines().map(|s| s.to_owned()).collect();
        let imports = parse_imports_from_lines(&lines);

        if imports.is_empty() {
            if json {
                changes.push(ImportChange {
                    file: file.to_string_lossy().into_owned(),
                    removed: vec![],
                    added: vec![],
                    sorted: vec![],
                });
            }
            continue;
        }

        let mut used_idents: HashSet<String> = HashSet::new();
        for line in &lines {
            let trimmed = line.trim_start();
            if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
                continue;
            }
            for word in extract_idents(line) {
                used_idents.insert(word);
            }
        }

        let mut used_imports: Vec<String> = Vec::new();
        let mut removed: Vec<String> = Vec::new();
        for imp in &imports {
            if imp.is_star {
                used_imports.push(format!("import {}.{}", imp.full_path, imp.local_name));
            } else if used_idents.contains(&imp.local_name) {
                if imp.local_name != imp.full_path.rsplit('.').next().unwrap_or(&imp.full_path) {
                    used_imports.push(format!("import {} as {}", imp.full_path, imp.local_name));
                } else {
                    used_imports.push(format!("import {}", imp.full_path));
                }
            } else {
                removed.push(format!("import {}", imp.full_path));
            }
        }

        used_imports.sort();

        let original_imports: Vec<String> = get_original_import_lines(&lines);
        let file_changed = used_imports != original_imports;
        if file_changed {
            had_changes = true;
        }

        if json {
            changes.push(ImportChange {
                file: file.to_string_lossy().into_owned(),
                removed: removed.clone(),
                added: vec![],
                sorted: used_imports.clone(),
            });
        } else if file_changed {
            println!("--- {}", file.display());
            if !removed.is_empty() {
                for r in &removed {
                    println!("- {r}");
                }
            }
            for imp in &used_imports {
                println!("  {imp}");
            }
            println!();
        }
    }

    if json {
        let output = serde_json::json!({
            "changed": had_changes,
            "files": changes,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).expect("serialize JSON")
        );
    } else if !had_changes {
        println!("All imports are already organized.");
    }
}

fn extract_idents(line: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    for c in line.chars() {
        if c.is_alphanumeric() || c == '_' {
            current.push(c);
        } else {
            if !current.is_empty()
                && current
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_')
            {
                result.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if !current.is_empty()
        && current
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
    {
        result.push(current);
    }
    result
}

fn get_original_import_lines(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter(|l| l.trim_start().starts_with("import "))
        .map(|l| l.trim().to_owned())
        .collect()
}
