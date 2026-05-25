//! Result types and output formatting for CLI.

use serde::Serialize;
use tower_lsp::lsp_types::Location;

/// A single CLI result entry.
#[derive(Debug, Serialize)]
pub(crate) struct CliResult {
    pub file: String,
    pub line: u32,
    pub col: u32,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub kind: String,
    pub name: String,
}

impl CliResult {
    pub(crate) fn from_location(loc: &Location, name: &str, kind: &str) -> Option<Self> {
        // Regular file:// URI — extract the local path.
        if let Ok(file) = loc.uri.to_file_path() {
            return Some(Self {
                file: file.to_string_lossy().into_owned(),
                line: loc.range.start.line + 1,
                col: loc.range.start.character + 1,
                kind: kind.to_owned(),
                name: name.to_owned(),
            });
        }
        // jar:file:// URI — show the JAR path as a pseudo-location so library
        // symbols are visible in CLI output rather than silently dropped.
        let uri_str = loc.uri.as_str();
        if let Some(jar_path) = uri_str.strip_prefix("jar:file://") {
            return Some(Self {
                file: format!("jar:{jar_path}"),
                line: loc.range.start.line + 1,
                col: 1,
                kind: kind.to_owned(),
                name: name.to_owned(),
            });
        }
        None
    }
}

pub(crate) fn print_results(results: &[CliResult], json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(results).unwrap_or_default()
        );
    } else {
        for r in results {
            if r.kind.is_empty() {
                println!("{}:{}:{}: {}", r.file, r.line, r.col, r.name);
            } else {
                println!("{}:{}:{}: {} {}", r.file, r.line, r.col, r.kind, r.name);
            }
        }
    }
}
