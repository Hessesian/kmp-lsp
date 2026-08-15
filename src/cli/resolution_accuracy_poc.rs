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
//! score. `Gap` names are the actionable bucket; `FilteredCandidate` is
//! ambiguous by design (see `ResolutionOutcome`'s doc) and needs
//! spot-checking, not blind trust.

use std::path::Path;

use tower_lsp::lsp_types::Url;

use crate::features::unresolved_symbol_diagnostics::{
    collect_resolution_outcomes, ResolutionAccuracyAggregator,
};
use crate::indexer::live_tree::{lang_for_path, parse_live};
use crate::indexer::Indexer;

/// Feed one already-indexed workspace file's reference outcomes into `aggregator`.
fn scan_file(
    indexer: &Indexer,
    uri: &Url,
    source: &str,
    file_label: &str,
    aggregator: &mut ResolutionAccuracyAggregator,
) {
    let Some(lang) = lang_for_path(uri.path()) else {
        return;
    };
    let Some(doc) = parse_live(source, lang) else {
        return;
    };
    indexer.store_live_tree(uri, source);
    let outcomes = collect_resolution_outcomes(indexer, uri, &doc);
    indexer.remove_live_tree(uri);
    for outcome in &outcomes {
        aggregator.add(file_label, outcome);
    }
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
        let n = crate::indexer::jar::index_jars(&index, &gradle_paths, &mut sidecar);
        eprintln!(
            "Indexed {n} compiled-jar symbols from {} gradle jars",
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
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        scan_file(&index, &uri, &source, &rel, &mut aggregator);
    }

    print_report(total_files, &aggregator);
}

fn print_report(total_files: usize, aggregator: &ResolutionAccuracyAggregator) {
    let recall = aggregator.recall();
    eprintln!("\n──────── resolution-accuracy summary ────────");
    eprintln!("files scanned          : {total_files}");
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

    eprintln!("\ntop Gap names (actionable — no candidate found anywhere):");
    for (name, sample) in aggregator.top_gaps(20) {
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
