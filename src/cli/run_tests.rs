//! Unit tests for `cli::run` filter application.

use super::*;
use std::fs;
use tempfile::TempDir;
use tower_lsp::lsp_types::{Position, Range, Url};

fn make_result(dir: &TempDir, rel: &str, name: &str) -> CliResult {
    let path = dir.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "").unwrap();
    let loc = Location {
        uri: Url::from_file_path(&path).unwrap(),
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 5,
            },
        },
    };
    CliResult::from_location(&loc, name, "").unwrap()
}

#[test]
fn apply_filters_enriches_module_and_relative_path() {
    let dir = TempDir::new().unwrap();
    let results = vec![make_result(&dir, "app/src/main/kotlin/A.kt", "A")];
    let filters = ResultFilters::default();
    let out = apply_filters(results, dir.path(), &filters);
    assert_eq!(out[0].module.as_deref(), Some("app"));
    assert_eq!(out[0].source_set.as_deref(), Some("main"));
    assert_eq!(
        out[0].relative_path.as_deref(),
        Some("app/src/main/kotlin/A.kt")
    );
}

#[test]
fn apply_filters_module_filter_uses_substring() {
    let dir = TempDir::new().unwrap();
    let results = vec![
        make_result(&dir, "features/play-domain/src/commonMain/kotlin/A.kt", "A"),
        make_result(&dir, "features/auth-domain/src/commonMain/kotlin/B.kt", "B"),
        make_result(&dir, "app/src/main/kotlin/C.kt", "C"),
    ];
    let filters = ResultFilters {
        module: Some("play".to_owned()),
        ..ResultFilters::default()
    };
    let out = apply_filters(results, dir.path(), &filters);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].name, "A");
}

#[test]
fn apply_filters_source_set_filter_keeps_only_matching() {
    let dir = TempDir::new().unwrap();
    let results = vec![
        make_result(&dir, "shared/src/commonMain/kotlin/A.kt", "A"),
        make_result(&dir, "shared/src/androidMain/kotlin/B.kt", "B"),
        make_result(&dir, "shared/src/iosMain/kotlin/C.kt", "C"),
    ];
    let filters = ResultFilters {
        source_sets: vec!["commonMain".to_owned(), "iosMain".to_owned()],
        ..ResultFilters::default()
    };
    let out = apply_filters(results, dir.path(), &filters);
    let names: Vec<_> = out.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["A", "C"]);
}

#[test]
fn apply_filters_limit_truncates_after_other_filters() {
    let dir = TempDir::new().unwrap();
    let results = vec![
        make_result(&dir, "shared/src/commonMain/kotlin/A.kt", "A"),
        make_result(&dir, "shared/src/commonMain/kotlin/B.kt", "B"),
        make_result(&dir, "shared/src/commonMain/kotlin/C.kt", "C"),
        make_result(&dir, "shared/src/commonMain/kotlin/D.kt", "D"),
    ];
    let filters = ResultFilters {
        limit: Some(2),
        ..ResultFilters::default()
    };
    let out = apply_filters(results, dir.path(), &filters);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].name, "A");
    assert_eq!(out[1].name, "B");
}

#[test]
fn apply_filters_zero_limit_returns_empty() {
    let dir = TempDir::new().unwrap();
    let results = vec![make_result(&dir, "shared/src/commonMain/kotlin/A.kt", "A")];
    let filters = ResultFilters {
        limit: Some(0),
        ..ResultFilters::default()
    };
    let out = apply_filters(results, dir.path(), &filters);
    assert!(out.is_empty());
}

#[test]
fn apply_filters_module_filter_drops_root_level_files() {
    // Files at the root-level `src/` have no enclosing module, so a module
    // filter should never match them.
    let dir = TempDir::new().unwrap();
    let results = vec![make_result(&dir, "src/main/kotlin/A.kt", "A")];
    let filters = ResultFilters {
        module: Some("anything".to_owned()),
        ..ResultFilters::default()
    };
    let out = apply_filters(results, dir.path(), &filters);
    assert!(out.is_empty());
}

#[test]
fn apply_filters_source_set_filter_drops_paths_without_src() {
    let dir = TempDir::new().unwrap();
    let results = vec![
        make_result(&dir, "shared/src/commonMain/kotlin/A.kt", "A"),
        make_result(&dir, "scripts/Release.kt", "Release"), // no src/
    ];
    let filters = ResultFilters {
        source_sets: vec!["commonMain".to_owned()],
        ..ResultFilters::default()
    };
    let out = apply_filters(results, dir.path(), &filters);
    let names: Vec<_> = out.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["A"]);
}

#[test]
fn apply_filters_combines_module_and_source_set() {
    let dir = TempDir::new().unwrap();
    let results = vec![
        make_result(&dir, "features/play-domain/src/commonMain/kotlin/A.kt", "A"),
        make_result(
            &dir,
            "features/play-domain/src/androidMain/kotlin/B.kt",
            "B",
        ),
        make_result(&dir, "features/auth-domain/src/commonMain/kotlin/C.kt", "C"),
    ];
    let filters = ResultFilters {
        module: Some("play".to_owned()),
        source_sets: vec!["commonMain".to_owned()],
        ..ResultFilters::default()
    };
    let out = apply_filters(results, dir.path(), &filters);
    let names: Vec<_> = out.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["A"]);
}
