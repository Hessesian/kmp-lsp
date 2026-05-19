//! Unit tests for `cli::output`.

use super::*;
use std::fs;
use tempfile::TempDir;
use tower_lsp::lsp_types::{Position, Range, Url};

fn loc_for(path: &std::path::Path, line: u32) -> Location {
    Location {
        uri: Url::from_file_path(path).unwrap(),
        range: Range {
            start: Position { line, character: 4 },
            end: Position {
                line,
                character: 14,
            },
        },
    }
}

fn make_file(dir: &TempDir, rel: &str) -> std::path::PathBuf {
    let path = dir.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "").unwrap();
    path
}

#[test]
fn from_location_populates_source_set_immediately() {
    let dir = TempDir::new().unwrap();
    let file = make_file(&dir, "features/play-domain/src/commonMain/kotlin/Foo.kt");
    let result = CliResult::from_location(&loc_for(&file, 10), "Foo", "").unwrap();
    assert_eq!(result.source_set.as_deref(), Some("commonMain"));
    assert!(result.module.is_none(), "module is set only by enrich");
    assert!(
        result.relative_path.is_none(),
        "relative_path is set only by enrich"
    );
    assert_eq!(result.line, 11, "lsp lines are 0-based, cli is 1-based");
    assert_eq!(result.col, 5);
}

#[test]
fn enrich_with_root_fills_module_and_relative_path() {
    let dir = TempDir::new().unwrap();
    let file = make_file(&dir, "features/play-domain/src/commonMain/kotlin/Foo.kt");
    let mut result = CliResult::from_location(&loc_for(&file, 0), "Foo", "").unwrap();
    result.enrich_with_root(dir.path());
    assert_eq!(result.module.as_deref(), Some("features/play-domain"));
    assert_eq!(
        result.relative_path.as_deref(),
        Some("features/play-domain/src/commonMain/kotlin/Foo.kt")
    );
}

#[test]
fn json_omits_absent_optional_fields() {
    let dir = TempDir::new().unwrap();
    let file = make_file(&dir, "scripts/Release.kt"); // no `src/` → no source_set
    let result = CliResult::from_location(&loc_for(&file, 0), "Release", "").unwrap();
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(
        !serialized.contains("sourceSet"),
        "sourceSet should be omitted when None: {serialized}"
    );
    assert!(
        !serialized.contains("relativePath"),
        "relativePath should be omitted when None: {serialized}"
    );
    assert!(
        !serialized.contains("module"),
        "module should be omitted when None: {serialized}"
    );
    assert!(
        !serialized.contains("signature"),
        "signature should be omitted when None: {serialized}"
    );
    // `kind: ""` skipped via skip_serializing_if str::is_empty.
    assert!(
        !serialized.contains("\"kind\""),
        "kind should be omitted when empty: {serialized}"
    );
}

#[test]
fn project_relative_moves_relative_path_into_file_field() {
    let dir = TempDir::new().unwrap();
    let file = make_file(&dir, "app/src/main/kotlin/Foo.kt");
    let mut result = CliResult::from_location(&loc_for(&file, 0), "Foo", "").unwrap();
    result.enrich_with_root(dir.path());
    let abs = result.file.clone();
    let rel = result
        .relative_path
        .clone()
        .expect("enrich populates relative_path");

    let projected = super::project_relative(result);
    assert_eq!(projected.file, rel, "file should hold the relative path");
    assert!(
        projected.relative_path.is_none(),
        "relative_path should be dropped after projection"
    );
    assert_ne!(
        projected.file, abs,
        "projected file must no longer be the absolute path"
    );

    // Serialized form should not carry a duplicate `relativePath` key.
    let serialized = serde_json::to_string(&projected).unwrap();
    assert!(!serialized.contains("relativePath"), "got: {serialized}");
    assert!(
        serialized.contains(&format!("\"file\":\"{rel}\"")),
        "got: {serialized}"
    );
}

#[test]
fn json_emits_present_optional_fields() {
    let dir = TempDir::new().unwrap();
    let file = make_file(&dir, "app/src/main/kotlin/Foo.kt");
    let mut result = CliResult::from_location(&loc_for(&file, 0), "Foo", "class").unwrap();
    result.enrich_with_root(dir.path());
    result.signature = Some("class Foo".to_owned());
    let serialized = serde_json::to_string(&result).unwrap();
    assert!(
        serialized.contains("\"kind\":\"class\""),
        "got: {serialized}"
    );
    assert!(serialized.contains("\"sourceSet\":\"main\""));
    assert!(serialized.contains("\"module\":\"app\""));
    assert!(serialized.contains("\"relativePath\":\"app/src/main/kotlin/Foo.kt\""));
    assert!(serialized.contains("\"signature\":\"class Foo\""));
}
