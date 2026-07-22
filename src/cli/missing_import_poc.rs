//! POC: missing-import diagnostic precision measurement over a workspace.
//!
//! Runs [`collect_missing_import_flags`] (shared with the live
//! `missing_import_diagnostics` LSP diagnostic — see
//! `features::missing_import_diagnostics` for the detection rules) over every
//! indexed workspace file and prints per-file flags plus an aggregate
//! precision summary.
//!
//! Run against a *compiling* project (e.g. nowInAndroid): every flag is by definition
//! a false positive, so the aggregate flag count is a direct precision signal — the
//! ideal is zero. Flagged-name frequencies point at systematic gaps in our scope model.

use std::collections::BTreeMap;
use std::path::Path;

use tower_lsp::lsp_types::Url;

use crate::features::missing_import_diagnostics::collect_missing_import_flags;
use crate::indexer::live_tree::{lang_for_path, parse_live};
use crate::indexer::Indexer;

/// Detect missing-import flags for one already-indexed workspace file.
fn flags_for_file(indexer: &Indexer, uri: &Url, source: &str) -> Vec<(String, u32)> {
    let Some(lang) = lang_for_path(uri.path()) else {
        return vec![];
    };
    let Some(doc) = parse_live(source, lang) else {
        return vec![];
    };
    // Store a live tree so the implicit-receiver inference uses the CST path (robust
    // to multi-line builder calls), as it would in the running LSP. Removed below.
    indexer.store_live_tree(uri, source);
    let flags = collect_missing_import_flags(indexer, uri, &doc);
    indexer.remove_live_tree(uri); // bound memory across the corpus
    flags.into_iter().map(|f| (f.name, f.line)).collect()
}

/// Run the POC over every indexed workspace `.kt`/`.java` file under `root` and print
/// per-file flags plus an aggregate precision summary.
pub(crate) async fn run_missing_imports(root: &Path) {
    eprintln!("Indexing {}...", root.display());
    let index = super::run::build_index(root, true).await;
    eprintln!(
        "Indexed: {} files, {} symbols",
        index.files.len(),
        index.definitions.len()
    );

    // The LSP warms the compiled-JAR index in the background; the CLI must do it
    // explicitly, otherwise every library import (`androidx.*`, etc.) looks unresolved.
    let gradle_paths = crate::indexer::jar::scan_gradle_jars(None);
    if !gradle_paths.is_empty() {
        let mut sidecar = index.jar_sidecar.lock().unwrap_or_else(|e| e.into_inner());
        let n = crate::indexer::jar::index_jars(&index, &gradle_paths, &mut sidecar);
        eprintln!(
            "Indexed {n} compiled-jar symbols from {} gradle jars",
            gradle_paths.len()
        );
    } else {
        eprintln!("(no gradle jars found — library imports will look unresolved)");
    }

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
        let flags = flags_for_file(&index, &uri, &source);
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
        for (name, line) in &flags {
            *by_name.entry(name.clone()).or_insert(0) += 1;
            sample
                .entry(name.clone())
                .or_insert_with(|| format!("{rel}:{}", line + 1));
            println!("{rel}:{} [missing-import]: {}", line + 1, name);
        }
    }

    // Aggregate: on a compiling project every flag is a false positive.
    eprintln!("\n──────── missing-import POC summary ────────");
    eprintln!("files scanned     : {total_files}");
    eprintln!("files with flags  : {files_with_flags}");
    eprintln!("total flags (= FP): {total_flags}");
    eprintln!("distinct names    : {}", by_name.len());
    eprintln!("\ntop flagged names (frequency — systematic FP sources):");
    let mut ranked: Vec<(&String, &usize)> = by_name.iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (name, count) in ranked.iter().take(30) {
        let where_ = sample.get(*name).map(String::as_str).unwrap_or("");
        eprintln!("  {count:>4}  {name:<32} e.g. {where_}");
    }
}
