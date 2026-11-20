//! Ambiguity ratchet: a resolution result says whether it is unique,
//! ambiguous, or absent. It does not leave the caller to guess from a list's
//! length or an empty `Option`.
//!
//! `Resolution<T>` (`src/indexer/infer/mod.rs`) has `Resolved` / `Ambiguous` /
//! `Unresolved`. Most of the string-path resolver still returns a bare
//! `Vec<Location>`, where "not found" is an empty list and "ambiguous" is a
//! list with more than one entry, by convention only. Each caller then decides
//! for itself what two entries mean. The `.resolved()` bridges go the other
//! way: they fold `Ambiguous` (or `ShapeFiltered::RuledOut`) back into an empty
//! result for callers that do not handle ambiguity yet.
//!
//! Each entry lists the files that held the pattern when the ratchet was
//! introduced, and how many times. A count only goes down: when you migrate a
//! function or a caller, lower its number (or drop the file) in the same
//! change. Never raise one or add a file. Return `Resolution<T>` (or a named
//! candidate type) instead, and match `Ambiguous` at the caller.
//!
//! Background and migration order: the "string-path cardinality" section of
//! `docs/architecture/unified-resolution-strategy.md`.
//!
//! The scan is literal text outside `//` comment lines. A signature split so
//! the arrow and the return type sit on different lines escapes it, and a
//! removal can hide an addition inside the same file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Literal text that hides ambiguity, the roots it is counted under, and the
/// files that still contain it.
struct Pattern {
    text: &'static str,
    roots: &'static [&'static str],
    /// `(file, occurrences)` for every file allowed to contain `text`.
    allowed: &'static [(&'static str, usize)],
}

const PATTERNS: &[Pattern] = &[
    Pattern {
        text: ".resolved()",
        roots: &["src"],
        allowed: &[
            ("src/features/definition.rs", 2),
            ("src/features/fill_when.rs", 1),
            ("src/features/hover.rs", 1),
            ("src/resolver/identity.rs", 1),
            ("src/semantic_tokens/resolve.rs", 1),
        ],
    },
    Pattern {
        text: "-> Vec<Location>",
        roots: &["src/resolver", "src/indexer", "src/indexer.rs"],
        allowed: &[
            ("src/indexer.rs", 3),
            ("src/indexer/lookup.rs", 1),
            ("src/indexer/resolution.rs", 3),
            ("src/resolver/container.rs", 1),
            ("src/resolver/extension.rs", 3),
            ("src/resolver/fd.rs", 4),
            ("src/resolver/find.rs", 6),
            ("src/resolver/imports.rs", 1),
            ("src/resolver/index_read.rs", 1),
            ("src/resolver/indexer_queries.rs", 9),
            ("src/resolver/infer.rs", 2),
            ("src/resolver/package.rs", 1),
            ("src/resolver/package_scope.rs", 3),
            ("src/resolver/platform_types.rs", 1),
            ("src/resolver/qualified.rs", 10),
            ("src/resolver/resolve.rs", 16),
            ("src/resolver/tie_break.rs", 3),
        ],
    },
];

struct SourceFile {
    /// Path from the crate root, `/`-separated on every platform.
    relative_path: String,
    text: String,
}

fn is_production_source(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    file_name.ends_with(".rs") && !file_name.ends_with("tests.rs")
}

fn relative_path(crate_root: &Path, path: &Path) -> String {
    let relative = path
        .strip_prefix(crate_root)
        .expect("path is under the crate root");
    let segments: Vec<_> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect();
    segments.join("/")
}

fn sources_under(roots: &[&str]) -> Vec<SourceFile> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut pending: Vec<PathBuf> = Vec::new();
    for root in roots {
        let root_path = crate_root.join(root);
        assert!(
            root_path.exists(),
            "root `{root}` no longer exists — update PATTERNS so it stays scanned"
        );
        pending.push(root_path);
    }
    let mut sources = Vec::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            let entries = std::fs::read_dir(&path).expect("source directory is readable");
            pending.extend(entries.map(|entry| entry.expect("directory entry").path()));
        } else if is_production_source(&path) {
            let text = std::fs::read_to_string(&path).expect("source file is readable");
            sources.push(SourceFile {
                relative_path: relative_path(crate_root, &path),
                text,
            });
        }
    }
    sources
}

/// The 1-based lines of `source` containing `text`, once per occurrence,
/// outside `//` comment lines.
fn lines_containing(source: &SourceFile, text: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    for (line_index, line) in source.text.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        lines.extend(std::iter::repeat_n(
            line_index + 1,
            line.matches(text).count(),
        ));
    }
    lines
}

/// One message per file whose occurrences of `pattern.text` differ from its
/// allowed count.
fn drift_of(pattern: &Pattern) -> Vec<String> {
    let mut allowed: BTreeMap<&str, usize> = pattern.allowed.iter().copied().collect();
    let mut drift = Vec::new();
    for source in sources_under(pattern.roots) {
        let lines = lines_containing(&source, pattern.text);
        let baseline = allowed.remove(source.relative_path.as_str()).unwrap_or(0);
        if lines.len() != baseline {
            drift.push(format!(
                "`{}` in {}: baseline {baseline}, found {} (lines {lines:?})",
                pattern.text,
                source.relative_path,
                lines.len()
            ));
        }
    }
    for (file, baseline) in allowed {
        drift.push(format!(
            "`{}` in {file}: baseline {baseline}, but the file was not scanned",
            pattern.text
        ));
    }
    drift
}

#[test]
fn resolution_gains_no_new_implicit_ambiguity() {
    let drift: Vec<String> = PATTERNS.iter().flat_map(drift_of).collect();
    assert!(
        drift.is_empty(),
        "resolution code drifted from the baselines in tests/ambiguity_ratchet.rs.\n\
         More than baseline: return `Resolution<T>` and match `Ambiguous` at the caller.\n\
         Fewer than baseline: lower the baseline to the new count.\n\n{}",
        drift.join("\n")
    );
}
