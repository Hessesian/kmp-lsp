//! Architecture ratchet: feature code asks the resolution catalogue; it does
//! not reach into resolver internals or raw index maps.
//!
//! Each entry lists the files that reached past the catalogue when the
//! ratchet was introduced, and how many times. A count only goes down: remove
//! a bypass and lower its number (or drop the file) in the same change. Never
//! raise one or add a file — ask `CstQuery` (`src/indexer/infer/mod.rs`) or
//! the `Resolver` trait (`src/resolver/api.rs`) instead, and extend them when
//! they cannot answer.
//!
//! The scan is literal text, so a reach spelled through a re-export or split
//! across lines escapes it, and a removal can hide an addition inside the
//! same file. Once `resolver::infer::` reaches zero, make that module private
//! and let the compiler hold the line instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FEATURE_ROOTS: &[&str] = &["src/features", "src/semantic_tokens", "src/inlay_hints.rs"];

/// Literal text feature code should not contain, and the files that still do.
struct Bypass {
    pattern: &'static str,
    /// `(file, occurrences)` for every file allowed to contain `pattern`.
    allowed: &'static [(&'static str, usize)],
}

const BYPASSES: &[Bypass] = &[
    Bypass {
        pattern: "resolver::infer::",
        allowed: &[
            ("src/features/fill_when.rs", 4),
            ("src/features/hover.rs", 1),
            ("src/features/nullable_call_diagnostics.rs", 1),
            ("src/semantic_tokens/resolve.rs", 1),
        ],
    },
    Bypass {
        pattern: "resolver::infer_lines::",
        allowed: &[],
    },
    Bypass {
        pattern: "infer_variable_type",
        allowed: &[
            ("src/features/fill_when.rs", 1),
            ("src/features/hover.rs", 1),
        ],
    },
    Bypass {
        pattern: "infer_type_in_lines",
        allowed: &[],
    },
    Bypass {
        pattern: ".definitions.",
        allowed: &[],
    },
    Bypass {
        pattern: ".jar_definitions.",
        allowed: &[],
    },
    Bypass {
        pattern: ".files.",
        allowed: &[
            ("src/features/references.rs", 1),
            ("src/features/references_verify.rs", 1),
        ],
    },
    Bypass {
        pattern: ".jar_files.",
        allowed: &[
            ("src/features/hover.rs", 1),
            ("src/features/references.rs", 1),
        ],
    },
    Bypass {
        pattern: ".live_lines",
        allowed: &[
            ("src/features/completion.rs", 1),
            ("src/features/references_verify.rs", 1),
        ],
    },
    Bypass {
        pattern: ".extension_by_receiver",
        allowed: &[("src/features/nullable_call_diagnostics.rs", 1)],
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

fn feature_sources() -> Vec<SourceFile> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut pending: Vec<PathBuf> = Vec::new();
    for root in FEATURE_ROOTS {
        let root_path = crate_root.join(root);
        assert!(
            root_path.exists(),
            "feature root `{root}` no longer exists — update FEATURE_ROOTS so it stays scanned"
        );
        pending.push(root_path);
    }
    let mut sources = Vec::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            let entries = std::fs::read_dir(&path).expect("feature directory is readable");
            pending.extend(entries.map(|entry| entry.expect("directory entry").path()));
        } else if is_production_source(&path) {
            let text = std::fs::read_to_string(&path).expect("feature source is readable");
            sources.push(SourceFile {
                relative_path: relative_path(crate_root, &path),
                text,
            });
        }
    }
    sources
}

/// The 1-based lines of `source` containing `pattern`, once per occurrence,
/// outside `//` comment lines.
fn lines_containing(source: &SourceFile, pattern: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    for (line_index, line) in source.text.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        lines.extend(std::iter::repeat_n(
            line_index + 1,
            line.matches(pattern).count(),
        ));
    }
    lines
}

/// One message per file whose occurrences of `bypass.pattern` differ from
/// its allowed count.
fn drift_of(bypass: &Bypass, sources: &[SourceFile]) -> Vec<String> {
    let mut allowed: BTreeMap<&str, usize> = bypass.allowed.iter().copied().collect();
    let mut drift = Vec::new();
    for source in sources {
        let lines = lines_containing(source, bypass.pattern);
        let baseline = allowed.remove(source.relative_path.as_str()).unwrap_or(0);
        if lines.len() != baseline {
            drift.push(format!(
                "`{}` in {}: baseline {baseline}, found {} (lines {lines:?})",
                bypass.pattern,
                source.relative_path,
                lines.len()
            ));
        }
    }
    for (file, baseline) in allowed {
        drift.push(format!(
            "`{}` in {file}: baseline {baseline}, but the file was not scanned",
            bypass.pattern
        ));
    }
    drift
}

#[test]
fn feature_code_gains_no_new_resolver_bypasses() {
    let sources = feature_sources();
    let drift: Vec<String> = BYPASSES
        .iter()
        .flat_map(|bypass| drift_of(bypass, &sources))
        .collect();
    assert!(
        drift.is_empty(),
        "feature code drifted from the bypass baselines in tests/architecture_ratchet.rs.\n\
         More than baseline: ask `CstQuery` or the `Resolver` trait instead of reaching in.\n\
         Fewer than baseline: lower the baseline to the new count.\n\n{}",
        drift.join("\n")
    );
}
