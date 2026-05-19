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
#[derive(Debug, Clone, Serialize)]
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
    /// When true, emit the legacy grep-style `path:line:col: name` format
    /// (one line per match, full path repeated). The default is grouped
    /// output (`rg --heading` style) which is cheaper when multiple matches
    /// share a file.
    pub flat: bool,
}

pub(crate) fn print_results(results: &[CliResult], opts: &PrintOpts) {
    if opts.json {
        // Compact JSON by default — this CLI is consumed by AI agents, where
        // pretty-printed whitespace is pure token tax. Pipe to `jq` if a human
        // needs to read it.
        //
        // When `relative` is set we collapse `file`+`relativePath` into a single
        // `file` field holding the relative path. Both fields carrying the same
        // information was the single biggest byte source in JSON output.
        if opts.relative {
            let projected: Vec<CliResult> = results
                .iter()
                .map(|r| project_relative(r.clone()))
                .collect();
            println!("{}", serde_json::to_string(&projected).unwrap_or_default());
        } else {
            println!("{}", serde_json::to_string(results).unwrap_or_default());
        }
        return;
    }
    let text = if opts.flat {
        format_flat(results, opts.relative)
    } else {
        format_grouped(results, opts.relative)
    };
    print!("{text}");
}

/// Legacy one-line-per-match: `<path>:<line>:<col>: [<kind>] <name>`. Pass-through
/// to grep / `cut -d: -f1` style pipelines.
pub(crate) fn format_flat(results: &[CliResult], relative: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for r in results {
        let path = path_for(r, relative);
        if r.kind.is_empty() {
            let _ = writeln!(out, "{}:{}:{}: {}", path, r.line, r.col, r.name);
        } else {
            let _ = writeln!(out, "{}:{}:{}: {} {}", path, r.line, r.col, r.kind, r.name);
        }
    }
    out
}

/// Default grouped layout: each file's path on its own line, followed by one
/// `<line>:<col>[ <kind>]` per match. Blank line between file groups.
///
/// `name` is omitted entirely — `find <NAME>` / `refs <NAME>` already pins
/// the query, so repeating it per row is pure token waste. `kind` is
/// included only when non-empty so smart-mode results disambiguate
/// `class Foo` vs `fun Foo`. Pass `--flat` to restore the grep-style format.
pub(crate) fn format_grouped(results: &[CliResult], relative: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let mut current: Option<&str> = None;
    for r in results {
        let path = path_for(r, relative);
        if current != Some(path) {
            if current.is_some() {
                out.push('\n');
            }
            let _ = writeln!(out, "{path}");
            current = Some(path);
        }
        if r.kind.is_empty() {
            let _ = writeln!(out, "{}:{}", r.line, r.col);
        } else {
            let _ = writeln!(out, "{}:{} {}", r.line, r.col, r.kind);
        }
    }
    out
}

fn path_for(r: &CliResult, relative: bool) -> &str {
    if relative {
        r.relative_path.as_deref().unwrap_or(&r.file)
    } else {
        &r.file
    }
}

/// Replace `file` with the relative path and drop `relative_path` to remove the
/// duplicate-field bloat from `--json --relative` output. Falls back to the
/// absolute path if no relative path was computed (shouldn't happen after
/// `enrich_with_root`, but be defensive).
fn project_relative(mut r: CliResult) -> CliResult {
    if let Some(rp) = r.relative_path.take() {
        r.file = rp;
    }
    r
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
