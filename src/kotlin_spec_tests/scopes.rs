use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::Indexer;
use crate::resolver::resolve_symbol;
use tower_lsp::lsp_types::{GotoDefinitionResponse, Position, SymbolKind, Url};

fn position_of_occurrence(source: &str, needle: &str, occurrence: usize) -> Position {
    let byte_offset = source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(byte_offset, _)| byte_offset)
        .expect("fixture occurrence must exist");
    let preceding_source = &source[..byte_offset];
    let line = preceding_source.matches('\n').count() as u32;
    let character = preceding_source
        .rsplit('\n')
        .next()
        .expect("split always yields one segment")
        .chars()
        .count() as u32;
    Position::new(line, character)
}

async fn definition_position(source: &str, needle: &str, occurrence: usize) -> Option<Position> {
    let specification_uri =
        Url::parse("file:///kotlin-spec/Scopes.kt").expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let position = position_of_occurrence(source, needle, occurrence);
    let cursor_context = CursorContext::build(&indexer, &specification_uri, position)
        .expect("fixture cursor must select an identifier");

    match find_definition(&cursor_context, &indexer, &specification_uri, position).await {
        Some(GotoDefinitionResponse::Scalar(location)) => Some(location.range.start),
        Some(GotoDefinitionResponse::Array(locations)) if locations.len() == 1 => {
            Some(locations[0].range.start)
        }
        Some(GotoDefinitionResponse::Array(_)) | Some(GotoDefinitionResponse::Link(_)) | None => {
            None
        }
    }
}

#[test]
fn ks_6_001_declaration_scopes_bind_types_and_values() {
    let source = "class ModelSpec\nval valueSpec: ModelSpec = ModelSpec()\nfun createSpec(): ModelSpec = valueSpec\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ScopeBindings.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    for (name, kind) in [
        ("ModelSpec", SymbolKind::CLASS),
        ("valueSpec", SymbolKind::PROPERTY),
        ("createSpec", SymbolKind::FUNCTION),
    ] {
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.name == name && symbol.kind == kind),
            "{name} must introduce a {kind:?} binding"
        );
    }
}

#[tokio::test]
#[ignore = "KS-6-002: kmp-lsp does not resolve forward references in declaration scopes"]
async fn ks_6_002_declaration_scope_allows_forward_reference() {
    let source = "val valueSpec: Int = 99\nclass HostSpec {\n    fun readSpec(): Int = valueSpec\n    val valueSpec: Int = 3\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 1).await,
        Some(Position::new(3, 8))
    );
}

#[tokio::test]
#[ignore = "KS-6-003: kmp-lsp does not model statement-scope binding order"]
async fn ks_6_003_statement_scope_binds_values_in_appearance_order() {
    let source = "val valueSpec: Int = 99\nfun readSpec(): Int {\n    val beforeSpec = valueSpec\n    val valueSpec: Int = 3\n    return beforeSpec + valueSpec\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 1).await,
        Some(Position::new(0, 4))
    );
    assert_eq!(
        definition_position(source, "valueSpec", 3).await,
        Some(Position::new(3, 8))
    );
}

#[test]
#[ignore = "KS-6-004: kmp-lsp does not diagnose same-scope value redeclarations"]
fn ks_6_004_same_scope_value_redeclaration_is_forbidden() {
    assert_source_parses(
        "val valueSpec: Int = 1\nfun readSpec(): Int { val valueSpec: Int = 2; return valueSpec }\n",
    );
    assert_source_has_syntax_error("val valueSpec: Int = 1\nval valueSpec: Int = 2\n");
}

#[test]
fn ks_6_005_same_scope_function_overloads_are_allowed() {
    let source = "fun renderSpec(valueSpec: Int): Int = valueSpec\nfun renderSpec(valueSpec: String): String = valueSpec\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ScopeOverloads.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let overloads = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .filter(|symbol| symbol.name == "renderSpec")
        .collect::<Vec<_>>();
    assert_eq!(overloads.len(), 2);
    assert!(overloads
        .iter()
        .all(|symbol| symbol.kind == SymbolKind::FUNCTION));
}

#[test]
#[ignore = "KS-6-006: kmp-lsp does not diagnose same-receiver property redeclarations"]
fn ks_6_006_same_receiver_property_redeclaration_is_forbidden() {
    assert_source_parses("val firstSpec: Int = 1\nval secondSpec: Int = 2\n");
    assert_source_has_syntax_error("val valueSpec: Int = 1\nval valueSpec: String = \"two\"\n");
}

#[test]
fn ks_6_007_top_level_import_introduces_a_binding() {
    let declaration_uri =
        Url::parse("file:///kotlin-spec/library/Values.kt").expect("declaration URI must be valid");
    let usage_uri =
        Url::parse("file:///kotlin-spec/client/Usage.kt").expect("usage URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package library\nval importedSpec: Int = 1\n",
    );
    indexer.index_content(
        &usage_uri,
        "package client\nimport library.importedSpec\nval copiedSpec: Int = importedSpec\n",
    );

    let locations = resolve_symbol(&indexer, "importedSpec", None, &usage_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, declaration_uri);
    assert_eq!(locations[0].range.start, Position::new(1, 4));
}

#[tokio::test]
#[ignore = "KS-6.1-001: kmp-lsp does not disambiguate transitively linked statement scopes"]
async fn ks_6_1_001_statement_scope_is_linked_to_directly_nested_scope() {
    let source = "val outerSpec: Int = 99\nfun readSpec(): Int {\n    val outerSpec: Int = 1\n    if (true) { while (true) { return outerSpec } }\n    return 0\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "outerSpec", 2).await,
        Some(position_of_occurrence(source, "outerSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-002: kmp-lsp does not disambiguate object and nested scopes"]
async fn ks_6_1_002_object_scope_is_linked_to_nested_scope() {
    let source = "val storedSpec: Int = 99\nobject RegistrySpec {\n    val storedSpec: Int = 1\n    fun readSpec(): Int = storedSpec\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "storedSpec", 2).await,
        Some(position_of_occurrence(source, "storedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-003: kmp-lsp does not model object links to superclass companions"]
async fn ks_6_1_003_object_scope_links_to_superclass_companion_non_transitively() {
    let source = "val inheritedSpec: Int = 99\nopen class BaseSpec {\n    companion object { val inheritedSpec: Int = 1; }\n}\nobject DerivedSpec : BaseSpec() { fun readSpec(): Int = inheritedSpec; }\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "inheritedSpec", 2).await,
        Some(position_of_occurrence(source, "inheritedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-004: kmp-lsp does not model object links to parent companions"]
async fn ks_6_1_004_object_scope_links_to_parent_classifier_companion() {
    let source = "val sharedSpec: Int = 99\nclass HostSpec {\n    companion object { val sharedSpec: Int = 1; }\n    object NestedSpec { fun readSpec(): Int = sharedSpec; }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "sharedSpec", 2).await,
        Some(position_of_occurrence(source, "sharedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-005: kmp-lsp does not disambiguate companion and nested scopes"]
async fn ks_6_1_005_companion_scope_is_linked_to_nested_scope() {
    let source = "val sharedSpec: Int = 99\nclass HostSpec {\n    companion object {\n        val sharedSpec: Int = 1\n        fun readSpec(): Int = sharedSpec\n    }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "sharedSpec", 2).await,
        Some(position_of_occurrence(source, "sharedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-006: kmp-lsp does not model companion links to superclass companions"]
async fn ks_6_1_006_companion_scope_links_to_superclass_companion_non_transitively() {
    let source = "val inheritedSpec: Int = 99\nopen class BaseSpec {\n    companion object { val inheritedSpec: Int = 1; }\n}\nclass DerivedSpec {\n    companion object : BaseSpec() { fun readSpec(): Int = inheritedSpec; }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "inheritedSpec", 2).await,
        Some(position_of_occurrence(source, "inheritedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-007: kmp-lsp does not model classifier links to companions"]
async fn ks_6_1_007_classifier_scope_links_to_its_companion() {
    let source = "val sharedSpec: Int = 99\nclass HostSpec {\n    companion object { val sharedSpec: Int = 1; }\n    fun readSpec(): Int = sharedSpec\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "sharedSpec", 2).await,
        Some(position_of_occurrence(source, "sharedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-008: kmp-lsp does not model inner-class links to parent classifiers"]
async fn ks_6_1_008_inner_class_scope_links_to_parent_classifier() {
    let source = "val outerValueSpec: Int = 99\nclass OuterSpec {\n    val outerValueSpec: Int = 1\n    inner class InnerSpec { fun readSpec(): Int = outerValueSpec; }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "outerValueSpec", 2).await,
        Some(position_of_occurrence(source, "outerValueSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-009: kmp-lsp does not disambiguate parameter scope links"]
async fn ks_6_1_009_function_parameter_scope_links_container_and_body() {
    let source = "val fallbackSpec: Int = 99\nval valueSpec: Int = 99\nclass HostSpec {\n    val fallbackSpec: Int = 1\n    fun readSpec(valueSpec: Int = fallbackSpec): Int = valueSpec\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "fallbackSpec", 2).await,
        Some(position_of_occurrence(source, "fallbackSpec", 1))
    );
    assert_eq!(
        definition_position(source, "valueSpec", 2).await,
        Some(position_of_occurrence(source, "valueSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-010: kmp-lsp does not model primary-constructor initialization links"]
async fn ks_6_1_010_primary_constructor_parameter_links_to_initialization_scope() {
    let source = "val valueSpec: Int = 99\nclass HostSpec(val valueSpec: Int) {\n    val copiedSpec: Int = valueSpec\n    init { println(valueSpec) }\n}\n";
    assert_source_parses(source);
    for occurrence in [2, 3] {
        assert_eq!(
            definition_position(source, "valueSpec", occurrence).await,
            Some(position_of_occurrence(source, "valueSpec", 1))
        );
    }
}

#[tokio::test]
#[ignore = "KS-6.1-011: kmp-lsp does not link objects to parent-superclass companions"]
async fn ks_6_1_011_object_scope_links_to_parent_classifier_superclass_companion() {
    let source = "val inheritedSpec: Int = 99\nopen class BaseSpec {\n    companion object { val inheritedSpec: Int = 1; }\n}\nclass HostSpec : BaseSpec() {\n    object NestedSpec { fun readSpec(): Int = inheritedSpec; }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "inheritedSpec", 2).await,
        Some(position_of_occurrence(source, "inheritedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-012: kmp-lsp does not link companions to parent-superclass companions"]
async fn ks_6_1_012_companion_scope_links_to_parent_classifier_superclass_companion() {
    let source = "val inheritedSpec: Int = 99\nopen class BaseSpec {\n    companion object { val inheritedSpec: Int = 1; }\n}\nclass HostSpec : BaseSpec() {\n    companion object { fun readSpec(): Int = inheritedSpec; }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "inheritedSpec", 2).await,
        Some(position_of_occurrence(source, "inheritedSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-013: kmp-lsp does not link companions to enclosing companions"]
async fn ks_6_1_013_companion_scope_links_to_parent_of_parent_companion() {
    let source = "val enclosingSpec: Int = 99\nclass OuterSpec {\n    companion object { val enclosingSpec: Int = 1; }\n    class NestedSpec {\n        companion object { fun readSpec(): Int = enclosingSpec; }\n    }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "enclosingSpec", 2).await,
        Some(position_of_occurrence(source, "enclosingSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-014: kmp-lsp does not enforce the primary-constructor upward-link boundary"]
async fn ks_6_1_014_primary_constructor_parameter_scope_excludes_classifier_body() {
    let source = "val sourceSpec: Int = 99\nclass HostSpec(val copiedSpec: Int = sourceSpec) {\n    val sourceSpec: Int = 1\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "sourceSpec", 1).await,
        Some(position_of_occurrence(source, "sourceSpec", 0))
    );
}

#[tokio::test]
#[ignore = "KS-6.1-015: kmp-lsp does not model classifier initialization scopes"]
async fn ks_6_1_015_initialization_block_links_to_classifier_initialization_scope() {
    let source = "val initializedSpec: Int = 99\nclass HostSpec(val valueSpec: Int) {\n    val initializedSpec: Int = valueSpec\n    init { println(initializedSpec) }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "initializedSpec", 2).await,
        Some(position_of_occurrence(source, "initializedSpec", 1))
    );
}

#[tokio::test]
async fn ks_6_2_001_simple_and_qualified_paths_reference_entities() {
    let source = "class FirstSpec { val valueSpec: Int = 1; }\nclass SecondSpec { val valueSpec: Int = 2; }\nval simpleSpec: FirstSpec = FirstSpec()\nval copiedSpec: Int = simpleSpec.valueSpec\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "simpleSpec", 1).await,
        Some(position_of_occurrence(source, "simpleSpec", 0))
    );
    assert_eq!(
        definition_position(source, "valueSpec", 2).await,
        Some(position_of_occurrence(source, "valueSpec", 0))
    );
}

#[tokio::test]
async fn ks_6_2_002_this_references_the_default_receiver() {
    let source = "class HostSpec {\n    val valueSpec: Int = 1\n    fun readSpec(): Int {\n        val valueSpec: Int = 99\n        return this.valueSpec\n    }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 2).await,
        Some(position_of_occurrence(source, "valueSpec", 0))
    );
}

#[tokio::test]
async fn ks_6_2_003_labeled_this_selects_the_labeled_receiver() {
    let source = "class OuterSpec {\n    val valueSpec: Int = 1\n    inner class InnerSpec {\n        val valueSpec: Int = 99\n        fun readSpec(): Int = this@OuterSpec.valueSpec\n    }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 2).await,
        Some(position_of_occurrence(source, "valueSpec", 0))
    );
}

#[tokio::test]
#[ignore = "KS-6.2-004: kmp-lsp does not resolve explicitly selected supertype members"]
async fn ks_6_2_004_super_type_qualifier_selects_the_named_supertype() {
    let source = "interface FirstSpec { fun renderSpec(): Int = 1; }\ninterface SecondSpec { fun renderSpec(): Int = 2; }\nclass HostSpec : FirstSpec, SecondSpec {\n    override fun renderSpec(): Int = super<FirstSpec>.renderSpec()\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "renderSpec", 3).await,
        Some(position_of_occurrence(source, "renderSpec", 0))
    );
}

#[tokio::test]
#[ignore = "KS-6.2-005: kmp-lsp resolves labeled super calls back to the override"]
async fn ks_6_2_005_labeled_super_selects_supertype_in_labeled_scope() {
    let source = "open class BaseSpec { open fun renderSpec(): Int = 1; }\nclass HostSpec : BaseSpec() {\n    override fun renderSpec(): Int = run { super<BaseSpec>@HostSpec.renderSpec() }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "renderSpec", 2).await,
        Some(position_of_occurrence(source, "renderSpec", 0))
    );
}

#[test]
fn ks_6_3_001_lambda_expressions_and_loops_may_be_labeled() {
    assert_source_parses(
        "fun readSpec(valuesSpec: List<Int>) {\n    valuesSpec.forEach lambdaSpec@ { return@lambdaSpec }\n    loopSpec@ for (valueSpec in valuesSpec) { if (valueSpec < 0) continue@loopSpec; if (valueSpec == 0) break@loopSpec }\n}\n",
    );
}

#[test]
fn ks_6_3_002_labels_may_reuse_the_same_identifier() {
    assert_source_parses(
        "fun readSpec() {\n    repeatedSpec@ repeatedSpec@ for (outerSpec in 0..1) {\n        repeatedSpec@ for (innerSpec in 0..1) { break@repeatedSpec }\n    }\n}\n",
    );
}

#[test]
#[ignore = "KS-6.3-003: kmp-lsp does not diagnose labels used outside their scope"]
fn ks_6_3_003_label_is_available_only_in_its_declaring_scope() {
    assert_source_parses(
        "fun validSpec() { loopSpec@ for (valueSpec in 0..1) { break@loopSpec } }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec() { loopSpec@ for (valueSpec in 0..1) { }\n    break@loopSpec\n}\n",
    );
}

#[tokio::test]
#[ignore = "KS-6.3-004: kmp-lsp does not resolve jump labels to their declarations"]
async fn ks_6_3_004_closest_matching_label_is_selected() {
    let source = "fun readSpec() {\n    repeatedSpec@ for (outerSpec in 0..1) {\n        repeatedSpec@ for (innerSpec in 0..1) { break@repeatedSpec }\n    }\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "repeatedSpec", 2).await,
        Some(position_of_occurrence(source, "repeatedSpec", 1))
    );
}
