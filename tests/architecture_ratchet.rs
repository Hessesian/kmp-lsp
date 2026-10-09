//! Architecture ratchet: feature code asks the resolution catalogue; it does
//! not reach into resolver internals or raw index maps.
//!
//! Each baseline is the number of such reaches that existed when the ratchet
//! was introduced. A baseline only goes down: remove a bypass and lower its
//! number in the same change. Never raise one — ask `CstQuery`
//! (`src/indexer/infer/mod.rs`) or the `Resolver` trait
//! (`src/resolver/api.rs`) instead, and extend them when they cannot answer.
//!
//! The scan is literal text, so a reach spelled through a re-export or split
//! across lines escapes it. Once `resolver::infer::` reaches zero, make that
//! module private and let the compiler hold the line instead.

use std::path::{Path, PathBuf};

const FEATURE_ROOTS: &[&str] = &["src/features", "src/semantic_tokens", "src/inlay_hints.rs"];

/// `(pattern, baseline)`: literal text and how many times feature code may contain it.
const BYPASSES: &[(&str, usize)] = &[
    ("resolver::infer::", 7),
    ("resolver::infer_lines::", 0),
    ("infer_variable_type", 2),
    ("infer_type_in_lines", 0),
    (".definitions.", 0),
    (".jar_definitions.", 0),
    (".files.", 2),
    (".jar_files.", 2),
    (".live_lines", 2),
    (".extension_by_receiver", 1),
];

struct SourceFile {
    path: PathBuf,
    text: String,
}

fn is_production_source(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    file_name.ends_with(".rs") && !file_name.ends_with("tests.rs")
}

fn feature_sources() -> Vec<SourceFile> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut pending: Vec<PathBuf> = FEATURE_ROOTS
        .iter()
        .map(|root| crate_root.join(root))
        .collect();
    let mut sources = Vec::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            let entries = std::fs::read_dir(&path).expect("feature directory is readable");
            pending.extend(entries.map(|entry| entry.expect("directory entry").path()));
        } else if is_production_source(&path) {
            let text = std::fs::read_to_string(&path).expect("feature source is readable");
            sources.push(SourceFile { path, text });
        }
    }
    sources
}

/// One `path:line` entry per occurrence of `pattern` outside `//` comment lines.
fn sites_containing(sources: &[SourceFile], pattern: &str) -> Vec<String> {
    let mut sites = Vec::new();
    for source in sources {
        for (line_index, line) in source.text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let site = format!("{}:{}", source.path.display(), line_index + 1);
            sites.extend(std::iter::repeat_n(site, line.matches(pattern).count()));
        }
    }
    sites.sort();
    sites
}

#[test]
fn feature_code_gains_no_new_resolver_bypasses() {
    let sources = feature_sources();
    assert!(!sources.is_empty(), "no feature sources found to scan");

    let mut drift = Vec::new();
    for &(pattern, baseline) in BYPASSES {
        let sites = sites_containing(&sources, pattern);
        if sites.len() != baseline {
            drift.push(format!(
                "`{pattern}`: baseline {baseline}, found {}.\n  {}",
                sites.len(),
                sites.join("\n  ")
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "feature code drifted from the bypass baselines in tests/architecture_ratchet.rs.\n\
         More than baseline: ask `CstQuery` or the `Resolver` trait instead of reaching in.\n\
         Fewer than baseline: lower the baseline to the new count.\n\n{}",
        drift.join("\n\n")
    );
}
