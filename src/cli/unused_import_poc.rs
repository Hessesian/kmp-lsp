//! POC: unused-import diagnostic precision measurement over a workspace.
//!
//! Runs [`collect_unused_import_flags`] (shared with the live
//! `unused_import_diagnostics` LSP diagnostic — see
//! `features::unused_import_diagnostics` for the detection rules) over every
//! workspace `.kt`/`.java` file and prints per-file flags plus an aggregate
//! summary.
//!
//! Unlike the `missing-imports` POC, this needs no JAR warming — detection is
//! a pure CST walk with no index reads at all.
//!
//! Run against a *compiling* project (e.g. nowInAndroid): every flag is by
//! definition either a genuine unused import (real value) or a detector gap
//! (precision signal) — same methodology as `missing_import_poc`. **Not**
//! every real project is expected to be clean here the way nowInAndroid is —
//! a large, actively-developed monorepo can carry genuine unused imports that
//! were simply never cleaned up, so a non-zero count on such a project needs
//! spot-checking against the actual file content before being attributed to
//! either category.

use std::collections::BTreeMap;
use std::path::Path;

use tower_lsp::lsp_types::Url;

use crate::features::unused_import_diagnostics::collect_unused_import_flags;
use crate::indexer::live_tree::{lang_for_path, parse_live};

/// Detect unused-import flags for one file's source text.
fn flags_for_file(uri: &Url, source: &str) -> Vec<(String, u32)> {
    let Some(lang) = lang_for_path(uri.path()) else {
        return vec![];
    };
    let Some(doc) = parse_live(source, lang) else {
        return vec![];
    };
    collect_unused_import_flags(&doc)
        .into_iter()
        .map(|flag| (flag.full_path, flag.line))
        .collect()
}

/// Run the POC over every indexed workspace `.kt`/`.java` file under `root` and print
/// per-file flags plus an aggregate summary.
pub(crate) async fn run_unused_imports(root: &Path) {
    eprintln!("Indexing {}...", root.display());
    let index = super::run::build_index(root, true).await;
    eprintln!(
        "Indexed: {} files, {} symbols",
        index.files.len(),
        index.definitions.len()
    );

    // Workspace source files only (exclude library/JAR URIs).
    let mut uris: Vec<String> = index
        .files
        .iter()
        .map(|e| e.key().clone())
        .filter(|u| u.starts_with("file://") && !index.library_uris.contains(u))
        .filter(|u| u.ends_with(".kt") || u.ends_with(".java"))
        .collect();
    uris.sort();

    let mut total_files = 0usize;
    let mut files_with_flags = 0usize;
    let mut total_flags = 0usize;
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    let mut sample: BTreeMap<String, String> = BTreeMap::new();

    for uri_str in &uris {
        let Ok(uri) = Url::parse(uri_str) else {
            continue;
        };
        let Ok(path) = uri.to_file_path() else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        total_files += 1;
        let flags = flags_for_file(&uri, &source);
        if flags.is_empty() {
            continue;
        }
        files_with_flags += 1;
        total_flags += flags.len();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        for (full_path, line) in &flags {
            *by_name.entry(full_path.clone()).or_insert(0) += 1;
            sample
                .entry(full_path.clone())
                .or_insert_with(|| format!("{rel}:{}", line + 1));
            println!("{rel}:{} [unused-import]: {}", line + 1, full_path);
        }
    }

    eprintln!("\n──────── unused-import POC summary ────────");
    eprintln!("files scanned     : {total_files}");
    eprintln!("files with flags  : {files_with_flags}");
    eprintln!("total flags       : {total_flags}");
    eprintln!("distinct imports  : {}", by_name.len());
    eprintln!("\ntop flagged imports (frequency):");
    let mut ranked: Vec<(&String, &usize)> = by_name.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (full_path, count) in ranked.iter().take(30) {
        let where_ = sample.get(*full_path).map(String::as_str).unwrap_or("");
        eprintln!("  {count:>4}  {full_path:<48} e.g. {where_}");
    }
}
