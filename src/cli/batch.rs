//! CLI `batch` subcommand — cross-file atomic modifications.
//!
//! Reads a JSON rule file and applies find-replace and insert operations
//! across multiple files. Designed for KMP refactoring where VM + binding
//! + caller + Koin module must change together.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct BatchRule {
    files: std::collections::HashMap<String, Vec<FileAction>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum FileAction {
    #[serde(rename = "replace")]
    Replace { old: String, new: String },
    #[serde(rename = "insert")]
    Insert {
        after_line: Option<u32>,
        before_line: Option<u32>,
        content: String,
    },
}

#[allow(clippy::unused_enumerate_index)]
pub(crate) fn run_batch(rule_file: &PathBuf, dry_run: bool) {
    let json = match std::fs::read_to_string(rule_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: read error: {e}", rule_file.display());
            std::process::exit(1);
        }
    };

    let rule: BatchRule = match serde_json::from_str(&json) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Invalid rule JSON: {e}");
            std::process::exit(1);
        }
    };

    let mut total_replaces = 0u32;
    let mut total_inserts = 0u32;
    let mut files_modified = 0u32;

    for (file_path, actions) in &rule.files {
        let original = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{}: read error: {e}", file_path);
                continue;
            }
        };

        let mut modified = original.clone();
        let mut file_changed = false;

        for action in actions {
            match action {
                FileAction::Replace { old, new } => {
                    if modified.contains(old.as_str()) {
                        modified = modified.replace(old.as_str(), new.as_str());
                        total_replaces += 1;
                        file_changed = true;
                    }
                }
                FileAction::Insert {
                    after_line,
                    before_line,
                    content,
                } => {
                    let lines: Vec<&str> = modified.lines().collect();
                    let insert_at = if let Some(al) = after_line {
                        (*al as usize).min(lines.len())
                    } else if let Some(bl) = before_line {
                        (bl.saturating_sub(1) as usize).min(lines.len())
                    } else {
                        0
                    };

                    let indent = lines
                        .get(insert_at.saturating_sub(1))
                        .map(|l| {
                            l.chars()
                                .take_while(|c| c.is_whitespace())
                                .collect::<String>()
                        })
                        .unwrap_or_default();

                    let inserted: Vec<String> = content
                        .split('\n')
                        .map(|c| {
                            if c.is_empty() {
                                String::new()
                            } else {
                                format!("{indent}{c}")
                            }
                        })
                        .collect();

                    let mut new_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
                    for _ in 0..inserted.len() {
                        new_lines.insert(insert_at, String::new());
                    }
                    for (j, ins) in inserted.iter().enumerate() {
                        new_lines[insert_at + j] = ins.clone();
                    }

                    modified = new_lines.join("\n");
                    total_inserts += 1;
                    file_changed = true;
                }
            }
        }

        if file_changed {
            if dry_run {
                println!("--- {} (dry-run) ---", file_path);
                // Show diff
                for (i, (o, m)) in original.lines().zip(modified.lines()).enumerate() {
                    if o != m {
                        println!("  L{}: -{}", i + 1, o);
                        println!("  L{}: +{}", i + 1, m);
                    }
                }
            } else {
                if let Err(e) = std::fs::write(file_path, &modified) {
                    eprintln!("{}: write error: {e}", file_path);
                } else {
                    files_modified += 1;
                }
            }
        }
    }

    if dry_run {
        println!(
            "dry-run: {} files, {} replaces, {} inserts",
            files_modified, total_replaces, total_inserts
        );
    } else {
        println!(
            "done: {} files, {} replaces, {} inserts",
            files_modified, total_replaces, total_inserts
        );
    }
}
