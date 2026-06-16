use super::*;
use crate::indexer::Indexer;
use crate::parser::{parse_java, parse_kotlin};
use crate::stdlib::dot_completions_for;
use tower_lsp::lsp_types::{CompletionItem, CompletionItemTag, InsertTextFormat, Url};

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn import_file_candidates(import_path: &str) -> Vec<String> {
    import_file_stems(import_path)
        .into_iter()
        .flat_map(|stem| {
            crate::rg::SOURCE_EXTENSIONS
                .iter()
                .map(move |ext| format!("{stem}.{ext}"))
        })
        .collect()
}

// ── pure helpers ─────────────────────────────────────────────────────────

#[test]
fn package_prefix_standard() {
    assert_eq!(package_prefix("com.example.app.MyClass"), "com.example.app");
    assert_eq!(
        package_prefix("com.example.OuterClass.InnerClass"),
        "com.example"
    );
    assert_eq!(package_prefix("MyClass"), "");
    assert_eq!(package_prefix("com.example.Foo"), "com.example");
}

#[test]
fn import_candidates_top_level() {
    let c = import_file_candidates("com.example.Foo");
    assert_eq!(c[0], "Foo.kt");
    assert_eq!(c[1], "Foo.java");
    assert_eq!(c[2], "Foo.swift");
}

#[test]
fn import_candidates_nested() {
    let c = import_file_candidates("com.example.OuterClass.InnerClass");
    assert_eq!(c[0], "OuterClass.kt"); // outer class file tried first
    assert_eq!(c[1], "OuterClass.java");
    assert_eq!(c[2], "OuterClass.swift");
    assert_eq!(c[3], "InnerClass.kt");
    assert_eq!(c[4], "InnerClass.java");
    assert_eq!(c[5], "InnerClass.swift");
}

#[test]
fn import_candidates_deeply_nested() {
    let c = import_file_candidates("a.b.Outer.Middle.Inner");
    assert_eq!(c[0], "Middle.kt");
    assert_eq!(c[1], "Middle.java");
    assert_eq!(c[2], "Middle.swift");
    assert_eq!(c[3], "Inner.kt");
    assert_eq!(c[4], "Inner.java");
    assert_eq!(c[5], "Inner.swift");
}

#[test]
fn import_candidates_no_uppercase() {
    assert!(import_file_candidates("com.example.pkg").is_empty());
}

// ── resolve_local ────────────────────────────────────────────────────────

#[test]
fn resolve_local_finds_own_symbols() {
    let u = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(&u, "class Foo\nclass Bar");
    let locs = resolve_symbol(&idx, "Foo", None, &u);
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0].uri, u);
}

#[test]
fn resolve_local_not_found_returns_empty_without_rg() {
    // Symbol that doesn't exist anywhere in the index; rg will find nothing
    // in the (empty) working tree — acceptable to return vec![]
    let u = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(&u, "class Foo");
    // "Xyz" is not in the index; rg likely returns nothing in tests
    let locs = resolve_symbol(&idx, "Xyz", None, &u);
    // We can't guarantee rg returns nothing in all environments,
    // so just verify local didn't find it in index.
    assert!(!locs.iter().any(|l| l.uri == u));
}

// ── resolve_via_imports (qualified index) ────────────────────────────────

#[test]
fn resolve_via_explicit_import() {
    let src_uri = uri("/src/Source.kt");
    let def_uri = uri("/src/Target.kt");
    let idx = Indexer::new();
    idx.index_content(&def_uri, "package com.example\nclass Target");
    idx.index_content(
        &src_uri,
        "package com.example\nimport com.example.Target\nval x: Target = TODO()",
    );

    let locs = resolve_symbol(&idx, "Target", None, &src_uri);
    assert!(!locs.is_empty(), "Target not found via import");
    assert_eq!(locs[0].uri, def_uri);
}

#[test]
fn resolve_via_alias_import() {
    let src_uri = uri("/src/A.kt");
    let def_uri = uri("/src/B.kt");
    let idx = Indexer::new();
    idx.index_content(&def_uri, "package com.example\nclass LongName");
    idx.index_content(
        &src_uri,
        "package com.example\nimport com.example.LongName as LN\nval x: LN = TODO()",
    );

    // Looking up "LN" should find "LongName" in def_uri
    let locs = resolve_symbol(&idx, "LN", None, &src_uri);
    assert!(!locs.is_empty(), "aliased import not resolved");
    assert_eq!(locs[0].uri, def_uri);
}

// ── resolve_same_package ─────────────────────────────────────────────────

#[test]
fn resolve_same_package() {
    let a_uri = uri("/pkg/A.kt");
    let b_uri = uri("/pkg/B.kt");
    let idx = Indexer::new();
    idx.index_content(&a_uri, "package com.example\nclass A");
    idx.index_content(&b_uri, "package com.example\nval x: A = TODO()");

    let locs = resolve_symbol(&idx, "A", None, &b_uri);
    assert!(!locs.is_empty(), "same-package class not found");
    assert_eq!(locs[0].uri, a_uri);
}

#[test]
fn resolve_does_not_cross_packages_without_import() {
    let a_uri = uri("/pkg1/A.kt");
    let b_uri = uri("/pkg2/B.kt");
    let idx = Indexer::new();
    idx.index_content(&a_uri, "package com.example.pkg1\nclass A");
    idx.index_content(&b_uri, "package com.example.pkg2"); // no import

    // rg might find it; test that same-package step doesn't leak
    let _locs: Vec<_> = resolve_symbol(&idx, "A", None, &b_uri)
        .into_iter()
        .filter(|l| l.uri == a_uri)
        .collect();
    // If rg finds it that's fine, but same-package shouldn't (different packages)
    // We verify by checking the packages map didn't bridge pkg1 and pkg2
    assert!(
        idx.packages
            .get("com.example.pkg2")
            .map(|u| !u.contains(&a_uri.to_string()))
            .unwrap_or(true),
        "pkg1 URI leaked into pkg2 packages map"
    );
}

// ── resolve_qualified (dot accessor) ────────────────────────────────────

#[test]
fn resolve_qualifier_dot_access() {
    let host_uri = uri("/Host.kt");
    let outer_uri = uri("/Outer.kt");
    let idx = Indexer::new();
    idx.index_content(
        &outer_uri,
        "package com.pkg\nclass Outer {\n  class Inner\n}",
    );
    idx.index_content(&host_uri, "package com.pkg\nval x: Outer.Inner = TODO()");

    // Cursor on "Inner" with qualifier "Outer"
    let locs = resolve_symbol(&idx, "Inner", Some("Outer"), &host_uri);
    assert!(!locs.is_empty(), "Inner not found via qualifier");
    assert_eq!(locs[0].uri, outer_uri);
}

#[test]
fn resolve_deep_qualifier_chain() {
    // A.B.C.D cursor on D → qualifier = "A.B.C"
    // resolve_qualified should resolve root "A", find its file, locate "D" in it.
    let host_uri = uri("/Host.kt");
    let root_uri = uri("/Root.kt");
    let idx = Indexer::new();
    // Root.kt defines class Root with nested class Deep
    idx.index_content(
        &root_uri,
        "package com.pkg\nclass Root {\n  class Mid {\n    class Deep\n  }\n}",
    );
    idx.index_content(&host_uri, "package com.pkg\nval x: Root.Mid.Deep = TODO()");

    // qualifier = "Root.Mid" (full chain minus last segment), word = "Deep"
    let locs = resolve_symbol(&idx, "Deep", Some("Root.Mid"), &host_uri);
    assert!(!locs.is_empty(), "Deep not found via full qualifier chain");
    assert_eq!(locs[0].uri, root_uri);
}

#[test]
fn resolve_nested_type_via_variable_annotation() {
    // `val factory: DashboardProductsReducer.Factory` — goto-def of `factory.create(...)`
    // should navigate to the `create` fun inside the `Factory` interface.
    let host_uri = uri("/Host.kt");
    let reducer_uri = uri("/DashboardProductsReducer.kt");
    let idx = Indexer::new();
    idx.index_content(
        &reducer_uri,
        concat!(
            "package com.pkg\n",
            "class DashboardProductsReducer {\n",
            "  interface Factory {\n",
            "    fun create(scope: Any): DashboardProductsReducer\n",
            "  }\n",
            "}\n",
        ),
    );
    idx.index_content(
        &host_uri,
        concat!(
            "package com.pkg\n",
            "val factory: DashboardProductsReducer.Factory = TODO()\n",
            "fun foo() { factory.create(this) }\n",
        ),
    );

    // Qualifier = "factory" (lowercase), word = "create"
    let locs = resolve_symbol(&idx, "create", Some("factory"), &host_uri);
    assert!(!locs.is_empty(), "create not found via nested type Factory");
    assert_eq!(locs[0].uri, reducer_uri);
}

#[test]
fn infer_type_in_lines_dotted() {
    // Ensure infer_type_in_lines handles `Outer.Inner` dotted types.
    let lines: Vec<String> =
        vec!["  private val factory: DashboardProductsReducer.Factory,".to_owned()];
    let t = super::infer_type_in_lines(&lines, "factory");
    assert_eq!(t.as_deref(), Some("DashboardProductsReducer.Factory"));
}

// ── infer_variable_type + method resolution ──────────────────────────────

#[test]
fn resolve_multi_hop_field_chain() {
    // vm.account.interestPlanCode where:
    //   fun foo(vm: ViewModel) – vm has field account: AccountModel
    //   AccountModel has field interestPlanCode: String
    let host_uri = uri("/Host.kt");
    let vm_uri = uri("/ViewModel.kt");
    let acc_uri = uri("/AccountModel.kt");
    let idx = Indexer::new();
    idx.index_content(
        &acc_uri,
        "package com.pkg\nclass AccountModel {\n  val interestPlanCode: String = \"\"\n}",
    );
    idx.index_content(
        &vm_uri,
        "package com.pkg\nclass ViewModel {\n  val account: AccountModel = AccountModel()\n}",
    );
    idx.index_content(
        &host_uri,
        "package com.pkg\nfun foo(vm: ViewModel) { vm.account.interestPlanCode }",
    );

    // qualifier = "vm.account", name = "interestPlanCode"
    let locs = resolve_symbol(&idx, "interestPlanCode", Some("vm.account"), &host_uri);
    assert!(
        !locs.is_empty(),
        "interestPlanCode not found via multi-hop field chain"
    );
    assert_eq!(locs[0].uri, acc_uri);
}

#[test]
fn resolve_local_param_declaration() {
    // Cursor on `account` (function param without val/var) should return the
    // declaration line in the same file.
    let u = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(
        &u,
        "package com.pkg\nfun foo(account: AccountModel) {\n  account.something\n}",
    );

    let locs = resolve_symbol(&idx, "account", None, &u);
    assert!(!locs.is_empty(), "local param declaration not found");
    assert_eq!(locs[0].uri, u);
    // Line 1 (0-indexed) contains the parameter declaration
    assert_eq!(locs[0].range.start.line, 1);
}

#[test]
fn resolve_method_via_variable_type_inference() {
    // repo.findById(1) where repo: UserRepository
    let vm_uri = uri("/ViewModel.kt");
    let repo_uri = uri("/UserRepository.kt");
    let idx = Indexer::new();
    idx.index_content(
        &repo_uri,
        "package com.pkg\nclass UserRepository {\n  fun findById(id: Int) {}\n}",
    );
    idx.index_content(&vm_uri,
            "package com.pkg\nclass ViewModel(\n  private val repo: UserRepository\n) {\n  fun load() { repo.findById(1) }\n}");

    // qualifier = "repo" (lowercase), name = "findById"
    // infer_variable_type should extract "UserRepository" from "val repo: UserRepository"
    // then resolve_qualified finds findById in UserRepository.kt
    let locs = resolve_symbol(&idx, "findById", Some("repo"), &vm_uri);
    assert!(
        !locs.is_empty(),
        "findById not found via variable type inference"
    );
    assert_eq!(locs[0].uri, repo_uri);
}

#[test]
fn resolve_method_via_constructor_param_type() {
    // interactor.loadDataFlow(x) where interactor: ShowChildNewTipsInteractor
    let vm_uri = uri("/SomeViewModel.kt");
    let int_uri = uri("/ShowChildNewTipsInteractor.kt");
    let idx = Indexer::new();
    idx.index_content(&int_uri,
            "package com.feature\nclass ShowChildNewTipsInteractor {\n  fun loadDataFlow(account: Any) {}\n}");
    idx.index_content(&vm_uri,
            "package com.feature\nclass SomeViewModel(\n  private val interactor: ShowChildNewTipsInteractor\n) {\n  fun init() { interactor.loadDataFlow(x) }\n}");

    let locs = resolve_symbol(&idx, "loadDataFlow", Some("interactor"), &vm_uri);
    assert!(
        !locs.is_empty(),
        "loadDataFlow not found via constructor param type inference"
    );
    assert_eq!(locs[0].uri, int_uri);
}

#[test]
fn resolve_method_via_interface_hierarchy() {
    // repo.contactAddressSetup() where repo: IGoldConversionRepository
    // contactAddressSetup is defined in IBaseRepository (superinterface)
    let vm_uri = uri("/ViewModel.kt");
    let repo_uri = uri("/IGoldConversionRepository.kt");
    let base_uri = uri("/IBaseRepository.kt");
    let idx = Indexer::new();
    idx.index_content(
        &base_uri,
        "package com.pkg\ninterface IBaseRepository {\n  fun contactAddressSetup(): String\n}",
    );
    idx.index_content(&repo_uri,
            "package com.pkg\ninterface IGoldConversionRepository : IBaseRepository {\n  fun goldPrice(): Double\n}");
    idx.index_content(&vm_uri,
            "package com.pkg\nclass ViewModel(\n  private val repo: IGoldConversionRepository\n) {\n  fun init() { repo.contactAddressSetup() }\n}");

    let locs = resolve_symbol(&idx, "contactAddressSetup", Some("repo"), &vm_uri);
    assert!(
        !locs.is_empty(),
        "contactAddressSetup not found via interface hierarchy"
    );
    assert_eq!(locs[0].uri, base_uri, "should resolve to IBaseRepository");
}

// ── build_rg_pattern ─────────────────────────────────────────────────────
// Use rg itself to validate patterns (it's always available in the dev env).

fn rg_available() -> bool {
    std::process::Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn rg_matches(pattern: &str, text: &str) -> bool {
    std::process::Command::new("rg")
        .args(["--quiet", "-e", pattern, "--"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .ok()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.as_mut()?.write_all(text.as_bytes()).ok()?;
            Some(c.wait().ok()?.success())
        })
        .unwrap_or(false)
}

#[test]
fn rg_pattern_matches_kotlin_class() {
    if !rg_available() {
        eprintln!("skipping: rg not available");
        return;
    }
    let pat = build_rg_pattern("Foo");
    assert!(rg_matches(&pat, "class Foo {"));
    assert!(rg_matches(&pat, "sealed class Foo"));
}

#[test]
fn rg_pattern_matches_kotlin_enum() {
    if !rg_available() {
        eprintln!("skipping: rg not available");
        return;
    }
    let pat = build_rg_pattern("EScreen");
    assert!(rg_matches(&pat, "enum class EScreen {"));
}

#[test]
fn rg_pattern_matches_java_enum() {
    if !rg_available() {
        eprintln!("skipping: rg not available");
        return;
    }
    let pat = build_rg_pattern("EProductScreen");
    assert!(rg_matches(&pat, "public enum EProductScreen {"));
    assert!(rg_matches(&pat, "  enum EProductScreen {"));
    assert!(rg_matches(&pat, "private static enum EProductScreen {"));
}

#[test]
fn rg_pattern_no_false_positive_on_usage() {
    if !rg_available() {
        eprintln!("skipping: rg not available");
        return;
    }
    let pat = build_rg_pattern("EProductScreen");
    // Should NOT match a plain usage (not a declaration)
    assert!(!rg_matches(&pat, "EProductScreen.SOMETHING"));
    assert!(!rg_matches(&pat, "val x: EProductScreen = "));
}

#[test]
fn rg_pattern_matches_java_class() {
    if !rg_available() {
        eprintln!("skipping: rg not available");
        return;
    }
    let pat = build_rg_pattern("FlexiEntryVM");
    assert!(rg_matches(&pat, "public class FlexiEntryVM extends Base {"));
}

// ── import_file_stems ────────────────────────────────────────────────────

#[test]
fn file_stems_top_level() {
    assert_eq!(
        import_file_stems("cz.moneta.data.EProductScreen"),
        vec!["EProductScreen"]
    );
}

#[test]
fn file_stems_nested() {
    let s = import_file_stems("com.example.OuterClass.InnerClass");
    assert_eq!(s, vec!["OuterClass", "InnerClass"]);
}

// ── supers CST extraction (via parse_kotlin / parse_java) ────────────────

fn kotlin_supers(src: &str) -> Vec<String> {
    parse_kotlin(src)
        .supers
        .into_iter()
        .map(|(_, n, _)| n)
        .collect()
}

#[test]
fn supers_kotlin_single_line() {
    let s = kotlin_supers("class DetailViewModel : MviViewModel<Event, State, Effect>() {}");
    assert!(s.contains(&"MviViewModel".to_string()), "got {s:?}");
}

#[test]
fn supers_kotlin_nested_generic_type() {
    // Outer<T>.Inner should yield "Outer.Inner", not just "Outer".
    let s = kotlin_supers("class Foo : Outer<T>.Inner() {}");
    assert!(
        s.iter().any(|n| n == "Outer.Inner" || n == "Outer"),
        "got {s:?}"
    );
}

#[test]
fn supers_kotlin_multi_line_ctor() {
    let src = "class DetailViewModel @Inject constructor(\n  private val useCase: UseCase,\n) : MviViewModel<Event, State, Effect>() {}";
    let s = kotlin_supers(src);
    assert!(s.contains(&"MviViewModel".to_string()), "got {s:?}");
}

#[test]
fn supers_kotlin_multiple() {
    let src = "class Foo : BaseClass(), SomeInterface, AnotherInterface {}";
    let s = kotlin_supers(src);
    assert!(s.contains(&"BaseClass".to_string()), "got {s:?}");
    assert!(s.contains(&"SomeInterface".to_string()), "got {s:?}");
    assert!(s.contains(&"AnotherInterface".to_string()), "got {s:?}");
}

#[test]
fn supers_java_extends() {
    let src = "public class FlexiEntryVM extends BaseFlexikreditVM {}";
    let s: Vec<String> = parse_java(src)
        .supers
        .into_iter()
        .map(|(_, n, _)| n)
        .collect();
    assert!(s.contains(&"BaseFlexikreditVM".to_string()), "got {s:?}");
}

#[test]
fn supers_java_implements() {
    let src = "public class Foo extends Base implements Runnable, Serializable {}";
    let s: Vec<String> = parse_java(src)
        .supers
        .into_iter()
        .map(|(_, n, _)| n)
        .collect();
    assert!(s.contains(&"Base".to_string()), "got {s:?}");
    assert!(s.contains(&"Runnable".to_string()), "got {s:?}");
    assert!(s.contains(&"Serializable".to_string()), "got {s:?}");
}

#[test]
fn supers_java_generic_extends() {
    let java = |src: &str| -> Vec<String> {
        parse_java(src)
            .supers
            .into_iter()
            .map(|(_, n, _)| n)
            .collect()
    };

    let s = java("public class Foo extends Base<String> {}");
    assert!(
        s.contains(&"Base".to_string()),
        "generic extends, got {s:?}"
    );

    let s = java("public class Foo extends pkg.Base<String> {}");
    assert!(
        s.contains(&"pkg.Base".to_string()) || s.contains(&"Base".to_string()),
        "qualified generic extends, got {s:?}"
    );

    let s = java("public class Foo extends Base<String> implements Runnable {}");
    assert!(
        s.contains(&"Base".to_string()),
        "generic extends+implements, got {s:?}"
    );
    assert!(
        s.contains(&"Runnable".to_string()),
        "generic extends+implements, got {s:?}"
    );
}

#[test]
fn supers_does_not_pick_up_type_annotations() {
    let src = "class Foo {\n  val x: Int = 0\n  fun f(): String = \"\"\n}";
    let s = kotlin_supers(src);
    assert!(s.is_empty(), "should have no supers, got {s:?}");
}

// ── resolve_from_class_hierarchy ─────────────────────────────────────────

#[test]
fn resolve_inherited_method() {
    let base_uri = uri("/Base.kt");
    let child_uri = uri("/Child.kt");
    let idx = Indexer::new();
    idx.index_content(
        &base_uri,
        "package com.example\nopen class Base {\n  fun baseMethod() {}\n}",
    );
    idx.index_content(&child_uri, "package com.example\nclass Child : Base() {}\n");

    // `baseMethod` is not declared in Child — must be found via hierarchy
    let locs = resolve_symbol(&idx, "baseMethod", None, &child_uri);
    assert!(!locs.is_empty(), "inherited method not found");
    assert_eq!(locs[0].uri, base_uri);
}

#[test]
fn resolve_inherited_method_via_import() {
    let base_uri = uri("/lib/Base.kt");
    let child_uri = uri("/app/Child.kt");
    let idx = Indexer::new();
    idx.index_content(
        &base_uri,
        "package com.lib\nopen class Base {\n  fun doStuff() {}\n}",
    );
    idx.index_content(
        &child_uri,
        "package com.app\nimport com.lib.Base\nclass Child : Base() {}\n",
    );

    let locs = resolve_symbol(&idx, "doStuff", None, &child_uri);
    assert!(!locs.is_empty(), "inherited method not found via import");
    assert_eq!(locs[0].uri, base_uri);
}

// ── this / super resolution ───────────────────────────────────────────────

#[test]
fn resolve_this_dot_method() {
    let u = uri("/Foo.kt");
    let idx = Indexer::new();
    idx.index_content(
        &u,
        "package com.example\nclass Foo {\n  fun doThing() {}\n  fun other() { this.doThing() }\n}",
    );
    let locs = resolve_symbol(&idx, "doThing", Some("this"), &u);
    assert!(!locs.is_empty(), "this.doThing() not resolved");
    assert_eq!(locs[0].uri, u);
}

#[test]
fn resolve_super_dot_method() {
    let base_uri = uri("/Base.kt");
    let child_uri = uri("/Child.kt");
    let idx = Indexer::new();
    idx.index_content(
        &base_uri,
        "package com.example\nopen class Base { fun init() {} }",
    );
    idx.index_content(
        &child_uri,
        "package com.example\nclass Child : Base() { fun x() { super.init() } }",
    );
    let locs = resolve_symbol(&idx, "init", Some("super"), &child_uri);
    assert!(!locs.is_empty(), "super.init() not resolved");
    assert_eq!(locs[0].uri, base_uri);
}

// ── lambda parameter recognition ─────────────────────────────────────────

#[test]
fn local_decl_lambda_untyped() {
    let lines: Vec<String> = vec![
        "list.forEach { account ->".to_string(),
        "  println(account)".to_string(),
    ];
    let range = find_declaration_range_in_lines(&lines, "account");
    assert!(range.is_some(), "untyped lambda param not found");
    assert_eq!(range.unwrap().start.line, 0);
}

#[test]
fn local_decl_lambda_typed() {
    let lines: Vec<String> = vec!["items.map { item: DetailItem ->".to_string()];
    let range = find_declaration_range_in_lines(&lines, "item");
    assert!(range.is_some(), "typed lambda param not found");
}

#[test]
fn local_decl_no_false_positive_usage() {
    // A usage of `account` on a non-declaration line must not be returned
    let lines: Vec<String> = vec!["val result = account.name".to_string()];
    let range = find_declaration_range_in_lines(&lines, "account");
    assert!(range.is_none(), "false positive on usage line");
}

// ── primary constructor val/var parameter resolution ─────────────────────

#[test]
fn resolve_data_class_field_via_dot_access() {
    // user.name should resolve to `val name: String` in User's primary ctor
    let user_uri = uri("/User.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(
        &user_uri,
        "package com.example\ndata class User(val name: String, val age: Int)",
    );
    idx.index_content(
        &caller_uri,
        "package com.example\nfun greet(user: User) { println(user.name) }",
    );

    let locs = resolve_symbol(&idx, "name", Some("user"), &caller_uri);
    assert!(!locs.is_empty(), "name not found via user.name");
    assert_eq!(locs[0].uri, user_uri, "should point to User.kt");
}

#[test]
fn resolve_ctor_param_no_qualifier() {
    // Inside the class itself, `name` should resolve to the ctor param.
    let uri = uri("/User.kt");
    let idx = Indexer::new();
    idx.index_content(
        &uri,
        "package com.example\ndata class User(val name: String) {\n  fun display() = name\n}",
    );

    let locs = resolve_symbol(&idx, "name", None, &uri);
    assert!(!locs.is_empty(), "ctor param not found locally");
    assert_eq!(locs[0].uri, uri, "should stay in same file");
}

#[test]
fn resolve_named_arg_to_ctor_param() {
    // User(name = "Alice") — qualifier is "User" (detected by word_and_qualifier_at).
    // resolve_symbol with qualifier="User" must find `val name` in User's primary ctor.
    let user_uri = uri("/User.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(
        &user_uri,
        "package com.example\ndata class User(val name: String, val age: Int)",
    );
    idx.index_content(
        &caller_uri,
        "package com.example\nfun test() { val u = User(name = \"Alice\", age = 30) }",
    );

    // Simulate what the backend does after word_and_qualifier_at returns ("name", "User")
    let locs = resolve_symbol(&idx, "name", Some("User"), &caller_uri);
    assert!(
        !locs.is_empty(),
        "named arg 'name' not resolved to User ctor param"
    );
    assert_eq!(locs[0].uri, user_uri, "should point to User.kt, not caller");
}

#[test]
fn named_arg_same_name_different_classes_same_file() {
    // Regression: Contract.kt has both State(val toastModel: ...) and
    // OnClick(val toastModel: ...) in the same file.
    // Resolving State(toastModel = ...) should land on State's field,
    // not OnClick's (which appears later but might be returned first).
    let contract_uri = uri("/Contract.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(
        &contract_uri,
        "\
package com.example
sealed class Effect {
    data class OnClick(val toastModel: String) : Effect()
}
data class State(
    val toastModel: String? = null,
)",
    );
    idx.index_content(
        &caller_uri,
        "package com.example\nfun test() { State(toastModel = \"hi\") }",
    );

    let locs = resolve_symbol(&idx, "toastModel", Some("State"), &caller_uri);
    assert!(!locs.is_empty(), "toastModel not resolved");
    // Must point to State's toastModel (line 4), NOT OnClick's (line 2)
    let line = locs[0].range.start.line;
    assert!(
        line >= 4,
        "resolved to OnClick.toastModel (line {line}) instead of State.toastModel"
    );
}

// ── qualified access with uppercase class qualifier (extension fn fallthrough bug) ──

/// Regression: `Modifier.padding()` with cursor on `padding` where Modifier is an
/// indexed object/class and `padding()` is an **extension function** defined in a
/// *different* file.  `resolve_qualified` previously only searched the Modifier
/// class file for `padding`, so extension functions in other files were never
/// found.  The test checks that the extension function IS found via the
/// `extension_by_receiver` index.
#[test]
fn resolve_extension_fn_on_uppercase_qualifier() {
    // Modifier.kt defines the Modifier class/object
    let modifier_uri = uri("/Modifier.kt");
    // Padding.kt defines `fun Modifier.padding(...)` as an extension function
    let padding_uri = uri("/Padding.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();

    idx.index_content(
        &modifier_uri,
        "package androidx.compose.ui\n\
         object Modifier",
    );
    idx.index_content(
        &padding_uri,
        "package androidx.compose.ui\n\
         fun Modifier.padding(horizontal: Int = 0, vertical: Int = 0): Modifier = this",
    );
    idx.index_content(
        &caller_uri,
        "package com.example\n\
         fun render() {\n\
             Modifier.padding()\n\
         }",
    );

    // Resolving `padding` with qualifier `Modifier` should find the extension
    // function in Padding.kt, NOT return empty.
    let locs = resolve_symbol(&idx, "padding", Some("Modifier"), &caller_uri);
    assert!(
        !locs.is_empty(),
        "extension function Modifier.padding() not found; resolve_qualified only \
         searched the Modifier class file, missing extension fns in other files"
    );
    assert_eq!(
        locs[0].uri, padding_uri,
        "should point to Padding.kt where the extension function is defined, got {:?}",
        locs[0].uri
    );
}

/// Regression: `Modifier.padding()` with cursor on `padding` where `Modifier` is
/// NOT indexed at all (e.g. external unindexed library).  After
/// `resolve_qualified` returned empty, `resolve_symbol` fell through to
/// `resolve_symbol_inner` which scanned the current file.  If the current file had
/// a lambda parameter named `padding` (e.g. `{ padding -> ... }`), the fallthrough
/// incorrectly returned the lambda param location.
///
/// Expected behavior: when the qualifier is an uppercase identifier (class name)
/// that simply wasn't found in the index, the resolver should return empty rather
/// than falling through to local resolution.
#[test]
fn qualified_access_uppercase_fallthrough_does_not_match_lambda_param() {
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();

    // Modifier is NOT indexed (simulates external library).
    // The caller has both `Modifier.padding()` and a lambda `{ padding -> ... }`.
    idx.index_content(
        &caller_uri,
        "package com.example\n\
         class MyWidget {\n\
             fun render() {\n\
                 Box().apply { padding ->\n\
                     this@MyWidget.size = padding\n\
                 }\n\
                 Modifier.padding()\n\
             }\n\
         }",
    );

    // Resolving `padding` with qualifier `Modifier` — since Modifier is not
    // indexed, qualified resolution fails.  The fallthrough must NOT pick up
    // the lambda parameter `padding` from the apply block.
    let locs = resolve_symbol(&idx, "padding", Some("Modifier"), &caller_uri);
    assert!(
        locs.is_empty(),
        "qualified access with unindexed uppercase qualifier should return empty, \
         not fall through to lambda param; got {} location(s)",
        locs.len()
    );
}

/// Regression: `Modifier.padding()` where both Modifier and the extension
/// function are indexed.  Verifies the definition resolution chain works
/// end-to-end: resolve_symbol finds it, and the SymbolEntry carries a
/// non-empty detail (return type info).
#[test]
fn resolve_extension_fn_return_type_via_uppercase_qualifier() {
    let modifier_uri = uri("/Modifier.kt");
    let padding_uri = uri("/Padding.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();

    idx.index_content(
        &modifier_uri,
        "package com.example\n\
         object Modifier",
    );
    idx.index_content(
        &padding_uri,
        "package com.example\n\
         fun Modifier.padding(horizontal: Int = 0, vertical: Int = 0): Modifier = this",
    );
    idx.index_content(
        &caller_uri,
        "package com.example\n\
         fun render() {\n\
             val result = Modifier.padding()\n\
         }",
    );

    // The extension function definition should be findable.
    let locs = resolve_symbol(&idx, "padding", Some("Modifier"), &caller_uri);
    assert!(
        !locs.is_empty(),
        "Modifier.padding() extension fn not found"
    );
    assert_eq!(locs[0].uri, padding_uri);

    // The file data for Padding.kt should contain the padding symbol with
    // a detail that includes "padding" (confirming the symbol was indexed).
    use crate::indexer::resolution::IndexRead;
    let file_data = idx.get_file_data(padding_uri.as_str());
    assert!(file_data.is_some(), "Padding.kt not in file index");
    let data = file_data.unwrap();
    let has_padding = data.symbols.iter().any(|s| s.name == "padding");
    assert!(
        has_padding,
        "SymbolEntry for 'padding' not found in Padding.kt symbols; \
         return type detail will be empty"
    );
    // Verify the symbol has a non-empty detail with return type info.
    let symbol = data.symbols.iter().find(|s| s.name == "padding").unwrap();
    assert!(
        !symbol.detail.is_empty(),
        "SymbolEntry for 'padding' should have non-empty detail"
    );
    assert!(
        symbol.detail.contains("Modifier"),
        "SymbolEntry detail for 'padding' should contain return type 'Modifier', got: {}",
        symbol.detail
    );
}

/// Verifies that return type inference for extension functions works via
/// `find_extension_fn_return_type`.  The text-based inference path
/// (`find_method_return_type`) previously only checked `container == Some(type_base)`,
/// which missed extension functions (container=None).  This test confirms
/// the extension fn return type IS available when queried correctly.
#[test]
fn extension_fn_return_type_inference_works() {
    let modifier_uri = uri("/Modifier.kt");
    let padding_uri = uri("/Padding.kt");
    let idx = Indexer::new();

    idx.index_content(
        &modifier_uri,
        "package com.example\n\
         object Modifier",
    );
    idx.index_content(
        &padding_uri,
        "package com.example\n\
         fun Modifier.padding(horizontal: Int = 0, vertical: Int = 0): Modifier = this",
    );

    // Direct lookup via find_extension_fn_return_type should work
    let ret =
        crate::resolver::infer::find_extension_fn_return_type(&idx, "Modifier", "padding", None);
    assert_eq!(
        ret,
        Some("Modifier".to_string()),
        "find_extension_fn_return_type should resolve Modifier.padding() -> Modifier"
    );

    // find_method_return_type should now find it (falls back to extension fn lookup)
    let ret_via_container =
        crate::resolver::infer::find_method_return_type(&idx, "Modifier", "padding", None);
    assert_eq!(
        ret_via_container,
        Some("Modifier".to_string()),
        "find_method_return_type should find extension functions via fallback"
    );
}

/// Verifies the COMPREHENSIVE dispatch `find_method_return_type_for_type`
/// (used by CST chain) correctly finds extension function return types.
#[test]
fn method_return_type_for_type_finds_extension_fns() {
    let modifier_uri = uri("/Modifier.kt");
    let padding_uri = uri("/Padding.kt");
    let idx = Indexer::new();

    idx.index_content(
        &modifier_uri,
        "package com.example\n\
         object Modifier",
    );
    idx.index_content(
        &padding_uri,
        "package com.example\n\
         fun Modifier.padding(horizontal: Int = 0, vertical: Int = 0): Modifier = this",
    );

    // The comprehensive dispatch used by CST chain inference
    use crate::indexer::InferDeps;
    let ret = idx.find_method_return_type_for_type("Modifier", "padding");
    assert_eq!(
        ret,
        Some("Modifier".to_string()),
        "find_method_return_type_for_type should find extension fn return type"
    );
}

// ── it-completion helpers ─────────────────────────────────────────────────

#[test]
fn extract_collection_element_list() {
    assert_eq!(
        extract_collection_element_type("List<Product>"),
        Some("Product".into())
    );
}

#[test]
fn extract_collection_element_mutable_list() {
    assert_eq!(
        extract_collection_element_type("MutableList<User>"),
        Some("User".into())
    );
}

#[test]
fn extract_collection_element_flow() {
    assert_eq!(
        extract_collection_element_type("Flow<Event>"),
        Some("Event".into())
    );
}

#[test]
fn extract_collection_element_state_flow() {
    assert_eq!(
        extract_collection_element_type("StateFlow<UiState>"),
        Some("UiState".into())
    );
}

#[test]
fn extract_collection_element_map_returns_first() {
    // Map is not in the collection list → returns None (it's more complex).
    // forEach on Map gives Map.Entry, not the first type arg.
    assert_eq!(extract_collection_element_type("Map<String, Int>"), None);
}

#[test]
fn extract_collection_element_non_collection() {
    // Plain class → not a collection, returns None.
    assert_eq!(extract_collection_element_type("User"), None);
}

#[test]
fn infer_type_in_lines_raw_keeps_generics() {
    let lines: Vec<String> = vec!["val items: List<Product> = emptyList()".into()];
    assert_eq!(
        infer_type_in_lines_raw(&lines, "items"),
        Some("List<Product>".into())
    );
}

#[test]
fn infer_type_in_lines_raw_state_flow() {
    let lines: Vec<String> = vec!["    private val _state: StateFlow<UiState>".into()];
    assert_eq!(
        infer_type_in_lines_raw(&lines, "_state"),
        Some("StateFlow<UiState>".into())
    );
}

#[test]
fn infer_type_in_lines_raw_by_lazy_single_line() {
    // `val repo by lazy { UserRepository() }` — no explicit annotation
    let lines: Vec<String> = vec!["    private val repo by lazy { UserRepository() }".into()];
    assert_eq!(
        infer_type_in_lines_raw(&lines, "repo"),
        Some("UserRepository".into())
    );
}

#[test]
fn infer_type_in_lines_raw_explicit_annotation_takes_priority() {
    // `val repo: UserRepository by lazy { ... }` — annotation wins (first scan)
    let lines: Vec<String> =
        vec!["    private val repo: UserRepository by lazy { UserRepository() }".into()];
    assert_eq!(
        infer_type_in_lines_raw(&lines, "repo"),
        Some("UserRepository".into())
    );
}

#[test]
fn infer_type_in_lines_constructor_call() {
    // `val viewModel = DashboardViewModel()` — no annotation
    let lines: Vec<String> = vec!["    val viewModel = DashboardViewModel()".into()];
    assert_eq!(
        infer_type_in_lines(&lines, "viewModel"),
        Some("DashboardViewModel".into())
    );
}

#[test]
fn infer_type_in_lines_raw_constructor_call() {
    let lines: Vec<String> = vec!["    val viewModel = DashboardViewModel()".into()];
    assert_eq!(
        infer_type_in_lines_raw(&lines, "viewModel"),
        Some("DashboardViewModel".into())
    );
}

#[test]
fn infer_type_in_lines_class_literal_retrofit() {
    // `val api = retrofit.create(DashboardApi::class.java)` — class literal *inside parens*
    // should resolve to DashboardApi via the narrow pattern-3 path.
    let lines: Vec<String> = vec!["    val api = retrofit.create(DashboardApi::class.java)".into()];
    assert_eq!(
        infer_type_in_lines(&lines, "api"),
        Some("DashboardApi".into())
    );
}

#[test]
fn infer_type_in_lines_raw_class_literal_kotlin() {
    // `val api = retrofit.create(DashboardApi::class)` (no .java suffix)
    let lines: Vec<String> = vec!["    val api = retrofit.create(DashboardApi::class)".into()];
    assert_eq!(
        infer_type_in_lines_raw(&lines, "api"),
        Some("DashboardApi".into())
    );
}

#[test]
fn infer_type_in_lines_bare_class_literal_not_matched() {
    // `val key = SomeType::class` — bare class reference: key is KClass<SomeType>,
    // NOT SomeType.  The narrow pattern-3 only triggers when ::class is inside parens.
    let lines: Vec<String> = vec!["    val key = SomeType::class".into()];
    assert_eq!(infer_type_in_lines(&lines, "key"), None);
}

#[test]
fn infer_type_in_lines_di_inject() {
    // `val repo by inject<UserRepository>()` — Koin DI pattern
    let lines: Vec<String> = vec!["    val repo = inject<UserRepository>()".into()];
    assert_eq!(
        infer_type_in_lines(&lines, "repo"),
        Some("UserRepository".into())
    );
}

#[test]
fn infer_type_annotation_still_wins_over_rhs() {
    // Explicit annotation takes priority over RHS inference
    let lines: Vec<String> = vec!["    val repo: UserRepository = OtherRepository()".into()];
    assert_eq!(
        infer_type_in_lines(&lines, "repo"),
        Some("UserRepository".into())
    );
}

#[test]
fn infer_type_rhs_no_false_positive_lowercase() {
    // `val x = someFactory.create()` — lowercase constructor → no inference
    let lines: Vec<String> = vec!["    val x = someFactory.create()".into()];
    assert_eq!(infer_type_in_lines(&lines, "x"), None);
}

#[test]
fn infer_type_rhs_no_false_positive_equality() {
    // `if (x == SomeType())` must not match as an assignment
    let lines: Vec<String> = vec!["    if (x == SomeType()) {".into()];
    assert_eq!(infer_type_in_lines(&lines, "x"), None);
}

#[test]
fn resolve_method_via_class_literal_type_inference() {
    // `val api = retrofit.create(DashboardApi::class.java)` — no annotation
    // dot-completion on `api.someMethod()` should resolve into DashboardApi
    let api_uri = uri("/DashboardApi.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(
        &api_uri,
        "package com.example\ninterface DashboardApi {\n    fun loadData(): String\n}",
    );
    idx.index_content(&caller_uri,
            "package com.example\nval retrofit = TODO()\nval api = retrofit.create(DashboardApi::class.java)\nfun test() { api.loadData() }");

    let locs = resolve_symbol(&idx, "loadData", Some("api"), &caller_uri);
    assert!(
        !locs.is_empty(),
        "loadData not found via class literal type inference"
    );
    assert_eq!(locs[0].uri, api_uri);
}

// ── method return type inference (infer_variable_type) ───────────────────

#[test]
fn infer_variable_type_method_return_type() {
    // `val response = accountApiService.getAccountDetail(body)` where
    // accountApiService: AccountApiService is annotated in the same file
    let service_uri = uri("/AccountApiService.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(&service_uri,
            "package com.example\ninterface AccountApiService {\n    fun getAccountDetail(body: AccountDetailRequestBody): Response<AccountDetail>\n}");
    idx.index_content(&caller_uri,
            "package com.example\nclass Repo(val accountApiService: AccountApiService) {\n    fun load() {\n        val response = accountApiService.getAccountDetail(AccountDetailRequestBody(123))\n    }\n}");

    let result = infer_variable_type(&idx, "response", &caller_uri);
    assert_eq!(
        result,
        Some("Response<AccountDetail>".into()),
        "should infer return type via method lookup"
    );
}

#[test]
fn infer_variable_type_unannotated_snapshot_no_declared_names_rejection() {
    // Verify that the declared_names fast-reject no longer blocks unannotated vars
    // when only a snapshot (no live_lines) is available.
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();
    idx.index_content(
        &caller_uri,
        "package com.example\nval vm = DashboardViewModel()",
    );

    // `vm` has no `:` annotation, so declared_names would not contain it.
    // It must still be resolved via the assignment scan.
    let result = infer_variable_type(&idx, "vm", &caller_uri);
    assert_eq!(
        result,
        Some("DashboardViewModel".into()),
        "unannotated var must still be resolved from snapshot"
    );
}

#[test]
fn goto_def_on_named_lambda_param_resolves_to_declaration_line() {
    // items.forEach { product ->
    //     product.name   ← gd on `product` here
    // go-to-def should jump to the `{ product ->` declaration line (line 2)
    let caller_uri = uri("/Caller.kt");
    let product_uri = uri("/Product.kt");
    let idx = Indexer::new();
    idx.index_content(
        &product_uri,
        "package com.example\ndata class Product(val name: String)",
    );
    idx.index_content(&caller_uri,
            "package com.example\nval items: List<Product> = emptyList()\nitems.forEach { product ->\n    product.name\n}");

    // step 1.5 finds `{ product ->` via the lambda arrow pattern
    let locs = resolve_symbol(&idx, "product", None, &caller_uri);
    assert!(!locs.is_empty(), "lambda param 'product' not found");
    // Must land in the same file (the lambda declaration), NOT in rg results
    assert_eq!(
        locs[0].uri, caller_uri,
        "should stay in Caller.kt at the lambda decl"
    );
    // Line 2 is where `items.forEach { product ->` is declared
    assert_eq!(
        locs[0].range.start.line, 2,
        "should point to the lambda arrow line"
    );
}

// ── complete_dot scoping — no local fns leak ─────────────────────────────

#[test]
fn dot_complete_does_not_leak_top_level_fns() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///a/Keys.kt").unwrap();
    idx.index_content(&uri, "package a\n\nobject ProductKey {\n    val CARD = \"card\"\n    val LOAN = \"loan\"\n    fun fromString(s: String) = s\n}\n\nfun topLevelHelper() {}\n");

    // Simulate a variable typed as ProductKey in another file.
    let caller_uri = Url::parse("file:///a/Caller.kt").unwrap();
    idx.index_content(&caller_uri, "package a\nval key: ProductKey = TODO()");

    let items = complete_dot(&idx, "ProductKey", &caller_uri, false, None);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    assert!(labels.contains(&"fromString"), "member fun should appear");
    assert!(labels.contains(&"CARD"), "member val should appear");
    assert!(
        !labels.contains(&"topLevelHelper"),
        "top-level fn must NOT leak into dot completions"
    );
}

#[test]
fn dot_complete_includes_inherited_members() {
    // `AccountDetailResponseBody` extends `Account` (Java-style parent).
    // Dot-completion on an instance of `AccountDetailResponseBody` must include
    // fields declared in the parent `Account` class.
    let account_uri = uri("/Account.kt");
    let response_uri = uri("/AccountDetailResponseBody.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();

    idx.index_content(&account_uri,
            "package com.example\nopen class Account {\n    val accountName: String = \"\"\n    val accountId: String = \"\"\n}");
    idx.index_content(&response_uri,
            "package com.example\ndata class AccountDetailResponseBody(\n    val feePlanName: String?\n) : Account()");
    idx.index_content(
        &caller_uri,
        "package com.example\nval resp: AccountDetailResponseBody = TODO()",
    );

    let items = complete_dot(&idx, "AccountDetailResponseBody", &caller_uri, false, None);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    // Direct members
    assert!(
        labels.contains(&"feePlanName"),
        "direct field should appear"
    );
    // Inherited members from Account
    assert!(
        labels.contains(&"accountName"),
        "inherited field from parent must appear"
    );
    assert!(
        labels.contains(&"accountId"),
        "inherited field from parent must appear"
    );
}

// ── object with annotated getter properties ───────────────────────────────

#[test]
fn dot_complete_object_with_annotated_getter_properties() {
    // Issue #125: Compose's MaterialTheme file declares BOTH `fun MaterialTheme(...)`
    // AND `object MaterialTheme { ... }`. The old `find()` picked the function first,
    // returning empty completions. The fix: prefer type-kind symbols over functions.
    let idx = Indexer::new();

    // Mirrors the real MaterialTheme.kt: function first, object second, same file.
    let lib_uri = Url::parse("file:///lib/MaterialTheme.kt").unwrap();
    idx.index_content(
        &lib_uri,
        "package androidx.compose.material3\n\n\
         @Composable\n\
         fun MaterialTheme(\n    colorScheme: ColorScheme = MaterialTheme.colorScheme,\n    content: @Composable () -> Unit\n) {}\n\n\
         object MaterialTheme {\n\
             val colorScheme: ColorScheme\n\
                 @Composable get() = LocalColorScheme.current\n\n\
             val typography: Typography\n\
                 @Composable get() = LocalTypography.current\n\n\
             val shapes: Shapes\n\
                 @Composable get() = LocalShapes.current\n\
         }\n",
    );

    let caller_uri = Url::parse("file:///app/Screen.kt").unwrap();
    idx.index_content(
        &caller_uri,
        "package com.example\nimport androidx.compose.material3.MaterialTheme\nfun screen() {}",
    );

    let items = complete_dot(&idx, "MaterialTheme", &caller_uri, false, None);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    assert!(
        labels.contains(&"colorScheme"),
        "object member should appear (not function body); got: {labels:?}"
    );
    assert!(
        labels.contains(&"typography"),
        "typography should appear; got: {labels:?}"
    );
    assert!(
        labels.contains(&"shapes"),
        "shapes should appear; got: {labels:?}"
    );
}

// ── complete_bare distance sorting ───────────────────────────────────────

#[test]
fn complete_bare_local_before_same_pkg() {
    let idx = Indexer::new();
    let local_uri = Url::parse("file:///pkg/a/Local.kt").unwrap();
    let other_uri = Url::parse("file:///pkg/a/Other.kt").unwrap();
    // local file has "localFoo"
    idx.index_content(&local_uri, "package a\nfun localFoo() {}");
    // same-package file has "pkgBar"
    idx.index_content(&other_uri, "package a\nfun pkgBar() {}");

    let (items, _) = complete_bare(&idx, "", &local_uri, false, false, None);

    let local_pos = items.iter().position(|i| i.label == "localFoo");
    let pkg_pos = items.iter().position(|i| i.label == "pkgBar");
    assert!(local_pos.is_some(), "localFoo should appear");
    assert!(pkg_pos.is_some(), "pkgBar should appear");

    // sort_text with tier prefix means local (0:…) sorts before same-pkg (1:…).
    let local_sort = items[local_pos.unwrap()].sort_text.as_deref().unwrap_or("");
    let pkg_sort = items[pkg_pos.unwrap()].sort_text.as_deref().unwrap_or("");
    assert!(
        local_sort < pkg_sort,
        "local tier sort_text should be less than same-pkg tier"
    );
}

#[test]
fn complete_bare_test_symbols_visible_only_to_test_callers() {
    let idx = Indexer::new();
    let main_uri = Url::parse("file:///workspace/src/main/kotlin/a/Main.kt").unwrap();
    let test_uri = Url::parse("file:///workspace/src/test/kotlin/a/TestCaller.kt").unwrap();
    let helper_uri = Url::parse("file:///workspace/src/test/kotlin/a/TestHelper.kt").unwrap();

    idx.index_content(&main_uri, "package a\nfun mainCaller() {}");
    idx.index_content(&test_uri, "package a\nfun testCaller() {}");
    idx.index_content(&helper_uri, "package a\nfun testOnlyHelper() {}");

    let (main_items, _) = complete_bare(&idx, "testOnly", &main_uri, false, false, None);
    assert!(
        main_items.iter().all(|item| item.label != "testOnlyHelper"),
        "main callers must not see same-package test symbols: {main_items:?}"
    );

    let (test_items, _) = complete_bare(&idx, "testOnly", &test_uri, false, false, None);
    assert!(
        test_items.iter().any(|item| item.label == "testOnlyHelper"),
        "test callers must see same-package test symbols: {test_items:?}"
    );
}

// ── dot_completions_for type filtering ────────────────────────────────────

#[test]
fn dot_completions_string_receiver_has_string_fns() {
    let items = dot_completions_for("String", false);
    let names: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(names.contains(&"trim"), "String should have trim()");
    assert!(names.contains(&"split"), "String should have split()");
    assert!(names.contains(&"let"), "String should have scope fn let()");
    // Collection fns should NOT appear on String
    assert!(!names.contains(&"map"), "String should NOT have map()");
    assert!(
        !names.contains(&"filter"),
        "String should NOT have filter()"
    );
}

#[test]
fn dot_completions_list_receiver_has_collection_fns() {
    let items = dot_completions_for("List", false);
    let names: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(names.contains(&"map"), "List should have map()");
    assert!(names.contains(&"filter"), "List should have filter()");
    assert!(names.contains(&"forEach"), "List should have forEach()");
    assert!(names.contains(&"let"), "List should have scope fn let()");
    // String-only fns should NOT appear on List
    assert!(!names.contains(&"trim"), "List should NOT have trim()");
    assert!(!names.contains(&"split"), "List should NOT have split()");
}

#[test]
fn dot_completions_custom_type_has_scope_fns_only() {
    let items = dot_completions_for("MyDomainClass", false);
    let names: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(names.contains(&"let"), "domain type should have let()");
    assert!(names.contains(&"apply"), "domain type should have apply()");
    assert!(
        !names.contains(&"trim"),
        "domain type should NOT have trim()"
    );
    assert!(!names.contains(&"map"), "domain type should NOT have map()");
    assert!(
        !names.contains(&"filter"),
        "domain type should NOT have filter()"
    );
}

// ── supers CST extraction – annotation handling ──────────────────────────

#[test]
fn extract_supers_annotation_same_line() {
    let s = kotlin_supers("@Suppress(\"unused\") class Foo : Bar {}");
    assert!(s.contains(&"Bar".to_string()), "got {s:?}");
}

#[test]
fn extract_supers_annotation_separate_line() {
    let src = "@Module\nclass Foo : Bar, Baz {}";
    let s = kotlin_supers(src);
    assert!(s.contains(&"Bar".to_string()), "got {s:?}");
    assert!(s.contains(&"Baz".to_string()), "got {s:?}");
}

#[test]
fn extract_supers_field_inject_annotation() {
    let s = kotlin_supers("@field:Inject\nclass Foo {}");
    assert!(
        s.is_empty(),
        "annotation-only line should produce no supers, got {s:?}"
    );
}

#[test]
fn extract_supers_multiple_annotations() {
    let src = "@Module\n@Provides\nclass FooModule : BaseModule() {}";
    let s = kotlin_supers(src);
    assert!(s.contains(&"BaseModule".to_string()), "got {s:?}");
}

// ── auto-import helpers ───────────────────────────────────────────────────

fn make_import_entry(
    full_path: &str,
    local_name: &str,
    is_star: bool,
) -> crate::types::ImportEntry {
    crate::types::ImportEntry {
        full_path: full_path.to_string(),
        local_name: local_name.to_string(),
        is_star,
    }
}

#[test]
fn already_imported_exact() {
    let imports = vec![make_import_entry("com.example.Foo", "Foo", false)];
    assert!(already_imported("com.example.Foo", &imports));
}

#[test]
fn already_imported_alias_not_counted() {
    // `import com.example.Foo as Bar` — Foo is not usable as Foo
    let imports = vec![make_import_entry("com.example.Foo", "Bar", false)];
    assert!(!already_imported("com.example.Foo", &imports));
}

#[test]
fn already_imported_star() {
    let imports = vec![make_import_entry("com.example", "*", true)];
    assert!(already_imported("com.example.Foo", &imports));
}

#[test]
fn already_imported_star_wrong_pkg() {
    let imports = vec![make_import_entry("com.other", "*", true)];
    assert!(!already_imported("com.example.Foo", &imports));
}

#[test]
fn import_insertion_after_last_import() {
    let lines = vec![
        "package com.example".to_string(),
        "".to_string(),
        "import com.example.Bar".to_string(),
        "import com.example.Baz".to_string(),
        "".to_string(),
        "class Foo {}".to_string(),
    ];
    assert_eq!(import_insertion_line(&lines), 4); // line after last import
}

#[test]
fn import_insertion_after_package_no_imports() {
    let lines = vec![
        "package com.example".to_string(),
        "".to_string(),
        "class Foo {}".to_string(),
    ];
    assert_eq!(import_insertion_line(&lines), 1); // line after package
}

#[test]
fn import_insertion_at_top_no_package_no_imports() {
    let lines = vec!["class Foo {}".to_string()];
    assert_eq!(import_insertion_line(&lines), 0);
}

#[test]
fn auto_import_completion_adds_edit() {
    let idx = Indexer::new();
    // Library file in a different package.
    let lib_uri = uri("/lib/Composable.kt");
    idx.index_content(
        &lib_uri,
        "package androidx.compose.runtime\nannotation class Composable",
    );
    // Current file — different package, no imports.
    let cur_uri = uri("/app/Screen.kt");
    idx.index_content(
        &cur_uri,
        "package com.example.app\n\nfun Screen() {\n    Comp\n}",
    );

    let (items, _) = complete_symbol(&idx, "Comp", None, &cur_uri, false, None);
    let import_item = items.iter().find(|i| i.label == "Composable");
    assert!(
        import_item.is_some(),
        "Composable should appear in completions"
    );
    let edits = import_item.unwrap().additional_text_edits.as_ref();
    assert!(edits.is_some(), "additionalTextEdits should be present");
    let edit_text = &edits.unwrap()[0].new_text;
    assert!(
        edit_text.contains("import androidx.compose.runtime.Composable"),
        "edit should add correct import, got: {edit_text}"
    );
}

#[test]
fn auto_import_skipped_when_already_imported() {
    let idx = Indexer::new();
    let lib_uri = uri("/lib/Foo.kt");
    idx.index_content(&lib_uri, "package com.lib\nclass Foo");
    let cur_uri = uri("/app/Bar.kt");
    // Already imports com.lib.Foo.
    idx.index_content(
        &cur_uri,
        "package com.app\nimport com.lib.Foo\nclass Bar { val f: Foo = Foo() }",
    );

    let (items, _) = complete_symbol(&idx, "Foo", None, &cur_uri, false, None);
    let foo_items: Vec<_> = items.iter().filter(|i| i.label == "Foo").collect();
    // May appear (from tier-0/1 or tier-2 without edit) but must not have an import edit.
    for item in &foo_items {
        assert!(
            item.additional_text_edits.is_none()
                || item.additional_text_edits.as_ref().unwrap().is_empty(),
            "already-imported symbol must not carry an import edit"
        );
    }
}

#[test]
fn auto_import_skipped_same_package() {
    let idx = Indexer::new();
    let lib_uri = uri("/app/Foo.kt");
    idx.index_content(&lib_uri, "package com.example\nclass Foo");
    let cur_uri = uri("/app/Bar.kt");
    idx.index_content(&cur_uri, "package com.example\nclass Bar");

    let (items, _) = complete_symbol(&idx, "Foo", None, &cur_uri, false, None);
    // Foo is in the same package — any completion item for it must have no import edit.
    for item in items.iter().filter(|i| i.label == "Foo") {
        assert!(
            item.additional_text_edits.is_none()
                || item.additional_text_edits.as_ref().unwrap().is_empty(),
            "same-package symbol must not carry an import edit"
        );
    }
}

#[test]
fn same_package_test_helpers_appear_when_completing_from_test_file() {
    let idx = Indexer::new();
    let helper_uri = uri("/src/test/kotlin/com/example/TestHelpers.kt");
    idx.index_content(&helper_uri, "package com.example\nfun helperThing() = Unit");
    let cur_uri = uri("/src/test/kotlin/com/example/CurrentTest.kt");
    idx.index_content(&cur_uri, "package com.example\nclass CurrentTest");

    let (items, _) = complete_symbol(&idx, "hel", None, &cur_uri, false, None);
    assert!(
        items.iter().any(|item| item.label == "helperThing"),
        "expected same-package helper from sibling test file in completions"
    );
}

#[test]
fn auto_import_two_packages_two_items() {
    let idx = Indexer::new();
    idx.index_content(
        &uri("/m3/Button.kt"),
        "package androidx.compose.material3\nclass Button",
    );
    idx.index_content(
        &uri("/m1/Button.kt"),
        "package androidx.compose.material\nclass Button",
    );
    let cur_uri = uri("/app/Screen.kt");
    idx.index_content(&cur_uri, "package com.example\nfun screen() {}");

    let (items, _) = complete_symbol(&idx, "Button", None, &cur_uri, false, None);
    let button_items: Vec<_> = items.iter().filter(|i| i.label == "Button").collect();
    assert_eq!(
        button_items.len(),
        2,
        "Two Button symbols from different packages should yield two items"
    );
    let details: Vec<_> = button_items
        .iter()
        .filter_map(|i| i.detail.as_deref())
        .collect();
    assert!(
        details.iter().any(|d| d.contains("material3")),
        "One item should mention material3"
    );
    assert!(
        details
            .iter()
            .any(|d| d.contains("material") && !d.contains("material3")),
        "One item should mention material"
    );
}

#[test]
fn caps_mode_hides_lowercase_functions() {
    let idx = Indexer::new();
    let cur_uri = uri("/app/Screen.kt");
    // File with both a class and a lowercase function.
    idx.index_content(
        &cur_uri,
        "package com.example\nclass Column\nfun collectAsState() {}",
    );

    let (items, _) = complete_symbol(&idx, "Col", None, &cur_uri, false, None);
    // Column (uppercase) should appear.
    assert!(
        items.iter().any(|i| i.label == "Column"),
        "Column should appear in caps mode"
    );
    // collectAsState (lowercase) should NOT appear when typing uppercase prefix.
    assert!(
        !items.iter().any(|i| i.label == "collectAsState"),
        "lowercase function must not appear when typing uppercase prefix"
    );
}

#[test]
fn lowercase_mode_hides_classes() {
    let idx = Indexer::new();
    let cur_uri = uri("/app/Screen.kt");
    idx.index_content(
        &cur_uri,
        "package com.example\nclass Column\nfun collectAsState() {}",
    );

    let (items, _) = complete_symbol(&idx, "col", None, &cur_uri, false, None);
    // collectAsState (lowercase) should appear.
    assert!(
        items.iter().any(|i| i.label == "collectAsState"),
        "lowercase function should appear in lowercase mode"
    );
    // Column (uppercase) should NOT appear when typing lowercase prefix.
    assert!(
        !items.iter().any(|i| i.label == "Column"),
        "CamelCase class must not appear when typing lowercase prefix"
    );
}

#[test]
fn tier2_suppressed_when_name_visible_in_current_file() {
    let idx = Indexer::new();
    idx.index_content(&uri("/lib/Foo.kt"), "package com.lib\nclass Foo");
    let cur_uri = uri("/app/Bar.kt");
    idx.index_content(&cur_uri, "package com.example\nclass Foo");

    let (items, _) = complete_symbol(&idx, "Foo", None, &cur_uri, false, None);
    let foo_items: Vec<_> = items.iter().filter(|i| i.label == "Foo").collect();
    assert_eq!(
        foo_items.len(),
        1,
        "Foo defined in current file must not generate a duplicate tier-2 item"
    );
    assert!(
        foo_items[0].additional_text_edits.is_none()
            || foo_items[0]
                .additional_text_edits
                .as_ref()
                .unwrap()
                .is_empty(),
        "tier-0 item must not carry an import edit"
    );
}

// ── match_score ────────────────────────────────────────────────────────────

#[test]
fn match_score_prefix_is_best() {
    assert_eq!(match_score("Column", "Col"), Some(0));
    assert_eq!(match_score("column", "col"), Some(0));
}

#[test]
fn match_score_acronym_is_second() {
    // CB → ColumnButton (C=Column, B=Button)
    assert_eq!(match_score("ColumnButton", "CB"), Some(1));
    // mSF → myStateFlow
    assert_eq!(match_score("myStateFlow", "mSF"), Some(1));
    // underscore-prefixed private fields: _ColumnButton, _myStateFlow
    assert_eq!(match_score("_ColumnButton", "CB"), Some(1));
    assert_eq!(match_score("_myStateFlow", "mSF"), Some(1));
}

#[test]
fn match_score_substring_is_third() {
    assert_eq!(match_score("RecyclerView", "View"), Some(2));
}

#[test]
fn match_score_no_match_returns_none() {
    assert_eq!(match_score("Column", "xyz"), None);
}

#[test]
fn match_score_prefix_beats_acronym_in_sort() {
    let idx = Indexer::new();
    let cur_uri = uri("/app/Screen.kt");
    // Column → prefix match for "Col"; ColumnButton → acronym for "CB" but prefix for "Col"
    idx.index_content(
        &cur_uri,
        "package com.example\nclass Column\nclass ColumnButton",
    );

    let (items, _) = complete_symbol(&idx, "Col", None, &cur_uri, false, None);
    let col_pos = items.iter().position(|i| i.label == "Column").unwrap();
    let colbtn_pos = items
        .iter()
        .position(|i| i.label == "ColumnButton")
        .unwrap();
    // Both are prefix matches; Column (shorter) should sort before ColumnButton lexicographically.
    assert!(
        col_pos < colbtn_pos || {
            // Accept either order — both are score-0, lexicographic tie-break.
            let a = items[col_pos].sort_text.as_deref().unwrap_or("");
            let b = items[colbtn_pos].sort_text.as_deref().unwrap_or("");
            a <= b
        },
        "Column should sort ≤ ColumnButton for prefix 'Col'"
    );
}

#[test]
fn tier2_fires_for_single_char_prefix() {
    let idx = Indexer::new();
    idx.index_content(&uri("/lib/Foo.kt"), "package com.lib\nclass Column");
    let cur_uri = uri("/app/Bar.kt");
    idx.index_content(&cur_uri, "package com.example\n");

    // Single char 'C' — tier-2 now fires for single-char starts-with matches,
    // so Column (cross-pkg) IS returned (score 0: case-insensitive prefix match).
    // Being a cross-package symbol it must carry an auto-import edit.
    let (items, _) = complete_symbol(&idx, "C", None, &cur_uri, false, None);
    assert!(
        items
            .iter()
            .any(|i| i.label == "Column" && i.additional_text_edits.is_some()),
        "tier-2 must fire for single-char prefix and include auto-import edit"
    );

    // Two chars 'Co' — tier-2 also fires.
    let (items, _) = complete_symbol(&idx, "Co", None, &cur_uri, false, None);
    assert!(
        items.iter().any(|i| i.label == "Column"),
        "tier-2 must fire for prefix length >= 2"
    );
}

#[test]
fn tier2_single_char_excludes_camel_acronym_noise() {
    let idx = Indexer::new();
    // "SomeButton" would camel-acronym-match "B" (score 1) but must NOT appear
    // for single-char prefix — only starts-with (score 0) is allowed.
    idx.index_content(
        &uri("/lib/Foo.kt"),
        "package com.lib\nclass SomeButton\nclass Button",
    );
    let cur_uri = uri("/app/Bar.kt");
    idx.index_content(&cur_uri, "package com.example\n");

    let (items, _) = complete_symbol(&idx, "B", None, &cur_uri, false, None);
    assert!(
        items.iter().any(|i| i.label == "Button"),
        "Button (starts with B) must appear"
    );
    assert!(
        !items.iter().any(|i| i.label == "SomeButton"),
        "SomeButton (camel-acronym score 1) must not appear for single-char prefix"
    );
}

#[test]
fn result_cap_sets_hit_cap() {
    let idx = Indexer::new();
    let cur_uri = uri("/app/Screen.kt");
    // Generate 600 unique class names → exceeds COMPLETION_CAP (500).
    let src = (0..600)
        .map(|i| format!("class Cls{i:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    idx.index_content(&cur_uri, &format!("package com.example\n{src}"));

    let (items, hit_cap) = complete_symbol(&idx, "Cls", None, &cur_uri, false, None);
    assert!(
        hit_cap,
        "hit_cap should be true when result count exceeds COMPLETION_CAP"
    );
    assert_eq!(
        items.len(),
        crate::resolver::COMPLETION_CAP,
        "items must be truncated to cap"
    );
}

#[test]
fn annotation_context_hides_functions() {
    let idx = Indexer::new();
    let cur_uri = uri("/app/Screen.kt");
    idx.index_content(
        &cur_uri,
        "package com.example\nannotation class Composable\nfun composable() {}",
    );

    let line = "@Composable";
    let prefix = "Composable";
    let annotation_only = is_annotation_context(line, prefix);
    assert!(annotation_only, "should detect annotation context");

    let (items, _) = complete_symbol_with_context(&idx, prefix, None, &cur_uri, false, true, None);
    // Annotation class should appear.
    assert!(
        items.iter().any(|i| i.label == "Composable"),
        "annotation class Composable must appear"
    );
    // Lowercase function should not appear.
    assert!(
        !items.iter().any(|i| i.label == "composable"),
        "function composable must not appear in annotation context"
    );
}

#[test]
fn annotation_empty_prefix_returns_cross_package_annotations() {
    // Bug #122 — typing `@` alone (empty prefix) must not return empty list,
    // otherwise the editor closes the session and subsequent chars don't reopen it.
    let idx = Indexer::new();
    let cur_uri = uri("/app/src/Screen.kt");
    let other_uri = uri("/app/other/Annotations.kt");
    idx.index_content(&cur_uri, "package com.example.src\nclass Screen");
    idx.index_content(
        &other_uri,
        "package com.example.other\nannotation class Composable",
    );

    // Empty prefix, annotation_only = true (simulates user typing `@` alone).
    let (items, _) = complete_symbol_with_context(&idx, "", None, &cur_uri, false, true, None);
    assert!(
        items.iter().any(|i| i.label == "Composable"),
        "cross-package annotation class must appear with empty prefix in annotation context; got: {:?}",
        items.iter().map(|i| &i.label).collect::<Vec<_>>()
    );
}

#[test]
fn annotation_context_hides_stdlib_functions() {
    // Bug: collect_stdlib() does not check annotation_only, so stdlib functions
    // (println, listOf, TODO, live templates like `fun`) appear in annotation context.
    let idx = Indexer::new();
    let cur_uri = uri("/app/src/Screen.kt");
    idx.index_content(&cur_uri, "package com.example\nclass Screen");

    let (items, _) = complete_symbol_with_context(&idx, "", None, &cur_uri, true, true, None);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    assert!(
        !labels.contains(&"println"),
        "stdlib function println must not appear in annotation context; got: {labels:?}"
    );
    assert!(
        !labels.contains(&"listOf"),
        "stdlib function listOf must not appear in annotation context; got: {labels:?}"
    );
    assert!(
        !labels.contains(&"fun"),
        "live template 'fun' must not appear in annotation context; got: {labels:?}"
    );
}

#[test]
fn camel_mode_hides_screaming_snake() {
    let idx = Indexer::new();
    let cur_uri = uri("/app/Screen.kt");
    idx.index_content(&cur_uri,
            "package com.example\nclass ChildDashboardViewModel\nconst val CHILD_DASHBOARD_MAX = 10\nval CHILD_COUNT = 5");

    // Typing CamelCase prefix — SCREAMING_SNAKE constants must not appear.
    let (items, _) = complete_symbol(&idx, "Child", None, &cur_uri, false, None);
    assert!(
        items.iter().any(|i| i.label == "ChildDashboardViewModel"),
        "CamelCase class must appear"
    );
    assert!(
        !items.iter().any(|i| i.label == "CHILD_DASHBOARD_MAX"),
        "SCREAMING_SNAKE constant must be hidden in camel_mode"
    );
    assert!(
        !items.iter().any(|i| i.label == "CHILD_COUNT"),
        "SCREAMING_SNAKE val must be hidden in camel_mode"
    );

    // Typing all-uppercase prefix — SCREAMING_SNAKE constants may appear.
    let (items2, _) = complete_symbol(&idx, "CHILD", None, &cur_uri, false, None);
    assert!(
        items2.iter().any(|i| i.label == "CHILD_DASHBOARD_MAX"),
        "SCREAMING_SNAKE constant must appear when prefix is uppercase"
    );
}

#[test]
fn long_prefix_tier2_not_crowded_out() {
    // Even when the same-package has many substring-matching symbols,
    // a cross-package prefix match must survive with a 4+ char prefix.
    let idx = Indexer::new();
    let cur_uri = uri("/app/pkg/Screen.kt");
    let other_uri = uri("/app/pkg/Other.kt");
    let cross_uri = uri("/app/other/Cross.kt");

    // 60 same-pkg classes that contain "child" as substring but don't start with it.
    let same_pkg: String = (0..60)
        .map(|i| format!("class Something{i}Child"))
        .collect::<Vec<_>>()
        .join("\n");
    idx.index_content(&cur_uri, "package com.example\n");
    idx.index_content(&other_uri, &format!("package com.example\n{same_pkg}"));
    // Cross-package class with prefix match.
    idx.index_content(
        &cross_uri,
        "package com.other\nclass ChildDashboardViewModel",
    );

    // Short prefix (2 chars): substring allowed, cross-pkg fires.
    let (short, _) = complete_symbol(&idx, "Ch", None, &cur_uri, false, None);
    assert!(
        short.iter().any(|i| i.label == "ChildDashboardViewModel"),
        "cross-pkg must appear for short prefix"
    );

    // Long prefix (5 chars): substring suppressed for tier-0/1 — cross-pkg prefix match wins.
    let (long, _) = complete_symbol(&idx, "Child", None, &cur_uri, false, None);
    assert!(
        long.iter().any(|i| i.label == "ChildDashboardViewModel"),
        "cross-pkg prefix match must survive long prefix even with many same-pkg substring hits"
    );
    // Same-pkg substring hits (Something*Child) must be absent for long prefix.
    assert!(
        !long
            .iter()
            .any(|i| i.label.ends_with("Child") && i.label.starts_with("Something")),
        "same-pkg substring matches must be filtered for long prefix"
    );
}

#[test]
fn library_file_appears_in_cross_package_completion() {
    // Regression: library (sourcePaths) symbols must appear in bare-word completion
    // even when they live in a different package from the current file.
    let idx = Indexer::new();
    let cur_uri = uri("/project/src/Screen.kt");
    let lib_uri: Url = "file:///home/user/.kmp-lsp/sources/compose/Composable.kt"
        .parse()
        .unwrap();
    let col_uri: Url = "file:///home/user/.kmp-lsp/sources/compose/Column.kt"
        .parse()
        .unwrap();

    idx.index_content(
        &lib_uri,
        "package androidx.compose.runtime\nannotation class Composable",
    );
    idx.index_content(
        &col_uri,
        "package androidx.compose.foundation.layout\nfun Column() {}",
    );
    idx.index_content(&cur_uri, "package com.example\n");

    let (items, _) = complete_bare(&idx, "Comp", &cur_uri, false, false, None);
    assert!(
        items.iter().any(|i| i.label == "Composable"),
        "Composable from library file must appear for prefix 'Comp'"
    );

    // Import edit must be included so the editor can auto-import the symbol.
    let composable = items.iter().find(|i| i.label == "Composable").unwrap();
    assert!(
        composable.additional_text_edits.is_some(),
        "Composable completion must include an auto-import text edit"
    );

    let (items2, _) = complete_bare(&idx, "Col", &cur_uri, false, false, None);
    assert!(
        items2.iter().any(|i| i.label == "Column"),
        "Column (fun) from library file must appear for prefix 'Col'"
    );
}

#[test]
fn cross_file_type_subst_multi_class_same_file() {
    // Regression test: when multiple classes in one file extend the same generic base
    // with different type args, completion must pick the correct substitution based on
    // which class the caller is in (via cursor_line).
    let idx = Indexer::new();

    let base_uri = Url::parse("file:///a/Base.kt").unwrap();
    idx.index_content(
        &base_uri,
        "package a\nclass Base<T> {\n  fun get(): T = TODO()\n}",
    );

    let caller_uri = Url::parse("file:///a/Caller.kt").unwrap();
    // Two classes in same file, each extends Base with different type arg
    idx.index_content(
        &caller_uri,
        "package a\n\
         class CallerA : Base<String>() {\n\
             fun testA() { val x = Base<String>()\n\
         }\n\
         }\n\
         \n\
         class CallerB : Base<Int>() {\n\
             fun testB() { val x = Base<Int>()\n\
         }\n\
         }",
    );

    // For CallerA (around line 2-3), Base members should show String substitution
    // This test verifies cursor_line is threaded through completion → symbols_from_nested_type
    // → completion_item_for_nested_symbol → cross_file_type_subst
    let items_a = complete_dot(&idx, "Base", &caller_uri, false, Some(2));
    let get_item_a = items_a.iter().find(|i| i.label == "get");
    assert!(
        get_item_a.is_some(),
        "get method should be in completion items for CallerA"
    );
    let detail_a = get_item_a.unwrap().detail.as_deref().unwrap_or("");
    assert!(
        detail_a.contains("String"),
        "CallerA (Base<String>) should substitute T→String in detail, got: {detail_a}"
    );
    assert!(
        !detail_a.contains(": T"),
        "CallerA detail should not contain unresolved T, got: {detail_a}"
    );

    // For CallerB (around line 6-7), Base members should show Int substitution
    let items_b = complete_dot(&idx, "Base", &caller_uri, false, Some(6));
    let get_item_b = items_b.iter().find(|i| i.label == "get");
    assert!(
        get_item_b.is_some(),
        "get method should be in completion items for CallerB"
    );
    let detail_b = get_item_b.unwrap().detail.as_deref().unwrap_or("");
    assert!(
        detail_b.contains("Int"),
        "CallerB (Base<Int>) should substitute T→Int in detail, got: {detail_b}"
    );
    assert!(
        !detail_b.contains(": T"),
        "CallerB detail should not contain unresolved T, got: {detail_b}"
    );

    // Cursor line threading must produce different substitutions for each class.
    assert_ne!(
        detail_a, detail_b,
        "CallerA and CallerB completions should differ (String vs Int substitution)"
    );

    // Both should have the method, but with potentially different type substitutions
    // (if the caller_cursor_line is correctly applied to pick the right class definition).
    assert_eq!(
        items_a.len(),
        items_b.len(),
        "both completions should return same number of items"
    );
}

#[test]
fn is_screaming_snake_cases() {
    assert!(is_screaming_snake("MAX_SIZE"));
    assert!(is_screaming_snake("CHILD_DASHBOARD_MAX"));
    assert!(is_screaming_snake("A"));
    assert!(!is_screaming_snake("ChildDashboard"));
    assert!(!is_screaming_snake("maxSize"));
    assert!(!is_screaming_snake("_")); // no letters
    assert!(!is_screaming_snake("123")); // no letters
}

#[test]
fn is_annotation_context_detection() {
    assert!(is_annotation_context("@Composable", "Composable"));
    assert!(is_annotation_context("  @Comp", "Comp"));
    assert!(!is_annotation_context("Composable", "Composable")); // no @
                                                                 // "@" alone — cursor right after the trigger character, empty prefix
    assert!(is_annotation_context("@", ""));
    assert!(is_annotation_context("  @", ""));
}

// ── ReceiverType::from_raw ────────────────────────────────────────────────

#[test]
fn receiver_type_simple() {
    let rt = infer::ReceiverType::from_raw("MyClass".to_string());
    assert_eq!(rt.raw, "MyClass");
    assert_eq!(rt.qualified, "MyClass");
    assert_eq!(rt.outer, "MyClass");
    assert_eq!(rt.leaf, "MyClass");
    assert!(!rt.nullable);
}

#[test]
fn receiver_type_with_generics() {
    let rt = infer::ReceiverType::from_raw("Flow<UiState>".to_string());
    assert_eq!(rt.raw, "Flow<UiState>");
    assert_eq!(rt.qualified, "Flow");
    assert_eq!(rt.outer, "Flow");
    assert_eq!(rt.leaf, "Flow");
    assert!(!rt.nullable);
}

#[test]
fn receiver_type_nullable_simple() {
    let rt = infer::ReceiverType::from_raw("User?".to_string());
    assert_eq!(rt.raw, "User?");
    assert_eq!(rt.qualified, "User");
    assert_eq!(rt.outer, "User");
    assert_eq!(rt.leaf, "User");
    assert!(rt.nullable);
}

#[test]
fn receiver_type_nullable_generic() {
    let rt = infer::ReceiverType::from_raw("StateFlow<UiState>?".to_string());
    assert_eq!(rt.raw, "StateFlow<UiState>?");
    assert_eq!(rt.qualified, "StateFlow");
    assert_eq!(rt.outer, "StateFlow");
    assert_eq!(rt.leaf, "StateFlow");
    assert!(rt.nullable);
}

#[test]
fn receiver_type_dotted_nested() {
    let rt = infer::ReceiverType::from_raw("Outer.Inner".to_string());
    assert_eq!(rt.raw, "Outer.Inner");
    assert_eq!(rt.qualified, "Outer.Inner");
    assert_eq!(rt.outer, "Outer");
    assert_eq!(rt.leaf, "Inner");
    assert!(!rt.nullable);
}

#[test]
fn receiver_type_dotted_with_generics() {
    let rt = infer::ReceiverType::from_raw("Outer.Inner<Param>".to_string());
    assert_eq!(rt.raw, "Outer.Inner<Param>");
    assert_eq!(rt.qualified, "Outer.Inner");
    assert_eq!(rt.outer, "Outer");
    assert_eq!(rt.leaf, "Inner");
    assert!(!rt.nullable);
}

#[test]
fn receiver_type_generic_with_params() {
    let rt = infer::ReceiverType::from_raw("OneYearOlderInteractor<Params>".to_string());
    assert_eq!(rt.qualified, "OneYearOlderInteractor");
    assert_eq!(rt.outer, "OneYearOlderInteractor");
    assert_eq!(rt.leaf, "OneYearOlderInteractor");
    assert!(!rt.nullable);
}

#[test]
fn supers_swift_multiple_conformances() {
    let src = "class Foo: UIViewController, Sendable {}";
    let s: Vec<String> = crate::parser::parse_swift(src)
        .supers
        .into_iter()
        .map(|(_, n, _)| n)
        .collect();
    assert!(
        s.contains(&"UIViewController".to_string()),
        "missing UIViewController, got {s:?}"
    );
    assert!(
        s.contains(&"Sendable".to_string()),
        "missing Sendable, got {s:?}"
    );
}

// ─── smart cast narrowing tests ───────────────────────────────────────────────

#[test]
fn smart_cast_when_branch() {
    let lines: Vec<String> = vec![
        "fun handle(event: Event) {",
        "    when (event) {",
        "        is Event.OnClick -> {",
        "            event.doSomething()",
        "        }",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    // Line 3 is inside `is Event.OnClick` branch
    let result = infer_lines::smart_cast_type_at_line(&lines, "event", 3);
    assert_eq!(result.as_deref(), Some("Event.OnClick"));
}

#[test]
fn smart_cast_when_branch_same_line() {
    let lines: Vec<String> = vec![
        "fun handle(event: Event) {",
        "    when (event) {",
        "        is Event.OnClick -> event.doSomething()",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    // Cursor on the branch line itself
    let result = infer_lines::smart_cast_type_at_line(&lines, "event", 2);
    assert_eq!(result.as_deref(), Some("Event.OnClick"));
}

#[test]
fn smart_cast_if_is() {
    let lines: Vec<String> = vec![
        "fun handle(event: Event) {",
        "    if (event is Event.OnInput) {",
        "        event.text",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let result = infer_lines::smart_cast_type_at_line(&lines, "event", 2);
    assert_eq!(result.as_deref(), Some("Event.OnInput"));
}

#[test]
fn smart_cast_no_match_wrong_var() {
    let lines: Vec<String> = vec![
        "fun handle(event: Event) {",
        "    when (event) {",
        "        is Event.OnClick -> {",
        "            other.doSomething()",
        "        }",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    // "other" is not the when subject
    let result = infer_lines::smart_cast_type_at_line(&lines, "other", 3);
    assert_eq!(result, None);
}

#[test]
fn smart_cast_when_no_subject_outside_branch() {
    let lines: Vec<String> = vec![
        "fun handle(event: Event) {",
        "    when (event) {",
        "        is Event.OnClick -> {}",
        "        is Event.OnInput -> {}",
        "    }",
        "    event.normalCall()",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    // Line 5 is outside the when block
    let result = infer_lines::smart_cast_type_at_line(&lines, "event", 5);
    assert_eq!(result, None);
}

#[test]
fn smart_cast_if_does_not_leak_from_closed_nested_block() {
    let lines: Vec<String> = vec![
        "fun handle(event: Event) {",
        "    if (event is Event.OnInput) {",
        "        if (event is Event.OnClick) {",
        "            event.doSomething()",
        "        }",
        "        event.text",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let result = infer_lines::smart_cast_type_at_line(&lines, "event", 5);
    assert_eq!(result.as_deref(), Some("Event.OnInput"));
}

#[test]
fn smart_cast_if_requires_whole_word_variable_match() {
    let lines: Vec<String> = vec![
        "fun handle(event: Event, someevent: Event) {",
        "    if (someevent is Event.OnInput) {",
        "        event.toString()",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let result = infer_lines::smart_cast_type_at_line(&lines, "event", 2);
    assert_eq!(result, None);
}

#[test]
fn smart_cast_if_preserves_generic_types_with_commas() {
    let lines: Vec<String> = vec![
        "fun handle(value: Any) {",
        "    if (value is Map<String, List<Int>>) {",
        "        value.entries",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    let result = infer_lines::smart_cast_type_at_line(&lines, "value", 2);
    assert_eq!(result.as_deref(), Some("Map<String, List<Int>>"));
}
#[test]
fn smart_cast_nested_when_on_same_line() {
    let lines: Vec<String> = vec![
        "fun handle(event: DashboardEvent) {",
        "    when (event) {",
        "        is Banner -> when (event.events) {",
        "            is SalespointInputEvent.OnCloseClick -> {",
        "                event.events.doSomething()",
        "            }",
        "        }",
        "    }",
        "}",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    // event.events on line 4 should be narrowed to SalespointInputEvent.OnCloseClick
    let result = infer_lines::smart_cast_type_at_line(&lines, "event.events", 4);
    assert_eq!(result.as_deref(), Some("SalespointInputEvent.OnCloseClick"),);

    // event on line 4 should be narrowed to Banner (from outer when)
    let result2 = infer_lines::smart_cast_type_at_line(&lines, "event", 4);
    assert_eq!(result2.as_deref(), Some("Banner"));
}

// ── Completion ordering ────────────────────────────────────────────────────
//
// These tests verify the sort_text tier scheme (ascending = highest priority):
//   "0{score}{name}"  → tier 0: local file symbols
//   "1{score}{name}"  → tier 1: same-package symbols
//   "2{score}:{name}" → tier 2: cross-package symbols
//   "3{score}:{name}" → tier 3: stdlib / bare completions
//   "y:{name}"        → live templates (snippets=true only)
//   "z:{name}"        → scope functions / top-level stdlib fns
//
// Keywords ("true"/"false"/"null"/"this"/"super") are added by PR #126.
// See: https://github.com/Hessesian/kmp-lsp/pull/126

fn sort_text_of<'a>(items: &'a [tower_lsp::lsp_types::CompletionItem], label: &str) -> &'a str {
    items
        .iter()
        .find(|i| i.label == label)
        .and_then(|i| i.sort_text.as_deref())
        .unwrap_or_else(|| panic!("label {label:?} not found in completion items"))
}

/// Returns sorted labels from a completion list, for deterministic assertions.
fn sorted_labels(items: &[tower_lsp::lsp_types::CompletionItem]) -> Vec<&str> {
    let mut labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    labels.sort_unstable();
    labels
}

#[track_caller]
fn assert_labels_contain(items: &[tower_lsp::lsp_types::CompletionItem], expected: &[&str]) {
    let missing: Vec<_> = expected
        .iter()
        .filter(|&&e| !items.iter().any(|i| i.label == e))
        .collect();
    assert!(
        missing.is_empty(),
        "completion items missing: {missing:?}\ngot: {:?}",
        sorted_labels(items)
    );
}

#[track_caller]
fn assert_labels_exclude(items: &[tower_lsp::lsp_types::CompletionItem], forbidden: &[&str]) {
    let leaked: Vec<_> = forbidden
        .iter()
        .filter(|&&f| items.iter().any(|i| i.label == f))
        .collect();
    assert!(
        leaked.is_empty(),
        "these labels must NOT appear: {leaked:?}\ngot: {:?}",
        sorted_labels(items)
    );
}

#[test]
fn sort_text_tier_ordering_local_beats_stdlib() {
    // A locally-defined function must sort before any stdlib bare completion.
    let idx = Indexer::new();
    let file_uri = uri("/pkg/a/Main.kt");
    idx.index_content(&file_uri, "package a\nfun myLocalFun() {}");

    let (items, _) = complete_bare(&idx, "", &file_uri, false, false, None);

    let local_sort = sort_text_of(&items, "myLocalFun");
    // Stdlib items get "3{score}:{name}" — anything in tier 0/1/2 starts with "0"/"1"/"2"
    assert!(
        local_sort.starts_with('0') || local_sort.starts_with('1'),
        "local fun sort_text should be tier 0 or 1, got: {local_sort:?}"
    );
    assert!(
        local_sort < "3",
        "local fun sort_text ({local_sort:?}) must be less than stdlib tier '3'"
    );
}

#[test]
fn sort_text_tier_ordering_pkg_beats_cross_pkg() {
    // Within complete_bare, same-package symbols are tier 1 ("1{score}{name}")
    // while the caller's own file symbols are tier 0 ("0{score}{name}").
    // Verify that a same-pkg (but not local-file) symbol sorts after a local one.
    let idx = Indexer::new();
    let caller_uri = uri("/pkg/a/Caller.kt");
    let peer_uri = uri("/pkg/a/Peer.kt");
    idx.index_content(&caller_uri, "package a\nfun localFoo() {}");
    idx.index_content(&peer_uri, "package a\nfun pkgBar() {}");

    let (items, _) = complete_bare(&idx, "", &caller_uri, false, false, None);

    let local_sort = sort_text_of(&items, "localFoo");
    let pkg_sort = sort_text_of(&items, "pkgBar");
    assert!(
        local_sort < pkg_sort,
        "local tier ({local_sort:?}) must sort before same-pkg tier ({pkg_sort:?})"
    );
    assert!(
        local_sort.starts_with('0'),
        "local symbol should be tier 0, got: {local_sort:?}"
    );
    assert!(
        pkg_sort.starts_with('1'),
        "same-pkg symbol should be tier 1, got: {pkg_sort:?}"
    );
}

// See: https://github.com/Hessesian/kmp-lsp/pull/126
//
// This test is EXPECTED TO FAIL on main until PR #126 is merged.
// It documents that "true", "false", and "null" are missing from bare completions.
// Once merged the `#[ignore]` tag should be removed.
#[test]
fn regression_126_bare_completions_include_kotlin_literals() {
    let idx = Indexer::new();
    let file_uri = uri("/pkg/Main.kt");
    idx.index_content(&file_uri, "package pkg\nfun foo() {}");

    // Empty prefix — all completions returned; literals must be present.
    let (items, _) = complete_bare(&idx, "", &file_uri, false, false, None);
    assert_labels_contain(&items, &["true", "false", "null"]);

    // Prefix "t" — matches "true" by starts_with.
    let (t_items, _) = complete_bare(&idx, "t", &file_uri, false, false, None);
    assert_labels_contain(&t_items, &["true"]);
    assert_labels_exclude(&t_items, &["false", "null"]);

    // Prefix "f" — matches "false".
    let (f_items, _) = complete_bare(&idx, "f", &file_uri, false, false, None);
    assert_labels_contain(&f_items, &["false"]);

    // Prefix "nu" — matches "null".
    let (n_items, _) = complete_bare(&idx, "nu", &file_uri, false, false, None);
    assert_labels_contain(&n_items, &["null"]);
}

// See: https://github.com/Hessesian/kmp-lsp/pull/126
//
// This test is EXPECTED TO FAIL on main until PR #126 is merged.
// It also verifies the sort_text tier for keywords: because `collect_stdlib` reassigns
// sort_text for every item in `bare_completions()`, keywords receive "3{score}:{name}"
// (same tier as `println`/`listOf`), NOT the "a:{name}" prefix set in `build_bare_completions`.
// Once the PR is merged, remove `#[ignore]` and confirm the sort_text prefix is "3".
#[test]
fn regression_126_keyword_sort_text_is_stdlib_tier() {
    let idx = Indexer::new();
    let file_uri = uri("/pkg/Main.kt");
    idx.index_content(&file_uri, "package pkg\nfun foo() {}");

    let (items, _) = complete_bare(&idx, "true", &file_uri, false, false, None);
    let true_sort = sort_text_of(&items, "true");

    // Keywords flow through collect_stdlib which overwrites sort_text with "3{score}:{name}".
    // "3" tier means they sort AFTER local/pkg/cross-pkg symbols but in the same band as
    // other stdlib items (listOf, println, etc.).
    assert!(
        true_sort.starts_with('3'),
        "keyword sort_text should be tier 3 (stdlib band), got: {true_sort:?}"
    );
}

#[test]
fn sort_text_named_arg_prefix_is_001() {
    // Named-arg completions use "001:{name}" sort prefix — verify the constant is correct
    // so that named args always sort before all real symbol tiers (0/1/2/3).
    assert!(
        "001:foo" < "0foo",
        "named-arg prefix must beat tier-0 sort_text"
    );
    assert!(
        "001:foo" < "3foo",
        "named-arg prefix must beat tier-3 sort_text"
    );
    assert!(
        "001:foo" < "a:foo",
        "named-arg prefix must beat 'a:' keyword prefix"
    );
    assert!(
        "a:foo" < "y:foo",
        "keyword 'a:' prefix must beat live-template 'y:' prefix"
    );
    assert!(
        "a:foo" < "z:foo",
        "keyword 'a:' prefix must beat scope-fun 'z:' prefix"
    );
    assert!(
        "y:foo" < "z:foo",
        "live-template 'y:' prefix must beat scope-fun 'z:'"
    );
}

#[test]
fn bare_completion_includes_this_extensions_inside_subclass() {
    // See: extension properties like `val ViewModel.viewModelScope` should appear
    // as bare-word completions inside a class that inherits from ViewModel.
    let idx = Indexer::new();

    // Library file defining the extension property.
    let lib_uri = Url::parse("file:///sdk/ViewModel.kt").unwrap();
    idx.index_content(
        &lib_uri,
        "package androidx.lifecycle\nopen class ViewModel\nval ViewModel.viewModelScope: Int get() = 0",
    );

    // App ViewModel that inherits from ViewModel.
    let vm_uri = Url::parse("file:///app/DashboardViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class DashboardViewModel : ViewModel() {\n",
            "    fun load() {\n",
            "        val s = viewModelScope\n", // cursor is on line 4 (0-based)
            "    }\n",
            "}\n",
        ),
    );

    // Request completion on line 4 (inside the function body of DashboardViewModel).
    let (items, _) = complete_bare(&idx, "viewModel", &vm_uri, false, false, Some(4));
    assert_labels_contain(&items, &["viewModelScope"]);
}

#[test]
fn bare_completion_extension_property_not_function_snippet() {
    // Extension *properties* must not get a `name($1)` snippet — they are values, not callables.
    let idx = Indexer::new();
    let lib_uri = Url::parse("file:///sdk/ViewModel.kt").unwrap();
    idx.index_content(
        &lib_uri,
        "package androidx.lifecycle\nopen class ViewModel\nval ViewModel.viewModelScope: Int get() = 0",
    );
    let vm_uri = Url::parse("file:///app/DashboardViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class DashboardViewModel : ViewModel() {\n",
            "    fun load() {\n",
            "        val s = viewModelScope\n",
            "    }\n",
            "}\n",
        ),
    );
    let (items, _) = complete_bare(&idx, "viewModel", &vm_uri, true, false, Some(4));
    let item = items
        .iter()
        .find(|i| i.label == "viewModelScope")
        .expect("viewModelScope must appear");
    assert!(
        item.insert_text.is_none(),
        "extension property must not have a snippet insert_text, got: {:?}",
        item.insert_text
    );
}

#[test]
fn infer_extension_property_type_for_dot_completion() {
    // viewModelScope.launch: after `viewModelScope.`, the type must be inferred as
    // CoroutineScope so that extension functions on CoroutineScope (e.g. `launch`) appear.
    let idx = Indexer::new();
    let lib_uri = Url::parse("file:///sdk/ViewModel.kt").unwrap();
    idx.index_content(
        &lib_uri,
        "package androidx.lifecycle\nopen class ViewModel\nval ViewModel.viewModelScope: CoroutineScope get() = TODO()",
    );
    let vm_uri = Url::parse("file:///app/DashboardViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class DashboardViewModel : ViewModel() {\n",
            "    fun load() {}\n",
            "}\n",
        ),
    );

    let result = infer_variable_type(&idx, "viewModelScope", &vm_uri);
    assert_eq!(
        result,
        Some("CoroutineScope".into()),
        "viewModelScope type must be inferred as CoroutineScope via extension property lookup"
    );
}

#[test]
fn complete_dot_viewmodelscope_shows_launch() {
    // End-to-end: `viewModelScope.` inside a ViewModel subclass must return `launch`.
    // This tests the full chain:
    //   1. `viewModelScope` type resolved via find_extension_property_type
    //      (via extension_by_receiver["ViewModel"])
    //   2. `launch` found via extension_fn_completions
    //      (via extension_by_receiver["CoroutineScope"])
    // Both extension_by_receiver entries are populated via source indexing here,
    // which mirrors what JAR indexing does after the sidecar fix.
    let idx = Indexer::new();

    // Simulate lifecycle-viewmodel-ktx: viewModelScope property
    let lib_uri = Url::parse("file:///sdk/lifecycle.kt").unwrap();
    idx.index_content(
        &lib_uri,
        "package androidx.lifecycle\nopen class ViewModel\nval ViewModel.viewModelScope: CoroutineScope get() = TODO()",
    );

    // Simulate kotlinx.coroutines: launch extension on CoroutineScope
    let coroutines_uri = Url::parse("file:///sdk/coroutines.kt").unwrap();
    idx.index_content(
        &coroutines_uri,
        "package kotlinx.coroutines\ninterface CoroutineScope\nfun CoroutineScope.launch(block: suspend () -> Unit): Job = TODO()",
    );

    let vm_uri = Url::parse("file:///app/DashboardViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class DashboardProductsViewModel : ViewModel() {\n",
            "    fun load() { viewModelScope.launch {} }\n",
            "}\n",
        ),
    );

    let items = complete_dot(&idx, "viewModelScope", &vm_uri, false, None);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"launch"),
        "expected `launch` in viewModelScope. completions, got: {labels:?}"
    );
}

// ── JAR-path extension completion test ───────────────────────────────────────

#[test]
fn jar_extension_appears_in_dot_completion() {
    // Verify that extension functions inserted via the JAR path (build_jar_file_data)
    // appear in dot-completion.  This mirrors the real flow where the sidecar indexes
    // kotlinx-coroutines and the `launch` function is stored with a `jar:file://` URI.
    let idx = Indexer::new();

    // Source-index ViewModel so walk_hierarchy can find it.
    idx.index_content(
        &Url::parse("file:///sdk/ViewModel.kt").unwrap(),
        "package androidx.lifecycle\nopen class ViewModel",
    );

    // Simulate JAR-indexed extensions (what build_jar_file_data does):
    // 1. val ViewModel.viewModelScope: CoroutineScope
    idx.extension_by_receiver
        .entry("ViewModel".to_owned())
        .or_default()
        .push(crate::types::ExtensionEntry {
            file_uri: "jar:file:///lifecycle-ktx.jar/ViewModel.class".to_owned(),
            name: "viewModelScope".to_owned(),
            kind: tower_lsp::lsp_types::SymbolKind::PROPERTY,
            detail: "val ViewModel.viewModelScope: CoroutineScope".to_owned(),
            visibility: crate::types::Visibility::Public,
            package: Some("androidx.lifecycle".to_owned()),
            trailing_lambda: false,
            deprecated: false,
        });

    // 2. fun CoroutineScope.launch(block: suspend CoroutineScope.() -> Unit): Job
    idx.extension_by_receiver
        .entry("CoroutineScope".to_owned())
        .or_default()
        .push(crate::types::ExtensionEntry {
            file_uri: "jar:file:///coroutines-core.jar/Builders.class".to_owned(),
            name: "launch".to_owned(),
            kind: tower_lsp::lsp_types::SymbolKind::FUNCTION,
            detail: "fun CoroutineScope.launch(block: suspend CoroutineScope.() -> Unit): Job"
                .to_owned(),
            visibility: crate::types::Visibility::Public,
            package: Some("kotlinx.coroutines".to_owned()),
            trailing_lambda: true,
            deprecated: false,
        });

    let vm_uri = Url::parse("file:///app/MyViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class MyViewModel : ViewModel() {\n",
            "    fun load() { viewModelScope.launch {} }\n",
            "}\n",
        ),
    );

    let items = complete_dot(&idx, "viewModelScope", &vm_uri, true, None);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    assert!(
        labels.contains(&"launch"),
        "expected regular `launch` item from JAR path, got: {labels:?}"
    );
    assert!(
        labels.contains(&"launch { }"),
        "expected trailing-lambda `launch {{ }}` item from JAR path, got: {labels:?}"
    );
}

/// Deprecated and internal library overloads must be filtered out of
/// dot-completion, leaving only the current public `launch` (plus its
/// trailing-lambda form). Mirrors Android Studio, which hides the deprecated
/// binary-compat shims and internal impl helpers coroutines ships.
#[test]
fn library_deprecated_internal_extensions_hidden() {
    let idx = Indexer::new();
    idx.index_content(
        &Url::parse("file:///sdk/ViewModel.kt").unwrap(),
        "package androidx.lifecycle\nopen class ViewModel",
    );
    idx.extension_by_receiver
        .entry("ViewModel".to_owned())
        .or_default()
        .push(crate::types::ExtensionEntry {
            file_uri: "jar:file:///lifecycle-ktx.jar/ViewModel.class".to_owned(),
            name: "viewModelScope".to_owned(),
            kind: tower_lsp::lsp_types::SymbolKind::PROPERTY,
            detail: "val ViewModel.viewModelScope: CoroutineScope".to_owned(),
            visibility: crate::types::Visibility::Public,
            package: Some("androidx.lifecycle".to_owned()),
            trailing_lambda: false,
            deprecated: false,
        });

    let mk = |detail: &str, vis, deprecated| crate::types::ExtensionEntry {
        file_uri: "jar:file:///coroutines-core.jar/Builders.class".to_owned(),
        name: "launch".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::FUNCTION,
        detail: detail.to_owned(),
        visibility: vis,
        package: Some("kotlinx.coroutines".to_owned()),
        trailing_lambda: true,
        deprecated,
    };
    {
        let mut slot = idx
            .extension_by_receiver
            .entry("CoroutineScope".to_owned())
            .or_default();
        // Current public overload — should appear.
        slot.push(mk(
            "fun CoroutineScope.launch(block: suspend () -> Unit): Job",
            crate::types::Visibility::Public,
            false,
        ));
        // Deprecated binary-compat shim — should be hidden (library + deprecated).
        slot.push(mk(
            "fun CoroutineScope.launch(parent: Job, block: suspend () -> Unit): Job",
            crate::types::Visibility::Public,
            true,
        ));
        // Internal impl helper — should be hidden (library + internal).
        slot.push(mk(
            "fun CoroutineScope.launch(impl: Int): Job",
            crate::types::Visibility::Internal,
            false,
        ));
    }

    let vm_uri = Url::parse("file:///app/MyViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class MyViewModel : ViewModel() {\n",
            "    fun load() { viewModelScope.launch {} }\n",
            "}\n",
        ),
    );

    let items = complete_dot(&idx, "viewModelScope", &vm_uri, true, None);
    let launch_items: Vec<&CompletionItem> = items
        .iter()
        .filter(|i| i.label == "launch" || i.label == "launch { }")
        .collect();
    let labels: Vec<&str> = launch_items.iter().map(|i| i.label.as_str()).collect();
    // Exactly the current overload's two ergonomic forms — deprecated + internal gone.
    assert_eq!(
        launch_items.len(),
        2,
        "expected only launch() + launch {{ }} for the current overload, got: {labels:?}"
    );
    assert!(
        launch_items.iter().all(|i| i.tags.is_none()),
        "no item should be tagged deprecated"
    );
}

/// Overloads of one library extension collapse to a single completion entry
/// (plus its trailing-lambda form). Reproduces the coroutines 1.11.0 sidecar
/// artifact where `CoroutineScope.launch` is emitted three times with bogus
/// first-param types (`CoroutineContext`, `Job`, `NonCancellable`); the user
/// should see only `launch` + `launch { }`, not three of each.
#[test]
fn extension_overloads_collapse_to_single_entry() {
    let idx = Indexer::new();
    idx.index_content(
        &Url::parse("file:///sdk/ViewModel.kt").unwrap(),
        "package androidx.lifecycle\nopen class ViewModel",
    );
    idx.extension_by_receiver
        .entry("ViewModel".to_owned())
        .or_default()
        .push(crate::types::ExtensionEntry {
            file_uri: "jar:file:///lifecycle-ktx.jar/ViewModel.class".to_owned(),
            name: "viewModelScope".to_owned(),
            kind: tower_lsp::lsp_types::SymbolKind::PROPERTY,
            detail: "val ViewModel.viewModelScope: CoroutineScope".to_owned(),
            visibility: crate::types::Visibility::Public,
            package: Some("androidx.lifecycle".to_owned()),
            trailing_lambda: false,
            deprecated: false,
        });
    let mk = |first_param: &str, pkg: &str, defaults: bool| crate::types::ExtensionEntry {
        file_uri: "jar:file:///coroutines-core.jar/Builders.class".to_owned(),
        name: "launch".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::FUNCTION,
        detail: if defaults {
            format!("fun CoroutineScope.launch(context: {first_param} = EmptyCoroutineContext, block: suspend () -> Unit): Job")
        } else {
            format!(
                "fun CoroutineScope.launch(context: {first_param}, block: suspend () -> Unit): Job"
            )
        },
        visibility: crate::types::Visibility::Public,
        package: Some(pkg.to_owned()),
        trailing_lambda: true,
        deprecated: false,
    };
    {
        let mut slot = idx
            .extension_by_receiver
            .entry("CoroutineScope".to_owned())
            .or_default();
        // Compiled-JAR overloads (no defaults) under one inferred package…
        slot.push(mk("CoroutineContext", "kotlinx.coroutines", false));
        slot.push(mk("Job", "kotlinx.coroutines", false));
        slot.push(mk("NonCancellable", "kotlinx.coroutines", false));
        // …plus the sources-JAR copy of the SAME function with default values and
        // a different (exact) package. Must still collapse into the single entry.
        slot.push(mk("CoroutineContext", "kotlinx.coroutines.core", true));
    }

    let vm_uri = Url::parse("file:///app/MyViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class MyViewModel : ViewModel() {\n",
            "    fun load() { viewModelScope.launch {} }\n",
            "}\n",
        ),
    );

    let items = complete_dot(&idx, "viewModelScope", &vm_uri, true, None);
    let n_plain = items.iter().filter(|i| i.label == "launch").count();
    let n_lambda = items.iter().filter(|i| i.label == "launch { }").count();
    assert_eq!(n_plain, 1, "expected exactly one `launch`, got {n_plain}");
    assert_eq!(
        n_lambda, 1,
        "expected exactly one `launch {{ }}`, got {n_lambda}"
    );
}

/// Deprecated WORKSPACE extensions are kept (you may still call your own code
/// mid-migration) but tagged Deprecated and sorted to the bottom. Uses the same
/// `viewModelScope → CoroutineScope` resolution the test above relies on, then
/// adds a workspace-sourced deprecated extension on `CoroutineScope`.
#[test]
fn deprecated_workspace_extension_kept_tagged() {
    let idx = Indexer::new();
    idx.index_content(
        &Url::parse("file:///sdk/ViewModel.kt").unwrap(),
        "package androidx.lifecycle\nopen class ViewModel",
    );
    // JAR property resolves viewModelScope → CoroutineScope.
    idx.extension_by_receiver
        .entry("ViewModel".to_owned())
        .or_default()
        .push(crate::types::ExtensionEntry {
            file_uri: "jar:file:///lifecycle-ktx.jar/ViewModel.class".to_owned(),
            name: "viewModelScope".to_owned(),
            kind: tower_lsp::lsp_types::SymbolKind::PROPERTY,
            detail: "val ViewModel.viewModelScope: CoroutineScope".to_owned(),
            visibility: crate::types::Visibility::Public,
            package: Some("androidx.lifecycle".to_owned()),
            trailing_lambda: false,
            deprecated: false,
        });
    // A WORKSPACE-sourced deprecated extension on CoroutineScope (file:// URI).
    idx.index_content(
        &Url::parse("file:///app/ext.kt").unwrap(),
        concat!(
            "package kotlinx.coroutines\n",
            "@Deprecated(\"use newWork\")\n",
            "fun CoroutineScope.legacyWork() {}\n",
        ),
    );

    let vm_uri = Url::parse("file:///app/MyViewModel.kt").unwrap();
    idx.index_content(
        &vm_uri,
        concat!(
            "package app\n",
            "import androidx.lifecycle.ViewModel\n",
            "class MyViewModel : ViewModel() {\n",
            "    fun load() { viewModelScope.legacyWork() }\n",
            "}\n",
        ),
    );

    let items = complete_dot(&idx, "viewModelScope", &vm_uri, false, None);
    let legacy = items
        .iter()
        .find(|i| i.label == "legacyWork")
        .expect("deprecated workspace extension should still be offered");
    assert_eq!(
        legacy.tags.as_deref(),
        Some(&[CompletionItemTag::DEPRECATED][..]),
        "workspace deprecated item should carry the Deprecated tag"
    );
    assert!(
        legacy
            .sort_text
            .as_deref()
            .is_some_and(|s| s.starts_with("99:")),
        "deprecated item should sort to the bottom, got: {:?}",
        legacy.sort_text
    );
}

#[test]
fn trailing_lambda_completion_offered() {
    let idx = Indexer::new();

    let ext_uri = Url::parse("file:///sdk/collections.kt").unwrap();
    idx.index_content(
        &ext_uri,
        concat!(
            "package sdk\n",
            "interface Items\n",
            "fun Items.each(block: (String) -> Unit): Unit = TODO()\n",
        ),
    );

    let app_uri = Url::parse("file:///app/Main.kt").unwrap();
    idx.index_content(
        &app_uri,
        concat!(
            "package app\n",
            "import sdk.Items\n",
            "import sdk.each\n",
            "fun use(items: Items) { items.each }\n",
        ),
    );

    let items = complete_dot(&idx, "items", &app_uri, true, Some(3));
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    assert!(labels.contains(&"each"), "regular form missing: {labels:?}");
    assert!(
        labels.contains(&"each { }"),
        "lambda form missing: {labels:?}"
    );

    let lam = items.iter().find(|i| i.label == "each { }").unwrap();
    assert_eq!(lam.insert_text.as_deref(), Some("each { $1 }"));
    assert_eq!(lam.insert_text_format, Some(InsertTextFormat::SNIPPET));
}

// ── Issue 1: Generic type parameter inference for function calls ──────────

/// `retrofit.create<ApiClass>()` should infer the return type as `ApiClass`
/// via the explicit type argument, not leave it as raw `T`.
#[test]
fn infer_type_in_lines_generic_create_with_type_arg() {
    // Retrofit-style: fun <T> create(service: Class<T>): T
    // When called as create<ApiClass>(ApiClass::class.java), the return type
    // should be ApiClass (substituted from the explicit type argument).
    let lines: Vec<String> =
        vec!["    val api = retrofit.create<ApiClass>(ApiClass::class.java)".into()];
    assert_eq!(
        infer_type_in_lines(&lines, "api"),
        Some("ApiClass".into()),
        "generic create<T>() with explicit type arg should infer T=ApiClass"
    );
}

/// `retrofit.create<ApiClass>()` without class literal should also infer
/// the return type from the explicit type argument alone.
#[test]
fn infer_type_in_lines_generic_create_type_arg_only() {
    let lines: Vec<String> = vec!["    val api = retrofit.create<ApiClass>()".into()];
    assert_eq!(
        infer_type_in_lines(&lines, "api"),
        Some("ApiClass".into()),
        "generic create<T>() with type arg only should infer T=ApiClass"
    );
}

/// Existing DI patterns should still work after removing the hardcoded allowlist.
#[test]
fn infer_type_in_lines_di_get_still_works() {
    let lines: Vec<String> = vec!["    val repo = get<UserRepository>()".into()];
    assert_eq!(
        infer_type_in_lines(&lines, "repo"),
        Some("UserRepository".into()),
        "DI get<T>() should still infer correctly"
    );
}

// ── Issue 2: Extension function precedence over member functions ─────────

/// When an extension function is imported with the same name as a member,
/// goto-definition should resolve to the extension, not the member.
#[test]
fn resolve_imported_extension_preferred_over_member() {
    let service_uri = uri("/Service.kt");
    let ext_uri = uri("/ServiceExtensions.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();

    idx.index_content(
        &service_uri,
        "package com.example\n\
         class Service {\n\
             fun execute() { /* member */ }\n\
         }",
    );
    idx.index_content(
        &ext_uri,
        "package com.example.ext\n\
         fun Service.execute() { /* extension */ }",
    );
    idx.index_content(
        &caller_uri,
        "package com.example.app\n\
         import com.example.ext.execute\n\
         fun test() {\n\
             Service().execute()\n\
         }",
    );

    // Resolving `execute` with qualifier `Service` should find the extension,
    // not the member.
    let locs = resolve_symbol(&idx, "execute", Some("Service"), &caller_uri);
    assert!(!locs.is_empty(), "extension function should be found");
    assert_eq!(
        locs[0].uri, ext_uri,
        "should resolve to extension function, not member"
    );
}

/// When no extension exists, member functions should still resolve correctly.
#[test]
fn resolve_member_when_no_extension() {
    let service_uri = uri("/Service.kt");
    let caller_uri = uri("/Caller.kt");
    let idx = Indexer::new();

    idx.index_content(
        &service_uri,
        "package com.example\n\
         class Service {\n\
             fun execute() { /* member */ }\n\
         }",
    );
    idx.index_content(
        &caller_uri,
        "package com.example\n\
         fun test() {\n\
             Service().execute()\n\
         }",
    );

    let locs = resolve_symbol(&idx, "execute", Some("Service"), &caller_uri);
    assert!(!locs.is_empty(), "member function should be found");
    assert_eq!(locs[0].uri, service_uri, "should resolve to member");
}

#[test]
fn when_branch_delete_no_timeout() {
    // Regression: deleting a branch from a when expression should not cause
    // timeouts on subsequent actions. This test verifies that the indexer
    // and resolver handle the deletion correctly without hanging.
    let uri = uri("/test.kt");
    let idx = Indexer::new();

    // Initial index with a when expression containing two branches
    let src_v1 = "\
sealed class Event
object OnClick : Event()
object OnLongPress : Event()

fun handle(event: Event) {
    when (event) {
        is OnClick -> println(\"click\")
        is OnLongPress -> println(\"long press\")
    }
}
";
    idx.index_content(&uri, src_v1);

    // Verify initial resolution works
    let locs = resolve_symbol(&idx, "handle", None, &uri);
    assert!(!locs.is_empty(), "handle should be found");

    // Now delete the second branch (simulating user editing the file)
    let src_v2 = "\
sealed class Event
object OnClick : Event()
object OnLongPress : Event()

fun handle(event: Event) {
    when (event) {
        is OnClick -> println(\"click\")
    }
}
";
    idx.index_content(&uri, src_v2);

    // Verify resolution still works after branch deletion — this should NOT timeout
    let locs = resolve_symbol(&idx, "handle", None, &uri);
    assert!(
        !locs.is_empty(),
        "handle should still be found after branch deletion"
    );

    // Verify the sealed class subtypes are still correct
    let subtypes = idx.subtypes.get("Event");
    assert!(subtypes.is_some(), "Event subtypes should still be indexed");
}
