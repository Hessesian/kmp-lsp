use super::{
    assert_source_contains_node_kind, assert_source_has_syntax_error, assert_source_parses,
};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::{Indexer, InferDeps};
use crate::resolver::resolve_symbol;
use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position, SymbolKind, Url};

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

async fn definition_locations(source: &str, needle: &str, occurrence: usize) -> Vec<Location> {
    let specification_uri = Url::parse("file:///kotlin-spec/Declarations.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let position = position_of_occurrence(source, needle, occurrence);
    let cursor_context = CursorContext::build(&indexer, &specification_uri, position)
        .expect("fixture cursor must select an identifier");

    match find_definition(&cursor_context, &indexer, &specification_uri, position).await {
        Some(GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(GotoDefinitionResponse::Array(locations)) => locations,
        Some(GotoDefinitionResponse::Link(_)) => {
            panic!("kmp-lsp definition feature returns locations, not location links")
        }
        None => Vec::new(),
    }
}

fn indexed_classifier_symbols(source: &str) -> Vec<(String, SymbolKind)> {
    let specification_uri = Url::parse("file:///kotlin-spec/ClassifierDeclarations.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let mut symbols: Vec<_> = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .filter(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::CLASS | SymbolKind::INTERFACE | SymbolKind::OBJECT
            )
        })
        .map(|symbol| (symbol.name, symbol.kind))
        .collect();
    symbols.sort_by(|left, right| left.0.cmp(&right.0));
    symbols
}

#[test]
fn ks_4_1_001_classifier_declarations_introduce_indexed_type_symbols() {
    let symbols = indexed_classifier_symbols(
        "class ScreenSpec\ninterface RenderableSpec\nobject RegistrySpec\n",
    );

    assert_eq!(
        symbols,
        vec![
            ("RegistrySpec".to_string(), SymbolKind::OBJECT),
            ("RenderableSpec".to_string(), SymbolKind::INTERFACE),
            ("ScreenSpec".to_string(), SymbolKind::CLASS),
        ]
    );
}

#[test]
#[ignore = "KS-4.1-002: tree-sitter-kotlin rejects the normative fun interface classifier form"]
fn ks_4_1_002_classifier_declarations_have_class_interface_and_object_forms() {
    assert_source_parses(
        "class ScreenSpec\ninterface RenderableSpec\nfun interface ActionSpec { fun run() }\nobject RegistrySpec\n",
    );
}

#[test]
fn ks_4_1_003_object_literal_is_anonymous_classifier_declaration() {
    let source = "interface RenderableSpec\nval renderer = object : RenderableSpec {}\n";
    assert_source_contains_node_kind(source, "object_literal");

    let symbols = indexed_classifier_symbols(source);
    assert_eq!(
        symbols,
        vec![("RenderableSpec".to_string(), SymbolKind::INTERFACE)]
    );
}

#[test]
fn ks_4_1_1_001_simple_class_combines_name_constructor_supertypes_and_body_members() {
    assert_source_parses(
        "open class BaseSpec\ninterface FirstSpec\ninterface SecondSpec\nclass WidgetSpec(val value: Int) : BaseSpec(), FirstSpec, SecondSpec {\n    constructor() : this(0)\n    init { require(value >= 0) }\n    val label: String = value.toString()\n    fun render(): String = label\n    companion object Named {}\n    class Nested\n}\n",
    );
}

#[test]
fn ks_4_1_1_002_supertype_specifiers_create_indexed_inheritance_edges() {
    let source = "open class BaseSpec\ninterface FirstSpec\ninterface MisleadingSpec\nclass WidgetSpec : BaseSpec(), FirstSpec\n";
    let specification_uri = Url::parse("file:///kotlin-spec/ClassSupertypes.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    for supertype in ["BaseSpec", "FirstSpec"] {
        let locations = indexer.subtypes_of(supertype);
        assert_eq!(locations.len(), 1, "expected one subtype of {supertype}");
        assert_eq!(locations[0].uri, specification_uri);
        assert_eq!(locations[0].range.start.line, 3);
    }
    assert!(indexer.subtypes_of("MisleadingSpec").is_empty());
}

#[test]
#[ignore = "KS-4.1.1-003: kmp-lsp does not diagnose object or inner-class supertypes"]
fn ks_4_1_1_003_object_and_inner_class_cannot_be_supertypes() {
    assert_source_parses(
        "open class BaseSpec\ninterface ContractSpec\nclass ValidSpec : BaseSpec(), ContractSpec\n",
    );
    assert_source_has_syntax_error("object RegistrySpec\nclass InvalidSpec : RegistrySpec()\n");
    assert_source_has_syntax_error(
        "class ContainerSpec { inner class InnerSpec }\nclass InvalidSpec : ContainerSpec.InnerSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.1.1-005: kmp-lsp does not diagnose multiple class inheritance"]
fn ks_4_1_1_005_single_class_and_multiple_interface_inheritance() {
    assert_source_parses(
        "open class BaseSpec\ninterface FirstSpec\ninterface SecondSpec\nclass ValidSpec : BaseSpec(), FirstSpec, SecondSpec\n",
    );
    assert_source_has_syntax_error(
        "open class FirstBaseSpec\nopen class SecondBaseSpec\nclass InvalidSpec : FirstBaseSpec(), SecondBaseSpec()\n",
    );
}

#[test]
fn ks_4_1_1_007_class_body_properties_and_functions_belong_to_class_scope() {
    let source = "fun renderSpec() = Unit\nval labelSpec = \"top-level\"\nclass WidgetSpec {\n    val labelSpec = \"member\"\n    fun renderSpec() = Unit\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/ClassMemberScope.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let symbols = indexer.file_symbols(&specification_uri);
    for member_name in ["labelSpec", "renderSpec"] {
        let mut matching_symbols = symbols.iter().filter(|symbol| symbol.name == member_name);
        assert_eq!(
            matching_symbols
                .next()
                .expect("top-level competitor must be indexed")
                .container,
            None
        );
        assert_eq!(
            matching_symbols
                .next()
                .expect("class member must be indexed")
                .container
                .as_deref(),
            Some("WidgetSpec")
        );
        assert!(matching_symbols.next().is_none());
    }
}

#[test]
fn ks_4_1_1_008_companion_members_resolve_through_class_and_companion_paths() {
    let source = "package specification\nclass WidgetSpec {\n    companion object FactorySpec {\n        fun createSpec() = WidgetSpec()\n    }\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/CompanionPaths.kt")
        .expect("specification fixture URI must be valid");
    let use_uri = Url::parse("file:///kotlin-spec/UseCompanionPaths.kt")
        .expect("specification use-site URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    indexer.index_content(
        &use_uri,
        "package specification\nfun useSpec() { WidgetSpec.createSpec(); WidgetSpec.FactorySpec.createSpec() }\n",
    );

    for qualifier in ["WidgetSpec", "WidgetSpec.FactorySpec"] {
        let locations = resolve_symbol(&indexer, "createSpec", Some(qualifier), &use_uri);
        assert_eq!(
            locations.len(),
            1,
            "expected one definition through {qualifier}"
        );
        assert_eq!(locations[0].range.start.line, 3);
    }
}

#[test]
fn ks_4_1_1_009_unnamed_companion_uses_implicit_companion_name() {
    let source = "class WidgetSpec {\n    companion object {\n        fun createSpec() = WidgetSpec()\n    }\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/ImplicitCompanion.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let symbols = indexer.file_symbols(&specification_uri);
    let companion = symbols
        .iter()
        .find(|symbol| symbol.name == "Companion")
        .expect("unnamed companion must be indexed with its implicit name");
    assert_eq!(companion.kind, SymbolKind::OBJECT);
    assert_eq!(companion.container.as_deref(), Some("WidgetSpec"));

    let companion_member = symbols
        .iter()
        .find(|symbol| symbol.name == "createSpec")
        .expect("companion member must be indexed");
    assert_eq!(companion_member.container.as_deref(), Some("Companion"));
}

#[test]
fn ks_4_1_1_010_nested_classifier_resolves_under_enclosing_class_name() {
    let source = "class MisleadingNestedSpec\nclass WidgetSpec {\n    class NestedSpec\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/NestedClassifier.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let locations =
        indexer.find_definition_qualified("NestedSpec", Some("WidgetSpec"), &specification_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 2);
}

#[test]
fn ks_4_1_1_011_parameterized_class_indexes_its_type_parameter_list() {
    let source = "class BoxSpec<ValueSpec>(val valueSpec: ValueSpec)\n";
    let specification_uri = Url::parse("file:///kotlin-spec/ParameterizedClass.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let box_symbol = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "BoxSpec")
        .expect("parameterized class must be indexed");
    assert_eq!(box_symbol.type_params, vec!["ValueSpec"]);
}

#[test]
fn ks_4_1_1_012_primary_constructor_distinguishes_parameter_and_property_forms() {
    let source =
        "class WidgetSpec(identifierSpec: String, val labelSpec: String, var countSpec: Int)\n";
    let specification_uri = Url::parse("file:///kotlin-spec/PrimaryConstructorParameters.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let symbols = indexer.file_symbols(&specification_uri);
    assert!(symbols.iter().all(|symbol| symbol.name != "identifierSpec"));
    let label = symbols
        .iter()
        .find(|symbol| symbol.name == "labelSpec")
        .expect("read-only property parameter must be indexed");
    assert_eq!(label.kind, SymbolKind::PROPERTY);
    assert_eq!(label.container.as_deref(), Some("WidgetSpec"));
    let count = symbols
        .iter()
        .find(|symbol| symbol.name == "countSpec")
        .expect("mutable property parameter must be indexed");
    assert_eq!(count.kind, SymbolKind::VARIABLE);
    assert_eq!(count.container.as_deref(), Some("WidgetSpec"));
}

#[test]
#[ignore = "KS-4.1.1-015: kmp-lsp does not validate superclass constructor invocation"]
fn ks_4_1_1_015_class_supertype_specifier_requires_valid_constructor_invocation() {
    assert_source_parses("open class BaseSpec(valueSpec: Int)\nclass ValidSpec : BaseSpec(1)\n");
    assert_source_has_syntax_error(
        "open class BaseSpec(valueSpec: Int)\nclass InvalidSpec : BaseSpec\n",
    );
}

#[test]
fn ks_4_1_1_016_secondary_constructor_supports_this_and_super_delegation_forms() {
    assert_source_parses(
        "open class BaseSpec(valueSpec: Int)\nclass PrimarySpec(valueSpec: Int) : BaseSpec(valueSpec) {\n    constructor() : this(0)\n}\nclass SecondarySpec : BaseSpec {\n    constructor(valueSpec: Int) : super(valueSpec)\n    constructor() : this(0)\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.1-017: kmp-lsp does not validate secondary delegation when a primary constructor exists"]
fn ks_4_1_1_017_secondary_constructor_with_primary_delegates_to_this() {
    assert_source_parses(
        "open class BaseSpec\nclass ValidSpec(valueSpec: Int) : BaseSpec() {\n    constructor() : this(0)\n}\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec\nclass InvalidSpec(valueSpec: Int) : BaseSpec() {\n    constructor() : super()\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.1-018: kmp-lsp does not require secondary constructor delegation to a non-Any superclass"]
fn ks_4_1_1_018_secondary_constructor_without_primary_delegates_to_super_or_this() {
    assert_source_parses(
        "open class BaseSpec(valueSpec: Int)\nclass ValidSpec : BaseSpec {\n    constructor(valueSpec: Int) : super(valueSpec)\n    constructor() : this(0)\n}\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec(valueSpec: Int)\nclass InvalidSpec : BaseSpec {\n    constructor(valueSpec: Int) {}\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.1-019: kmp-lsp does not detect secondary constructor delegation cycles"]
fn ks_4_1_1_019_secondary_constructor_delegation_cannot_form_loop() {
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    constructor(valueSpec: Int) : this(valueSpec.toString())\n    constructor(valueSpec: String) : this(valueSpec.length)\n}\n",
    );
}

#[test]
fn ks_4_1_1_020_constructors_accept_varargs_and_default_parameter_values() {
    assert_source_parses(
        "class WidgetSpec(val labelSpec: String = \"default\", vararg val valuesSpec: Int) {\n    constructor(vararg valuesSpec: Int) : this(valuesSpec = valuesSpec)\n}\n",
    );
}

#[tokio::test]
#[ignore = "KS-4.1.1-022: kmp-lsp does not resolve plain constructor parameters through constructor scopes"]
async fn ks_4_1_1_022_constructor_parameters_resolve_in_their_linked_scopes() {
    let source = "val valueSpec = 99\nval textSpec = \"misleading\"\nclass WidgetSpec(valueSpec: Int) {\n    val copiedSpec = valueSpec\n    constructor(textSpec: String) : this(textSpec.length) {\n        println(textSpec)\n    }\n}\n";

    let primary_locations = definition_locations(source, "valueSpec", 2).await;
    assert_eq!(primary_locations.len(), 1);
    assert_eq!(primary_locations[0].range.start, Position::new(2, 17));

    for occurrence in [2, 3] {
        let secondary_locations = definition_locations(source, "textSpec", occurrence).await;
        assert_eq!(secondary_locations.len(), 1);
        assert_eq!(secondary_locations[0].range.start, Position::new(4, 16));
    }
}

#[test]
#[ignore = "KS-4.1.1-024: kmp-lsp does not diagnose inner classes declared in interfaces"]
fn ks_4_1_1_024_inner_class_cannot_be_declared_in_interface() {
    assert_source_parses("class ContainerSpec {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("interface ContractSpec {\n    inner class InnerSpec {}\n}\n");
}

#[test]
#[ignore = "KS-4.1.1-025: kmp-lsp does not diagnose inner classes declared in statement scopes"]
fn ks_4_1_1_025_inner_class_cannot_be_declared_in_statement_scope() {
    assert_source_parses("class ContainerSpec {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("fun createSpec() {\n    inner class InnerSpec {}\n}\n");
}

#[test]
#[ignore = "KS-4.1.1-026: kmp-lsp does not diagnose inner classes declared in objects"]
fn ks_4_1_1_026_inner_class_cannot_be_declared_in_object() {
    assert_source_parses("class ContainerSpec {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("object RegistrySpec {\n    inner class InnerSpec {}\n}\n");
}

#[test]
#[ignore = "KS-4.1.1-027: kmp-lsp does not diagnose non-inner classifiers declared in object literals"]
fn ks_4_1_1_027_object_literal_allows_only_inner_classifiers() {
    assert_source_parses("val validSpec = object {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("val invalidClassSpec = object {\n    class NestedSpec {}\n}\n");
    assert_source_has_syntax_error(
        "val invalidInterfaceSpec = object {\n    interface NestedSpec {}\n}\n",
    );
}

#[test]
fn ks_4_1_1_028_interface_inheritance_accepts_delegation_and_indexes_edge() {
    let source = "interface ContractSpec {\n    fun renderSpec(): String\n}\nclass WidgetSpec(delegateSpec: ContractSpec) : ContractSpec by delegateSpec\n";
    assert_source_parses(source);

    let specification_uri = Url::parse("file:///kotlin-spec/InheritanceDelegation.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let locations = indexer.subtypes_of("ContractSpec");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, specification_uri);
    assert_eq!(locations[0].range.start.line, 3);
}

#[test]
#[ignore = "KS-4.1.1-029: kmp-lsp does not diagnose inheritance delegation to a class supertype"]
fn ks_4_1_1_029_only_interface_inheritance_can_be_delegated() {
    assert_source_parses(
        "interface ContractSpec\nclass ValidSpec(delegateSpec: ContractSpec) : ContractSpec by delegateSpec\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec\nclass InvalidSpec(delegateSpec: BaseSpec) : BaseSpec by delegateSpec\n",
    );
}

#[test]
#[ignore = "KS-4.1.1-030: kmp-lsp does not validate the inheritance delegate value type"]
fn ks_4_1_1_030_inheritance_delegate_value_must_be_interface_subtype() {
    assert_source_parses(
        "interface ContractSpec\nclass DelegateSpec : ContractSpec\nclass ValidSpec(delegateSpec: DelegateSpec) : ContractSpec by delegateSpec\n",
    );
    assert_source_has_syntax_error(
        "interface ContractSpec\nclass UnrelatedSpec\nclass InvalidSpec(delegateSpec: UnrelatedSpec) : ContractSpec by delegateSpec\n",
    );
}

#[test]
#[ignore = "KS-4.1.1-033: kmp-lsp does not diagnose class-member access from a delegation expression"]
fn ks_4_1_1_033_delegation_expression_cannot_access_class_members() {
    assert_source_parses(
        "interface ContractSpec\nclass ValidSpec(delegateSpec: ContractSpec) : ContractSpec by delegateSpec\n",
    );
    assert_source_has_syntax_error(
        "interface ContractSpec\ninterface MarkerSpec\nclass InvalidSpec : ContractSpec by delegateSpec, MarkerSpec {\n    val delegateSpec: ContractSpec = object : ContractSpec {}\n}\n",
    );
}

#[test]
fn ks_4_1_1_035_abstract_class_is_indexed_as_class() {
    let source = "abstract class BaseSpec\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert_eq!(symbols, vec![("BaseSpec".to_string(), SymbolKind::CLASS)]);
}

#[test]
#[ignore = "KS-4.1.1-036: kmp-lsp does not diagnose direct abstract-class construction"]
fn ks_4_1_1_036_abstract_class_cannot_be_instantiated_directly() {
    assert_source_parses("abstract class BaseSpec\nclass ConcreteSpec : BaseSpec()\n");
    assert_source_has_syntax_error("abstract class BaseSpec\nval invalidSpec = BaseSpec()\n");
}

#[test]
fn ks_4_1_1_037_abstract_class_accepts_abstract_members() {
    let source = "abstract class BaseSpec {\n    abstract val labelSpec: String\n    abstract fun renderSpec(): String\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/AbstractMembers.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    for member_name in ["labelSpec", "renderSpec"] {
        let member = symbols
            .iter()
            .find(|symbol| symbol.name == member_name)
            .expect("abstract member must be indexed");
        assert_eq!(member.container.as_deref(), Some("BaseSpec"));
        assert!(member.detail.contains("abstract"));
    }
}

#[test]
#[ignore = "KS-4.1.1-038: kmp-lsp does not diagnose missing abstract-member implementations"]
fn ks_4_1_1_038_concrete_subtype_implements_abstract_members() {
    assert_source_parses(
        "abstract class BaseSpec {\n    abstract fun renderSpec(): String\n}\nclass ValidSpec : BaseSpec() {\n    override fun renderSpec() = \"valid\"\n}\n",
    );
    assert_source_has_syntax_error(
        "abstract class BaseSpec {\n    abstract fun renderSpec(): String\n}\nclass InvalidSpec : BaseSpec()\n",
    );
}

#[test]
fn ks_4_1_2_001_data_class_indexes_product_type_and_data_properties() {
    let source = "data class RowSpec(val labelSpec: String, var countSpec: Int)\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataClassProduct.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    let data_class = symbols
        .iter()
        .find(|symbol| symbol.name == "RowSpec")
        .expect("data class must be indexed");
    assert_eq!(data_class.kind, SymbolKind::STRUCT);
    for property_name in ["labelSpec", "countSpec"] {
        let property = symbols
            .iter()
            .find(|symbol| symbol.name == property_name)
            .expect("data property must be indexed");
        assert_eq!(property.container.as_deref(), Some("RowSpec"));
    }
}

#[test]
#[ignore = "KS-4.1.2-002: kmp-lsp does not diagnose non-property data-class parameters"]
fn ks_4_1_2_002_data_class_primary_parameters_must_be_properties() {
    assert_source_parses("data class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("data class InvalidSpec(valueSpec: Int)\n");
}

#[test]
fn ks_4_1_2_007_generated_copy_matches_data_property_names_and_types() {
    let source = "data class RowSpec(val labelSpec: String, var countSpec: Int) {\n    val transientSpec: Boolean = false\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataClassCopy.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let copy = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "copy" && symbol.container.as_deref() == Some("RowSpec"))
        .expect("data class copy must be synthesized in the source index");
    assert_eq!(copy.params, "labelSpec: String, countSpec: Int");
    assert_eq!(copy.param_counts.1, 2);
    assert!(!copy.params.contains("transientSpec"));
    assert!(copy.detail.ends_with("): RowSpec"));
}

#[test]
fn ks_4_1_2_009_generated_copy_parameters_default_to_current_properties() {
    let source = "data class RowSpec(val labelSpec: String, val countSpec: Int)\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataClassCopyDefaults.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let copy = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "copy" && symbol.container.as_deref() == Some("RowSpec"))
        .expect("data class copy must be synthesized in the source index");
    assert_eq!(copy.param_counts, (0, 2));
}

#[test]
#[ignore = "KS-4.1.2-010: kmp-lsp does not synthesize typed data-class component functions"]
fn ks_4_1_2_010_generated_component_has_property_type_and_value_position() {
    let source = "data class RowSpec(val labelSpec: String, val countSpec: Int)\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataClassComponents.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    let first_component = symbols
        .iter()
        .find(|symbol| symbol.name == "component1")
        .expect("component1 must be synthesized");
    assert!(first_component.detail.ends_with(": String"));
    let second_component = symbols
        .iter()
        .find(|symbol| symbol.name == "component2")
        .expect("component2 must be synthesized");
    assert!(second_component.detail.ends_with(": Int"));
}

#[test]
#[ignore = "KS-4.1.2-011: kmp-lsp does not synthesize operator data-class component functions"]
fn ks_4_1_2_011_generated_component_is_operator_function() {
    let source = "data class RowSpec(val labelSpec: String)\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataClassComponentOperator.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let component = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "component1")
        .expect("component1 must be synthesized");
    assert_eq!(component.kind, SymbolKind::OPERATOR);
    assert!(component.detail.starts_with("operator fun component1"));
}

#[test]
#[ignore = "KS-4.1.2-012: kmp-lsp does not synthesize data-class component functions"]
fn ks_4_1_2_012_generated_component_count_matches_data_property_count() {
    let source = "data class RowSpec(val labelSpec: String, val countSpec: Int) {\n    val transientSpec = false\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataClassComponentCount.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let component_names: Vec<_> = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .filter(|symbol| symbol.name.starts_with("component"))
        .map(|symbol| symbol.name)
        .collect();
    assert_eq!(component_names, vec!["component1", "component2"]);
}

#[test]
#[ignore = "KS-4.1.2-013: kmp-lsp does not synthesize component functions needed to expose data-property-only generation"]
fn ks_4_1_2_013_only_constructor_data_properties_participate_in_generated_api() {
    let source = "data class RowSpec(val valueSpec: Int) {\n    val transientSpec: String = \"ignored\"\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataClassDataProperties.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    let copy = symbols
        .iter()
        .find(|symbol| symbol.name == "copy")
        .expect("copy must be synthesized");
    assert_eq!(copy.params, "valueSpec: Int");
    let component_names: Vec<_> = symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with("component"))
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert_eq!(component_names, vec!["component1"]);
}

#[test]
fn ks_4_1_2_015_equals_hashcode_and_tostring_may_be_explicit() {
    let source = "data class RowSpec(val valueSpec: Int) {\n    override fun equals(otherSpec: Any?): Boolean = otherSpec is RowSpec && otherSpec.valueSpec == valueSpec\n    override fun hashCode(): Int = valueSpec\n    override fun toString(): String = \"RowSpec\"\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ExplicitDataFunctions.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    for function_name in ["equals", "hashCode", "toString"] {
        let function = symbols
            .iter()
            .find(|symbol| symbol.name == function_name)
            .expect("explicit data function must be indexed");
        assert_eq!(function.container.as_deref(), Some("RowSpec"));
        assert!(function.detail.contains("override"));
    }
}

#[test]
#[ignore = "KS-4.1.2-017: kmp-lsp does not diagnose explicit data-class copy or component functions"]
fn ks_4_1_2_017_copy_and_component_functions_cannot_be_explicit() {
    assert_source_parses(
        "data class ValidSpec(val valueSpec: Int) {\n    fun helperSpec() = valueSpec\n}\n",
    );
    assert_source_has_syntax_error(
        "data class InvalidCopySpec(val valueSpec: Int) {\n    fun copy(valueSpec: Int = this.valueSpec): InvalidCopySpec = InvalidCopySpec(valueSpec)\n}\n",
    );
    assert_source_has_syntax_error(
        "data class InvalidComponentSpec(val valueSpec: Int) {\n    operator fun component1(): Int = valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.2-022: kmp-lsp does not diagnose inheritance from a data class"]
fn ks_4_1_2_022_data_class_is_closed_to_inheritance() {
    assert_source_parses("data class LeafSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "data class BaseSpec(val valueSpec: Int)\nclass InvalidSpec(valueSpec: Int) : BaseSpec(valueSpec)\n",
    );
}

#[test]
#[ignore = "KS-4.1.2-023: kmp-lsp does not diagnose a data class without a primary constructor"]
fn ks_4_1_2_023_data_class_requires_primary_constructor() {
    assert_source_parses("data class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("data class InvalidSpec {\n    val valueSpec: Int = 0\n}\n");
}

#[test]
#[ignore = "KS-4.1.2-024: kmp-lsp does not diagnose an empty data-class primary constructor"]
fn ks_4_1_2_024_data_class_requires_at_least_one_data_property() {
    assert_source_parses("data class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("data class InvalidSpec()\n");
}

#[test]
#[ignore = "KS-4.1.2-025: kmp-lsp does not diagnose vararg data properties"]
fn ks_4_1_2_025_data_property_cannot_be_vararg() {
    assert_source_parses("data class ValidSpec(val valuesSpec: IntArray)\n");
    assert_source_has_syntax_error("data class InvalidSpec(vararg val valuesSpec: Int)\n");
}

#[test]
fn ks_4_1_2_026_data_object_indexes_zero_property_unit_type() {
    let source = "data object EmptySpec\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert_eq!(symbols, vec![("EmptySpec".to_string(), SymbolKind::OBJECT)]);
}

#[test]
fn ks_4_1_2_030_data_object_generates_no_copy_or_component_functions() {
    let source = "data object EmptySpec\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DataObjectGeneratedApi.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    assert!(symbols.iter().all(|symbol| symbol.name != "copy"));
    assert!(symbols
        .iter()
        .all(|symbol| !symbol.name.starts_with("component")));
}

#[test]
fn ks_4_1_2_031_data_object_tostring_may_be_explicit() {
    let source =
        "data object EmptySpec {\n    override fun toString(): String = \"EmptySpec\"\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/DataObjectToString.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let to_string = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "toString")
        .expect("explicit data-object toString must be indexed");
    assert_eq!(to_string.container.as_deref(), Some("EmptySpec"));
    assert!(to_string.detail.contains("override"));
}

#[test]
#[ignore = "KS-4.1.2-032: kmp-lsp does not diagnose explicit data-object equals or hashCode"]
fn ks_4_1_2_032_data_object_equals_and_hashcode_cannot_be_explicit() {
    assert_source_parses(
        "data object ValidSpec {\n    override fun toString(): String = \"ValidSpec\"\n}\n",
    );
    assert_source_has_syntax_error(
        "data object InvalidEqualsSpec {\n    override fun equals(otherSpec: Any?): Boolean = this === otherSpec\n}\n",
    );
    assert_source_has_syntax_error(
        "data object InvalidHashSpec {\n    override fun hashCode(): Int = 0\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.2-032: kmp-lsp does not diagnose inherited data-object equals or hashCode"]
fn ks_4_1_2_032_data_object_equals_and_hashcode_cannot_be_inherited() {
    assert_source_has_syntax_error(
        "open class IdentityBaseSpec {\n    final override fun equals(otherSpec: Any?): Boolean = this === otherSpec\n    final override fun hashCode(): Int = 0\n}\ndata object InvalidSpec : IdentityBaseSpec()\n",
    );
}

#[test]
fn ks_4_1_2_033_data_object_obeys_regular_object_shape_restrictions() {
    assert_source_parses("data object ValidSpec\n");
    assert_source_has_syntax_error("data object GenericSpec<ValueSpec>\n");
    assert_source_has_syntax_error("data object ConstructedSpec()\n");
}

#[test]
#[ignore = "KS-4.1.2-034: kmp-lsp does not diagnose a data companion object"]
fn ks_4_1_2_034_companion_object_cannot_be_data_object() {
    assert_source_parses("class HostSpec {\n    companion object RegistrySpec\n}\n");
    assert_source_has_syntax_error("class HostSpec {\n    data companion object RegistrySpec\n}\n");
}

#[test]
#[ignore = "KS-4.1.2-035: kmp-lsp does not diagnose the data object-literal form"]
fn ks_4_1_2_035_object_literal_cannot_be_data_object() {
    assert_source_parses("val validSpec = object {}\n");
    assert_source_has_syntax_error("val invalidSpec = data object {}\n");
}

#[test]
fn ks_4_1_3_001_enum_class_indexes_predefined_entry_values() {
    let source = "enum class StateSpec {\n    READY,\n    STOPPED\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/EnumEntries.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    let enum_class = symbols
        .iter()
        .find(|symbol| symbol.name == "StateSpec")
        .expect("enum class must be indexed");
    assert_eq!(enum_class.kind, SymbolKind::ENUM);
    for entry_name in ["READY", "STOPPED"] {
        let entry = symbols
            .iter()
            .find(|symbol| symbol.name == entry_name)
            .expect("enum entry must be indexed");
        assert_eq!(entry.kind, SymbolKind::ENUM_MEMBER);
        assert_eq!(entry.container.as_deref(), Some("StateSpec"));
    }
}

#[test]
#[ignore = "KS-4.1.3-002: kmp-lsp does not diagnose direct enum-class construction"]
fn ks_4_1_3_002_enum_values_cannot_be_constructed_outside_entries() {
    assert_source_parses("enum class StateSpec { READY }\nval validSpec = StateSpec.READY\n");
    assert_source_has_syntax_error(
        "enum class StateSpec { READY }\nval invalidSpec = StateSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.1.3-004: kmp-lsp does not diagnose an enum with an explicit base class"]
fn ks_4_1_3_004_enum_class_cannot_have_another_base_class() {
    assert_source_parses("interface ContractSpec\nenum class ValidSpec : ContractSpec { READY }\n");
    assert_source_has_syntax_error(
        "open class BaseSpec\nenum class InvalidSpec : BaseSpec() { READY }\n",
    );
}

#[test]
#[ignore = "KS-4.1.3-005: kmp-lsp does not diagnose inheritance from an enum class"]
fn ks_4_1_3_005_enum_class_is_final_and_cannot_be_inherited() {
    assert_source_parses("enum class LeafSpec { READY }\n");
    assert_source_has_syntax_error(
        "enum class BaseSpec { READY }\nclass InvalidSpec : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.1.3-006: kmp-lsp does not diagnose enum-class type parameters"]
fn ks_4_1_3_006_enum_class_cannot_have_type_parameters() {
    assert_source_parses("enum class ValidSpec { READY }\n");
    assert_source_has_syntax_error("enum class InvalidSpec<ValueSpec> { READY }\n");
}

#[test]
fn ks_4_1_3_007_enum_entry_resolves_as_static_member_callable() {
    let declaration_uri = Url::parse("file:///kotlin-spec/EnumDeclaration.kt")
        .expect("specification fixture URI must be valid");
    let use_uri = Url::parse("file:///kotlin-spec/EnumUse.kt")
        .expect("specification use-site URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package specification\nenum class StateSpec {\n    READY,\n    STOPPED\n}\n",
    );
    indexer.index_content(
        &use_uri,
        "package specification\nval selectedSpec = StateSpec.READY\n",
    );

    let locations = resolve_symbol(&indexer, "READY", Some("StateSpec"), &use_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, declaration_uri);
    assert_eq!(locations[0].range.start.line, 2);
}

#[test]
#[ignore = "KS-4.1.3-008: kmp-lsp assigns enum-entry body members to the enum class container"]
fn ks_4_1_3_008_enum_entry_body_accepts_entry_specific_declarations() {
    let source = "enum class DirectionSpec {\n    UP {\n        override fun labelSpec(): String = \"up\"\n    },\n    DOWN;\n    open fun labelSpec(): String = \"down\"\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/EnumEntryBody.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    let override_function = symbols
        .iter()
        .find(|symbol| symbol.name == "labelSpec" && symbol.range.start.line == 2)
        .expect("entry-specific override must be indexed");
    assert_eq!(override_function.container.as_deref(), Some("UP"));
}

#[test]
fn ks_4_1_3_009_enum_class_may_have_zero_entries() {
    let source = "enum class EmptySpec {}\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert!(
        symbols.is_empty(),
        "enum is not a class/interface/object symbol"
    );
    let specification_uri = Url::parse("file:///kotlin-spec/EmptyEnum.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let enum_symbol = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "EmptySpec")
        .expect("zero-entry enum must be indexed");
    assert_eq!(enum_symbol.kind, SymbolKind::ENUM);
}

#[test]
fn ks_4_1_3_010_enum_entry_name_has_string_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/EnumName.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "enum class StateSpec { READY, STOPPED }\n",
    );
    assert_eq!(
        indexer.find_field_type("StateSpec", "name").as_deref(),
        Some("String")
    );
}

#[test]
fn ks_4_1_3_012_enum_entry_ordinal_has_int_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/EnumOrdinal.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "enum class StateSpec { READY, STOPPED }\n",
    );
    assert_eq!(
        indexer.find_field_type("StateSpec", "ordinal").as_deref(),
        Some("Int")
    );
}

#[test]
#[ignore = "KS-4.1.3-015: kmp-lsp assigns entry-specific compareTo to the enum class container"]
fn ks_4_1_3_015_compareto_may_be_overridden_in_enum_and_entry() {
    let source = "enum class RankSpec {\n    HIGH {\n        override fun compareTo(otherSpec: RankSpec): Int = 1\n    },\n    LOW;\n    override fun compareTo(otherSpec: RankSpec): Int = 0\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/EnumCompareTo.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    let entry_override = symbols
        .iter()
        .find(|symbol| symbol.name == "compareTo" && symbol.range.start.line == 2)
        .expect("entry compareTo override must be indexed");
    assert_eq!(entry_override.container.as_deref(), Some("HIGH"));
    let class_override = symbols
        .iter()
        .find(|symbol| symbol.name == "compareTo" && symbol.range.start.line == 5)
        .expect("enum compareTo override must be indexed");
    assert_eq!(class_override.container.as_deref(), Some("RankSpec"));
}

#[test]
#[ignore = "KS-4.1.3-017: kmp-lsp assigns entry-specific toString to the enum class container"]
fn ks_4_1_3_017_tostring_may_be_overridden_in_enum_and_entry() {
    let source = "enum class StateSpec {\n    READY {\n        override fun toString(): String = \"ready\"\n    },\n    STOPPED;\n    override fun toString(): String = name\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/EnumToString.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    let entry_override = symbols
        .iter()
        .find(|symbol| symbol.name == "toString" && symbol.range.start.line == 2)
        .expect("entry toString override must be indexed");
    assert_eq!(entry_override.container.as_deref(), Some("READY"));
    let class_override = symbols
        .iter()
        .find(|symbol| symbol.name == "toString" && symbol.range.start.line == 5)
        .expect("enum toString override must be indexed");
    assert_eq!(class_override.container.as_deref(), Some("StateSpec"));
}

#[test]
fn ks_4_1_3_018_enum_entries_property_has_bounded_list_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/EnumEntriesProperty.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "enum class StateSpec { READY, STOPPED }\nclass MisleadingSpec { val entries: String = \"wrong\" }\n",
    );
    assert_eq!(
        indexer.find_field_type("StateSpec", "entries").as_deref(),
        Some("List<StateSpec>")
    );
    assert_ne!(
        indexer
            .find_field_type("MisleadingSpec", "entries")
            .as_deref(),
        Some("List<MisleadingSpec>")
    );
}

#[test]
fn ks_4_1_3_020_enum_valueof_returns_enum_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/EnumValueOf.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, "enum class StateSpec { READY }\n");
    assert_eq!(
        indexer
            .find_method_return_type_for_type("StateSpec", "valueOf")
            .as_deref(),
        Some("StateSpec")
    );
}

#[test]
fn ks_4_1_3_023_enum_values_returns_array_of_enum_type() {
    let specification_uri = Url::parse("file:///kotlin-spec/EnumValues.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, "enum class StateSpec { READY }\n");
    assert_eq!(
        indexer
            .find_method_return_type_for_type("StateSpec", "values")
            .as_deref(),
        Some("Array<StateSpec>")
    );
}

#[test]
fn ks_4_1_4_001_annotation_class_introduces_indexed_classifier() {
    let source = "annotation class RouteSpec(val pathSpec: String)\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert_eq!(symbols, vec![("RouteSpec".to_string(), SymbolKind::CLASS)]);
}

#[test]
#[ignore = "KS-4.1.4-002: kmp-lsp does not diagnose annotation secondary constructors"]
fn ks_4_1_4_002_annotation_class_cannot_have_secondary_constructors() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    constructor() : this(0)\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-003: kmp-lsp does not diagnose non-property annotation parameters"]
fn ks_4_1_4_003_annotation_constructor_parameters_require_property_syntax() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("annotation class InvalidSpec(valueSpec: Int)\n");
}

#[test]
fn ks_4_1_4_004_annotation_constructor_properties_are_indexed() {
    let source = "annotation class RouteSpec(val pathSpec: String, val prioritySpec: Int = 0)\n";
    let specification_uri = Url::parse("file:///kotlin-spec/AnnotationProperties.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    for property_name in ["pathSpec", "prioritySpec"] {
        let property = symbols
            .iter()
            .find(|symbol| symbol.name == property_name)
            .expect("annotation constructor property must be indexed");
        assert_eq!(property.kind, SymbolKind::PROPERTY);
        assert_eq!(property.container.as_deref(), Some("RouteSpec"));
    }
}

#[test]
#[ignore = "KS-4.1.4-006: kmp-lsp does not diagnose additional annotation interfaces"]
fn ks_4_1_4_006_annotation_class_cannot_implement_additional_interfaces() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error(
        "interface ContractSpec\nannotation class InvalidSpec : ContractSpec\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-007: kmp-lsp does not diagnose annotation base classes"]
fn ks_4_1_4_007_annotation_class_cannot_specify_a_base_class() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error(
        "open class BaseSpec\nannotation class InvalidSpec : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-008: kmp-lsp does not diagnose inheritance from annotations"]
fn ks_4_1_4_008_annotation_class_is_closed_to_inheritance() {
    assert_source_parses("annotation class LeafSpec\n");
    assert_source_has_syntax_error("annotation class BaseSpec\nclass InvalidSpec : BaseSpec()\n");
}

#[test]
#[ignore = "KS-4.1.4-009: kmp-lsp does not diagnose annotation member functions"]
fn ks_4_1_4_009_annotation_class_cannot_declare_member_functions() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    fun helperSpec(): Int = valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-010: kmp-lsp does not diagnose extra annotation properties"]
fn ks_4_1_4_010_annotation_class_cannot_declare_extra_properties() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    val extraSpec: Int = valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-011: kmp-lsp does not diagnose annotation overrides"]
fn ks_4_1_4_011_annotation_class_cannot_declare_overrides() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    override fun toString(): String = valueSpec.toString()\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-012: kmp-lsp does not diagnose annotation companion objects"]
fn ks_4_1_4_012_annotation_class_cannot_have_companion_object() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec {\n    companion object RegistrySpec\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-013: kmp-lsp does not diagnose nested annotation classes"]
fn ks_4_1_4_013_annotation_class_cannot_have_nested_class() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error("annotation class InvalidSpec {\n    class NestedSpec\n}\n");
}

#[test]
fn ks_4_1_4_014_annotation_parameters_accept_allowed_scalar_types() {
    assert_source_parses(
        "import kotlin.reflect.KClass\nannotation class ScalarSpec(\n    val textSpec: String,\n    val classSpec: KClass<*>,\n    val byteSpec: Byte,\n    val shortSpec: Short,\n    val intSpec: Int,\n    val longSpec: Long,\n    val floatSpec: Float,\n    val doubleSpec: Double,\n    val charSpec: Char,\n    val booleanSpec: Boolean,\n)\n",
    );
}

#[test]
fn ks_4_1_4_015_annotation_parameters_accept_annotations_and_arrays() {
    assert_source_parses(
        "annotation class NestedSpec(val valueSpec: Int)\nannotation class CompositeSpec(\n    val nestedSpec: NestedSpec,\n    val nestedArraySpec: Array<NestedSpec>,\n    val stringArraySpec: Array<String>,\n    val numberArraySpec: IntArray,\n)\n",
    );
}

#[test]
#[ignore = "KS-4.1.4-016: kmp-lsp does not diagnose cyclic annotation types"]
fn ks_4_1_4_016_annotation_types_cannot_reference_themselves_cyclically() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: String)\n");
    assert_source_has_syntax_error("annotation class DirectSpec(val valueSpec: DirectSpec)\n");
    assert_source_has_syntax_error(
        "annotation class FirstSpec(val secondSpec: SecondSpec)\nannotation class SecondSpec(val firstSpec: Array<FirstSpec>)\n",
    );
}

#[test]
fn ks_4_1_4_017_annotation_class_may_declare_type_parameters() {
    assert_source_parses("annotation class MarkerSpec<ElementSpec>\n");
}

#[test]
#[ignore = "KS-4.1.4-018: kmp-lsp does not diagnose annotation type-parameter properties"]
fn ks_4_1_4_018_annotation_constructor_cannot_use_its_type_parameter() {
    assert_source_parses("annotation class ValidSpec<ElementSpec>(val valueSpec: String)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec<ElementSpec>(val valueSpec: ElementSpec)\n",
    );
}

#[tokio::test]
async fn ks_4_1_4_019_annotation_class_can_be_instantiated_directly() {
    let source =
        "annotation class RouteSpec(val pathSpec: String)\nval routeSpec = RouteSpec(\"home\")\n";
    assert_source_parses(source);
    let locations = definition_locations(source, "RouteSpec", 1).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 0);
}

#[test]
fn ks_4_1_4_020_annotation_class_may_have_no_parameters() {
    let source = "annotation class MarkerSpec\n@MarkerSpec class ScreenSpec\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert_eq!(
        symbols,
        vec![
            ("MarkerSpec".to_string(), SymbolKind::CLASS),
            ("ScreenSpec".to_string(), SymbolKind::CLASS),
        ]
    );
}

#[test]
fn ks_4_1_4_021_annotation_constructor_supports_vararg_properties() {
    let source = "import kotlin.reflect.KClass\nannotation class TypesSpec(vararg val classesSpec: KClass<out Annotation>)\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/AnnotationVararg.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let property = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "classesSpec")
        .expect("vararg annotation property must be indexed");
    assert_eq!(property.kind, SymbolKind::PROPERTY);
    assert_eq!(property.container.as_deref(), Some("TypesSpec"));
}

#[test]
fn ks_4_1_5_001_value_class_accepts_value_and_inline_declaration_modifiers() {
    let source = "value class IdentifierSpec(val valueSpec: String)\ninline class LegacyIdentifierSpec(val valueSpec: String)\n";
    assert_source_parses(source);
    assert_eq!(
        indexed_classifier_symbols(source),
        vec![
            ("IdentifierSpec".to_string(), SymbolKind::CLASS),
            ("LegacyIdentifierSpec".to_string(), SymbolKind::CLASS),
        ]
    );
}

#[test]
#[ignore = "KS-4.1.5-002: kmp-lsp does not diagnose inheritance from value classes"]
fn ks_4_1_5_002_value_class_is_closed_to_inheritance() {
    assert_source_parses("value class LeafSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "value class BaseSpec(val valueSpec: Int)\nclass InvalidSpec(valueSpec: Int) : BaseSpec(valueSpec)\n",
    );
}

#[test]
#[ignore = "KS-4.1.5-003: kmp-lsp does not diagnose incompatible value-class modifiers"]
fn ks_4_1_5_003_value_class_rejects_inner_data_and_enum_forms() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "class HostSpec { inner value class InvalidSpec(val valueSpec: Int) }\n",
    );
    assert_source_has_syntax_error("data value class InvalidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("value enum class InvalidSpec { ENTRY_SPEC }\n");
}

#[test]
#[ignore = "KS-4.1.5-004: kmp-lsp does not validate the value-class primary constructor"]
fn ks_4_1_5_004_value_class_requires_one_constructor_property() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("value class MissingConstructorSpec\n");
    assert_source_has_syntax_error("value class EmptyConstructorSpec()\n");
    assert_source_has_syntax_error("value class BareParameterSpec(valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "value class MultiplePropertiesSpec(val firstSpec: Int, val secondSpec: Int)\n",
    );
}

#[test]
#[ignore = "KS-4.1.5-005: kmp-lsp does not diagnose vararg value-class data properties"]
fn ks_4_1_5_005_value_class_data_property_cannot_be_vararg() {
    assert_source_parses("value class ValidSpec(val valuesSpec: IntArray)\n");
    assert_source_has_syntax_error("value class InvalidSpec(vararg val valuesSpec: Int)\n");
}

#[test]
#[ignore = "KS-4.1.5-006: kmp-lsp does not diagnose non-public value-class data properties"]
fn ks_4_1_5_006_value_class_data_property_must_be_public() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("value class InvalidSpec(private val valueSpec: Int)\n");
}

#[test]
#[ignore = "KS-4.1.5-007: kmp-lsp does not diagnose value-class equals or hashCode overrides"]
fn ks_4_1_5_007_value_class_cannot_override_equals_or_hashcode() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "value class InvalidEqualsSpec(val valueSpec: Int) {\n    override fun equals(otherSpec: Any?): Boolean = false\n}\n",
    );
    assert_source_has_syntax_error(
        "value class InvalidHashSpec(val valueSpec: Int) {\n    override fun hashCode(): Int = valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-4.1.5-008: kmp-lsp does not diagnose value-class base classes"]
fn ks_4_1_5_008_value_class_cannot_have_a_base_class_besides_any() {
    assert_source_parses(
        "interface ContractSpec\nvalue class ValidSpec(val valueSpec: Int) : ContractSpec\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec\nvalue class InvalidSpec(val valueSpec: Int) : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-4.1.5-009: kmp-lsp does not diagnose value-class backing fields"]
fn ks_4_1_5_009_other_value_class_properties_cannot_have_backing_fields() {
    assert_source_parses(
        "value class ValidSpec(val valueSpec: Int) {\n    val doubledSpec: Int get() = valueSpec * 2\n}\n",
    );
    assert_source_has_syntax_error(
        "value class InvalidSpec(val valueSpec: Int) {\n    val storedSpec: Int = valueSpec * 2\n}\n",
    );
}

#[test]
fn ks_4_1_5_010_value_class_accepts_computed_properties_without_backing_fields() {
    let source = "value class IdentifierSpec(val valueSpec: String) {\n    val lengthSpec: Int get() = valueSpec.length\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ValueComputedProperty.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let property = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "lengthSpec")
        .expect("computed value-class property must be indexed");
    assert_eq!(property.kind, SymbolKind::PROPERTY);
    assert_eq!(property.container.as_deref(), Some("IdentifierSpec"));
}

#[test]
fn ks_4_1_5_011_inline_modifier_preserves_legacy_value_class_syntax() {
    assert_source_parses("inline class LegacyIdentifierSpec(val valueSpec: String)\n");
}

#[test]
fn ks_4_1_5_015_value_class_may_override_tostring_explicitly() {
    let source = "value class IdentifierSpec(val valueSpec: String) {\n    override fun toString(): String = \"id:\" + valueSpec\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ValueToString.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let to_string = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "toString")
        .expect("explicit value-class toString must be indexed");
    assert_eq!(to_string.container.as_deref(), Some("IdentifierSpec"));
    assert!(to_string.detail.contains("override"));
}
