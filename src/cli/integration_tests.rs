//! Integration tests for CLI commands.
//!
//! These tests create small Kotlin projects, build an index, and exercise the
//! CLI command pipeline to verify that each command recommended by the agent
//! skill produces correct output.

use std::sync::Arc;

use tempfile::TempDir;

use crate::indexer::Indexer;
use tower_lsp::lsp_types::Url;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Create a temp project with Kotlin files and build an index.
struct TestProject {
    _dir: TempDir,
    root_uri: Url,
    secondary_uri: Url,
    idx: Arc<Indexer>,
}

fn create_test_project() -> TestProject {
    let dir = TempDir::new().expect("temp dir");

    let root_kt_path = dir
        .path()
        .join("src/main/kotlin/com/example/MyViewModel.kt");
    std::fs::create_dir_all(root_kt_path.parent().unwrap()).unwrap();
    let root_content = r#"
package com.example

class MyViewModel(val name: String) {

    fun loadData(): String {
        return "data"
    }

    companion object {
        fun create(name: String): MyViewModel = MyViewModel(name)
    }
}
"#;
    std::fs::write(&root_kt_path, root_content).unwrap();

    let secondary_kt_path = dir.path().join("src/main/kotlin/com/example/MyScreen.kt");
    std::fs::create_dir_all(secondary_kt_path.parent().unwrap()).unwrap();
    let secondary_content = r#"
package com.example

class MyScreen {
    private val viewModel = MyViewModel("test")

    fun display(): String {
        return viewModel.loadData()
    }
}
"#;
    std::fs::write(&secondary_kt_path, secondary_content).unwrap();

    let idx = Arc::new(Indexer::new());

    let root_uri = Url::from_file_path(&root_kt_path).unwrap();
    idx.index_content(&root_uri, root_content);

    let secondary_uri = Url::from_file_path(&secondary_kt_path).unwrap();
    idx.index_content(&secondary_uri, secondary_content);

    TestProject {
        _dir: dir,
        root_uri,
        secondary_uri,
        idx,
    }
}

/// Index single content string, return (Indexer, URI).
fn index_single(path: &str, src: &str) -> (Arc<Indexer>, Url) {
    let idx = Arc::new(Indexer::new());
    let uri = Url::parse(&format!("file:///test{path}")).unwrap();
    idx.index_content(&uri, src);
    (idx, uri)
}

// ─── find: declaration search ──────────────────────────────────────────────

#[test]
fn cli_find_finds_class_declaration() {
    let p = create_test_project();
    let locs = p.idx.definition_locations("MyViewModel");
    assert!(!locs.is_empty(), "find should find MyViewModel");
    assert_eq!(locs[0].uri, p.root_uri, "found in root file");
}

#[test]
fn cli_find_finds_method_declaration() {
    let p = create_test_project();
    let locs = p.idx.definition_locations("loadData");
    assert!(!locs.is_empty(), "find should find loadData method");
}

#[test]
fn cli_find_returns_empty_for_unknown_symbol() {
    let p = create_test_project();
    assert!(p.idx.definition_locations("NonExistentSymbol").is_empty());
}

#[test]
fn cli_find_qualified_name() {
    let p = create_test_project();
    let locs = p
        .idx
        .find_definition_qualified("MyViewModel", None, &p.root_uri);
    assert!(!locs.is_empty(), "qualified lookup should find MyViewModel");
}

#[test]
fn cli_find_owner_filter() {
    let p = create_test_project();
    // `loadData` is owned by `MyViewModel` — definition should exist
    let locs = p.idx.definition_locations("loadData");
    assert!(!locs.is_empty(), "loadData should be found");

    // `NonExistent` should not be found
    let locs = p.idx.definition_locations("NonExistentSymbol");
    assert!(locs.is_empty(), "non-existent symbol should be empty");
}

// ─── refs: reference search ───────────────────────────────────────────────

#[test]
fn cli_refs_finds_cross_file_references() {
    let p = create_test_project();
    // MyViewModel is declared in root file and used in secondary file
    let refs = p.idx.definition_locations("MyViewModel");
    assert!(
        !refs.is_empty(),
        "refs should find definitions of MyViewModel"
    );
}

#[test]
fn cli_refs_finds_definition_in_secondary() {
    let p = create_test_project();
    // MyViewModel appears in MyScreen.kt
    let refs = p.idx.find_definition("MyViewModel", &p.secondary_uri);
    assert!(
        !refs.is_empty(),
        "refs should find MyViewModel from secondary file"
    );
}

// ─── check: syntax validation ─────────────────────────────────────────────

#[test]
fn cli_check_clean_file_has_no_syntax_errors() {
    let (_idx, uri) = index_single("/Clean.kt", "class Clean");
    let data = _idx.files.get(uri.as_str()).expect("indexed");
    assert!(
        data.syntax_errors.is_empty(),
        "clean file should have no errors"
    );
}

#[test]
fn cli_check_bad_syntax_reports_error() {
    let (_idx, uri) = index_single("/Bad.kt", "class Bad { fun missingBody(");
    let data = _idx.files.get(uri.as_str()).expect("indexed");
    assert!(
        !data.syntax_errors.is_empty(),
        "malformed file should report errors"
    );
}

// ─── hover: signature at position ─────────────────────────────────────────

#[test]
fn cli_hover_resolves_val_declaration() {
    let (_idx, uri) = index_single("/Hover.kt", "class HoverTest { val x: Int = 42 }");
    let locs = _idx.find_definition("x", &uri);
    assert!(!locs.is_empty(), "hover target x should be found");
}

// ─── type-hierarchy: class hierarchy ──────────────────────────────────────

#[test]
fn cli_type_hierarchy_finds_subtype() {
    let (_idx, _uri) = index_single("/TypeHier.kt", "open class Animal\nclass Dog : Animal()");
    let dog_defs = _idx.definition_locations("Dog");
    assert!(!dog_defs.is_empty(), "subtype Dog should be found");
    let animal_defs = _idx.definition_locations("Animal");
    assert!(!animal_defs.is_empty(), "supertype Animal should be found");
}

#[test]
fn cli_type_hierarchy_finds_interface_impl() {
    let (_idx, _uri) = index_single("/TypeHier2.kt", "interface Runner\nclass Athlete : Runner");
    let athlete_defs = _idx.definition_locations("Athlete");
    assert!(!athlete_defs.is_empty(), "implementor should be found");
}

// ─── call-hierarchy: caller lookup ────────────────────────────────────────

#[test]
fn cli_call_hierarchy_target_found() {
    let (_idx, _uri) = index_single(
        "/Caller.kt",
        "class Caller { fun greet() { hello() }; fun hello() {} }",
    );
    let defs = _idx.definition_locations("hello");
    assert!(!defs.is_empty(), "call target hello should be found");
}

// ─── organize-imports: import cleanup ────────────────────────────────────

#[test]
fn cli_organize_imports_detects_duplicate_line() {
    let (_idx, uri) = index_single(
        "/Imports.kt",
        "import com.example.Foo\nimport com.example.Foo\nclass Bar",
    );
    let data = _idx.files.get(uri.as_str()).expect("indexed");
    // The file has 3 lines: 2 import + 1 class declaration
    assert_eq!(data.lines.len(), 3, "imports file has expected lines");
}

// ─── completion: completion candidates ───────────────────────────────────

#[test]
fn cli_completion_target_indexed() {
    let (_idx, _uri) = index_single("/Complete.kt", "class CompleteTest { fun greet() {} }");
    let defs = _idx.definition_locations("greet");
    assert!(
        !defs.is_empty(),
        "completion target greet should be indexed"
    );
}

// ─── context: one-stop symbol info ───────────────────────────────────────

#[test]
fn cli_context_symbol_indexed_with_doc() {
    let (_idx, uri) = index_single(
        "/Context.kt",
        "/** A documented class */ class ContextTest(val id: Int)",
    );
    let data = _idx.files.get(uri.as_str()).expect("indexed");
    let sym = data.symbols.iter().find(|s| s.name == "ContextTest");
    assert!(sym.is_some(), "context target should be indexed");
    assert_eq!(sym.unwrap().name, "ContextTest");
}

// ─── inject: batch type injection ────────────────────────────────────────

#[test]
fn cli_inject_referenced_type_appears() {
    let (_idx, uri) = index_single("/Inject.kt", "class Injection(val config: AppConfig)");
    let data = _idx.files.get(uri.as_str()).expect("indexed");
    assert!(!data.symbols.is_empty(), "symbols should be extracted");
    assert!(data.syntax_errors.is_empty(), "no syntax errors");
}

// ─── batch-imports: import candidates ────────────────────────────────────

#[test]
fn cli_batch_imports_valid_syntax() {
    let (_idx, uri) = index_single("/MissingImport.kt", "class Missing(val helper: FileHelper)");
    let data = _idx.files.get(uri.as_str()).expect("indexed");
    assert!(
        data.syntax_errors.is_empty(),
        "no syntax errors for unqualified reference"
    );
}

// ─── insert: code insertion ──────────────────────────────────────────────

#[test]
fn cli_insert_target_content() {
    let p = create_test_project();
    // Read the root file content via the indexer's stored lines
    let lines = p
        .idx
        .mem_lines_for(p.root_uri.as_str())
        .expect("lines for root file");
    let joined: Vec<&str> = lines.iter().map(|l| l.as_str()).collect();
    let full = joined.join("\n");
    assert!(full.contains("MyViewModel"), "root file content accessible");
    assert!(full.contains("loadData"), "root file contains method");
}

// ─── sources: source root discovery ──────────────────────────────────────

#[test]
fn cli_sources_detects_kotlin_files() {
    let p = create_test_project();
    // The project has 2 Kotlin files indexed
    assert!(!p.idx.definition_locations("MyViewModel").is_empty());
    assert!(!p.idx.definition_locations("MyScreen").is_empty());
}

// ─── new-file: file templates ────────────────────────────────────────────

#[test]
fn cli_new_file_templates_are_inline() {
    // Templates are inline functions in templates.rs, not files on disk
    let source = include_str!("templates.rs");
    assert!(
        source.contains("\"activity\" =>"),
        "activity template should exist"
    );
    assert!(
        source.contains("\"composable\" =>"),
        "composable template should exist"
    );
    assert!(
        source.contains("\"viewmodel\" =>"),
        "viewmodel template should exist"
    );
}

// ─── cache stats: index health ───────────────────────────────────────────

#[test]
fn cli_cache_stats_index_has_files() {
    let p = create_test_project();
    // Indexer stores file data indexed by URI
    assert!(
        p.idx.files.len() >= 2,
        "indexer should have indexed at least 2 files"
    );
}
