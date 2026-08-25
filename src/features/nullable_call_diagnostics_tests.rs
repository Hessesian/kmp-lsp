use tower_lsp::lsp_types::Url;

use crate::indexer::live_tree::parse_live;
use crate::indexer::Indexer;

use super::nullable_dot_call_diagnostics;

fn uri(path: &str) -> Url {
    Url::parse(&format!("file:///test{path}")).unwrap()
}

fn setup(sources: &[(&str, &str)]) -> (Url, Indexer, String) {
    let idx = Indexer::new();
    let mut last_uri = uri("/test.kt");
    let mut last_src = String::new();
    for (path, src) in sources {
        let file_uri = uri(path);
        idx.index_content(&file_uri, src);
        idx.store_live_tree(&file_uri, src);
        last_uri = file_uri;
        last_src = (*src).to_string();
    }
    (last_uri, idx, last_src)
}

fn run_diagnostics(
    idx: &Indexer,
    uri: &Url,
    source: &str,
) -> Vec<tower_lsp::lsp_types::Diagnostic> {
    let doc = parse_live(source, tree_sitter_kotlin::LANGUAGE.into()).unwrap();
    nullable_dot_call_diagnostics(idx, uri, &doc)
}

#[test]
fn member_access_on_nullable_receiver_without_safe_call_warns() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("loadData"));
}

#[test]
fn fires_while_jar_indexing_is_in_progress() {
    // Regression guard: this diagnostic must NOT be gated on `jar_phase`.
    // Its true positives resolve to workspace-local symbols (populated by the
    // fast source scan), so gating on JAR loading only hid the diagnostic for
    // the whole — potentially many-second — indexing window. Force the phase to
    // `InProgress` and assert the member access is still flagged.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.loadData()\n",
            "}\n",
        ),
    )]);
    *idx.jar_phase.lock().unwrap() = crate::indexer::jar_phase::JarPhase::InProgress;
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "diagnostic must fire even while JAR indexing is in progress: {diags:?}"
    );
    assert!(diags[0].message.contains("loadData"));
}

#[test]
fn member_access_on_nullable_receiver_with_safe_call_is_clean() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo?.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "safe call should not warn: {diags:?}");
}

#[test]
fn member_access_on_nullable_receiver_with_non_null_assertion_is_clean() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo!!.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "!! assertion should not warn: {diags:?}");
}

#[test]
fn member_access_on_non_nullable_receiver_is_clean() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun loadData() {}\n",
            "}\n",
            "fun caller(repo: Repository) {\n",
            "    repo.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "non-nullable receiver should not warn: {diags:?}"
    );
}

#[test]
fn extension_on_non_nullable_receiver_warns_when_called_on_nullable() {
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository\n",
            "fun Repository.loadData() {}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("loadData"));
}

#[test]
fn extension_on_nullable_receiver_is_clean() {
    // `fun Repository?.loadDataOrDefault()` is itself safe to call on a
    // nullable receiver without `?.` — matches Kotlin's `String?.isNullOrEmpty()`.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository\n",
            "fun Repository?.loadDataOrDefault() {}\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.loadDataOrDefault()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "extension declared on a nullable receiver should not warn: {diags:?}"
    );
}

#[test]
fn member_extension_out_of_scope_does_not_drive_nullable_diagnostic() {
    // Regression guard for the Compose member-extension short-circuit added to
    // `extension_is_in_scope`: `weight` is a MEMBER extension of `ColumnScope`
    // in an unrelated, unimported package, and this caller has no active
    // `ColumnScope` receiver at all — `m.weight(1f)` is just an unrelated call
    // on some other type that happens to share the name. Treating the member
    // extension as unconditionally "in scope" here would wrongly emit a `?.`
    // warning for a call this diagnostic cannot actually verify.
    let (uri, idx, src) = setup(&[
        (
            "/scope.kt",
            concat!(
                "package androidx.compose.foundation.layout\n",
                "class Modifier\n",
                "interface ColumnScope {\n",
                "    fun Modifier.weight(weight: Float): Modifier\n",
                "}\n",
            ),
        ),
        (
            "/caller.kt",
            concat!(
                "package com.example.app\n",
                "class Modifier\n",
                "fun caller(m: Modifier?) {\n",
                "    m.weight(1f)\n",
                "}\n",
            ),
        ),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "an out-of-scope member extension must not drive this diagnostic: {diags:?}"
    );
}

#[test]
fn member_extension_in_same_package_does_not_drive_nullable_diagnostic() {
    // Regression guard for the narrower gap in the fix above: `extension_in_scope_here`
    // passes `None` instead of `entry.container` to `extension_is_in_scope`, which
    // blocks the member-extension short-circuit -- but the ordinary package-match
    // branch underneath can still independently succeed when the caller happens to
    // share `ColumnScope`'s package, with zero evidence `ColumnScope` is an active
    // dispatch receiver at this call site. A same-package member extension must be
    // just as unresolved here as the unimported one above.
    let (uri, idx, src) = setup(&[
        (
            "/scope.kt",
            concat!(
                "package androidx.compose.foundation.layout\n",
                "class Modifier\n",
                "interface ColumnScope {\n",
                "    fun Modifier.weight(weight: Float): Modifier\n",
                "}\n",
            ),
        ),
        (
            "/caller.kt",
            concat!(
                "package androidx.compose.foundation.layout\n",
                "fun caller(m: Modifier?) {\n",
                "    m.weight(1f)\n",
                "}\n",
            ),
        ),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "a same-package member extension must not drive this diagnostic either: {diags:?}"
    );
}

#[test]
fn unresolved_member_on_nullable_receiver_is_skipped() {
    // Neither a member nor a known extension — avoid guessing.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository\n",
            "fun caller(repo: Repository?) {\n",
            "    repo.unknownMethod()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "unresolved member should not warn: {diags:?}"
    );
}

#[test]
fn member_access_on_nullable_field_chain_warns() {
    // `outer.inner.loadData()` where `inner: Inner?` is a nullable field —
    // the field-access chain is the receiver of the plain `.` call.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Inner {\n",
            "    fun loadData() {}\n",
            "}\n",
            "class Outer {\n",
            "    val inner: Inner? = null\n",
            "}\n",
            "fun caller(outer: Outer) {\n",
            "    outer.inner.loadData()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("loadData"));
}

#[test]
fn member_access_on_nullable_data_class_field_warns() {
    // The user's report: a nullable field of a `data class` accessed directly
    // as `holder.repo.load()`.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun load() {}\n",
            "}\n",
            "data class Holder(val repo: Repository?)\n",
            "fun caller(holder: Holder) {\n",
            "    holder.repo.load()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("load"));
}

#[test]
fn safe_call_on_nullable_field_chain_is_clean() {
    // `holder.repo?.load()` — safe call on the nullable field is fine.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun load() {}\n",
            "}\n",
            "data class Holder(val repo: Repository?)\n",
            "fun caller(holder: Holder) {\n",
            "    holder.repo?.load()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "safe call should not warn: {diags:?}");
}

#[test]
fn member_access_on_non_nullable_field_chain_is_clean() {
    // `holder.repo.load()` where `repo: Repository` (non-nullable) is fine.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Repository {\n",
            "    fun load() {}\n",
            "}\n",
            "data class Holder(val repo: Repository)\n",
            "fun caller(holder: Holder) {\n",
            "    holder.repo.load()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "non-nullable field should not warn: {diags:?}"
    );
}

#[test]
fn member_access_on_local_val_from_cross_file_field_in_live_path_warns() {
    // The user's live-editor report: `val texts = arg.texts` (a local val whose
    // initializer is a nullable field of a data class declared in *another*
    // file), then `texts.member` with a plain `.`.
    //
    // This exercises the *live* inference path specifically: the editor buffer
    // populates `live_lines`, which `infer_variable_type_core` consults first.
    // The local `val texts = arg.texts` has no type annotation, so the live line
    // scan can't infer it, and the field declaration that *would* give the type
    // lives in a different file (so the current-file scan can't see it either).
    // Inference must therefore fall through to the CST-derived field-access RHS
    // data — which the live branch previously skipped, silently dropping the
    // diagnostic that the CLI/indexed path produced.
    //
    // `Confirmation` (the field's class) is placed in the access file so member
    // resolution doesn't depend on the ripgrep cross-file fallback, which isn't
    // available for the in-memory virtual URIs used in these tests.
    let arg_src = "data class Arg(val texts: Confirmation?)\n";
    let mapper_src = concat!(
        "class Confirmation {\n",
        "    fun bar() {}\n",
        "}\n",
        "fun map(arg: Arg) {\n",
        "    val texts = arg.texts\n",
        "    texts.bar()\n",
        "}\n",
    );
    let arg_uri = uri("/arg.kt");
    let mapper_uri = uri("/mapper.kt");
    let idx = Indexer::new();
    idx.index_content(&arg_uri, arg_src);
    idx.index_content(&mapper_uri, mapper_src);
    idx.store_live_tree(&mapper_uri, mapper_src);
    // Mimic the live editor path (didOpen/didChange populate live_lines).
    idx.set_live_lines(&mapper_uri, mapper_src);

    let diags = run_diagnostics(&idx, &mapper_uri, mapper_src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("bar"));
}

#[test]
fn member_access_on_local_val_from_deeply_nested_field_in_live_path_warns() {
    // The user's *actual* live case: the local val is initialized from a
    // nullable field whose type is deeply-nested (`Outer.Mid.Leaf?`), declared
    // in another file. Combines the live-path inference fix with the nested
    // member-confirmation traversal (`resolve_qualified` must split the nested
    // remainder `Mid.Leaf` into individual segments, not search for a literal
    // `"Mid.Leaf"` symbol).
    //
    // The nested type lives in the access file so member resolution doesn't
    // need the ripgrep cross-file fallback (unavailable for virtual test URIs).
    let arg_src = "data class Arg(val texts: Outer.Mid.Leaf?)\n";
    let mapper_src = concat!(
        "class Outer {\n",
        "    class Mid {\n",
        "        class Leaf {\n",
        "            fun bar() {}\n",
        "        }\n",
        "    }\n",
        "}\n",
        "fun map(arg: Arg) {\n",
        "    val texts = arg.texts\n",
        "    texts.bar()\n",
        "}\n",
    );
    let arg_uri = uri("/arg.kt");
    let mapper_uri = uri("/mapper.kt");
    let idx = Indexer::new();
    idx.index_content(&arg_uri, arg_src);
    idx.index_content(&mapper_uri, mapper_src);
    idx.store_live_tree(&mapper_uri, mapper_src);
    idx.set_live_lines(&mapper_uri, mapper_src);

    let diags = run_diagnostics(&idx, &mapper_uri, mapper_src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("bar"));
}

#[test]
fn member_access_on_deeply_nested_field_type_warns() {
    // The user's report: the field's type is a deeply-nested `Bar.Baz.Foo?`.
    // Member resolution must traverse the full nested-type qualifier.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Bar {\n",
            "    class Baz {\n",
            "        class Foo {\n",
            "            fun bar() {}\n",
            "        }\n",
            "    }\n",
            "}\n",
            "data class Data(val foo: Bar.Baz.Foo?)\n",
            "fun caller(data: Data) {\n",
            "    data.foo.bar()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("bar"));
}

#[test]
fn safe_call_on_deeply_nested_field_type_is_clean() {
    // `data.foo?.bar()` on a deeply-nested nullable field type — safe call is fine.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Bar {\n",
            "    class Baz {\n",
            "        class Foo {\n",
            "            fun bar() {}\n",
            "        }\n",
            "    }\n",
            "}\n",
            "data class Data(val foo: Bar.Baz.Foo?)\n",
            "fun caller(data: Data) {\n",
            "    data.foo?.bar()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(diags.is_empty(), "safe call should not warn: {diags:?}");
}

#[test]
fn unknown_member_on_deeply_nested_field_type_is_skipped() {
    // Unresolved member on a nested field type — skip rather than guess.
    let (uri, idx, src) = setup(&[(
        "/a.kt",
        concat!(
            "class Bar {\n",
            "    class Baz {\n",
            "        class Foo {\n",
            "            fun bar() {}\n",
            "        }\n",
            "    }\n",
            "}\n",
            "data class Data(val foo: Bar.Baz.Foo?)\n",
            "fun caller(data: Data) {\n",
            "    data.foo.totallyUnknownMember()\n",
            "}\n",
        ),
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "unresolved member should not warn: {diags:?}"
    );
}

#[test]
fn deeply_nested_member_resolves_in_owner_file_despite_same_named_decoy() {
    // Regression for the user's real workspace (19 same-named `TextsResponseBody`
    // classes across modules): `resolve_qualified` must traverse the nested
    // remainder (`Scenes.Confirmation`) *inside the file the outer type resolved
    // to*, splitting it per-level. Before the fix it searched for a single
    // literal `"Scenes.Confirmation"` symbol (never indexed), then fell back to a
    // GLOBAL `resolve_symbol("Scenes.Confirmation")` that — at scale — resolved to
    // an unrelated same-named nested type, so the member was never confirmed and
    // the diagnostic silently skipped.
    //
    // The decoy here is a *different* `Scenes.Confirmation` (no `tabCzech`) in the
    // access file's own package; the real type is reached via an explicit import.
    // Without the per-level split, the global fallback resolves the decoy and the
    // member is lost; with it, traversal stays anchored to the imported owner file.
    let real_src = concat!(
        "package real\n",
        "class TextsResponseBody {\n",
        "    class Scenes {\n",
        "        class Confirmation {\n",
        "            val tabCzech: String? = null\n",
        "        }\n",
        "    }\n",
        "}\n",
    );
    let decoy_src = concat!(
        "package demo\n",
        "class Scenes {\n",
        "    class Confirmation {\n",
        "        val somethingElse: String? = null\n",
        "    }\n",
        "}\n",
    );
    let mapper_src = concat!(
        "package demo\n",
        "import real.TextsResponseBody\n",
        "data class Arg(val texts: TextsResponseBody.Scenes.Confirmation?)\n",
        "fun map(arg: Arg) {\n",
        "    val texts = arg.texts\n",
        "    texts.tabCzech\n",
        "}\n",
    );
    let real_uri = uri("/real.kt");
    let decoy_uri = uri("/decoy.kt");
    let mapper_uri = uri("/mapper.kt");
    let idx = Indexer::new();
    idx.index_content(&real_uri, real_src);
    idx.index_content(&decoy_uri, decoy_src);
    idx.index_content(&mapper_uri, mapper_src);
    idx.store_live_tree(&mapper_uri, mapper_src);
    idx.set_live_lines(&mapper_uri, mapper_src);

    let diags = run_diagnostics(&idx, &mapper_uri, mapper_src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("tabCzech"));
}

#[test]
fn direct_nested_field_chain_member_resolves_despite_same_named_decoy() {
    // Sibling to `deeply_nested_member_resolves_in_owner_file_despite_same_named_decoy`,
    // but for the *direct* field-chain shape `arg.texts.tabCzech` (no intermediate
    // `val texts = arg.texts`). The two shapes take different paths through
    // `resolve_qualified`:
    //   * local-val: `infer_variable_type("texts")` yields the dotted nested type,
    //     handled by the per-level `find_name_in_uri` loop (the path fixed by the
    //     `inner_type.split('.')` change).
    //   * direct:    `qualifier == "arg.texts"`; the loop hits the `texts` *field*
    //     branch and hands the dotted field type straight to `resolve_symbol`,
    //     relying on its dotted handler to stay anchored to the imported owner
    //     file rather than the same-named decoy.
    // The decoy is a different `Scenes.Confirmation` (no `tabCzech`) in the access
    // file's own package; the real type is reached via an explicit import.
    let real_src = concat!(
        "package real\n",
        "class TextsResponseBody {\n",
        "    class Scenes {\n",
        "        class Confirmation {\n",
        "            val tabCzech: String? = null\n",
        "        }\n",
        "    }\n",
        "}\n",
    );
    let decoy_src = concat!(
        "package demo\n",
        "class Scenes {\n",
        "    class Confirmation {\n",
        "        val somethingElse: String? = null\n",
        "    }\n",
        "}\n",
    );
    let mapper_src = concat!(
        "package demo\n",
        "import real.TextsResponseBody\n",
        "data class Arg(val texts: TextsResponseBody.Scenes.Confirmation?)\n",
        "fun map(arg: Arg) {\n",
        "    arg.texts.tabCzech\n",
        "}\n",
    );
    let real_uri = uri("/real.kt");
    let decoy_uri = uri("/decoy.kt");
    let mapper_uri = uri("/mapper.kt");
    let indexer = Indexer::new();
    indexer.index_content(&real_uri, real_src);
    indexer.index_content(&decoy_uri, decoy_src);
    indexer.index_content(&mapper_uri, mapper_src);
    indexer.store_live_tree(&mapper_uri, mapper_src);
    indexer.set_live_lines(&mapper_uri, mapper_src);

    let diagnostics = run_diagnostics(&indexer, &mapper_uri, mapper_src);
    assert_eq!(
        diagnostics.len(),
        1,
        "expected one diagnostic: {diagnostics:?}"
    );
    assert!(diagnostics[0].message.contains("tabCzech"));
}

/// Regression: `collect_nav_nodes` recurses over every CST node
/// unconditionally, and `pure_field_chain` recurses through an arbitrarily
/// long field-access chain — neither had a depth guard. Before
/// `MAX_CST_DESCENT_DEPTH` was added, a long `a.b.c.d…` chain (ordinary
/// Kotlin, no malformed input needed) overflowed an 8 MiB stack (Linux's
/// real main-thread default). This drives `nullable_dot_call_diagnostics`
/// — the production entry point — several times past the old crash
/// threshold; without the guard this aborts the whole test process rather
/// than failing an assertion.
#[test]
fn nullable_diagnostics_survives_a_pathologically_deep_field_chain() {
    let n = 5_000; // several times MAX_CST_DESCENT_DEPTH (512)
    let mut chain = String::from("a");
    for i in 0..n {
        chain.push_str(&format!(".b{i}"));
    }
    let src = format!("fun f(a: Any) {{\n    {chain}\n}}\n");
    let (uri, idx, src) = setup(&[("/a.kt", &src)]);
    let _ = run_diagnostics(&idx, &uri, &src);
}
