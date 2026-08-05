use tower_lsp::lsp_types::{CodeActionKind, CodeActionOrCommand, Diagnostic, Url};

use crate::indexer::live_tree::parse_live;
use crate::indexer::Indexer;

use super::{missing_import_actions, missing_import_diagnostics};

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
    // fqns_for_name reads importable_fqns, which index_content alone does not
    // populate — it's rebuilt on the dirty-check path the real scan pipeline
    // drives; tests must trigger it explicitly.
    idx.rebuild_bare_name_cache();
    (last_uri, idx, last_src)
}

fn run_diagnostics(
    idx: &Indexer,
    uri: &Url,
    source: &str,
) -> Vec<tower_lsp::lsp_types::Diagnostic> {
    let doc = parse_live(source, tree_sitter_kotlin::language()).unwrap();
    missing_import_diagnostics(idx, uri, &doc)
}

#[test]
fn flags_an_unimported_type_with_a_known_fqn_elsewhere() {
    let (uri, idx, src) = setup(&[
        ("/lib/Foo.kt", "package com.example.lib\nclass Foo\n"),
        ("/app/Caller.kt", "package app\nfun use(): Foo = Foo()\n"),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");
    assert!(diags[0].message.contains("Foo"));
    assert_eq!(
        diags[0].severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::WARNING)
    );
}

#[test]
fn no_diagnostic_when_explicitly_imported() {
    let (uri, idx, src) = setup(&[
        ("/lib/Foo.kt", "package com.example.lib\nclass Foo\n"),
        (
            "/app/Caller.kt",
            "package app\nimport com.example.lib.Foo\nfun use(): Foo = Foo()\n",
        ),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "explicit import should suppress: {diags:?}"
    );
}

#[test]
fn no_diagnostic_for_default_import_stdlib_type() {
    let (uri, idx, src) = setup(&[(
        "/app/Caller.kt",
        "package app\nfun use(): Result<Int> = TODO()\n",
    )]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "kotlin.Result is default-import, must not be flagged: {diags:?}"
    );
}

/// While JAR indexing is in flight the "importable" and default-import checks
/// read partial jar_definitions/jar_qualified data, so diagnostics are
/// suppressed to avoid a transient false positive. They resume (and are
/// republished by the actor) once indexing reaches a terminal phase — same
/// contract as call_arg_diagnostics.
#[test]
fn diagnostics_suppressed_while_jars_loading() {
    use crate::indexer::jar_phase::JarPhase;
    let (uri, idx, src) = setup(&[
        ("/lib/Foo.kt", "package com.example.lib\nclass Foo\n"),
        ("/app/Caller.kt", "package app\nfun use(): Foo = Foo()\n"),
    ]);

    *idx.jar_phase.lock().unwrap() = JarPhase::InProgress;
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "diagnostics must be suppressed while JARs load: {diags:?}"
    );

    *idx.jar_phase.lock().unwrap() = JarPhase::Ready { count: 1 };
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        !diags.is_empty(),
        "diagnostics must resume once JAR indexing is done"
    );
}

#[test]
fn offers_an_import_quickfix_for_a_flagged_diagnostic() {
    let (uri, idx, src) = setup(&[
        ("/lib/Foo.kt", "package com.example.lib\nclass Foo\n"),
        ("/app/Caller.kt", "package app\nfun use(): Foo = Foo()\n"),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(diags.len(), 1, "expected one diagnostic: {diags:?}");

    let actions = missing_import_actions(&idx, &uri, &diags);
    assert_eq!(
        actions.len(),
        1,
        "expected exactly one import candidate: {actions:?}"
    );
    let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
        panic!("expected a CodeAction, got {:?}", actions[0]);
    };
    assert_eq!(action.title, "Import 'com.example.lib.Foo'");
    assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
    assert_eq!(
        action.diagnostics.as_deref(),
        Some(&diags[..]),
        "quickfix must be associated back to the diagnostic it fixes"
    );
    let file_edits = action
        .edit
        .as_ref()
        .and_then(|edit| edit.changes.as_ref())
        .and_then(|changes| changes.get(&uri))
        .expect("edit must target the caller file");
    assert_eq!(file_edits.len(), 1);
    assert!(
        file_edits[0]
            .new_text
            .contains("import com.example.lib.Foo"),
        "expected an import insertion, got {:?}",
        file_edits[0].new_text
    );
}

/// Regression: `EnumType.CONSTANT` (or any `Type.member` navigation root) parses
/// as a bare `simple_identifier`, not `type_identifier` — tree-sitter-kotlin only
/// emits `type_identifier` in type-annotation positions, never expression
/// positions. `collect_candidates` only ever looked at `type_identifier` nodes for
/// bare-class detection, so a capitalized navigation root like this was never even
/// considered as a candidate — an unimported `DarkThemeConfig` referenced only as
/// `DarkThemeConfig.LIGHT` inside a `when` branch produced zero diagnostics.
#[test]
fn flags_an_unimported_enum_constant_used_as_navigation_root() {
    let (uri, idx, src) = setup(&[
        (
            "/lib/DarkThemeConfig.kt",
            "package com.example.lib\nenum class DarkThemeConfig { LIGHT, DARK }\n",
        ),
        (
            "/app/Caller.kt",
            "package app\nfun use(x: Int): Boolean = when (x) {\n    1 -> DarkThemeConfig.LIGHT == DarkThemeConfig.LIGHT\n    else -> false\n}\n",
        ),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert_eq!(
        diags.len(),
        1,
        "expected one diagnostic for the unimported DarkThemeConfig navigation root: {diags:?}"
    );
    assert!(diags[0].message.contains("DarkThemeConfig"));
}

/// Regression: a capitalized navigation root that's already resolvable (imported,
/// same-package, a local declaration, etc.) must still be exempted — this proves
/// the fix only widens *candidate collection*, not the existing reachability logic.
#[test]
fn no_diagnostic_for_an_imported_enum_constant_used_as_navigation_root() {
    let (uri, idx, src) = setup(&[
        (
            "/lib/DarkThemeConfig.kt",
            "package com.example.lib\nenum class DarkThemeConfig { LIGHT, DARK }\n",
        ),
        (
            "/app/Caller.kt",
            "package app\nimport com.example.lib.DarkThemeConfig\nfun use(x: Int): Boolean = when (x) {\n    1 -> DarkThemeConfig.LIGHT == DarkThemeConfig.LIGHT\n    else -> false\n}\n",
        ),
    ]);
    let diags = run_diagnostics(&idx, &uri, &src);
    assert!(
        diags.is_empty(),
        "explicit import should suppress the navigation-root candidate too: {diags:?}"
    );
}

/// Regression: `missing_import_actions` identified "one of ours" purely by
/// `diagnostic.source == "kmp-lsp"`, a string shared by every diagnostic producer
/// in the LSP, then blindly parsed the message as `'Name' is ...`. `fill_when`'s
/// non-exhaustive-`when` diagnostic ("'when' is missing branches: ...") also
/// starts with a quoted word, so it parses as a flagged name "when" — and in a
/// real project, JAR symbols coincidentally named `when` (e.g. Kotlin compiler
/// internals) give `fqns_for_name` real FQNs to offer, producing a nonsensical
/// "Import 'when'" quick-fix attached to the when-exhaustiveness diagnostic. This
/// test reproduces the same collision shape (a foreign, quote-prefixed message
/// whose leading word happens to name a real importable symbol) without depending
/// on tree-sitter's handling of the literal keyword `when`.
#[test]
fn no_import_action_for_a_foreign_diagnostic_that_starts_with_a_quoted_importable_name() {
    let (uri, idx, _src) = setup(&[
        ("/lib/Foo.kt", "package com.example.lib\nclass Foo\n"),
        ("/app/Caller.kt", "package app\nfun use() {}\n"),
    ]);
    let foreign = vec![Diagnostic {
        message: "'Foo' is missing branches: A, B".to_owned(),
        source: Some("kmp-lsp".to_owned()),
        ..Default::default()
    }];
    let actions = missing_import_actions(&idx, &uri, &foreign);
    assert!(
        actions.is_empty(),
        "must not offer an import fix for a foreign diagnostic that merely starts with a quoted word: {actions:?}"
    );
}

#[test]
fn no_import_action_for_an_unrelated_diagnostic() {
    let (uri, idx, _src) = setup(&[("/app/Caller.kt", "package app\nfun use() {}\n")]);
    let unrelated = vec![Diagnostic {
        message: "some other diagnostic".to_owned(),
        source: Some("kmp-lsp".to_owned()),
        ..Default::default()
    }];
    let actions = missing_import_actions(&idx, &uri, &unrelated);
    assert!(
        actions.is_empty(),
        "must not fire for diagnostics that aren't ours: {actions:?}"
    );
}
