use tower_lsp::lsp_types::{CodeActionKind, CodeActionOrCommand, Diagnostic, DiagnosticTag, Url};

use crate::indexer::live_tree::parse_live;

use super::{unused_import_actions, unused_import_diagnostics};

fn uri() -> Url {
    Url::parse("file:///test.kt").unwrap()
}

fn run_diagnostics(source: &str) -> Vec<Diagnostic> {
    let doc = parse_live(source, tree_sitter_kotlin::language()).unwrap();
    unused_import_diagnostics(&doc)
}

#[test]
fn flags_a_genuinely_unused_import() {
    let source =
        "package app\n\nimport com.example.lib.Unused\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let diags = run_diagnostics(source);
    assert_eq!(
        diags.len(),
        1,
        "expected one unused-import diagnostic: {diags:?}"
    );
    assert!(diags[0].message.contains("com.example.lib.Unused"));
    assert_eq!(
        diags[0].severity,
        Some(tower_lsp::lsp_types::DiagnosticSeverity::HINT)
    );
    assert_eq!(diags[0].tags, Some(vec![DiagnosticTag::UNNECESSARY]));
    assert_eq!(
        diags[0].range.start.line, 2,
        "import is on line 2 (0-based)"
    );
}

#[test]
fn no_diagnostic_for_a_bare_call_use() {
    let source =
        "package app\n\nimport com.example.lib.doThing\n\nfun demo() {\n    doThing()\n}\n";
    let diags = run_diagnostics(source);
    assert!(diags.is_empty(), "doThing() is a real use: {diags:?}");
}

#[test]
fn no_diagnostic_for_a_bare_type_use() {
    let source = "package app\n\nimport com.example.lib.Thing\n\nfun demo(): Thing = TODO()\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "Thing as a return type is a real use: {diags:?}"
    );
}

#[test]
fn no_diagnostic_for_a_member_or_extension_call_use() {
    let source = "package app\n\nimport com.example.lib.firstOrNull\n\nfun demo(list: List<Int>) {\n    list.firstOrNull()\n}\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "list.firstOrNull() uses the extension function's own name: {diags:?}"
    );
}

#[test]
fn no_diagnostic_for_an_annotation_use() {
    let source = "package app\n\nimport com.example.lib.Composable\n\n@Composable\nfun Demo() {}\n";
    let diags = run_diagnostics(source);
    assert!(diags.is_empty(), "@Composable is a real use: {diags:?}");
}

/// Found running the CLI precision benchmark against nowInAndroid: `by`
/// property delegation desugars to a `getValue`/`setValue` call the compiler
/// synthesizes from the `by` keyword -- the name never appears as literal
/// identifier text, so it must be exempted by name, not caught by widening
/// the identifier walk (there is nothing to widen to).
#[test]
fn no_diagnostic_for_a_property_delegate_operator_import() {
    let source = "package app\n\nimport androidx.compose.runtime.getValue\n\nfun demo() {\n    val x by lazy { 1 }\n    println(x)\n}\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "getValue backs the `by` delegate, used implicitly: {diags:?}"
    );
}

/// Same root cause as the property-delegate case, via Gradle's Kotlin DSL
/// `assign`/`invoke`/`get` operator conventions rather than Kotlin's own `by`.
#[test]
fn no_diagnostic_for_a_gradle_dsl_operator_import() {
    let source = "package app\n\nimport org.gradle.kotlin.dsl.assign\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "assign backs Gradle's `=` DSL sugar, used implicitly: {diags:?}"
    );
}

/// PR review finding: `componentN` is unbounded (any class can declare
/// `operator fun component6()` and beyond), not just component1..component5.
#[test]
fn no_diagnostic_for_a_component_n_import_beyond_five() {
    let source = "package app\n\nimport com.example.lib.component6\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "component6 backs destructuring for a 6-property data class, used implicitly: {diags:?}"
    );
}

/// `componentX` (non-digit suffix) is a real, ordinary name, not the
/// destructuring convention -- must still be flagged when genuinely unused.
#[test]
fn flags_a_name_merely_starting_with_component_but_not_component_n() {
    let source = "package app\n\nimport com.example.lib.componentDidMount\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let diags = run_diagnostics(source);
    assert_eq!(
        diags.len(),
        1,
        "componentDidMount is not a componentN operator convention: {diags:?}"
    );
}

/// Found running the same benchmark: tree-sitter-kotlin does not parse KDoc
/// comment bodies into structured sub-nodes at all (`multiline_comment` is
/// one opaque leaf), so a name referenced only via a `[Reference]` KDoc link
/// needs its own text scan over comment content -- there is no CST node to
/// widen the walk to.
#[test]
fn no_diagnostic_for_a_kdoc_reference_only_use() {
    let source = "package app\n\nimport com.example.lib.Target\n\n/**\n * Uses [Target] internally.\n */\nfun demo() {}\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "Target is referenced via a KDoc [Reference] link: {diags:?}"
    );
}

/// Real build break found deleting every flagged import across a real
/// ~13k-file monorepo (Moneta) and compiling the result: bare `$identifier`
/// string-template interpolation parses to its own `interpolated_identifier`
/// leaf node -- a genuine CST-shape gap, not a "nothing to widen to" case
/// like the two exemptions above.
#[test]
fn no_diagnostic_for_a_bare_string_template_interpolation_use() {
    let source = "package app\n\nimport com.example.lib.PATTERN\n\nfun demo() {\n    val regex = Regex(\"^$PATTERN$\")\n    println(regex)\n}\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "PATTERN is used via bare $PATTERN string interpolation: {diags:?}"
    );
}

/// Braced `${...}` interpolation was already correctly handled before this
/// fix -- pinned as a regression guard now that the bare-form fix landed
/// alongside it, so a future change can't silently break one while fixing
/// the other.
#[test]
fn no_diagnostic_for_a_braced_string_template_interpolation_use() {
    let source = "package app\n\nimport com.example.lib.PATTERN\n\nfun demo() {\n    println(\"value: ${PATTERN}\")\n}\n";
    let diags = run_diagnostics(source);
    assert!(
        diags.is_empty(),
        "PATTERN is used only via ${{PATTERN}} braced string interpolation: {diags:?}"
    );
}

#[test]
fn star_imports_are_never_flagged() {
    let source =
        "package app\n\nimport com.example.lib.*\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let diags = run_diagnostics(source);
    assert!(diags.is_empty(), "star imports are out of scope: {diags:?}");
}

#[test]
fn aliased_import_checks_the_alias_not_the_original_name() {
    let used = "package app\n\nimport com.example.lib.Original as Renamed\n\nfun demo(): Renamed = TODO()\n";
    assert!(
        run_diagnostics(used).is_empty(),
        "the alias Renamed is used, must not be flagged"
    );

    let unused = "package app\n\nimport com.example.lib.Original as Renamed\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let diags = run_diagnostics(unused);
    assert_eq!(
        diags.len(),
        1,
        "neither Original nor Renamed is used: {diags:?}"
    );
}

#[test]
fn offers_a_remove_import_quickfix_for_a_flagged_diagnostic() {
    let source =
        "package app\n\nimport com.example.lib.Unused\n\nfun demo() {\n    println(\"hi\")\n}\n";
    let diags = run_diagnostics(source);
    assert_eq!(diags.len(), 1);

    let actions = unused_import_actions(&uri(), &diags);
    assert_eq!(actions.len(), 1, "expected one quick-fix: {actions:?}");
    let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
        panic!("expected a CodeAction, got {:?}", actions[0]);
    };
    assert_eq!(action.title, "Remove unused import");
    assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));

    let file_edits = action
        .edit
        .as_ref()
        .and_then(|edit| edit.changes.as_ref())
        .and_then(|changes| changes.get(&uri()))
        .expect("edit must target the file");
    assert_eq!(file_edits.len(), 1);
    assert_eq!(file_edits[0].new_text, "", "must delete, not replace");
    assert_eq!(file_edits[0].range.start.line, 2);
    assert_eq!(
        file_edits[0].range.end.line, 3,
        "must delete the whole line including its newline"
    );
}
