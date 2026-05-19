//! Result types and output formatting for CLI.

use std::path::Path;

use serde::Serialize;
use tower_lsp::lsp_types::Location;

use super::path_meta;

/// A single CLI result entry.
///
/// Optional fields (`relative_path`, `module`, `source_set`, `signature`) are
/// omitted from JSON output when absent. `kind` is omitted only when empty so
/// callers that already populate it (e.g. semantic-aware paths) don't lose the
/// information.
#[derive(Debug, Serialize)]
pub(crate) struct CliResult {
    pub file: String,
    pub line: u32,
    pub col: u32,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub kind: String,
    pub name: String,
    #[serde(rename = "relativePath", skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(rename = "sourceSet", skip_serializing_if = "Option::is_none")]
    pub source_set: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl CliResult {
    pub(crate) fn from_location(loc: &Location, name: &str, kind: &str) -> Option<Self> {
        let file = loc.uri.to_file_path().ok()?;
        let file_str = file.to_string_lossy().into_owned();
        let source_set = path_meta::source_set(&file);
        Some(Self {
            file: file_str,
            line: loc.range.start.line + 1,
            col: loc.range.start.character + 1,
            kind: kind.to_owned(),
            name: name.to_owned(),
            relative_path: None,
            module: None,
            source_set,
            signature: None,
        })
    }

    /// Populate `module` and `relative_path` against `root`. `source_set` is
    /// already filled during construction because it doesn't need the root.
    pub(crate) fn enrich_with_root(&mut self, root: &Path) {
        let path = Path::new(&self.file);
        self.module = path_meta::module(path, root);
        self.relative_path = Some(path_meta::relative_path(path, root));
    }
}

/// Print options for `print_results`. Avoids ballooning the signature of every
/// caller as the option set grows.
pub(crate) struct PrintOpts {
    pub json: bool,
    /// When true, plain-text output uses `relative_path` (if available) in
    /// place of the absolute path. JSON output always carries both.
    pub relative: bool,
}

pub(crate) fn print_results(results: &[CliResult], opts: &PrintOpts) {
    if opts.json {
        // Compact JSON by default — this CLI is consumed by AI agents, where
        // pretty-printed whitespace is pure token tax. Pipe to `jq` if a human
        // needs to read it.
        println!("{}", serde_json::to_string(results).unwrap_or_default());
        return;
    }
    for r in results {
        let path = if opts.relative {
            r.relative_path.as_deref().unwrap_or(&r.file)
        } else {
            &r.file
        };
        if r.kind.is_empty() {
            println!("{}:{}:{}: {}", path, r.line, r.col, r.name);
        } else {
            println!("{}:{}:{}: {} {}", path, r.line, r.col, r.kind, r.name);
        }
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
