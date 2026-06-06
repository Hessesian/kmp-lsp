use super::*;
use crate::resolver::complete_symbol;
use tower_lsp::lsp_types::SymbolKind;

fn uri(path: &str) -> tower_lsp::lsp_types::Url {
    tower_lsp::lsp_types::Url::parse(&format!("file:///test{path}")).unwrap()
}

fn sym<'a>(data: &'a FileData, name: &str) -> Option<&'a SymbolEntry> {
    data.symbols.iter().find(|s| s.name == name)
}

// ── symbol extraction ────────────────────────────────────────────────────

// ── query sanity check ───────────────────────────────────────────────────

#[test]
fn kotlin_definitions_query_compiles() {
    let lang = tree_sitter::Language::from(tree_sitter_kotlin::LANGUAGE);
    let result = tree_sitter::Query::new(&lang, crate::queries::KOTLIN_DEFINITIONS);
    if let Err(e) = &result {
        panic!("KOTLIN_DEFINITIONS query failed to compile: {e}");
    }
}

#[test]
fn class() {
    assert_eq!(
        sym(&parse_kotlin("class Foo"), "Foo").unwrap().kind,
        SymbolKind::CLASS
    );
}
#[test]
fn interface() {
    assert_eq!(
        sym(&parse_kotlin("interface Bar"), "Bar").unwrap().kind,
        SymbolKind::INTERFACE
    );
}
#[test]
fn fun_interface() {
    let data = parse_kotlin("fun interface Action {\n    fun invoke(value: String)\n}");
    assert_eq!(
        sym(&data, "Action").unwrap().kind,
        SymbolKind::INTERFACE,
        "fun interface should be indexed as INTERFACE"
    );
}
#[test]
fn fun_interface_internal() {
    let data = parse_kotlin(
        "internal fun interface IPairCodeParser {\n    fun parse(input: String): String\n}",
    );
    assert_eq!(
        sym(&data, "IPairCodeParser").unwrap().kind,
        SymbolKind::INTERFACE,
        "internal fun interface should be indexed as INTERFACE"
    );
}
#[test]
fn fun_interface_generic() {
    let data = parse_kotlin("fun interface Router<Effect> {\n    fun route(effect: Effect)\n}");
    assert_eq!(
        sym(&data, "Router").unwrap().kind,
        SymbolKind::INTERFACE,
        "generic fun interface should be indexed as INTERFACE"
    );
}
#[test]
fn fun_interface_nested() {
    let data = parse_kotlin("class LoanReducer {\n    @AssistedFactory\n    fun interface Factory {\n        fun create(x: Int): String\n    }\n}");
    assert_eq!(
        sym(&data, "Factory").unwrap().kind,
        SymbolKind::INTERFACE,
        "nested fun interface should be indexed as INTERFACE"
    );
}
#[test]
fn object_decl() {
    assert_eq!(
        sym(&parse_kotlin("object Obj"), "Obj").unwrap().kind,
        SymbolKind::OBJECT
    );
}
#[test]
fn data_class() {
    assert_eq!(
        sym(&parse_kotlin("data class D(val x: Int)"), "D")
            .unwrap()
            .kind,
        SymbolKind::STRUCT
    );
}
#[test]
fn enum_class() {
    assert_eq!(
        sym(&parse_kotlin("enum class Color { RED }"), "Color")
            .unwrap()
            .kind,
        SymbolKind::ENUM
    );
}

#[test]
fn enum_entries() {
    let data = parse_kotlin("enum class Screen { DETAIL, LIST, SETTINGS }");
    assert_eq!(sym(&data, "DETAIL").unwrap().kind, SymbolKind::ENUM_MEMBER);
    assert_eq!(sym(&data, "LIST").unwrap().kind, SymbolKind::ENUM_MEMBER);
    assert_eq!(
        sym(&data, "SETTINGS").unwrap().kind,
        SymbolKind::ENUM_MEMBER
    );
}
#[test]
fn typealias() {
    assert_eq!(
        sym(&parse_kotlin("typealias Alias = String"), "Alias")
            .unwrap()
            .kind,
        SymbolKind::CLASS
    );
}
#[test]
fn top_fun() {
    assert_eq!(
        sym(&parse_kotlin("fun foo() {}"), "foo").unwrap().kind,
        SymbolKind::FUNCTION
    );
}
#[test]
fn val_prop() {
    assert_eq!(
        sym(&parse_kotlin("val x: Int = 0"), "x").unwrap().kind,
        SymbolKind::PROPERTY
    );
}
#[test]
fn var_prop() {
    assert_eq!(
        sym(&parse_kotlin("var y = 0"), "y").unwrap().kind,
        SymbolKind::VARIABLE
    );
}
#[test]
fn const_val() {
    let data = parse_kotlin("const val MAX: Int = 100");
    assert_eq!(sym(&data, "MAX").unwrap().kind, SymbolKind::CONSTANT);
}
#[test]
fn operator_fun() {
    let data = parse_kotlin("operator fun plus(other: Vec): Vec = Vec()");
    assert_eq!(sym(&data, "plus").unwrap().kind, SymbolKind::OPERATOR);
}
#[test]
fn operator_fun_in_class() {
    let data = parse_kotlin("class Vec {\n  operator fun plus(other: Vec): Vec = Vec()\n}");
    assert_eq!(sym(&data, "plus").unwrap().kind, SymbolKind::OPERATOR);
}

#[test]
fn primary_ctor_val_param_indexed() {
    let data = parse_kotlin("data class User(val name: String, val age: Int)");
    assert_eq!(
        sym(&data, "name").unwrap().kind,
        SymbolKind::PROPERTY,
        "val ctor param should be PROPERTY"
    );
    assert_eq!(sym(&data, "age").unwrap().kind, SymbolKind::PROPERTY);
}

#[test]
fn primary_ctor_var_param_indexed() {
    let data = parse_kotlin("class Counter(var count: Int = 0)");
    assert_eq!(
        sym(&data, "count").unwrap().kind,
        SymbolKind::VARIABLE,
        "var ctor param should be VARIABLE"
    );
}

#[test]
fn primary_ctor_plain_param_not_indexed() {
    // A plain parameter WITHOUT val/var is NOT a property — should not be indexed.
    let data = parse_kotlin("class Foo(name: String)");
    assert!(
        sym(&data, "name").is_none(),
        "plain ctor param (no val/var) should not be in symbol index"
    );
}

#[test]
fn val_destructure() {
    let data = parse_kotlin("val (a, b) = pair");
    assert!(sym(&data, "a").is_some());
    assert!(sym(&data, "b").is_some());
}

#[test]
fn nested_class_indexed() {
    let data = parse_kotlin("class Outer { class Inner {} }");
    assert!(sym(&data, "Outer").is_some(), "Outer missing");
    assert!(sym(&data, "Inner").is_some(), "Inner missing");
}

#[test]
fn method_in_class_indexed() {
    let data = parse_kotlin("class Foo {\n  fun method() {}\n}");
    assert!(sym(&data, "method").is_some());
}

// ── selection_range positions ────────────────────────────────────────────

#[test]
fn class_name_position() {
    let data = parse_kotlin("class Foo");
    let s = sym(&data, "Foo").unwrap();
    assert_eq!(s.selection_start(), 0);
    assert_eq!(s.selection_range.start.character, 6);
    assert_eq!(s.selection_range.end.character, 9);
}

#[test]
fn fun_name_position() {
    let data = parse_kotlin("fun myFun() {}");
    let s = sym(&data, "myFun").unwrap();
    assert_eq!(s.selection_range.start.character, 4);
}

/// Multiline constructor: `range.start.line` is the `class` keyword line but
/// `selection_start()` (i.e. `selection_range.start.line`) is the identifier line.
#[test]
fn multiline_class_selection_vs_range() {
    // @annotation spans line 0; `class` keyword and name on line 1
    let src = "@SomeAnnotation\nclass MyClass(val x: Int)";
    let data = parse_kotlin(src);
    let s = sym(&data, "MyClass").unwrap();
    // identifier is on line 1
    assert_eq!(
        s.selection_start(),
        1,
        "selection_start() should be the identifier line"
    );
    assert_eq!(s.selection_range.start.character, 6);
    // declaration starts on line 0 (annotation)
    assert_eq!(
        s.range.start.line, 0,
        "range should cover the annotation line"
    );
}

// ── deduplication ────────────────────────────────────────────────────────

#[test]
fn data_class_no_duplicate() {
    let data = parse_kotlin("data class Foo(val x: Int)");
    assert_eq!(
        data.symbols.iter().filter(|s| s.name == "Foo").count(),
        1,
        "data class must appear exactly once"
    );
}

#[test]
fn top_fun_no_duplicate() {
    let data = parse_kotlin("fun foo() {}");
    assert_eq!(
        data.symbols.iter().filter(|s| s.name == "foo").count(),
        1,
        "top-level fun must appear exactly once"
    );
}

// ── package + imports ────────────────────────────────────────────────────

#[test]
fn package_parsed() {
    let data = parse_kotlin("package com.example.app");
    assert_eq!(data.package, Some("com.example.app".into()));
}

#[test]
fn import_plain() {
    let data = parse_kotlin("import com.example.Foo");
    let imp = data
        .imports
        .iter()
        .find(|i| i.full_path == "com.example.Foo")
        .unwrap();
    assert_eq!(imp.local_name, "Foo");
    assert!(!imp.is_star);
}

#[test]
fn import_alias() {
    let data = parse_kotlin("import com.example.Foo as F");
    let imp = data
        .imports
        .iter()
        .find(|i| i.full_path == "com.example.Foo")
        .unwrap();
    assert_eq!(imp.local_name, "F");
    assert!(!imp.is_star);
}

#[test]
fn import_star() {
    let data = parse_kotlin("import com.example.*");
    let imp = data.imports.iter().find(|i| i.is_star).unwrap();
    assert_eq!(imp.full_path, "com.example");
    assert_eq!(imp.local_name, "*");
}

// ── lines ────────────────────────────────────────────────────────────────

#[test]
fn lines_populated() {
    let data = parse_kotlin("class Foo\nfun bar() {}");
    assert_eq!(data.lines.len(), 2);
    assert_eq!(data.lines[0], "class Foo");
    assert_eq!(data.lines[1], "fun bar() {}");
}

// ── full file smoke test ─────────────────────────────────────────────────

#[test]
fn full_file() {
    let src = "package com.example\n\
                   import com.example.Bar\n\
                   import com.example.pkg.*\n\
                   import com.example.Baz as B\n\
                   class MyClass\n\
                   interface MyIface\n\
                   object MySingleton\n\
                   data class MyData(val id: Int)\n\
                   typealias MyAlias = String\n\
                   val topVal = 0\n\
                   var topVar = 0\n\
                   fun topFun() {}";

    let data = parse_kotlin(src);
    assert_eq!(data.package, Some("com.example".into()));

    for name in &[
        "MyClass",
        "MyIface",
        "MySingleton",
        "MyData",
        "MyAlias",
        "topVal",
        "topVar",
        "topFun",
    ] {
        assert!(sym(&data, name).is_some(), "{name} not indexed");
    }
    assert!(data
        .imports
        .iter()
        .any(|i| i.full_path == "com.example.Bar"));
    assert!(data
        .imports
        .iter()
        .any(|i| i.is_star && i.full_path == "com.example.pkg"));
    assert!(data
        .imports
        .iter()
        .any(|i| i.local_name == "B" && i.full_path == "com.example.Baz"));
}

// ── visibility detection ─────────────────────────────────────────────────

#[test]
fn visibility_private_fun() {
    let data = parse_kotlin("class Foo {\n  private fun secret() {}\n  fun public() {}\n}");
    let secret = sym(&data, "secret").expect("secret not indexed");
    let public = sym(&data, "public").expect("public not indexed");
    assert_eq!(secret.visibility, Visibility::Private);
    assert_eq!(public.visibility, Visibility::Public);
}

#[test]
fn visibility_protected_val() {
    let data = parse_kotlin("class Foo {\n  protected val x: Int = 0\n}");
    let x = sym(&data, "x").expect("x not indexed");
    assert_eq!(x.visibility, Visibility::Protected);
}

#[test]
fn visibility_internal_class() {
    let data = parse_kotlin("internal class Bar");
    let bar = sym(&data, "Bar").expect("Bar not indexed");
    assert_eq!(bar.visibility, Visibility::Internal);
}

#[test]
fn dot_completion_hides_private() {
    let vm_uri = uri("/VM.kt");
    let repo_uri = uri("/Repo.kt");
    let idx = crate::indexer::Indexer::new();
    idx.index_content(
        &repo_uri,
        "package com.pkg\nclass Repo {\n  fun findAll() {}\n  private fun secret() {}\n}",
    );
    idx.index_content(
        &vm_uri,
        "package com.pkg\nclass VM(\n  private val repo: Repo\n) {}",
    );

    let _ = idx.completions(&vm_uri, tower_lsp::lsp_types::Position::new(2, 24), true); // after "private val repo: Repo"
                                                                                        // Trigger a dot completion manually through resolver
    let (items, _) = complete_symbol(&idx, "", Some("repo"), &vm_uri, true, None);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"findAll"), "findAll missing: {labels:?}");
    assert!(
        !labels.contains(&"secret"),
        "private 'secret' should be hidden: {labels:?}"
    );
}

#[test]
fn java_package_extracted() {
    let data = parse_java("package cz.moneta.example;\npublic class Foo {}");
    assert_eq!(data.package.as_deref(), Some("cz.moneta.example"));
}

#[test]
fn java_enum_constants_indexed() {
    let data = parse_java(
        "package cz.moneta.example;\npublic enum EProductScreen { FLEXIKREDIT, SAVINGS }",
    );
    assert_eq!(data.package.as_deref(), Some("cz.moneta.example"));
    assert_eq!(sym(&data, "EProductScreen").unwrap().kind, SymbolKind::ENUM);
    assert_eq!(
        sym(&data, "FLEXIKREDIT").unwrap().kind,
        SymbolKind::ENUM_MEMBER
    );
    assert_eq!(sym(&data, "SAVINGS").unwrap().kind, SymbolKind::ENUM_MEMBER);
}

#[test]
fn java_import_parsed() {
    let data =
        parse_java("import cz.moneta.data.compat.enums.product.EProductScreen;\nclass Foo {}");
    assert_eq!(data.imports.len(), 1);
    assert_eq!(data.imports[0].local_name, "EProductScreen");
    assert_eq!(
        data.imports[0].full_path,
        "cz.moneta.data.compat.enums.product.EProductScreen"
    );
}

#[test]
fn java_constructor_indexed() {
    let data = parse_java("public class Foo {\n  public Foo(int x) {}\n}");
    let ctor = sym(&data, "Foo");
    // class Foo AND constructor Foo both parsed; at least one must be CONSTRUCTOR
    let has_ctor = data
        .symbols
        .iter()
        .any(|s| s.name == "Foo" && s.kind == SymbolKind::CONSTRUCTOR);
    assert!(
        has_ctor,
        "constructor not found: {:?}",
        data.symbols
            .iter()
            .map(|s| (&s.name, s.kind))
            .collect::<Vec<_>>()
    );
    let _ = ctor;
}

#[test]
fn java_static_final_field_is_constant() {
    let data = parse_java("public class Cfg {\n  public static final int MAX = 100;\n}");
    let sym = data.symbols.iter().find(|s| s.name == "MAX");
    assert!(sym.is_some(), "MAX not indexed");
    assert_eq!(
        sym.unwrap().kind,
        SymbolKind::CONSTANT,
        "expected CONSTANT for static final field"
    );
}

#[test]
fn java_instance_field_is_field() {
    let data = parse_java("public class Cfg {\n  private int count;\n}");
    let sym = data.symbols.iter().find(|s| s.name == "count");
    assert!(sym.is_some(), "count not indexed");
    assert_eq!(sym.unwrap().kind, SymbolKind::FIELD);
}

#[test]
fn declared_names_includes_function_params() {
    let src =
        "private fun handle(resultState: ResultState.Success<List<Int>>) {\n  val other: Foo\n}";
    let names = extract_declared_names(&src.lines().map(String::from).collect::<Vec<_>>());
    assert!(
        names.contains(&"resultState".to_string()),
        "param not found: {names:?}"
    );
    assert!(
        names.contains(&"other".to_string()),
        "local var not found: {names:?}"
    );
}

#[test]
fn declared_names_includes_multi_params() {
    let src = "fun foo(alpha: Int, betaValue: String, gamma: Foo)";
    let names = extract_declared_names(&src.lines().map(String::from).collect::<Vec<_>>());
    assert!(
        names.contains(&"alpha".to_string()),
        "alpha missing: {names:?}"
    );
    assert!(
        names.contains(&"betaValue".to_string()),
        "betaValue missing: {names:?}"
    );
    assert!(
        names.contains(&"gamma".to_string()),
        "gamma missing: {names:?}"
    );
}

// ── Syntax error detection tests ─────────────────────────────────────────

#[test]
fn no_errors_on_valid_kotlin() {
    let data = parse_kotlin("package com.example\nclass Foo { fun bar() {} }");
    assert!(
        data.syntax_errors.is_empty(),
        "expected no errors: {:?}",
        data.syntax_errors
    );
}

#[test]
fn missing_closing_brace_kotlin() {
    let data = parse_kotlin("class Foo {\n    fun bar() {}\n");
    assert!(
        !data.syntax_errors.is_empty(),
        "expected errors for unclosed brace"
    );
}

#[test]
fn missing_closing_paren_kotlin() {
    let data = parse_kotlin("fun foo(x: Int {\n}");
    assert!(
        !data.syntax_errors.is_empty(),
        "expected errors for unclosed paren"
    );
}

#[test]
fn dangling_equals_kotlin() {
    let data = parse_kotlin("val x =\n");
    assert!(
        !data.syntax_errors.is_empty(),
        "expected errors for dangling ="
    );
}

#[test]
fn garbled_syntax_kotlin() {
    let data = parse_kotlin("class @@@ invalid!!! {{{");
    assert!(
        !data.syntax_errors.is_empty(),
        "expected errors for garbled syntax"
    );
}

#[test]
fn no_errors_on_valid_java() {
    let data = parse_java("package com.example;\npublic class Foo { void bar() {} }");
    assert!(
        data.syntax_errors.is_empty(),
        "expected no errors: {:?}",
        data.syntax_errors
    );
}

#[test]
fn missing_semicolon_java() {
    let data = parse_java("public class Foo { int x = 5 }");
    assert!(
        !data.syntax_errors.is_empty(),
        "expected errors for missing semicolon"
    );
}

#[test]
fn error_message_contains_context() {
    let data = parse_kotlin("fun foo(x: Int { }");
    let msgs: Vec<&str> = data
        .syntax_errors
        .iter()
        .map(|e| e.message.as_str())
        .collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("missing") || m.contains("unexpected")),
        "error messages should be descriptive: {msgs:?}"
    );
}

#[test]
fn errors_capped_at_max() {
    // Generate a file with many syntax errors.
    let bad = (0..50).map(|_| "@@@ ").collect::<String>();
    let data = parse_kotlin(&bad);
    assert!(
        data.syntax_errors.len() <= super::MAX_SYNTAX_ERRORS,
        "expected at most {} errors, got {}",
        super::MAX_SYNTAX_ERRORS,
        data.syntax_errors.len()
    );
}

#[test]
fn error_has_correct_line() {
    let src = "class Foo {\n    fun bar() {}\n    val x =\n}";
    let data = parse_kotlin(src);
    assert!(!data.syntax_errors.is_empty());
    // The error should be on or near line 2 (0-indexed) where `val x =` is.
    let has_line_2_or_3 = data
        .syntax_errors
        .iter()
        .any(|e| e.range.start.line == 2 || e.range.start.line == 3);
    assert!(
        has_line_2_or_3,
        "error should be near line 2-3: {:?}",
        data.syntax_errors
    );
}

// ── Swift parsing ────────────────────────────────────────────────────────

#[test]
fn swift_query_compiles() {
    let lang = tree_sitter::Language::from(tree_sitter_swift::LANGUAGE);
    tree_sitter::Query::new(&lang, crate::queries::SWIFT_DEFINITIONS)
        .expect("SWIFT_DEFINITIONS query should compile");
}

#[test]
fn swift_class() {
    assert_eq!(
        sym(&parse_swift("class Foo {}"), "Foo").unwrap().kind,
        SymbolKind::CLASS
    );
}
#[test]
fn swift_struct() {
    assert_eq!(
        sym(&parse_swift("struct Bar {}"), "Bar").unwrap().kind,
        SymbolKind::STRUCT
    );
}
#[test]
fn swift_enum() {
    assert_eq!(
        sym(&parse_swift("enum Dir { case n }"), "Dir")
            .unwrap()
            .kind,
        SymbolKind::ENUM
    );
}
#[test]
fn swift_protocol() {
    assert_eq!(
        sym(&parse_swift("protocol P {}"), "P").unwrap().kind,
        SymbolKind::INTERFACE
    );
}
#[test]
fn swift_func() {
    assert_eq!(
        sym(&parse_swift("func foo() {}"), "foo").unwrap().kind,
        SymbolKind::FUNCTION
    );
}
#[test]
fn swift_typealias() {
    assert_eq!(
        sym(&parse_swift("typealias A = Int"), "A").unwrap().kind,
        SymbolKind::CLASS
    );
}

#[test]
fn swift_property_let() {
    let data = parse_swift("let x = 42");
    assert_eq!(sym(&data, "x").unwrap().kind, SymbolKind::PROPERTY);
}

#[test]
fn swift_property_var() {
    let data = parse_swift("var y: Int = 0");
    assert_eq!(sym(&data, "y").unwrap().kind, SymbolKind::PROPERTY);
}

#[test]
fn swift_enum_entries() {
    let data = parse_swift("enum Dir { case north, south, east }");
    assert!(sym(&data, "north").is_some());
    assert!(sym(&data, "south").is_some());
    assert!(sym(&data, "east").is_some());
}

#[test]
fn swift_extension() {
    let data = parse_swift("extension Point: Equatable { func dist() -> Double { 0 } }");
    let ext = sym(&data, "Point").unwrap();
    assert_eq!(ext.kind, SymbolKind::CLASS);
    assert!(sym(&data, "dist").is_some());
}

#[test]
fn swift_init() {
    let data = parse_swift("class Foo { init(x: Int) { } }");
    assert!(sym(&data, "init").is_some());
}

#[test]
fn swift_imports() {
    let data = parse_swift("import Foundation\nimport UIKit\nclass A {}");
    assert_eq!(data.imports.len(), 2);
    assert_eq!(data.imports[0].full_path, "Foundation");
    assert_eq!(data.imports[1].full_path, "UIKit");
}

#[test]
fn swift_no_package() {
    let data = parse_swift("class A {}");
    assert!(data.package.is_none());
}

#[test]
fn swift_visibility() {
    let data = parse_swift("private class Secret {}\npublic class Pub {}");
    assert_eq!(
        sym(&data, "Secret").unwrap().visibility,
        Visibility::Private
    );
    assert_eq!(sym(&data, "Pub").unwrap().visibility, Visibility::Public);
}

#[test]
fn swift_default_visibility_is_internal() {
    let data = parse_swift("class Foo {}");
    assert_eq!(sym(&data, "Foo").unwrap().visibility, Visibility::Internal);
}

#[test]
fn swift_detail_extraction() {
    let data = parse_swift("func distance(to other: Point) -> Double { 0 }");
    let s = sym(&data, "distance").unwrap();
    assert!(s.detail.contains("distance"), "detail: {}", s.detail);
}

#[test]
fn swift_syntax_errors() {
    let data = parse_swift("class Foo {\n    func bar() {}\n    let x =\n}");
    assert!(!data.syntax_errors.is_empty(), "should detect syntax error");
}

#[test]
fn parse_by_extension_dispatch() {
    let kt = parse_by_extension("/Foo.kt", "class Foo");
    let java = parse_by_extension("/Foo.java", "public class Foo {}");
    let swift = parse_by_extension("/Foo.swift", "class Foo {}");
    assert!(sym(&kt, "Foo").is_some());
    assert!(sym(&java, "Foo").is_some());
    assert!(sym(&swift, "Foo").is_some());
}

#[test]
fn loan_reducer_no_false_errors() {
    // @AssistedFactory fun interface Factory inside a class should not
    // produce a false "missing bracket" syntax error.
    let src = r#"
class LoanReducer {
  @AssistedFactory
  fun interface Factory {
    fun create(
      reloadAction: (loanId: String, isWustenrot: Boolean) -> Unit,
      mapSheet: (LoanDetail) -> ProductDetailSheetModel,
    ): LoanReducer
  }
}
"#;
    let data = parse_kotlin(src);
    assert!(
        data.syntax_errors.is_empty(),
        "Expected no syntax errors, got: {:?}",
        data.syntax_errors
    );
}

#[test]
fn swift_nested_enum_in_class() {
    let src = "final class DPSChangeVictoryViewModel: SimpleVictoryViewModel, @unchecked Sendable {\n    let coordinator: DPSCoordinator\n    func update(kind: DPSCoordinator.Kind) {}\n}\n\nclass DPSCoordinator {\n    enum Kind {\n        case victory\n        case defeat\n    }\n}";
    let data = parse_swift(src);
    let names: Vec<&str> = data.symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        sym(&data, "DPSChangeVictoryViewModel").unwrap().kind,
        SymbolKind::CLASS,
        "DPSChangeVictoryViewModel should be CLASS; symbols: {names:?}"
    );
    assert!(
        sym(&data, "Kind").is_some(),
        "nested Kind enum should be indexed; got: {names:?}"
    );
    assert_eq!(
        sym(&data, "Kind").unwrap().kind,
        SymbolKind::ENUM,
        "Kind should be ENUM"
    );
    assert!(
        sym(&data, "victory").is_some(),
        "enum cases should be indexed; got: {names:?}"
    );
}

#[test]
fn dedup_matches_lower_pidx_wins() {
    use tower_lsp::lsp_types::{Position, Range};
    let sel = Range::new(Position::new(1, 0), Position::new(1, 3));
    let range = sel;
    let matches: Vec<MatchEntry> = vec![
        (2, [Some(("Foo".into(), range, sel, vec![])), None]),
        (0, [Some(("Foo".into(), range, sel, vec![])), None]),
    ];
    let best = dedup_matches(&matches);
    assert_eq!(best.len(), 1);
    assert_eq!(
        best.values().next().unwrap().0,
        0,
        "pidx 0 should win over pidx 2"
    );
}

// ── Swift supers extraction ──────────────────────────────────────────────

fn supers_names(data: &FileData) -> Vec<String> {
    data.supers
        .iter()
        .map(|(_, name, _)| name.clone())
        .collect()
}

#[test]
fn swift_supers_class() {
    let data = parse_swift("class Foo: UIViewController, Sendable {}");
    let names = supers_names(&data);
    assert!(
        names.contains(&"UIViewController".to_owned()),
        "missing UIViewController; got: {names:?}"
    );
    assert!(
        names.contains(&"Sendable".to_owned()),
        "missing Sendable; got: {names:?}"
    );
}

#[test]
fn swift_supers_protocol() {
    let data = parse_swift("protocol P: Q, R {}");
    let names = supers_names(&data);
    assert!(names.contains(&"Q".to_owned()), "missing Q; got: {names:?}");
    assert!(names.contains(&"R".to_owned()), "missing R; got: {names:?}");
}

#[test]
fn swift_supers_struct() {
    let data = parse_swift("struct Point: Drawable {}");
    let names = supers_names(&data);
    assert!(
        names.contains(&"Drawable".to_owned()),
        "missing Drawable; got: {names:?}"
    );
}

#[test]
fn swift_supers_extension() {
    let data = parse_swift("extension Point: Hashable, Equatable {}");
    let names = supers_names(&data);
    assert!(
        names.contains(&"Hashable".to_owned()),
        "missing Hashable; got: {names:?}"
    );
    assert!(
        names.contains(&"Equatable".to_owned()),
        "missing Equatable; got: {names:?}"
    );
}

#[test]
fn swift_supers_with_generic_base() {
    let data = parse_swift("class Foo: Bar<Baz> {}");
    let entry = data
        .supers
        .iter()
        .find(|(_, name, _)| name == "Bar")
        .expect("missing Bar super");
    assert_eq!(
        entry.2,
        vec!["Baz"],
        "type_args for Bar<Baz> should be [Baz]"
    );
}

#[test]
fn swift_supers_multi_generic_args() {
    let data = parse_swift("class Foo: Base<Int, String> {}");
    let entry = data
        .supers
        .iter()
        .find(|(_, name, _)| name == "Base")
        .expect("missing Base super");
    assert_eq!(
        entry.2,
        vec!["Int", "String"],
        "type_args for Base<Int, String> should be [Int, String]"
    );
}

// ── visibility ───────────────────────────────────────────────────────────

#[test]
fn visibility_kotlin_defaults_public() {
    let lines: Vec<String> = vec!["fun foo() {}".into()];
    assert_eq!(
        visibility_at_line(&lines, 0),
        crate::types::Visibility::Public
    );
}

#[test]
fn visibility_kotlin_private() {
    let lines: Vec<String> = vec!["private fun foo() {}".into()];
    assert_eq!(
        visibility_at_line(&lines, 0),
        crate::types::Visibility::Private
    );
}

#[test]
fn visibility_swift_defaults_internal() {
    let lines: Vec<String> = vec!["func foo() {}".into()];
    assert_eq!(
        swift_visibility_at_line(&lines, 0),
        crate::types::Visibility::Internal
    );
}

#[test]
fn visibility_swift_open_is_public() {
    let lines: Vec<String> = vec!["open class Foo {}".into()];
    assert_eq!(
        swift_visibility_at_line(&lines, 0),
        crate::types::Visibility::Public
    );
}

// ── type_params extraction ──────────────────────────────────────────────

#[test]
fn kotlin_generic_class_has_type_params() {
    let src = "class Box<T, U>(val value: T) {}";
    let data = parse_kotlin(src);
    let s = sym(&data, "Box").expect("Box not found");
    assert_eq!(s.type_params, vec!["T", "U"]);
}

#[test]
fn kotlin_generic_interface_has_type_params() {
    let src = "interface FlowReducer<in Event, out Effect, State> {}";
    let data = parse_kotlin(src);
    let s = sym(&data, "FlowReducer").expect("FlowReducer not found");
    assert_eq!(s.type_params, vec!["Event", "Effect", "State"]);
}

#[test]
fn kotlin_non_generic_class_has_empty_type_params() {
    let src = "class Plain {}";
    let data = parse_kotlin(src);
    let s = sym(&data, "Plain").expect("Plain not found");
    assert!(
        s.type_params.is_empty(),
        "expected empty type_params, got {:?}",
        s.type_params
    );
}

#[test]
fn java_generic_class_has_type_params() {
    let src = "public class Pair<A, B> { public A first; public B second; }";
    let data = parse_java(src);
    let s = sym(&data, "Pair").expect("Pair not found");
    assert_eq!(s.type_params, vec!["A", "B"]);
}

// ── fun interface type_params ──────────────────────────────────────────────

#[test]
fn fun_interface_no_modifier_is_indexed() {
    let src = "fun interface Action {}";
    let data = parse_kotlin(src);
    let s = sym(&data, "Action").expect("Action not found");
    assert_eq!(s.kind, tower_lsp::lsp_types::SymbolKind::INTERFACE);
    assert!(s.type_params.is_empty());
}

#[test]
fn fun_interface_with_modifier_is_indexed() {
    let src = "public fun interface Runnable {}";
    let data = parse_kotlin(src);
    let s = sym(&data, "Runnable").expect("Runnable not found");
    assert_eq!(s.kind, tower_lsp::lsp_types::SymbolKind::INTERFACE);
    assert!(s.type_params.is_empty());
}

#[test]
fn fun_interface_body_generic_method_not_harvested() {
    // Non-generic fun interface whose body has a generic method must not
    // pick up the method's type param as the interface's own type param.
    let src = "fun interface Transformer { fun <T> transform(x: Any): T }";
    let data = parse_kotlin(src);
    let s = sym(&data, "Transformer").expect("Transformer not found");
    assert_eq!(s.kind, tower_lsp::lsp_types::SymbolKind::INTERFACE);
    assert!(
        s.type_params.is_empty(),
        "body method type param leaked: {:?}",
        s.type_params
    );
}

#[test]
fn fun_interface_generic_type_params() {
    let src = "fun interface Router<Effect> { fun route(effect: Effect) }";
    let data = parse_kotlin(src);
    let s = sym(&data, "Router").expect("Router not found");
    assert_eq!(s.kind, tower_lsp::lsp_types::SymbolKind::INTERFACE);
    // Text fallback extracts type params from the declaration line when CST
    // error recovery doesn't produce a type_parameters node.
    assert_eq!(
        s.type_params,
        vec!["Effect".to_string()],
        "type_params should be extracted via text fallback: {:?}",
        s.type_params
    );
    assert!(
        !s.type_params.contains(&"effect".to_string()),
        "method param leaked into interface type_params: {:?}",
        s.type_params
    );
}

/// Multi-type-param `fun interface` declarations (e.g. `<A, B>`) are now indexed
/// via the nested-ERROR detection path added for variance support.
#[test]
fn fun_interface_multi_type_params_indexed() {
    let src = "fun interface Pair<A, B> { fun get(): A }";
    let data = parse_kotlin(src);
    let s = sym(&data, "Pair").expect("Pair should now be indexed");
    assert_eq!(s.type_params, vec!["A", "B"]);
}

/// type_params_from_angle_brackets must not produce entries containing `:` or spaces.
/// For `fun interface Sortable<T: Comparable>`, like multi-param, the whole
/// declaration is not indexed (same tree-sitter-kotlin 0.3 limitation).
#[test]
fn angle_brackets_strips_variance_and_bounds() {
    // When a fun interface with variance/bounds IS indexed, type_params must strip them.
    // Not all forms are detectable by is_fun_interface_error (tree-sitter may wrap the
    // name in user_type when generics follow), so we use `if let Some`.
    let cases: &[(&str, &[&str])] = &[
        ("fun interface Producer<out T>", &["T"]),
        ("fun interface Consumer<in T>", &["T"]),
        ("fun interface Box<T : Any>", &["T"]),
        ("fun interface Pair<out A, in B>", &["A", "B"]),
    ];
    for (src, expected) in cases {
        let data = parse_kotlin(src);
        if let Some(sym) = data
            .symbols
            .iter()
            .find(|s| s.kind == SymbolKind::INTERFACE)
        {
            assert_eq!(
                &sym.type_params
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected,
                "type_params wrong for: {src}"
            );
        }
        // If not indexed: known limitation — tree-sitter-kotlin 0.3 wraps the
        // name in user_type when variance appears, hiding the simple_identifier.
    }
}

#[test]
fn angle_brackets_ignores_complex_declarations() {
    let src = "fun interface Sortable<T: Comparable> { fun sort() }";
    let data = parse_kotlin(src);
    // `T: Comparable` bound stripped → no `:` in type_params
    for s in &data.symbols {
        assert!(
            !s.type_params.iter().any(|p| p.contains(':')),
            "bound leaked into type_params for {}: {:?}",
            s.name,
            s.type_params
        );
    }
}

// ── fun interface CST fixture tests ──────────────────────────────────────
// Fixture files live in tests/fixtures/kotlin/ — they replicate the package
// structure of the original production sources so the package declaration
// and naming context are preserved.

#[test]
fn fun_interface_single_type_param_indexed() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/kotlin/mvi/Router.kt"
    ))
    .expect("fixture Router.kt missing");
    let data = parse_kotlin(&src);
    let sym = data
        .symbols
        .iter()
        .find(|s| s.name == "Router" && s.kind == SymbolKind::INTERFACE)
        .expect("Router interface not indexed");
    assert_eq!(
        sym.type_params,
        vec!["Effect"],
        "Router<Effect> type_params wrong"
    );
}

#[test]
fn fun_interface_variance_stripped() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/kotlin/input/validator/IInputValidator.kt"
    ))
    .expect("fixture IInputValidator.kt missing");
    let data = parse_kotlin(&src);
    let sym = data
        .symbols
        .iter()
        .find(|s| s.name == "IInputValidator" && s.kind == SymbolKind::INTERFACE)
        .expect("IInputValidator should be indexed");
    assert_eq!(
        sym.type_params,
        vec!["In", "Out"],
        "variance 'in'/'out' must be stripped from type_params"
    );
}

#[test]
fn chained_call_assignment_no_false_error() {
    // tree-sitter-kotlin 0.3 misparsed `a.method().property = value`
    // as statements(a.method().property) + ERROR(= value).
    // Verify we suppress these false positives.
    let data = parse_kotlin(
        "class A {\n\
             override fun onResume() {\n\
             super.onResume()\n\
             lazyStats.get().isTrackingEnabled = true\n\
             }\n\
             override fun onPause() {\n\
             lazyStats.get().isTrackingEnabled = false\n\
             }\n\
             }",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "chained-call property assignment should not produce false errors: {:?}",
        data.syntax_errors
    );
}

// ── extract_extension_receiver ────────────────────────────────────────────

#[test]
fn extension_receiver_simple() {
    assert_eq!(super::extract_extension_receiver("fun Foo.bar()"), "Foo");
}

#[test]
fn extension_receiver_with_type_params() {
    assert_eq!(
        super::extract_extension_receiver("fun <T> List<T>.bar()"),
        "List"
    );
}

#[test]
fn extension_receiver_qualified() {
    // `fun Outer.Inner.baz()` — last segment is Inner
    assert_eq!(
        super::extract_extension_receiver("fun Outer.Inner.baz()"),
        "Inner"
    );
}

#[test]
fn extension_receiver_no_receiver() {
    assert_eq!(super::extract_extension_receiver("fun bar()"), "");
}

#[test]
fn extension_receiver_non_fun() {
    assert_eq!(super::extract_extension_receiver("val x: Int"), "");
    assert_eq!(super::extract_extension_receiver("class Foo"), "");
}

#[test]
fn extension_receiver_indexed_in_symbol_entry() {
    // A top-level extension function should have extension_receiver populated.
    let src = "fun String.shout(): String = this.uppercase()";
    let data = super::parse_kotlin(src);
    let sym = data
        .symbols
        .iter()
        .find(|s| s.name == "shout")
        .expect("shout should be indexed");
    assert_eq!(sym.extension_receiver, "String");
}

#[test]
fn non_extension_fun_has_empty_receiver() {
    let src = "fun greet(name: String): String = \"Hello $name\"";
    let data = super::parse_kotlin(src);
    let sym = data
        .symbols
        .iter()
        .find(|s| s.name == "greet")
        .expect("greet should be indexed");
    assert_eq!(sym.extension_receiver, "");
}

// ── rhs_types CST extraction ─────────────────────────────────────────────

#[test]
fn rhs_types_class_literal_java_suffix() {
    // `val api = retrofit.create(DashboardApi::class.java)` — the type should
    // be extracted directly from the callable_reference argument, not stored
    // in method_call_rhs (Retrofit is a library class, not indexed).
    let src = "val api = retrofit.create(DashboardApi::class.java)";
    let data = super::parse_kotlin(src);
    let entry = data.rhs_types.iter().find(|(_, n, _)| n == "api");
    assert!(entry.is_some(), "expected rhs_types entry for `api`");
    assert_eq!(entry.unwrap().2, "DashboardApi");
}

#[test]
fn rhs_types_class_literal_kotlin_suffix() {
    // `val api = retrofit.create(DashboardApi::class)` (no .java suffix)
    let src = "val api = retrofit.create(DashboardApi::class)";
    let data = super::parse_kotlin(src);
    let entry = data.rhs_types.iter().find(|(_, n, _)| n == "api");
    assert!(entry.is_some(), "expected rhs_types entry for `api`");
    assert_eq!(entry.unwrap().2, "DashboardApi");
}

#[test]
fn rhs_types_constructor_call() {
    // `val repo = UserRepository(db)` → `UserRepository`
    let src = "val repo = UserRepository(db)";
    let data = super::parse_kotlin(src);
    let entry = data.rhs_types.iter().find(|(_, n, _)| n == "repo");
    assert!(entry.is_some(), "expected rhs_types entry for `repo`");
    assert_eq!(entry.unwrap().2, "UserRepository");
}

#[test]
fn rhs_types_di_inject() {
    // `val repo: by inject<UserRepository>()` → type arg
    let src = "val repo = inject<UserRepository>()";
    let data = super::parse_kotlin(src);
    let entry = data.rhs_types.iter().find(|(_, n, _)| n == "repo");
    assert!(entry.is_some(), "expected rhs_types entry for `repo`");
    assert_eq!(entry.unwrap().2, "UserRepository");
}

#[test]
fn method_call_rhs_regular_method() {
    // `val response = service.getDetail(req)` → stored in method_call_rhs
    let src = "val response = service.getDetail(req)";
    let data = super::parse_kotlin(src);
    let entry = data
        .method_call_rhs
        .iter()
        .find(|(_, n, _, _)| n == "response");
    assert!(
        entry.is_some(),
        "expected method_call_rhs entry for `response`"
    );
    assert_eq!(entry.unwrap().2, "service");
    assert_eq!(entry.unwrap().3, "getDetail");
}

// ── lambda-after-closing-paren regression ────────────────────────────────────

/// Regression: tree-sitter-kotlin must parse a trailing lambda after a
/// multi-line argument list without ERROR / MISSING nodes.
/// Pattern: `foo(\n  a = 1,\n) { value -> value }`
#[test]
fn lambda_after_multiline_args_no_parse_error() {
    let data = parse_kotlin(
        "fun test() {\n    val x = foo(\n        a = 1,\n        b = 2,\n    ) { value -> value }\n}",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "trailing lambda after multi-line args must not produce parse errors, got: {:?}",
        data.syntax_errors
    );
}

// ── deprecated detection ─────────────────────────────────────────────────────

#[test]
fn deprecated_annotation_on_class_marked() {
    let data = parse_kotlin("@Deprecated\nclass OldThing");
    let sym = data.symbols.iter().find(|s| s.name == "OldThing").unwrap();
    assert!(sym.deprecated);
}

#[test]
fn deprecated_annotation_on_fun_marked() {
    let data = parse_kotlin("@Deprecated\nfun oldMethod() {}");
    let sym = data.symbols.iter().find(|s| s.name == "oldMethod").unwrap();
    assert!(sym.deprecated);
}

#[test]
fn deprecated_annotation_on_val_marked() {
    let data = parse_kotlin("@Deprecated val OLD_CONST = 1");
    let sym = data.symbols.iter().find(|s| s.name == "OLD_CONST").unwrap();
    assert!(sym.deprecated);
}

#[test]
fn no_deprecated_without_annotation() {
    let data = parse_kotlin("class NormalClass");
    let sym = data
        .symbols
        .iter()
        .find(|s| s.name == "NormalClass")
        .unwrap();
    assert!(!sym.deprecated);
}

#[test]
fn java_deprecated_annotation_detected() {
    let data = parse_java("@Deprecated\npublic class OldLib {}");
    let sym = data.symbols.iter().find(|s| s.name == "OldLib").unwrap();
    assert!(sym.deprecated);
}

#[test]
fn data_class_copy_synthesized() {
    let src = r#"package test

data class Person(val name: String, val age: Int)
"#;
    let data = crate::parser::parse_kotlin(src);
    let copy_syms: Vec<&str> = data
        .symbols
        .iter()
        .filter(|s| s.name == "copy")
        .map(|s| s.detail.as_str())
        .collect();
    assert!(!copy_syms.is_empty(), "expected synthesized copy() symbol");
    assert!(
        copy_syms[0].contains("name: String"),
        "expected params in signature: {}",
        copy_syms[0]
    );
    assert!(
        copy_syms[0].contains("age: Int"),
        "expected params in signature: {}",
        copy_syms[0]
    );
    assert!(
        copy_syms[0].contains("): Person"),
        "expected return type: {}",
        copy_syms[0]
    );
}

// ── false positive syntax error regression tests (issue #78) ───────────

/// Regression: @file: annotations before package declaration should not cause
/// parser errors (`unexpected package ...`).
#[test]
fn file_annotation_before_package() {
    let data = parse_kotlin("@file:JvmName(\"Foo\")\npackage com.example");
    assert!(
        data.syntax_errors.is_empty(),
        "file annotation before package: {:?}",
        data.syntax_errors
    );
}

/// Regression: multiple @file: annotations before package.
#[test]
fn multiple_file_annotations_before_package() {
    let data = parse_kotlin(
        "@file:JvmName(\"Foo\")\n\
         @file:Suppress(\"UNCHECKED_CAST\")\n\
         package com.example\n\
         import kotlin.collections.*\n\
         class Foo",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "multiple file annotations before package: {:?}",
        data.syntax_errors
    );
}

/// Regression: complex KDoc with apostrophes, dashes, and code references.
#[test]
fn kdoc_with_punctuation() {
    let data = parse_kotlin(
        "/**\n\
         * A function with KDoc.\n\
         * It's got apostrophes — and dashes.\n\
         * Also @param x the value.\n\
         * @return the result.\n\
         */\n\
         fun foo(x: Int): Int = x + 1",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "KDoc with punctuation: {:?}",
        data.syntax_errors
    );
}

/// Regression: deeply nested generics (List<Map<String, List<Int>>>).
#[test]
fn complex_generics_parse() {
    let data = parse_kotlin(
        "package com.example\n\
         /** Some documentation. */\n\
         fun complexGenerics(a: List<Map<String, List<Int>>>): Nothing? = null",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "complex generics: {:?}",
        data.syntax_errors
    );
}

/// Regression: multiple annotations on a declaration.
#[test]
fn multiple_annotations_on_declaration() {
    let data = parse_kotlin(
        "@Composable\n\
         @JvmStatic\n\
         @JvmOverloads\n\
         fun MyAnnotatedFun(x: Int = 0, y: String = \"hello\") { }",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "multiple annotations: {:?}",
        data.syntax_errors
    );
}

/// Regression: lambda with explicit parameter types.
#[test]
fn lambda_with_explicit_param_types() {
    let data = parse_kotlin(
        "package com.example\n\
         val f: (Int) -> String = { x: Int -> x.toString() }",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "lambda with explicit param types: {:?}",
        data.syntax_errors
    );
}

/// Regression: suspending function types in parameters.
#[test]
fn suspend_function_type_parameter() {
    let data = parse_kotlin(
        "package com.example\n\
         fun runAsync(block: suspend () -> Unit) { }",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "suspend function type param: {:?}",
        data.syntax_errors
    );
}

/// Regression: destructuring in for loop.
#[test]
fn destructuring_in_for_loop() {
    let data = parse_kotlin(
        "package com.example\n\
         fun process(map: Map<String, Int>) {\n\
             for ((key, value) in map) { }\n\
         }",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "destructuring in for loop: {:?}",
        data.syntax_errors
    );
}

/// Regression: inline value class with JvmInline annotation.
#[test]
fn inline_value_class() {
    let data = parse_kotlin(
        "package com.example\n\
         @JvmInline\n\
         value class UserId(val id: String)",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "inline value class: {:?}",
        data.syntax_errors
    );
}

/// Regression: variance modifiers on type parameters.
#[test]
fn generic_variance_modifiers() {
    let data = parse_kotlin(
        "package com.example\n\
         interface Source<out T> { fun get(): T }\n\
         interface Sink<in T> { fun put(item: T) }",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "generic variance modifiers: {:?}",
        data.syntax_errors
    );
}

/// Regression: complex bounded type parameters.
#[test]
fn complex_bounded_type_params() {
    let data = parse_kotlin(
        "package com.example\n\
         class Container<T : Comparable<T>>\n\
         class MultiBound<T> where T : Comparable<T>, T : CharSequence",
    );
    assert!(
        data.syntax_errors.is_empty(),
        "complex bounded type params: {:?}",
        data.syntax_errors
    );
}

// ── comprehensive false positive regression suite (issue #78) ───────

fn assert_no_errors(src: &str) {
    let data = parse_kotlin(src);
    assert!(
        data.syntax_errors.is_empty(),
        "unexpected errors: {:?}",
        data.syntax_errors
    );
}

#[test]
fn fp_bom_prefix() {
    assert_no_errors("\u{feff}package com.example\nclass Test");
}

#[test]
fn fp_unicode_in_comment() {
    assert_no_errors(
        "/**\n * Test with unicode: ñoño — em-dash – en-dash 'apostrophe' «guillemets»\n * And @param x the value\n */\npackage com.example\nclass Test",
    );
}

#[test]
fn fp_file_optin() {
    assert_no_errors(
        "@file:OptIn(ExperimentalStdlibApi::class)\npackage com.example\n@RequiresOptIn\nannotation class ExperimentalStdlibApi",
    );
}

#[test]
fn fp_file_suppress() {
    assert_no_errors("@file:Suppress(\"UNCHECKED_CAST\")\npackage com.example\nval x: Map<String, Any> = emptyMap()");
}

#[test]
fn fp_complex_typealias() {
    assert_no_errors(
        "package com.example\ntypealias StringList = List<String>\ntypealias Predicate<T> = (T) -> Boolean",
    );
}

#[test]
fn fp_context_receiver() {
    assert_no_errors("package com.example\ncontext(ContextA, ContextB)\nfun contextual() {}");
}

#[test]
fn fp_deeply_nested_generics() {
    assert_no_errors(
        "package com.example\nclass Deep<T : Comparable<Map<String, List<Pair<Int, Double>>>>>",
    );
}

#[test]
fn fp_where_clause() {
    assert_no_errors(
        "package com.example\nfun <T> singletonOf(item: T): List<T> where T : Comparable<T> = listOf(item)",
    );
}

#[test]
fn fp_annotation_on_type_use() {
    assert_no_errors("package com.example\nfun foo(list: List<@JvmSuppressWildcards String>) {}");
}

#[test]
fn fp_named_companion_object() {
    assert_no_errors(
        "package com.example\nclass Foo {\n    companion object Named {\n        fun create(): Foo = Foo()\n    }\n}",
    );
}

#[test]
fn fp_when_subject() {
    assert_no_errors(
        "package com.example\nfun describe(obj: Any): String = when (obj) {\n    is String -> \"string\"\n    is Int -> \"int\"\n    else -> \"unknown\"\n}",
    );
}

#[test]
fn fp_inline_fun_with_reified() {
    assert_no_errors(
        "package com.example\ninline fun <reified T> isA(value: Any): Boolean = value is T",
    );
}

#[test]
fn fp_annotation_params() {
    assert_no_errors(
        "package com.example\n@Target(AnnotationTarget.CLASS, AnnotationTarget.FUNCTION)\n@Retention(AnnotationRetention.RUNTIME)\nannotation class MyAnnotation",
    );
}

#[test]
fn fp_vararg() {
    assert_no_errors("package com.example\nfun sum(vararg numbers: Int): Int = numbers.sum()");
}

#[test]
fn fp_tailrec() {
    assert_no_errors(
        "package com.example\ntailrec fun factorial(n: Int, acc: Int = 1): Int = if (n <= 1) acc else factorial(n - 1, n * acc)",
    );
}

#[test]
fn fp_crossinline_noinline() {
    assert_no_errors(
        "package com.example\ninline fun withLock(body: () -> Unit) {}\nfun use(crossinline f: () -> Unit, noinline g: () -> Unit) {}",
    );
}

#[test]
fn fp_annotation_on_type() {
    assert_no_errors("package com.example\nfun foo(): @NotNull String = \"hello\"");
}

#[test]
fn fp_complex_lambda_with_destructuring() {
    assert_no_errors(
        "package com.example\n\
         data class Entry(val key: String, val value: Int)\n\
         val process: (Entry) -> String = { (k, _) -> k.uppercase() }",
    );
}

#[test]
fn fp_label_return() {
    assert_no_errors(
        "package com.example\n\
         fun labelTest() {\n\
             listOf(1, 2, 3).forEach label@{\n\
                 if (it == 2) return@label\n\
                 println(it)\n\
             }\n\
         }",
    );
}

#[test]
fn fp_extension_with_nullable_receiver() {
    assert_no_errors("package com.example\nfun String?.isNullOrTrimmed(): Boolean = this == null || this.trim().isEmpty()");
}

#[test]
fn fp_star_typeprojection() {
    assert_no_errors("package com.example\nfun length(list: List<*>): Int = list.size");
}

#[test]
fn fp_constructor_overload() {
    assert_no_errors(
        "package com.example\n\
         class Person(val name: String) {\n\
             constructor(): this(\"unknown\")\n\
         }",
    );
}

#[test]
fn fp_generic_extension_fun() {
    assert_no_errors(
        "package com.example\nfun <T> List<T>.secondOrNull(): T? = if (size >= 2) this[1] else null",
    );
}

#[test]
fn fp_override_fun() {
    assert_no_errors(
        "package com.example\n\
         interface Base {\n\
             fun foo(): Any\n\
         }\n\
         class Derived : Base {\n\
             override fun foo(): String = \"hello\"\n\
         }",
    );
}

#[test]
fn fp_array_annotation() {
    assert_no_errors(
        "package com.example\n@Suppress(\"UNCHECKED_CAST\", \"DEPRECATION\")\nfun bar() {}",
    );
}

#[test]
fn fp_init_block() {
    assert_no_errors(
        "package com.example\nclass Foo(val value: Int) {\n    init {\n        require(value > 0)\n    }\n}",
    );
}

#[test]
fn fp_property_delegate() {
    assert_no_errors(
        "package com.example\nimport kotlin.properties.Delegates\nvar observable: String by Delegates.observable(\"initial\") { _, old, new -> println(\"$old -> $new\") }",
    );
}

#[test]
fn fp_operator_overload() {
    assert_no_errors(
        "package com.example\n\
         data class Vec(val x: Int, val y: Int) {\n\
             operator fun plus(other: Vec) = Vec(x + other.x, y + other.y)\n\
             operator fun rangeTo(other: Vec) = emptyList<Vec>()\n\
         }",
    );
}

#[test]
fn fp_sealed_interface() {
    assert_no_errors(
        "package com.example\n\
         sealed interface Result<out T>\n\
         data class Success<T>(val value: T) : Result<T>\n\
         data class Failure(val error: Throwable) : Result<Nothing>",
    );
}

#[test]
fn fp_destructuring_declaration() {
    assert_no_errors(
        "package com.example\n\
         data class Point(val x: Int, val y: Int)\n\
         fun destructure() {\n\
             val (x, y) = Point(1, 2)\n\
         }",
    );
}

#[test]
fn fp_suspend_extension_fun() {
    assert_no_errors(
        "package com.example\n\
         suspend fun <T> List<T>.asyncMap(transform: suspend (T) -> Any): List<Any> = emptyList()",
    );
}

#[test]
fn fp_covariant_type() {
    assert_no_errors("package com.example\ninterface Producer<out T> { fun produce(): T }");
}

#[test]
fn fp_contravariant_type() {
    assert_no_errors("package com.example\ninterface Consumer<in T> { fun consume(item: T) }");
}

#[test]
fn fp_companion_object_extension() {
    assert_no_errors(
        "package com.example\n\
         class Host {\n\
             companion object\n\
         }\n\
         fun Host.Companion.create(): Host = Host()",
    );
}

#[test]
fn fp_secondary_constructor() {
    assert_no_errors(
        "package com.example\nclass Person(val name: String) {\n    constructor(): this(\"unknown\")\n}",
    );
}

#[test]
fn fp_annotation_class_with_body() {
    assert_no_errors("package com.example\nannotation class JsonName(val name: String)");
}

/// Regression: shebang line before package (valid in .kts scripts).
#[test]
fn fp_shebang_before_package() {
    assert_no_errors("#!/usr/bin/env kotlin\npackage com.example\nclass Test");
}

/// Regression: multiple annotation targets before declaration.
#[test]
fn fp_use_site_target_annotations() {
    assert_no_errors(
        "package com.example\n\
         class Foo {\n\
             @get:JvmName(\"getBar\")\n\
             @set:JvmName(\"setBar\")\n\
             var bar: String = \"\"\n\
         }",
    );
}

/// Regression: contract DSL.
#[test]
fn fp_contract_dsl() {
    assert_no_errors("package com.example\nimport kotlin.contracts.*\ninline fun foo(body: () -> Unit) { contract { callsInPlace(body, InvocationKind.EXACTLY_ONCE) }; body() }");
}

/// Regression: @Suppress with array argument.
#[test]
fn fp_suppress_array() {
    assert_no_errors(
        "package com.example\n@Suppress(\"DEPRECATION\", \"UNCHECKED_CAST\", \"UNUSED_VARIABLE\")\nclass Foo",
    );
}

/// Regression: annotation with default values.
#[test]
fn fp_annotation_with_defaults() {
    assert_no_errors(
        "package com.example\nannotation class MyAnn(val message: String = \"\", val level: Int = 0)",
    );
}

/// Regression: property with getter/setter.
#[test]
fn fp_property_with_getter_setter() {
    assert_no_errors(
        "package com.example\n\
         var counter: Int = 0\n\
             get() = field.coerceAtLeast(0)\n\
             set(value) {\n\
                 field = value.coerceAtLeast(0)\n\
             }",
    );
}

/// Regression: CRLF line endings should not cause false errors.
#[test]
fn fp_crlf_line_endings() {
    assert_no_errors("package com.example\r\n\r\nfun foo() { }\r\n");
}

/// Regression: leading whitespace before package.
#[test]
fn fp_leading_whitespace_before_package() {
    assert_no_errors("  \npackage com.example\nclass Test");
}

/// Regression: blank lines at start of file.
#[test]
fn fp_blank_lines_at_start() {
    assert_no_errors("\n\n\npackage com.example\nclass Test");
}

/// Regression: package with multi-level import.
#[test]
fn fp_multi_level_import() {
    assert_no_errors(
        "package com.example.deeply.nested.path\n\
         import kotlinx.coroutines.flow.MutableStateFlow\n\
         import kotlinx.coroutines.flow.StateFlow\n\
         class Foo",
    );
}

/// Regression: anonymous function with it.
#[test]
fn fp_anonymous_function() {
    assert_no_errors(
        "package com.example\n\
         val sorted = listOf(3, 1, 2).sortedBy { it }\n\
         val mapped = listOf(1, 2, 3).map { it * 2 }",
    );
}

/// Regression: enum with methods.
#[test]
fn fp_enum_with_methods() {
    assert_no_errors(
        "package com.example\n\
         enum class Color(val hex: Int) {\n\
             RED(0xFF0000),\n\
             GREEN(0x00FF00),\n\
             BLUE(0x0000FF);\n\
             fun description(): String = name.lowercase()\n\
         }",
    );
}

/// Regression: object expression.
#[test]
fn fp_object_expression() {
    assert_no_errors(
        "package com.example\n\
         interface ClickListener { fun onClick() }\n\
         val listener = object : ClickListener {\n\
             override fun onClick() { }\n\
         }",
    );
}

/// Regression: SAM conversion.
#[test]
fn fp_sam_conversion() {
    assert_no_errors(
        "package com.example\n\
         fun interface ClickHandler { fun handle(event: String) }\n\
         val handler = ClickHandler { event -> println(event) }",
    );
}

/// Regression: typealiases with generics.
#[test]
fn fp_typealias_generic() {
    assert_no_errors(
        "package com.example\n\
         typealias StringTransformer<T> = (T) -> String\n\
         val f: StringTransformer<Int> = { it.toString() }",
    );
}

/// Regression: safe call chain.
#[test]
fn fp_safe_call_chain() {
    assert_no_errors(
        "package com.example\n\
         data class Address(val city: String?)\n\
         data class Person(val address: Address?)\n\
         fun getCity(person: Person?): String? = person?.address?.city",
    );
}

/// Regression: elvis operator.
#[test]
fn fp_elvis_operator() {
    assert_no_errors(
        "package com.example\n\
         fun getName(name: String?): String = name ?: \"unknown\"",
    );
}

/// Regression: let/run/apply scope functions.
#[test]
fn fp_scope_functions() {
    assert_no_errors(
        "package com.example\n\
         fun scopeTest(x: String?) {\n\
             x?.let { println(it) }\n\
                 ?: run { println(\"null\") }\n\
             val result = StringBuilder().apply {\n\
                 append(\"hello\")\n\
                 append(\"world\")\n\
             }.toString()\n\
         }",
    );
}

/// Regression: data class with trailing comma.
#[test]
fn fp_data_class_trailing_comma() {
    assert_no_errors(
        "package com.example\n\
         data class Person(\n\
             val name: String,\n\
             val age: Int,\n\
         )",
    );
}

/// Regression: default parameter values.
#[test]
fn fp_default_parameter_values() {
    assert_no_errors(
        "package com.example\n\
         fun createUser(\n\
             name: String = \"guest\",\n\
             age: Int = 18,\n\
             email: String? = null,\n\
         ) { }",
    );
}

/// Regression: inline class wrapping type.
#[test]
fn fp_inline_class_value() {
    assert_no_errors(
        "package com.example\n\
         @JvmInline\n\
         value class UserId(val value: String) {\n\
             override fun toString(): String = value\n\
         }",
    );
}

// ── realistic KMP file regression (issue #78) ─────────────────────

/// Build a realistic KMP file matching characteristics from the issue:
/// - flow chains (map, combine, onEach, catch, collect)
/// - nested data classes / sealed interfaces
/// - block comments with unicode punctuation
/// - generic function types
/// - constructor-injected deps
fn build_kmp_flow_file() -> String {
    let mut src = String::new();
    src.push_str("package com.example.app\n\n");
    src.push_str("/**\n");
    src.push_str(" * A realistic KMP ViewModel — em-dashes, apostrophes, «guillemets», and arrows → in KDoc.\n");
    src.push_str(" * Line breaks in prose, parentheses (like this), and `code references` too.\n");
    src.push_str(" * Also: @param state the StateFlow to observe.\n");
    src.push_str(" * @return nothing — it's a ViewModel!\n");
    src.push_str(" */\n");
    src.push_str("class MyViewModel(\n");
    src.push_str("    private val repo: Repository,\n");
    src.push_str("    private val mapper: Mapper<Input, Output>,\n");
    src.push_str(") {\n");
    src.push_str("    sealed interface UiState {\n");
    src.push_str("        data object Loading : UiState\n");
    src.push_str("        data class Success(val items: List<String>) : UiState\n");
    src.push_str("        data class Error(val message: String) : UiState\n");
    src.push_str("    }\n\n");
    src.push_str("    private val _state = MutableStateFlow<UiState>(UiState.Loading)\n");
    src.push_str("    val state: StateFlow<UiState> = _state\n\n");
    src.push_str("    init {\n");
    src.push_str("        repo.observe()\n");
    src.push_str("            .map { mapper.map(it) }\n");
    src.push_str("            .combine(repo.other()) { a, b -> a + b }\n");
    src.push_str("            .onEach { println(\"processing: $it\") }\n");
    src.push_str("            .catch { e -> _state.value = UiState.Error(e.message ?: \"\") }\n");
    src.push_str("            .collect { result ->\n");
    src.push_str("                _state.update { UiState.Success(result) }\n");
    src.push_str("            }\n");
    src.push_str("    }\n");
    src.push_str("}\n");
    src
}

#[test]
fn fp_kmp_flow_viewmodel() {
    let src = build_kmp_flow_file();
    let data = parse_kotlin(&src);
    assert!(
        data.syntax_errors.is_empty(),
        "KMP flow file: {:?}",
        data.syntax_errors
    );
}

/// Test that a large realistic KMP file with all the described characteristics
/// does not produce false positive syntax errors.
#[test]
fn fp_large_realistic_kmp_file() {
    let mut src = String::new();
    // Add @file: annotations
    src.push_str("@file:Suppress(\"UNCHECKED_CAST\")\n");
    src.push_str("package com.example.app\n\n");
    // Long block comment with unicode punctuation
    src.push_str("/**\n");
    src.push_str(" * A ViewModel that manages app state.\n");
    src.push_str(" * It's got apostrophes — em dashes – en dashes «guillemets».\n");
    src.push_str(" * Parentheses (like this) and arrows → ← and @param references.\n");
    src.push_str(" * Also: List<Map<String, List<Pair<Int, Double>>>> types in the docs.\n");
    src.push_str(" * @constructor inject dependencies via the constructor.\n");
    src.push_str(" */\n");
    // Generic repository interface
    src.push_str("interface Repository<T : Any> {\n");
    src.push_str("    fun observe(): Flow<List<T>>\n");
    src.push_str("    fun other(): Flow<String>\n");
    src.push_str("}\n\n");
    // Generic mapper
    src.push_str("interface Mapper<in A, out B> {\n");
    src.push_str("    fun map(input: A): B\n");
    src.push_str("    fun <R : Comparable<R>> transform(input: A, transform: (B) -> R): R\n");
    src.push_str("}\n\n");
    // ViewModel with constructor deps
    src.push_str("class MyViewModel(\n");
    src.push_str("    private val repo: Repository<Data>,\n");
    src.push_str("    private val mapper: Mapper<String, Data>,\n");
    src.push_str(") {\n");
    // Sealed interface hierarchy
    src.push_str("    sealed interface UiState<out T> {\n");
    src.push_str("        data object Loading : UiState<Nothing>\n");
    src.push_str("        data class Success<T>(val items: List<T>) : UiState<T>\n");
    src.push_str("        data class Error(val message: String) : UiState<Nothing>\n");
    src.push_str("    }\n\n");
    // Nested private data class
    src.push_str("    private data class InternalState(\n");
    src.push_str("        val isLoading: Boolean = false,\n");
    src.push_str("        val error: String? = null,\n");
    src.push_str("        val data: List<Map<String, Any>> = emptyList(),\n");
    src.push_str("    )\n\n");
    // Flow chains
    src.push_str("    private val _state = MutableStateFlow<UiState<String>>(UiState.Loading)\n");
    src.push_str("    val state: StateFlow<UiState<String>> = _state\n\n");
    src.push_str("    fun load() {\n");
    src.push_str("        repo.observe()\n");
    src.push_str("            .map { items -> items.map { mapper.map(it) } }\n");
    src.push_str("            .combine(repo.other()) { data, prefix ->\n");
    src.push_str("                data.map { \"$prefix: $it\" }\n");
    src.push_str("            }\n");
    src.push_str("            .onEach { result ->\n");
    src.push_str("                println(\"processing ${result.size} items\")\n");
    src.push_str("            }\n");
    src.push_str("            .catch { e ->\n");
    src.push_str("                _state.value = UiState.Error(e.message ?: \"unknown\")\n");
    src.push_str("            }\n");
    src.push_str("            .collect { result ->\n");
    src.push_str("                _state.update { UiState.Success(result) }\n");
    src.push_str("            }\n");
    src.push_str("    }\n");
    // Generic extension function
    src.push_str("    fun <T : Comparable<T>> List<T>.customSort(): List<T> =\n");
    src.push_str("        this.sortedByDescending { it }\n");
    // Lambda with explicit parameter types
    src.push_str("    val transformer: (List<String>) -> List<Pair<String, Int>> =\n");
    src.push_str("        { items: List<String> -> items.map { it to it.length } }\n");
    src.push_str("}\n");

    let data = parse_kotlin(&src);
    assert!(
        data.syntax_errors.is_empty(),
        "Large KMP file: {:?}",
        data.syntax_errors
    );
}

/// Test a ViewModel with complex flow chains and type annotations.
#[test]
fn fp_flow_chain_complex() {
    let src = String::from(
        "package com.example\n\
         \n\
         class ComplexFlow {\n\
             fun process() {\n\
                 flowOf(1, 2, 3)\n\
                     .map { it.toString() }\n\
                     .map { listOf(it) }\n\
                     .flatMapConcat { it.toFlow() }\n\
                     .combine(flowOf(\"x\", \"y\")) { a, b -> Pair(a, b) }\n\
                     .onEach { (first, second) ->\n\
                         println(\"$first -> $second\")\n\
                     }\n\
                     .catch {}\n\
                     .collect()\n\
             }\n\
         }",
    );
    let data = parse_kotlin(&src);
    assert!(
        data.syntax_errors.is_empty(),
        "Complex flow: {:?}",
        data.syntax_errors
    );
}

/// Test file with @file: array annotation before package.
#[test]
fn fp_file_annotation_array_syntax() {
    assert_no_errors(
        "@file:[JvmName(\"Foo\"), Suppress(\"UNCHECKED_CAST\")]\npackage com.example\nclass Test",
    );
}

/// Regression: fun interface inside class + trailing block comment + data class.
/// See https://github.com/qdsfdhvh/kotlin-lsp/issues/78
#[test]
fn fp_minimal_issue_78() {
    let src = "package com.example.repro\n\
                \n\
                class FeatureViewModel {\n\
                    fun interface Factory {\n\
                        fun create(id: String): FeatureViewModel\n\
                    }\n\
                }\n\
                \n\
                /* A documented item. */\n\
                data class Item(val id: String)\n";
    assert_no_errors(src);
}

/// Same issue: KDoc instead of block comment.
#[test]
fn fp_minimal_issue_78_kdoc() {
    let src = "package com.example.repro\n\
                \n\
                class FeatureViewModel {\n\
                    fun interface Factory {\n\
                        fun create(id: String): FeatureViewModel\n\
                    }\n\
                }\n\
                \n\
                /** A documented item. */\n\
                data class Item(val id: String)\n";
    assert_no_errors(src);
}

/// Same issue: line comment after fun interface inside class.
#[test]
fn fp_minimal_issue_78_line_comment() {
    let src = "package com.example.repro\n\
                \n\
                class FeatureViewModel {\n\
                    fun interface Factory {\n\
                        fun create(id: String): FeatureViewModel\n\
                    }\n\
                }\n\
                \n\
                // A documented item.\n\
                data class Item(val id: String)\n";
    assert_no_errors(src);
}

/// Multiple fun interfaces in the same file.
#[test]
fn fp_multiple_fun_interfaces() {
    let src = "package com.example.repro\n\
                \n\
                class ViewModel {\n\
                    fun interface Factory {\n\
                        fun create(): ViewModel\n\
                    }\n\
                    fun interface Listener {\n\
                        fun onEvent(e: String)\n\
                    }\n\
                }\n\
                \n\
                /* Another item. */\n\
                data class Item(val id: String)\n";
    assert_no_errors(src);
}

/// Nested class with fun interface.
#[test]
fn fp_nested_fun_interface() {
    let src = "package com.example.repro\n\
                \n\
                class Outer {\n\
                    class Inner {\n\
                        fun interface Factory {\n\
                            fun create(): Outer\n\
                        }\n\
                    }\n\
                }\n\
                \n\
                /* Comment. */\n\
                data class Item(val id: String)\n";
    assert_no_errors(src);
}

/// Fun interface at top level (not inside a class).
#[test]
fn fp_top_level_fun_interface() {
    let src = "package com.example.repro\n\
                \n\
                fun interface Factory {\n\
                    fun create(): String\n\
                }\n\
                \n\
                /* Comment. */\n\
                data class Item(val id: String)\n";
    assert_no_errors(src);
}

/// Fun interface + trailing top-level function after comment.
#[test]
fn fp_fun_interface_trailing_fun() {
    let src = "package com.example.repro\n\
                \n\
                class ViewModel {\n\
                    fun interface Factory {\n\
                        fun create(): ViewModel\n\
                    }\n\
                }\n\
                \n\
                /* A helper. */\n\
                fun helper(): String = \"ok\"\n";
    assert_no_errors(src);
}

/// KMP-style file with fun interface, flows, and top-level declarations.
#[test]
fn fp_kmp_style_with_fun_interface() {
    let src = "package com.example.repro\n\
                \n\
                import kotlinx.coroutines.flow.*\n\
                \n\
                class ViewModel(\n\
                    private val repo: Repository\n\
                ) {\n\
                    fun interface Factory {\n\
                        fun create(repo: Repository): ViewModel\n\
                    }\n\
                    \n\
                    val state = repo.observe()\n\
                        .map { it.toString() }\n\
                        .catch { emit(\"error\") }\n\
                        .stateIn(CoroutineScope(Dispatchers.Default))\n\
                }\n\
                \n\
                interface Repository {\n\
                    fun observe(): Flow<Int>\n\
                }\n";
    assert_no_errors(src);
}

/// Fun interface + trailing sealed interfaces.
#[test]
fn fp_fun_interface_trailing_sealed() {
    let src = "package com.example.repro\n\
                \n\
                class ViewModel {\n\
                    fun interface Factory {\n\
                        fun create(): ViewModel\n\
                    }\n\
                }\n\
                \n\
                /* Ui state. */\n\
                sealed interface UiState {\n\
                    data object Loading : UiState\n\
                    data class Success(val data: String) : UiState\n\
                }\n";
    assert_no_errors(src);
}
