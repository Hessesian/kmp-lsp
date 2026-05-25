use tower_lsp::lsp_types::*;

use crate::features::generate_equals::build_generate_equals_action;
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

fn find_action<'a>(actions: &'a [CodeActionOrCommand], keyword: &str) -> Option<&'a CodeAction> {
    actions.iter().find_map(|a| match a {
        CodeActionOrCommand::CodeAction(ca) if ca.title.contains(keyword) => Some(ca),
        _ => None,
    })
}

fn action_new_text<'a>(action: &'a CodeAction, uri: &'a Url) -> &'a str {
    &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[uri][0].new_text
}

#[test]
fn generates_to_string_action() {
    let src = "\
class Person(val name: String, val age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("toString")),
        "expected toString action, got: {:?}",
        titles
    );
}

#[test]
fn to_string_includes_all_properties() {
    let src = "\
class Person(val name: String, val age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let ca = find_action(&actions, "toString").expect("expected toString action");
    let new_text = action_new_text(ca, &u);
    assert!(new_text.contains("name="));
    assert!(new_text.contains("age="));
    assert!(new_text.contains("Person"));
    assert!(new_text.contains("${"));
}

#[test]
fn generates_equals_action() {
    let src = "\
class Person(val name: String, val age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("equals")),
        "expected equals action, got: {:?}",
        titles
    );
}

#[test]
fn equals_compares_all_properties() {
    let src = "\
class Person(val name: String, val age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let ca = find_action(&actions, "equals").expect("expected equals action");
    let new_text = action_new_text(ca, &u);
    assert!(new_text.contains("name == other.name"));
    assert!(new_text.contains("age == other.age"));
    assert!(new_text.contains("other as Person"));
}

#[test]
fn generates_hash_code_action() {
    let src = "\
class Person(val name: String, val age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("hashCode")),
        "expected hashCode action, got: {:?}",
        titles
    );
}

#[test]
fn hash_code_uses_name_and_age() {
    let src = "\
class Person(val name: String, val age: Int) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let ca = find_action(&actions, "hashCode").expect("expected hashCode action");
    let new_text = action_new_text(ca, &u);
    assert!(new_text.contains("name.hashCode()"));
    assert!(new_text.contains("age.hashCode()"));
}

#[test]
fn generates_all_action_when_multiple_missing() {
    let src = "\
class Person(val name: String) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("Generate all")),
        "expected 'Generate all' action, got: {:?}",
        titles
    );
}

#[test]
fn skips_existing_to_string() {
    let src = "\
class Person(val name: String) {
    override fun toString(): String = \"test\"
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        !titles.iter().any(|t| t.contains("toString")),
        "toString should be skipped, got: {:?}",
        titles
    );
}

#[test]
fn skips_existing_equals() {
    let src = "\
class Person(val name: String) {
    override fun equals(other: Any?): Boolean = true
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        !titles.iter().any(|t| t.contains("equals")),
        "equals should be skipped, got: {:?}",
        titles
    );
}

#[test]
fn no_actions_when_no_primary_ctor_params() {
    let src = "\
class Empty {
}
";
    let idx = setup(&[("/Empty.kt", src)]);
    let u = uri("/Empty.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    assert!(
        actions.is_empty(),
        "no actions expected for class without params, got {}",
        actions.len()
    );
}

#[test]
fn no_actions_for_non_kotlin_file() {
    let src = "\
class Foo {
}
";
    let idx = Indexer::new();
    let u = Url::parse("file:///test/Test.java").unwrap();
    idx.index_content(&u, src);
    idx.set_live_lines(&u, src);
    idx.store_live_tree(&u, src);
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    assert!(actions.is_empty(), "no actions for Java file");
}

#[test]
fn generates_actions_for_same_file_class() {
    let src = "\
open class Base(val x: String) {
}

class DeriveMe(val name: String) {
}
";
    let idx = Indexer::new();
    let u = uri("/Test.kt");
    idx.index_content(&u, src);
    idx.set_live_lines(&u, src);
    idx.store_live_tree(&u, src);
    let actions = build_generate_equals_action(&idx, &u, cursor_at(3, 7));
    assert!(!actions.is_empty(), "expected actions for DeriveMe class");
}

#[test]
fn no_actions_when_all_methods_exist() {
    let src = "\
class Person(val name: String) {
    override fun toString(): String = \"\"
    override fun equals(other: Any?): Boolean = true
    override fun hashCode(): Int = 0
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    assert!(
        actions.is_empty(),
        "no actions expected when all methods exist, got {}",
        actions.len()
    );
}

#[test]
fn hash_code_uses_safe_call_for_nullable() {
    let src = "\
class Person(val name: String?) {
}
";
    let idx = setup(&[("/Person.kt", src)]);
    let u = uri("/Person.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let ca = find_action(&actions, "hashCode").expect("expected hashCode action");
    let new_text = action_new_text(ca, &u);
    assert!(
        new_text.contains("name?.hashCode() ?: 0"),
        "expected safe call for nullable: {}",
        new_text
    );
}

#[test]
fn inserts_methods_when_class_has_no_body() {
    let src = "class NoBody(val name: String)";
    let idx = setup(&[("/NoBody.kt", src)]);
    let u = uri("/NoBody.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    assert!(
        !actions.is_empty(),
        "expected actions even without class body"
    );
    let ca = find_action(&actions, "toString").expect("expected toString action");
    let new_text = action_new_text(ca, &u);
    assert!(new_text.contains("override fun toString"));
}

#[test]
fn generated_to_string_uses_kotlin_template() {
    let src = "\
class Point(val x: Int, val y: Int) {
}
";
    let idx = setup(&[("/Point.kt", src)]);
    let u = uri("/Point.kt");
    let actions = build_generate_equals_action(&idx, &u, cursor_at(0, 7));
    let ca = find_action(&actions, "toString").expect("expected toString action");
    let new_text = action_new_text(ca, &u);
    assert!(new_text.contains("Point"));
    assert!(new_text.contains("${x}"));
    assert!(new_text.contains("${y}"));
}
