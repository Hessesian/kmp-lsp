use tower_lsp::lsp_types::*;

use crate::features::generate_constructor::build_generate_constructor_action;
use crate::indexer::Indexer;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn setup(files: &[(&str, &str)]) -> Indexer {
    let indexer = Indexer::new();
    for (path, source_code) in files {
        let document_uri = uri(path);
        indexer.index_content(&document_uri, source_code);
        indexer.set_live_lines(&document_uri, source_code);
        indexer.store_live_tree(&document_uri, source_code);
    }
    indexer
}

fn cursor_at(line: u32, column: u32) -> Range {
    Range::new(Position::new(line, column), Position::new(line, column))
}

// ─── Happy path ───────────────────────────────────────────────────────────────

const BASE_CLASS: &str = "\
open class Base(val name: String, val age: Int)
";

const DERIVED_EMPTY: &str = "\
class Derived : Base
";

#[test]
fn generates_constructor_for_empty_derived_class() {
    let indexer = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", DERIVED_EMPTY)]);
    let document_uri = uri("/Derived.kt");
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(action.is_some(), "expected generate-constructor action");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(code_action) => {
            assert!(
                code_action.title.contains("Derived"),
                "title: {}",
                code_action.title
            );
            assert!(
                code_action.title.contains("name"),
                "title: {}",
                code_action.title
            );
            assert!(
                code_action.title.contains("age"),
                "title: {}",
                code_action.title
            );
            let edit = code_action.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&document_uri).unwrap();
            assert_eq!(
                edits.len(),
                2,
                "expected 2 edits (constructor + super arguments)"
            );
            assert!(edits[0].new_text.contains("val name: String"));
            assert!(edits[0].new_text.contains("val age: Int"));
            assert_eq!(edits[1].new_text, "(name, age)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Class with val/var parameters ────────────────────────────────────────────

const BASE_VAL_PARAMS: &str = "\
open class Base(name: String, val age: Int)
";

#[test]
fn generates_constructor_for_mixed_parameter_prefixes() {
    let indexer = setup(&[
        ("/Base.kt", BASE_VAL_PARAMS),
        ("/Derived.kt", DERIVED_EMPTY),
    ]);
    let document_uri = uri("/Derived.kt");
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(action.is_some(), "expected generate-constructor action");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(code_action) => {
            let edit = code_action.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&document_uri).unwrap();
            assert!(edits[0].new_text.contains("val name: String"));
            assert!(edits[0].new_text.contains("val age: Int"));
            assert_eq!(edits[1].new_text, "(name, age)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Already has primary constructor ──────────────────────────────────────────

#[test]
fn no_action_when_class_has_primary_constructor() {
    let source_code = "\
class Derived(x: Int) : Base
";
    let indexer = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", source_code)]);
    let document_uri = uri("/Derived.kt");
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(
        action.is_none(),
        "no action when constructor already present"
    );
}

// ─── Supertype already has constructor arguments ───────────────────────────────

#[test]
fn no_action_when_supertype_has_arguments() {
    let source_code = "\
class Derived : Base(\"hello\", 5)
";
    let indexer = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", source_code)]);
    let document_uri = uri("/Derived.kt");
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(
        action.is_none(),
        "no action when supertype already has arguments"
    );
}

// ─── No supertype ─────────────────────────────────────────────────────────────

#[test]
fn no_action_when_no_supertype() {
    let source_code = "\
class Standalone
";
    let indexer = Indexer::new();
    let document_uri = uri("/Standalone.kt");
    indexer.index_content(&document_uri, source_code);
    indexer.set_live_lines(&document_uri, source_code);
    indexer.store_live_tree(&document_uri, source_code);
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(action.is_none(), "no action for class without supertype");
}

// ─── Cursor not on a class ───────────────────────────────────────────────────

#[test]
fn no_action_when_cursor_not_on_class() {
    let indexer = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", DERIVED_EMPTY)]);
    let document_uri = uri("/Derived.kt");
    // Cursor on invalid position
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(5, 0));
    assert!(action.is_none(), "no action when cursor not on class");
}

// ─── Supertype with generic parameters ────────────────────────────────────────

const BASE_GENERIC: &str = "\
open class Base<T>(val item: T, val count: Int)
";

const DERIVED_GENERIC: &str = "\
class Derived : Base<String>
";

#[test]
fn generates_constructor_for_generic_supertype() {
    let indexer = setup(&[("/Base.kt", BASE_GENERIC), ("/Derived.kt", DERIVED_GENERIC)]);
    let document_uri = uri("/Derived.kt");
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(action.is_some(), "expected action for generic supertype");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(code_action) => {
            let edit = code_action.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&document_uri).unwrap();
            // The assertion has been corrected to expect "String" instead of "T"
            // because the types are now successfully substituted to produce compilable code.
            assert!(
                edits[0].new_text.contains("val item: String"),
                "Expected item type to be String but got: {}",
                edits[0].new_text
            );
            assert!(edits[0].new_text.contains("val count: Int"));
            assert_eq!(edits[1].new_text, "(item, count)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Supertype with no parameters ─────────────────────────────────────────────

const BASE_NO_PARAMS: &str = "\
open class Base
";

#[test]
fn no_action_when_supertype_has_no_parameters() {
    let source_code = "\
class Derived : Base
";
    let indexer = setup(&[("/Base.kt", BASE_NO_PARAMS), ("/Derived.kt", source_code)]);
    let document_uri = uri("/Derived.kt");
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(
        action.is_none(),
        "no action when supertype has no parameters"
    );
}

// ─── Multi-line class declaration ─────────────────────────────────────────────

#[test]
fn generates_constructor_for_supertype_with_complex_parameters() {
    let source_code = "\
open class Complex(
    val firstName: String,
    val lastName: String,
    val age: Int,
)
";
    let derived = "\
class Simple : Complex
";
    let indexer = setup(&[("/Complex.kt", source_code), ("/Simple.kt", derived)]);
    let document_uri = uri("/Simple.kt");
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(action.is_some(), "expected action for complex parameters");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(code_action) => {
            let edit = code_action.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&document_uri).unwrap();
            assert!(edits[0].new_text.contains("val firstName: String"));
            assert!(edits[0].new_text.contains("val lastName: String"));
            assert!(edits[0].new_text.contains("val age: Int"));
            assert_eq!(edits[1].new_text, "(firstName, lastName, age)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── File is not Kotlin ───────────────────────────────────────────────────────

#[test]
fn no_action_for_non_kotlin_file() {
    let source_code = "\
class Foo : Bar
";
    let indexer = Indexer::new();
    let document_uri = Url::parse("file:///test/Test.java").unwrap();
    indexer.index_content(&document_uri, source_code);
    indexer.set_live_lines(&document_uri, source_code);
    indexer.store_live_tree(&document_uri, source_code);
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(0, 8));
    assert!(action.is_none(), "no action for Java file");
}

// ─── Supertype in same file ───────────────────────────────────────────────────

#[test]
fn generates_constructor_for_same_file_supertype() {
    let source_code = "\
open class Base(val x: String)

class Derived : Base
";
    let indexer = Indexer::new();
    let document_uri = uri("/Test.kt");
    indexer.index_content(&document_uri, source_code);
    indexer.set_live_lines(&document_uri, source_code);
    indexer.store_live_tree(&document_uri, source_code);
    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(2, 8));
    assert!(action.is_some(), "expected action for same-file supertype");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(code_action) => {
            let edit = code_action.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&document_uri).unwrap();
            assert!(edits[0].new_text.contains("val x: String"));
            assert_eq!(edits[1].new_text, "(x)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Resolver fallback (file on disk, not via index_content) ─────────────────

#[test]
fn generates_constructor_when_only_on_disk_not_in_index() {
    let temporary_directory = tempfile::tempdir().unwrap();
    let file_path = temporary_directory.path().join("Test.kt");
    let source_code = "\
open class Base(val x: String)

class Derived : Base
";
    std::fs::write(&file_path, source_code).unwrap();
    let document_uri = Url::from_file_path(&file_path).unwrap();

    let indexer = Indexer::new();
    // Only set up live tree — NO index_content, so definitions index is empty.
    indexer.set_live_lines(&document_uri, source_code);
    indexer.store_live_tree(&document_uri, source_code);

    let action = build_generate_constructor_action(&indexer, &document_uri, cursor_at(2, 8));
    assert!(
        action.is_some(),
        "expected action via resolver fallback (disk indexing)"
    );
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(code_action) => {
            let edit = code_action.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&document_uri).unwrap();
            assert!(
                edits[0].new_text.contains("val x: String"),
                "{}",
                edits[0].new_text
            );
            assert_eq!(edits[1].new_text, "(x)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── split_parameters unit test ───────────────────────────────────────────────────

#[test]
fn split_parameters_simple() {
    let result = super::split_parameters("val x: Int, val y: String");
    assert_eq!(result, vec!["val x: Int", "val y: String"]);
}

#[test]
fn split_parameters_with_generics() {
    let result = super::split_parameters("val items: List<String>, val count: Int");
    assert_eq!(result, vec!["val items: List<String>", "val count: Int"]);
}

#[test]
fn split_parameters_nested_generics() {
    let result = super::split_parameters("val map: Map<String, List<Int>>, val name: String");
    assert_eq!(
        result,
        vec!["val map: Map<String, List<Int>>", "val name: String"]
    );
}

#[test]
fn split_parameters_single() {
    let result = super::split_parameters("val x: Int");
    assert_eq!(result, vec!["val x: Int"]);
}

#[test]
fn split_parameters_empty() {
    let result = super::split_parameters("");
    assert!(result.is_empty());
}

// ─── parse_parameter unit test ────────────────────────────────────────────────────

#[test]
fn parse_parameter_val() {
    assert_eq!(
        super::parse_parameter("val name: String"),
        Some(("name", "String"))
    );
}

#[test]
fn parse_parameter_no_modifiers() {
    assert_eq!(
        super::parse_parameter("name: String"),
        Some(("name", "String"))
    );
}

#[test]
fn parse_parameter_var() {
    assert_eq!(super::parse_parameter("var x: Int"), Some(("x", "Int")));
}

#[test]
fn parse_parameter_with_default() {
    assert_eq!(
        super::parse_parameter("val x: Int = 42"),
        Some(("x", "Int"))
    );
}

#[test]
fn parse_parameter_no_colon() {
    assert_eq!(super::parse_parameter("just_name"), None);
}
