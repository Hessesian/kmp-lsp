//! CLI `inject` subcommand — batch type injection for files.
//!
//! Reads a Kotlin/Java/Swift file, extracts all referenced type names,
//! resolves their signatures, and returns them as a single batch.
//! Designed as an AI-agent Read Hook.

use std::path::Path;

use serde::Serialize;

#[derive(Debug, Serialize)]
struct InjectEntry {
    name: String,
    signature: String,
    line: u32,
}

/// Run the `inject` subcommand.
pub(crate) async fn run_inject(file: &Path, root: &Path, json: bool, limit: usize) {
    use crate::indexer::{Indexer, NoopReporter};
    use crate::parser::parse_by_extension;
    use std::sync::Arc;

    // Build index
    let idx = {
        let idx = Arc::new(Indexer::new());
        Arc::clone(&idx)
            .index_workspace_full(root, Arc::new(NoopReporter))
            .await;
        idx
    };

    let content = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: read error: {e}", file.display());
            std::process::exit(1);
        }
    };

    let uri = match tower_lsp::lsp_types::Url::from_file_path(file) {
        Ok(u) => u,
        Err(_) => {
            eprintln!("Invalid file path: {}", file.display());
            std::process::exit(1);
        }
    };

    // Collect unique type names referenced in the file
    let mut type_names: Vec<(String, u32)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") || trimmed.starts_with("package ") {
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        // Extract uppercase identifiers
        let mut word = String::new();
        for ch in line.chars() {
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
            } else {
                if !word.is_empty()
                    && word.chars().next().map_or(false, |c| c.is_uppercase())
                    && !is_keyword(&word)
                    && seen.insert(word.clone())
                {
                    type_names.push((word.clone(), line_no as u32 + 1));
                }
                word.clear();
            }
        }
        // Check last word on line
        if !word.is_empty()
            && word.chars().next().map_or(false, |c| c.is_uppercase())
            && !is_keyword(&word)
            && seen.insert(word.clone())
        {
            type_names.push((word, line_no as u32 + 1));
        }
    }

    if type_names.len() > limit {
        type_names.truncate(limit);
    }

    // Resolve each type
    let mut entries: Vec<InjectEntry> = Vec::new();
    for (name, line) in &type_names {
        let sig = crate::indexer::resolution::resolve_symbol_info(
            idx.as_ref(),
            name,
            None,
            &uri,
            crate::indexer::resolution::SubstitutionContext::None,
            &crate::indexer::resolution::ResolveOptions::hover(),
        )
        .map(|s| s.signature)
        .unwrap_or_default();
        entries.push(InjectEntry {
            name: name.clone(),
            signature: sig,
            line: *line,
        });
    }

    if json {
        let output = serde_json::json!({
            "file": file.to_string_lossy(),
            "count": entries.len(),
            "types": entries,
        });
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    } else {
        for entry in &entries {
            if entry.signature.is_empty() {
                println!("{}:{}: <unknown>", entry.line, entry.name);
            } else {
                println!("{}:{}: {}", entry.line, entry.name, entry.signature);
            }
        }
    }
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "fun"
            | "val"
            | "var"
            | "class"
            | "interface"
            | "object"
            | "enum"
            | "import"
            | "package"
            | "return"
            | "if"
            | "else"
            | "when"
            | "for"
            | "while"
            | "do"
            | "try"
            | "catch"
            | "finally"
            | "throw"
            | "override"
            | "private"
            | "protected"
            | "internal"
            | "public"
            | "abstract"
            | "open"
            | "final"
            | "sealed"
            | "data"
            | "inline"
            | "suspend"
            | "operator"
            | "infix"
            | "tailrec"
            | "external"
            | "const"
            | "lateinit"
            | "companion"
            | "init"
            | "constructor"
            | "this"
            | "super"
            | "true"
            | "false"
            | "null"
            | "is"
            | "as"
            | "in"
            | "where"
            | "by"
            | "get"
            | "set"
            | "out"
            | "reified"
            | "crossinline"
            | "noinline"
            | "expect"
            | "actual"
            | "typealias"
            | "annotation"
            | "String"
            | "Int"
            | "Long"
            | "Float"
            | "Double"
            | "Boolean"
            | "Byte"
            | "Short"
            | "Char"
            | "Unit"
            | "Any"
            | "Nothing"
    )
}
