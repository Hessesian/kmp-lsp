use tower_lsp::lsp_types::*;

use crate::features::generate_overrides::build_generate_overrides_action;
use crate::indexer::Indexer;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
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
fn offers_override_for_public_method() {
    let base_src = "\
open class Base {
    fun foo(): String = \"\"
}
";
    let derived_src = "\
class Derived : Base() {
}
";
    let idx = Indexer::new();
    let base_u = uri("/Base.kt");
    let derived_u = uri("/Derived.kt");
    idx.index_content(&base_u, base_src);
    idx.index_content(&derived_u, derived_src);
    idx.set_live_lines(&derived_u, derived_src);
    idx.store_live_tree(&derived_u, derived_src);
    let actions = build_generate_overrides_action(&idx, &derived_u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("foo")),
        "expected override for foo, got: {:?}",
        titles
    );
}

#[test]
fn no_override_for_existing_method() {
    let base_src = "\
open class Base {
    fun foo(): String = \"\"
}
";
    let derived_src = "\
class Derived : Base() {
    override fun foo(): String = \"overridden\"
}
";
    let idx = Indexer::new();
    let base_u = uri("/Base.kt");
    let derived_u = uri("/Derived.kt");
    idx.index_content(&base_u, base_src);
    idx.index_content(&derived_u, derived_src);
    idx.set_live_lines(&derived_u, derived_src);
    idx.store_live_tree(&derived_u, derived_src);
    let actions = build_generate_overrides_action(&idx, &derived_u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        !titles.iter().any(|t| t.contains("foo")),
        "expected foo to be skipped, got: {:?}",
        titles
    );
}

#[test]
fn no_override_for_private_method() {
    let base_src = "\
open class Base {
    private fun secret() = \"shh\"
}
";
    let derived_src = "\
class Derived : Base() {
}
";
    let idx = Indexer::new();
    let base_u = uri("/Base.kt");
    let derived_u = uri("/Derived.kt");
    idx.index_content(&base_u, base_src);
    idx.index_content(&derived_u, derived_src);
    idx.set_live_lines(&derived_u, derived_src);
    idx.store_live_tree(&derived_u, derived_src);
    let actions = build_generate_overrides_action(&idx, &derived_u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        !titles.iter().any(|t| t.contains("secret")),
        "expected no override for private method, got: {:?}",
        titles
    );
}

#[test]
fn no_overrides_for_non_kotlin_file() {
    let idx = Indexer::new();
    let u = Url::parse("file:///test/Test.java").unwrap();
    idx.index_content(&u, "class Foo extends Bar {\n}\n");
    idx.set_live_lines(&u, "class Foo extends Bar {\n}\n");
    idx.store_live_tree(&u, "class Foo extends Bar {\n}\n");
    let actions = build_generate_overrides_action(&idx, &u, cursor_at(0, 7));
    assert!(actions.is_empty(), "no overrides for Java file");
}

#[test]
fn generates_all_override_action_when_multiple_methods() {
    let base_src = "\
open class Base {
    fun foo(): String = \"\"
    open fun bar(x: Int) {}
}
";
    let derived_src = "\
class Derived : Base() {
}
";
    let idx = Indexer::new();
    let base_u = uri("/Base.kt");
    let derived_u = uri("/Derived.kt");
    idx.index_content(&base_u, base_src);
    idx.index_content(&derived_u, derived_src);
    idx.set_live_lines(&derived_u, derived_src);
    idx.store_live_tree(&derived_u, derived_src);
    let actions = build_generate_overrides_action(&idx, &derived_u, cursor_at(0, 7));
    let titles = action_titles(&actions);
    assert!(
        titles.iter().any(|t| t.contains("Override all")),
        "expected 'Override all' action, got: {:?}",
        titles
    );
}
