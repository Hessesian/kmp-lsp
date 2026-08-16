//! POC: resolution-accuracy recall benchmark over a workspace.
//!
//! Runs [`collect_resolution_outcomes`] (shared with a future live
//! diagnostic — see `features::unresolved_symbol_diagnostics`) over every
//! indexed workspace file and prints an aggregate recall summary plus a
//! repeat-resolution/cache-candidate report.
//!
//! Unlike the missing-imports/unused-imports precision POCs (run against a
//! project you know compiles, so every flag is a false positive), there is
//! no compiler ground truth for recall — treat this as a trend metric:
//! compare the same corpus before/after a resolver change, not an absolute
//! score. This also only exercises `classify_cursor`+`resolve_identity`, not
//! the full goto-definition pipeline (no rg-grep fallback, no local-scope/
//! parameter/lambda-param resolution) — so its numbers are a conservative
//! floor, not what a user would see from goto-definition. Member-ref `Gap`
//! names are the actionable bucket; bare-ref `Gap`s are mostly expected
//! noise (locals/params this benchmark's resolver can't see, by design);
//! `FilteredCandidate` is ambiguous by design (see `ResolutionOutcome`'s
//! doc) and needs spot-checking, not blind trust.

use std::path::Path;

use tower_lsp::lsp_types::Url;

use crate::features::unresolved_symbol_diagnostics::{
    collect_resolution_outcomes, ResolutionAccuracyAggregator,
};
use crate::indexer::live_tree::{lang_for_path, parse_live};
use crate::indexer::Indexer;

/// Feed one already-indexed workspace file's reference outcomes into
/// `aggregator`. Returns `None` if the file was skipped (unsupported
/// language or unparsable), or `Some(has_parse_error)` if it was scanned —
/// callers must not count a skipped file as scanned. `has_parse_error` is
/// the visible proxy for "this file's identifiers may have gone through
/// `classify_symbol_at`'s expensive speculative brace-repair path"
/// (`lambda_doc_at` re-parses the whole file, up to `MAX_BRACE_REPAIRS`
/// times, whenever the tree has an error and no enclosing lambda is found).
fn scan_file(
    indexer: &Indexer,
    uri: &Url,
    source: &str,
    file_label: &str,
    aggregator: &mut ResolutionAccuracyAggregator,
) -> Option<bool> {
    let lang = lang_for_path(uri.path())?;
    let doc = parse_live(source, lang)?;
    let has_parse_error = doc.tree.root_node().has_error();
    indexer.store_live_tree(uri, source);
    let outcomes = collect_resolution_outcomes(indexer, uri, &doc);
    indexer.remove_live_tree(uri);
    for outcome in &outcomes {
        aggregator.add(file_label, outcome);
    }
    Some(has_parse_error)
}

/// Run the benchmark over every indexed workspace `.kt`/`.java` file under
/// `root` and print an aggregate recall + cache-candidate summary.
pub(crate) async fn run_resolution_accuracy(root: &Path) {
    eprintln!("Indexing {}...", root.display());
    let index = super::run::build_index(root, true).await;
    eprintln!(
        "Indexed: {} files, {} symbols",
        index.files.len(),
        index.definitions.len()
    );

    // Member-ref recall depends on library-typed receivers resolving —
    // without warming the compiled-JAR index, every library type looks
    // unresolvable and member recall would be an artifact of missing setup,
    // not resolver quality. Same requirement `missing_import_poc` documents.
    let gradle_paths = crate::indexer::jar::scan_gradle_jars(None);
    if !gradle_paths.is_empty() {
        let mut sidecar = index.jar_sidecar.lock().unwrap_or_else(|e| e.into_inner());
        let jar_symbol_count = crate::indexer::jar::index_jars(&index, &gradle_paths, &mut sidecar);
        eprintln!(
            "Indexed {jar_symbol_count} compiled-jar symbols from {} gradle jars",
            gradle_paths.len()
        );
    } else {
        eprintln!("(no gradle jars found — library-typed receivers will look unresolved)");
    }

    let mut uris: Vec<String> = index
        .files
        .iter()
        .map(|e| e.key().clone())
        .filter(|u| u.starts_with("file://") && !index.library_uris.contains(u))
        .filter(|u| u.ends_with(".kt") || u.ends_with(".java"))
        .collect();
    uris.sort();

    let mut aggregator = ResolutionAccuracyAggregator::default();
    let mut total_files = 0usize;
    let mut files_with_parse_errors = 0usize;
    let total_uris = uris.len();
    for (uri_index, uri_string) in uris.iter().enumerate() {
        let Ok(uri) = Url::parse(uri_string) else {
            continue;
        };
        let Ok(path) = uri.to_file_path() else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let relative_path = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        // A real corpus can take a while — print progress so a long run
        // doesn't look silent/hung (matches `missing_import_poc`'s
        // streaming-output convention).
        eprintln!("[{}/{total_uris}] {relative_path}", uri_index + 1);
        if let Some(has_parse_error) =
            scan_file(&index, &uri, &source, &relative_path, &mut aggregator)
        {
            total_files += 1;
            if has_parse_error {
                files_with_parse_errors += 1;
            }
        }
    }

    print_report(total_files, files_with_parse_errors, &aggregator);
}

fn print_report(
    total_files: usize,
    files_with_parse_errors: usize,
    aggregator: &ResolutionAccuracyAggregator,
) {
    let recall = aggregator.recall();
    eprintln!("\n──────── resolution-accuracy summary ────────");
    eprintln!(
        "NOTE: this measures classify_cursor+resolve_identity only — not the full\n\
         goto-definition pipeline (no rg-grep fallback, no local-scope/parameter/\n\
         lambda-param resolution). Real user-facing resolution is higher than these\n\
         numbers. Treat this as a floor / trend metric: compare runs on the same\n\
         corpus over time, don't read the percentage as an absolute accuracy score."
    );
    eprintln!("files scanned           : {total_files}");
    eprintln!(
        "files with parse errors : {files_with_parse_errors} (identifiers in these may have hit the speculative brace-repair path)"
    );
    eprintln!(
        "member refs             : {} ({} CstResolved, {:.1}% recall)",
        recall.member_total,
        recall.member_cst_resolved,
        recall.member_recall_pct()
    );
    eprintln!(
        "bare refs               : {} ({} resolved, {:.1}% recall)",
        recall.bare_total,
        recall.bare_success,
        recall.bare_recall_pct()
    );
    eprintln!(
        "FilteredCandidate total : {} (ambiguous — spot-check, not a plain miss)",
        recall.filtered_candidate_total
    );
    eprintln!(
        "Gap total               : {} member (actionable) + {} bare (expected — mostly locals/params)",
        recall.member_gap_total, recall.bare_gap_total
    );

    eprintln!("\ntop member-ref Gap names (actionable — no candidate found anywhere):");
    for (name, sample) in aggregator.top_member_gaps(20) {
        eprintln!(
            "  {:>4}  {name:<32} e.g. {}",
            sample.count, sample.sample_location
        );
    }

    eprintln!(
        "\ntop bare-ref Gap names (mostly locals/params outside this benchmark's resolution surface — not actionable):"
    );
    for (name, sample) in aggregator.top_bare_gaps(20) {
        eprintln!(
            "  {:>4}  {name:<32} e.g. {}",
            sample.count, sample.sample_location
        );
    }

    eprintln!("\ntop FilteredCandidate names (ambiguous — spot-check, not a plain miss):");
    for (name, sample) in aggregator.top_filtered_candidates(20) {
        eprintln!(
            "  {:>4}  {name:<32} e.g. {}",
            sample.count, sample.sample_location
        );
    }

    eprintln!("\ntop cache candidates (same symbol resolved to the same place, repeatedly):");
    for candidate in aggregator.cache_candidates(20) {
        let receiver = candidate.receiver_type.as_deref().unwrap_or("<bare>");
        eprintln!(
            "  {:>4}  {receiver}.{:<24} -> {}",
            candidate.count, candidate.name, candidate.location
        );
    }

    eprintln!(
        "\nhigh-frequency but unstable (not a simple cache key — same name, different targets):"
    );
    for hot in aggregator.unstable_hot_keys(10) {
        let receiver = hot.receiver_type.as_deref().unwrap_or("<bare>");
        eprintln!(
            "  {:>4}  {receiver}.{:<24} -> {} distinct locations",
            hot.count, hot.name, hot.distinct_locations
        );
    }
}
