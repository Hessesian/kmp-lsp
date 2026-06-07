//! CLI `insert` subcommand — insert code at a specific line.
//!
//! Inserts text before or after a given line number in a file.
//! Supports --in-place to write back instead of printing to stdout.

use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Serialize)]
struct InsertResult {
    file: String,
    inserted_at: u32,
    lines_before: usize,
    lines_after: usize,
}

#[allow(clippy::unused_enumerate_index)]
pub(crate) fn run_insert(
    file: &PathBuf,
    line: u32,
    before: bool,
    after: bool,
    content: &str,
    in_place: bool,
) {
    if before == after {
        eprintln!("insert requires exactly one of --before or --after");
        std::process::exit(1);
    }

    let original = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: read error: {e}", file.display());
            std::process::exit(1);
        }
    };

    let lines: Vec<&str> = original.lines().collect();
    let insert_at = if after {
        line as usize
    } else {
        (line as usize).saturating_sub(1)
    }
    .min(lines.len());

    let indent = lines
        .get(insert_at.saturating_sub(1))
        .map(|l| {
            l.chars()
                .take_while(|c| c.is_whitespace())
                .collect::<String>()
        })
        .unwrap_or_default();

    let content_lines: Vec<&str> = content.split('\n').collect();
    let insert_lines: Vec<String> = content_lines
        .iter()
        .map(|c| {
            if c.is_empty() {
                String::new()
            } else {
                format!("{indent}{c}")
            }
        })
        .collect();

    let mut result: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    for (offset, inserted) in insert_lines.iter().enumerate() {
        result.insert(insert_at + offset, inserted.clone());
    }

    let mut new_content = result.join("\n");
    if original.ends_with('\n') {
        new_content.push('\n');
    }

    if in_place {
        if let Err(e) = std::fs::write(file, &new_content) {
            eprintln!("{}: write error: {e}", file.display());
            std::process::exit(1);
        }
        let info = InsertResult {
            file: file.to_string_lossy().into_owned(),
            inserted_at: line,
            lines_before: lines.len(),
            lines_after: result.len(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&info).unwrap_or_default()
        );
    } else {
        println!("{new_content}");
    }
}
