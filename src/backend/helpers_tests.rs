use crate::features::references::resolve_scope_with_qualifier;

// ── resolve_scope_with_qualifier ──────────────────────────────────────────

fn make_indexer_with(src: &str, uri: &tower_lsp::lsp_types::Url) -> crate::indexer::Indexer {
    let indexer = crate::indexer::Indexer::new();
    indexer.index_content(uri, src);
    indexer
}

/// Lowercase names at the declaration site should get package scope (not parent_class).
/// This narrows rg search to same-package files rather than the whole codebase,
/// without wrongly attempting to qualify with a class name (which would produce
/// `ClassName.methodName` patterns that almost never appear in real Kotlin code).
/// Off-declaration-site lowercase names still return `(None, None)`.
#[test]
fn scope_lowercase_decl_gets_package_scope() {
    let uri = tower_lsp::lsp_types::Url::parse("file:///t.kt").unwrap();
    let src = "package demo\nclass Foo { val descriptiveNumber: String = \"\" }";
    let indexer = make_indexer_with(src, &uri);
    let (parent, pkg) = resolve_scope_with_qualifier(&indexer, &uri, 1, "descriptiveNumber", None);
    assert_eq!(parent, None, "lowercase member must not get a parent_class");
    assert_eq!(
        pkg.as_deref(),
        Some("demo"),
        "declaration site: lowercase member gets package scope"
    );
}

/// Uppercase names on the declaration line should use enclosing class + package.
#[test]
fn scope_uppercase_on_declaration_uses_enclosing_class() {
    let uri = tower_lsp::lsp_types::Url::parse("file:///t.kt").unwrap();
    let src = "package demo\nclass Outer {\n    class Inner\n}";
    let indexer = make_indexer_with(src, &uri);
    // `Inner` is declared on line 2 inside `Outer`
    let (parent, pkg) = resolve_scope_with_qualifier(&indexer, &uri, 2, "Inner", None);
    assert_eq!(
        parent.as_deref(),
        Some("Outer"),
        "declaration site: parent should be enclosing class"
    );
    assert_eq!(pkg.as_deref(), Some("demo"));
}
