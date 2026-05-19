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

fn make_result_with_loc(dir: &TempDir, rel: &str, line: u32, col: u32, name: &str) -> CliResult {
    let path = make_file(dir, rel);
    let loc = Location {
        uri: Url::from_file_path(&path).unwrap(),
        range: Range {
            start: Position {
                line: line - 1,
                character: col - 1,
            },
            end: Position {
                line: line - 1,
                character: col + 9,
            },
        },
    };
    let mut r = CliResult::from_location(&loc, name, "").unwrap();
    r.enrich_with_root(dir.path());
    r
}

#[test]
fn grouped_format_collapses_repeated_paths() {
    let dir = TempDir::new().unwrap();
    let results = vec![
        make_result_with_loc(&dir, "app/src/main/kotlin/Foo.kt", 4, 9, "greet"),
        make_result_with_loc(&dir, "app/src/main/kotlin/Foo.kt", 5, 19, "greet"),
        make_result_with_loc(&dir, "app/src/main/kotlin/Foo.kt", 11, 19, "greet"),
        make_result_with_loc(&dir, "shared/src/commonMain/kotlin/Bar.kt", 22, 5, "greet"),
    ];
    let out = super::format_grouped(&results, true);
    assert_eq!(
        out,
        "app/src/main/kotlin/Foo.kt [app main]\n\
         4:9\n\
         5:19\n\
         11:19\n\
         \n\
         shared/src/commonMain/kotlin/Bar.kt [shared commonMain]\n\
         22:5\n"
    );
    // Sanity check: grouped should always be at least somewhat shorter than
    // flat once a file has 2+ matches. Real-world workspace paths (60+ chars)
    // make the gap dramatic; this small fixture demonstrates the direction.
    let flat = super::format_flat(&results, true);
    assert!(
        out.len() < flat.len(),
        "grouped should be shorter than flat; grouped={} flat={}",
        out.len(),
        flat.len()
    );
}

#[test]
fn grouped_format_annotation_omitted_when_module_and_source_set_unknown() {
    // A top-level file (no `module`, no `src/<sourceSet>/`) shouldn't get a
    // trailing `[]` — the annotation collapses to empty.
    let dir = TempDir::new().unwrap();
    let results = vec![make_result_with_loc(&dir, "Top.kt", 1, 1, "Top")];
    let out = super::format_grouped(&results, true);
    assert_eq!(out, "Top.kt\n1:1\n");
}

#[test]
fn grouped_format_annotation_shows_only_present_field() {
    // sourceSet only — file lives under src/<sourceSet>/ but no enclosing
    // module dir.
    let dir = TempDir::new().unwrap();
    let results = vec![make_result_with_loc(
        &dir,
        "src/commonMain/kotlin/Foo.kt",
        1,
        1,
        "Foo",
    )];
    let out = super::format_grouped(&results, true);
    assert!(
        out.starts_with("src/commonMain/kotlin/Foo.kt [commonMain]"),
        "got: {out}"
    );
}

#[test]
fn grouped_format_includes_kind_when_present() {
    let dir = TempDir::new().unwrap();
    let path = make_file(&dir, "app/src/main/kotlin/Foo.kt");
    let loc = Location {
        uri: Url::from_file_path(&path).unwrap(),
        range: Range {
            start: Position {
                line: 2,
                character: 6,
            },
            end: Position {
                line: 2,
                character: 9,
            },
        },
    };
    // Build CliResult with non-empty kind directly (smart-mode path).
    let mut r = CliResult::from_location(&loc, "Foo", "class").unwrap();
    r.enrich_with_root(dir.path());
    let out = super::format_grouped(&[r], true);
    assert!(out.contains("3:7 class"), "got: {out}");
}

#[test]
fn flat_format_preserves_grep_style() {
    let dir = TempDir::new().unwrap();
    let results = vec![make_result_with_loc(
        &dir,
        "app/src/main/kotlin/Foo.kt",
        4,
        9,
        "greet",
    )];
    let out = super::format_flat(&results, true);
    assert_eq!(out, "app/src/main/kotlin/Foo.kt:4:9: greet\n");
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
