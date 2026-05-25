use tower_lsp::lsp_types::*;

use crate::features::generate_accessors::build_generate_accessors_action;
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

fn action_titles(actions: &[CodeActionOrCommand]) -> Vec<&str> {
    actions
        .iter()
        .filter_map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) => Some(ca.title.as_str()),
            CodeActionOrCommand::Command(_) => None,
        })
        .collect()
}

#[test]
fn generates_getter_for_val_property() {
    let src = "\
class Person(val name: String) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("getName")),
        "expected getter for name, got: {:?}",
        titles
    );
    assert!(
        !titles.iter().any(|t| t.contains("setName")),
        "no setter for val, got: {:?}",
        titles
    );
}

#[test]
fn generates_getter_and_setter_for_var_property() {
    let src = "\
class Person(var name: String) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("getName")),
        "expected getter, got: {:?}",
        titles
    );
    assert!(
        titles.iter().any(|t| t.contains("setName")),
        "expected setter, got: {:?}",
        titles
    );
}

#[test]
fn getter_returns_property_value() {
    let src = "\
class Person(val name: String) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let ca = actions
        .iter()
        .find_map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) if ca.title.contains("getName") => Some(ca),
            _ => None,
        })
        .expect("expected getName action");
    let new_text =
        &ca.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri("/Person.kt")][0].new_text;
    assert!(new_text.contains("fun getName(): String = name"));
}

#[test]
fn setter_assigns_value() {
    let src = "\
class Person(var name: String) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let ca = actions
        .iter()
        .find_map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) if ca.title.contains("setName") => Some(ca),
            _ => None,
        })
        .expect("expected setName action");
    let new_text =
        &ca.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri("/Person.kt")][0].new_text;
    assert!(new_text.contains("fun setName(value: String) {"));
    assert!(new_text.contains("name = value"));
}

#[test]
fn skips_existing_getter() {
    let src = "\
class Person(val name: String) {
    fun getName(): String = name
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        !titles.iter().any(|t| t.contains("getName")),
        "getName should be skipped, got: {:?}",
        titles
    );
}

#[test]
fn skips_existing_setter() {
    let src = "\
class Person(var name: String) {
    fun setName(value: String) { name = value }
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        !titles.iter().any(|t| t.contains("setName")),
        "setName should be skipped, got: {:?}",
        titles
    );
}

#[test]
fn no_accessors_when_no_params() {
    let src = "\
class Empty {
}
";
    let idx = setup(&[("/Empty.kt", src)]);
    let u = uri("/Empty.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    assert!(
        actions.is_empty(),
        "no actions expected for class without params"
    );
}

#[test]
fn no_accessors_for_non_kotlin_file() {
    let idx = Indexer::new();
    let u = Url::parse("file:///test/Test.java").unwrap();
    idx.index_content(&u, "class Foo {\n");
    idx.set_live_lines(&u, "class Foo {\n");
    idx.store_live_tree(&u, "class Foo {\n");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    assert!(actions.is_empty(), "no actions for Java file");
}

#[test]
fn generates_all_getters_when_multiple_val() {
    let src = "\
class Person(val name: String, val age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("Generate all getters")),
        "expected 'Generate all getters' action, got: {:?}",
        titles
    );
}

#[test]
fn generates_all_setters_when_multiple_var() {
    let src = "\
class Person(var name: String, var age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("Generate all setters")),
        "expected 'Generate all setters' action, got: {:?}",
        titles
    );
}

#[test]
fn handles_same_file_multiple_classes() {
    let src = "\
open class Base(val x: String) {
}

class Other(val name: String) {
}
";
    let idx = Indexer::new();
    let u = uri("/Test.kt");
    idx.index_content(&u, src);
    idx.set_live_lines(&u, src);
    idx.store_live_tree(&u, src);
    let actions = build_generate_accessors_action(&idx, &u, cursor_at(3, 7));
    assert!(!actions.is_empty(), "expected actions for Other class");
}
