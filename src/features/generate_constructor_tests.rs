use tower_lsp::lsp_types::*;

use crate::features::generate_constructor::build_generate_constructor_action;
use crate::indexer::Indexer;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn setup(files: &[(&str, &str)]) -> Indexer {
    let idx = Indexer::new();
    for (path, src) in files {
        let u = uri(path);
        idx.index_content(&u, src);
        idx.set_live_lines(&u, src);
        idx.store_live_tree(&u, src);
    }
    idx
}

fn cursor_at(line: u32, col: u32) -> Range {
    Range::new(Position::new(line, col), Position::new(line, col))
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
    let idx = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", DERIVED_EMPTY)]);
    let u = uri("/Derived.kt");
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(action.is_some(), "expected generate-constructor action");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(ca) => {
            assert!(ca.title.contains("Derived"), "title: {}", ca.title);
            assert!(ca.title.contains("name"), "title: {}", ca.title);
            assert!(ca.title.contains("age"), "title: {}", ca.title);
            let edit = ca.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&u).unwrap();
            assert_eq!(edits.len(), 2, "expected 2 edits (ctor + super args)");
            assert!(edits[0].new_text.contains("val name: String"));
            assert!(edits[0].new_text.contains("val age: Int"));
            assert_eq!(edits[1].new_text, "(name, age)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Class with val/var params ────────────────────────────────────────────────

const BASE_VAL_PARAMS: &str = "\
open class Base(name: String, val age: Int)
";

#[test]
fn generates_constructor_for_mixed_param_prefixes() {
    let idx = setup(&[
        ("/Base.kt", BASE_VAL_PARAMS),
        ("/Derived.kt", DERIVED_EMPTY),
    ]);
    let u = uri("/Derived.kt");
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(action.is_some(), "expected generate-constructor action");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(ca) => {
            let edit = ca.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&u).unwrap();
            assert!(edits[0].new_text.contains("val name: String"));
            assert!(edits[0].new_text.contains("val age: Int"));
            assert_eq!(edits[1].new_text, "(name, age)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Already has primary constructor ──────────────────────────────────────────

#[test]
fn no_action_when_class_has_primary_ctor() {
    let src = "\
class Derived(x: Int) : Base
";
    let idx = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", src)]);
    let u = uri("/Derived.kt");
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(
        action.is_none(),
        "no action when constructor already present"
    );
}

// ─── Supertype already has constructor args ───────────────────────────────────

#[test]
fn no_action_when_supertype_has_args() {
    let src = "\
class Derived : Base(\"hello\", 5)
";
    let idx = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", src)]);
    let u = uri("/Derived.kt");
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(
        action.is_none(),
        "no action when supertype already has args"
    );
}

// ─── No supertype ─────────────────────────────────────────────────────────────

#[test]
fn no_action_when_no_supertype() {
    let src = "\
class Standalone
";
    let idx = Indexer::new();
    let u = uri("/Standalone.kt");
    idx.index_content(&u, src);
    idx.set_live_lines(&u, src);
    idx.store_live_tree(&u, src);
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(action.is_none(), "no action for class without supertype");
}

// ─── Cursor not on a class ───────────────────────────────────────────────────

#[test]
fn no_action_when_cursor_not_on_class() {
    let idx = setup(&[("/Base.kt", BASE_CLASS), ("/Derived.kt", DERIVED_EMPTY)]);
    let u = uri("/Derived.kt");
    // Cursor on invalid position
    let action = build_generate_constructor_action(&idx, &u, cursor_at(5, 0));
    assert!(action.is_none(), "no action when cursor not on class");
}

// ─── Supertype with generic params ────────────────────────────────────────────

const BASE_GENERIC: &str = "\
open class Base<T>(val item: T, val count: Int)
";

const DERIVED_GENERIC: &str = "\
class Derived : Base<String>
";

#[test]
fn generates_constructor_for_generic_supertype() {
    let idx = setup(&[("/Base.kt", BASE_GENERIC), ("/Derived.kt", DERIVED_GENERIC)]);
    let u = uri("/Derived.kt");
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(action.is_some(), "expected action for generic supertype");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(ca) => {
            let edit = ca.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&u).unwrap();
            assert!(edits[0].new_text.contains("val item: T"));
            assert!(edits[0].new_text.contains("val count: Int"));
            assert_eq!(edits[1].new_text, "(item, count)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Supertype with no params ─────────────────────────────────────────────────

const BASE_NO_PARAMS: &str = "\
open class Base
";

#[test]
fn no_action_when_supertype_has_no_params() {
    let src = "\
class Derived : Base
";
    let idx = setup(&[("/Base.kt", BASE_NO_PARAMS), ("/Derived.kt", src)]);
    let u = uri("/Derived.kt");
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(action.is_none(), "no action when supertype has no params");
}

// ─── Multi-line class declaration ─────────────────────────────────────────────

#[test]
fn generates_constructor_for_supertype_with_complex_params() {
    let src = "\
open class Complex(
    val firstName: String,
    val lastName: String,
    val age: Int,
)
";
    let derived = "\
class Simple : Complex
";
    let idx = setup(&[("/Complex.kt", src), ("/Simple.kt", derived)]);
    let u = uri("/Simple.kt");
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(action.is_some(), "expected action for complex params");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(ca) => {
            let edit = ca.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&u).unwrap();
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
    let src = "\
class Foo : Bar
";
    let idx = Indexer::new();
    let u = Url::parse("file:///test/Test.java").unwrap();
    idx.index_content(&u, src);
    idx.set_live_lines(&u, src);
    idx.store_live_tree(&u, src);
    let action = build_generate_constructor_action(&idx, &u, cursor_at(0, 8));
    assert!(action.is_none(), "no action for Java file");
}

// ─── Supertype in same file ───────────────────────────────────────────────────

#[test]
fn generates_constructor_for_same_file_supertype() {
    let src = "\
open class Base(val x: String)

class Derived : Base
";
    let idx = Indexer::new();
    let u = uri("/Test.kt");
    idx.index_content(&u, src);
    idx.set_live_lines(&u, src);
    idx.store_live_tree(&u, src);
    let action = build_generate_constructor_action(&idx, &u, cursor_at(2, 8));
    assert!(action.is_some(), "expected action for same-file supertype");
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(ca) => {
            let edit = ca.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&u).unwrap();
            assert!(edits[0].new_text.contains("val x: String"));
            assert_eq!(edits[1].new_text, "(x)");
        }
        _ => panic!("expected CodeAction"),
    }
}

// ─── Resolver fallback (file on disk, not via index_content) ─────────────────

#[test]
fn generates_constructor_when_only_on_disk_not_in_index() {
    let tmp = tempfile::tempdir().unwrap();
    let file_path = tmp.path().join("Test.kt");
    let src = "\
open class Base(val x: String)

class Derived : Base
";
    std::fs::write(&file_path, src).unwrap();
    let u = Url::from_file_path(&file_path).unwrap();

    let idx = Indexer::new();
    // Only set up live tree — NO index_content, so definitions index is empty.
    idx.set_live_lines(&u, src);
    idx.store_live_tree(&u, src);

    let action = build_generate_constructor_action(&idx, &u, cursor_at(2, 8));
    assert!(
        action.is_some(),
        "expected action via resolver fallback (disk indexing)"
    );
    match action.unwrap() {
        CodeActionOrCommand::CodeAction(ca) => {
            let edit = ca.edit.unwrap();
            let changes = edit.changes.unwrap();
            let edits = changes.get(&u).unwrap();
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

// ─── split_params unit test ───────────────────────────────────────────────────

#[test]
fn split_params_simple() {
    let result = super::split_params("val x: Int, val y: String");
    assert_eq!(result, vec!["val x: Int", "val y: String"]);
}

#[test]
fn split_params_with_generics() {
    let result = super::split_params("val items: List<String>, val count: Int");
    assert_eq!(result, vec!["val items: List<String>", "val count: Int"]);
}

#[test]
fn split_params_nested_generics() {
    let result = super::split_params("val map: Map<String, List<Int>>, val name: String");
    assert_eq!(
        result,
        vec!["val map: Map<String, List<Int>>", "val name: String"]
    );
}

#[test]
fn split_params_single() {
    let result = super::split_params("val x: Int");
    assert_eq!(result, vec!["val x: Int"]);
}

#[test]
fn split_params_empty() {
    let result = super::split_params("");
    assert!(result.is_empty());
}

// ─── parse_param unit test ────────────────────────────────────────────────────

#[test]
fn parse_param_val() {
    assert_eq!(
        super::parse_param("val name: String"),
        Some(("name", "String"))
    );
}

#[test]
fn parse_param_no_mod() {
    assert_eq!(super::parse_param("name: String"), Some(("name", "String")));
}

#[test]
fn parse_param_var() {
    assert_eq!(super::parse_param("var x: Int"), Some(("x", "Int")));
}

#[test]
fn parse_param_with_default() {
    assert_eq!(super::parse_param("val x: Int = 42"), Some(("x", "Int")));
}

#[test]
fn parse_param_no_colon() {
    assert_eq!(super::parse_param("just_name"), None);
}
