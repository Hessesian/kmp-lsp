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
fn ks_declarations_0001_declarations_introduce_program_entities() {
    let source = "class EntityTypeSpec\nfun entityFunctionSpec() = Unit\nval entityValueSpec = 1\ntypealias EntityAliasSpec = EntityTypeSpec\n";
    let specification_uri = Url::parse("file:///kotlin-spec/DeclarationEntities.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);

    for (entity_name, entity_kind) in [
        ("EntityTypeSpec", SymbolKind::CLASS),
        ("entityFunctionSpec", SymbolKind::FUNCTION),
        ("entityValueSpec", SymbolKind::PROPERTY),
        ("EntityAliasSpec", SymbolKind::CLASS),
    ] {
        let entity = symbols
            .iter()
            .find(|symbol| symbol.name == entity_name)
            .expect("declaration must introduce an indexed entity");
        assert_eq!(entity.kind, entity_kind);
    }
}

#[test]
fn ks_declarations_0002_named_and_anonymous_declarations() {
    let source = "object NamedObjectSpec\nval anonymousObjectSpec = object {}\n";
    assert_source_contains_node_kind(source, "object_literal");

    let symbols = indexed_classifier_symbols(source);
    assert_eq!(
        symbols,
        vec![("NamedObjectSpec".to_string(), SymbolKind::OBJECT)]
    );
}

#[test]
fn ks_declarations_0004_named_declaration_introduces_binding() {
    let source = "fun bindSpec() = Unit\nclass OwnerSpec { fun bindSpec() = Unit }\n";
    let specification_uri = Url::parse("file:///kotlin-spec/NamedBinding.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let locations = resolve_symbol(&indexer, "bindSpec", Some("OwnerSpec"), &specification_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(
        locations[0].range.start,
        position_of_occurrence(source, "bindSpec", 1)
    );
}

#[test]
fn ks_declarations_0006_classifier_declarations_introduce_indexed_type_symbols() {
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
fn ks_declarations_0007_classifier_declarations_have_class_interface_and_object_forms() {
    assert_source_parses("class ScreenSpec\ninterface RenderableSpec\nobject RegistrySpec\n");
}

#[test]
fn ks_declarations_0008_object_literal_is_anonymous_classifier_declaration() {
    let source = "interface RenderableSpec\nval renderer = object : RenderableSpec {}\n";
    assert_source_contains_node_kind(source, "object_literal");

    let symbols = indexed_classifier_symbols(source);
    assert_eq!(
        symbols,
        vec![("RenderableSpec".to_string(), SymbolKind::INTERFACE)]
    );
}

#[test]
fn ks_declarations_0009_simple_class_combines_name_constructor_supertypes_and_body_members() {
    assert_source_parses(
        "open class BaseSpec\ninterface FirstSpec\ninterface SecondSpec\nclass WidgetSpec(val value: Int) : BaseSpec(), FirstSpec, SecondSpec {\n    constructor() : this(0)\n    init { require(value >= 0) }\n    val label: String = value.toString()\n    fun render(): String = label\n    companion object Named {}\n    class Nested\n}\n",
    );
}

#[test]
fn ks_declarations_0010_supertype_specifiers_create_indexed_inheritance_edges() {
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
#[ignore = "KS-DECLARATIONS-0011: kmp-lsp does not diagnose object or inner-class supertypes"]
fn ks_declarations_0011_object_and_inner_class_cannot_be_supertypes() {
    assert_source_parses(
        "open class BaseSpec\ninterface ContractSpec\nclass ValidSpec : BaseSpec(), ContractSpec\n",
    );
    assert_source_has_syntax_error("object RegistrySpec\nclass InvalidSpec : RegistrySpec()\n");
    assert_source_has_syntax_error(
        "class ContainerSpec { inner class InnerSpec }\nclass InvalidSpec : ContainerSpec.InnerSpec()\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0013: kmp-lsp does not diagnose multiple class inheritance"]
fn ks_declarations_0013_single_class_and_multiple_interface_inheritance() {
    assert_source_parses(
        "open class BaseSpec\ninterface FirstSpec\ninterface SecondSpec\nclass ValidSpec : BaseSpec(), FirstSpec, SecondSpec\n",
    );
    assert_source_has_syntax_error(
        "open class FirstBaseSpec\nopen class SecondBaseSpec\nclass InvalidSpec : FirstBaseSpec(), SecondBaseSpec()\n",
    );
}

#[test]
fn ks_declarations_0015_class_body_properties_and_functions_belong_to_class_scope() {
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
fn ks_declarations_0016_companion_members_resolve_through_class_and_companion_paths() {
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
fn ks_declarations_0017_unnamed_companion_uses_implicit_companion_name() {
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
fn ks_declarations_0018_nested_classifier_resolves_under_enclosing_class_name() {
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
fn ks_declarations_0019_parameterized_class_indexes_its_type_parameter_list() {
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
fn ks_declarations_0020_primary_constructor_distinguishes_parameter_and_property_forms() {
    let source = "class WidgetSpec(identifierSpec: String, val labelSpec: String, var countSpec: Int) {\n    constructor() : this(\"id\", \"label\", 0)\n}\n";
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
#[ignore = "KS-DECLARATIONS-0023: kmp-lsp does not validate superclass constructor invocation"]
fn ks_declarations_0023_class_supertype_specifier_requires_valid_constructor_invocation() {
    assert_source_parses("open class BaseSpec(valueSpec: Int)\nclass ValidSpec : BaseSpec(1)\n");
    assert_source_has_syntax_error(
        "open class BaseSpec(valueSpec: Int)\nclass InvalidSpec : BaseSpec\n",
    );
}

#[test]
fn ks_declarations_0024_secondary_constructor_supports_this_and_super_delegation_forms() {
    assert_source_parses(
        "open class BaseSpec(valueSpec: Int)\nclass PrimarySpec(valueSpec: Int) : BaseSpec(valueSpec) {\n    constructor() : this(0)\n}\nclass SecondarySpec : BaseSpec {\n    constructor(valueSpec: Int) : super(valueSpec)\n    constructor() : this(0)\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0025: kmp-lsp does not validate secondary delegation when a primary constructor exists"]
fn ks_declarations_0025_secondary_constructor_with_primary_delegates_to_this() {
    assert_source_parses(
        "open class BaseSpec\nclass ValidSpec(valueSpec: Int) : BaseSpec() {\n    constructor() : this(0)\n}\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec\nclass InvalidSpec(valueSpec: Int) : BaseSpec() {\n    constructor() : super()\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0026: kmp-lsp does not require secondary constructor delegation to a non-Any superclass"]
fn ks_declarations_0026_secondary_constructor_without_primary_delegates_to_super_or_this() {
    assert_source_parses(
        "open class BaseSpec(valueSpec: Int)\nclass ValidSpec : BaseSpec {\n    constructor(valueSpec: Int) : super(valueSpec)\n    constructor() : this(0)\n}\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec(valueSpec: Int)\nclass InvalidSpec : BaseSpec {\n    constructor(valueSpec: Int) {}\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0027: kmp-lsp does not detect secondary constructor delegation cycles"]
fn ks_declarations_0027_secondary_constructor_delegation_cannot_form_loop() {
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    constructor(valueSpec: Int) : this(valueSpec.toString())\n    constructor(valueSpec: String) : this(valueSpec.length)\n}\n",
    );
}

#[test]
fn ks_declarations_0028_constructors_accept_varargs_and_default_parameter_values() {
    assert_source_parses(
        "class WidgetSpec(val labelSpec: String = \"default\", vararg val valuesSpec: Int) {\n    constructor(vararg valuesSpec: Int) : this(valuesSpec = valuesSpec)\n}\n",
    );
}

#[tokio::test]
#[ignore = "KS-DECLARATIONS-0030: kmp-lsp does not resolve plain constructor parameters through constructor scopes"]
async fn ks_declarations_0030_constructor_parameters_resolve_in_their_linked_scopes() {
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
#[ignore = "KS-DECLARATIONS-0032: kmp-lsp does not diagnose inner classes declared in interfaces"]
fn ks_declarations_0032_inner_class_cannot_be_declared_in_interface() {
    assert_source_parses("class ContainerSpec {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("interface ContractSpec {\n    inner class InnerSpec {}\n}\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0033: kmp-lsp does not diagnose inner classes declared in statement scopes"]
fn ks_declarations_0033_inner_class_cannot_be_declared_in_statement_scope() {
    assert_source_parses("class ContainerSpec {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("fun createSpec() {\n    inner class InnerSpec {}\n}\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0034: kmp-lsp does not diagnose inner classes declared in objects"]
fn ks_declarations_0034_inner_class_cannot_be_declared_in_object() {
    assert_source_parses("class ContainerSpec {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("object RegistrySpec {\n    inner class InnerSpec {}\n}\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0035: kmp-lsp does not diagnose non-inner classifiers declared in object literals"]
fn ks_declarations_0035_object_literal_allows_only_inner_classifiers() {
    assert_source_parses("val validSpec = object {\n    inner class InnerSpec {}\n}\n");
    assert_source_has_syntax_error("val invalidClassSpec = object {\n    class NestedSpec {}\n}\n");
    assert_source_has_syntax_error(
        "val invalidInterfaceSpec = object {\n    interface NestedSpec {}\n}\n",
    );
}

#[test]
fn ks_declarations_0038_interface_inheritance_accepts_delegation_and_indexes_edge() {
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
#[ignore = "KS-DECLARATIONS-0036: kmp-lsp does not diagnose inheritance delegation to a class supertype"]
fn ks_declarations_0036_only_interface_inheritance_can_be_delegated() {
    assert_source_parses(
        "interface ContractSpec\nclass ValidSpec(delegateSpec: ContractSpec) : ContractSpec by delegateSpec\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec\nclass InvalidSpec(delegateSpec: BaseSpec) : BaseSpec by delegateSpec\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0037: kmp-lsp does not validate the inheritance delegate value type"]
fn ks_declarations_0037_inheritance_delegate_value_must_be_interface_subtype() {
    assert_source_parses(
        "interface ContractSpec\nclass DelegateSpec : ContractSpec\nclass ValidSpec(delegateSpec: DelegateSpec) : ContractSpec by delegateSpec\n",
    );
    assert_source_has_syntax_error(
        "interface ContractSpec\nclass UnrelatedSpec\nclass InvalidSpec(delegateSpec: UnrelatedSpec) : ContractSpec by delegateSpec\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0041: kmp-lsp does not diagnose class-member access from a delegation expression"]
fn ks_declarations_0041_delegation_expression_cannot_access_class_members() {
    assert_source_parses(
        "interface ContractSpec\nclass ValidSpec(delegateSpec: ContractSpec) : ContractSpec by delegateSpec\n",
    );
    assert_source_has_syntax_error(
        "interface ContractSpec\ninterface MarkerSpec\nclass InvalidSpec : ContractSpec by delegateSpec, MarkerSpec {\n    val delegateSpec: ContractSpec = object : ContractSpec {}\n}\n",
    );
}

#[test]
fn ks_declarations_0043_abstract_class_is_indexed_as_class() {
    let source = "abstract class BaseSpec\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert_eq!(symbols, vec![("BaseSpec".to_string(), SymbolKind::CLASS)]);
}

#[test]
#[ignore = "KS-DECLARATIONS-0044: kmp-lsp does not diagnose direct abstract-class construction"]
fn ks_declarations_0044_abstract_class_cannot_be_instantiated_directly() {
    assert_source_parses("abstract class BaseSpec\nclass ConcreteSpec : BaseSpec()\n");
    assert_source_has_syntax_error("abstract class BaseSpec\nval invalidSpec = BaseSpec()\n");
}

#[test]
fn ks_declarations_0045_abstract_class_accepts_abstract_members() {
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
#[ignore = "KS-DECLARATIONS-0046: kmp-lsp does not diagnose missing abstract-member implementations"]
fn ks_declarations_0046_concrete_subtype_implements_abstract_members() {
    assert_source_parses(
        "abstract class BaseSpec {\n    abstract fun renderSpec(): String\n}\nclass ValidSpec : BaseSpec() {\n    override fun renderSpec() = \"valid\"\n}\n",
    );
    assert_source_has_syntax_error(
        "abstract class BaseSpec {\n    abstract fun renderSpec(): String\n}\nclass InvalidSpec : BaseSpec()\n",
    );
}

#[test]
fn ks_declarations_0047_data_class_indexes_product_type_and_data_properties() {
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
#[ignore = "KS-DECLARATIONS-0048: kmp-lsp does not diagnose non-property data-class parameters"]
fn ks_declarations_0048_data_class_primary_parameters_must_be_properties() {
    assert_source_parses("data class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("data class InvalidSpec(valueSpec: Int)\n");
}

#[test]
fn ks_declarations_0053_generated_copy_matches_data_property_names_and_types() {
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
fn ks_declarations_0055_generated_copy_parameters_default_to_current_properties() {
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
#[ignore = "KS-DECLARATIONS-0056: kmp-lsp does not synthesize typed data-class component functions"]
fn ks_declarations_0056_generated_component_has_property_type_and_value_position() {
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
#[ignore = "KS-DECLARATIONS-0057: kmp-lsp does not synthesize operator data-class component functions"]
fn ks_declarations_0057_generated_component_is_operator_function() {
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
#[ignore = "KS-DECLARATIONS-0058: kmp-lsp does not synthesize data-class component functions"]
fn ks_declarations_0058_generated_component_count_matches_data_property_count() {
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
#[ignore = "KS-DECLARATIONS-0059: kmp-lsp does not synthesize component functions needed to expose data-property-only generation"]
fn ks_declarations_0059_only_constructor_data_properties_participate_in_generated_api() {
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
fn ks_declarations_0061_equals_hashcode_and_tostring_may_be_explicit() {
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
#[ignore = "KS-DECLARATIONS-0063: kmp-lsp does not diagnose explicit data-class copy or component functions"]
fn ks_declarations_0063_copy_and_component_functions_cannot_be_explicit() {
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
#[ignore = "KS-DECLARATIONS-0068: kmp-lsp does not diagnose inheritance from a data class"]
fn ks_declarations_0068_data_class_is_closed_to_inheritance() {
    assert_source_parses("data class LeafSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "data class BaseSpec(val valueSpec: Int)\nclass InvalidSpec(valueSpec: Int) : BaseSpec(valueSpec)\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0069: kmp-lsp does not diagnose a data class without a primary constructor"]
fn ks_declarations_0069_data_class_requires_primary_constructor() {
    assert_source_parses("data class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("data class InvalidSpec {\n    val valueSpec: Int = 0\n}\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0070: kmp-lsp does not diagnose an empty data-class primary constructor"]
fn ks_declarations_0070_data_class_requires_at_least_one_data_property() {
    assert_source_parses("data class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("data class InvalidSpec()\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0071: kmp-lsp does not diagnose vararg data properties"]
fn ks_declarations_0071_data_property_cannot_be_vararg() {
    assert_source_parses("data class ValidSpec(val valuesSpec: IntArray)\n");
    assert_source_has_syntax_error("data class InvalidSpec(vararg val valuesSpec: Int)\n");
}

#[test]
fn ks_declarations_0072_data_object_indexes_zero_property_unit_type() {
    let source = "data object EmptySpec\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert_eq!(symbols, vec![("EmptySpec".to_string(), SymbolKind::OBJECT)]);
}

#[test]
fn ks_declarations_0076_data_object_generates_no_copy_or_component_functions() {
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
fn ks_declarations_0077_data_object_tostring_may_be_explicit() {
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
#[ignore = "KS-DECLARATIONS-0078: kmp-lsp does not diagnose explicit data-object equals or hashCode"]
fn ks_declarations_0078_data_object_equals_and_hashcode_cannot_be_explicit() {
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
#[ignore = "KS-DECLARATIONS-0078: kmp-lsp does not diagnose inherited data-object equals or hashCode"]
fn ks_declarations_0078_data_object_equals_and_hashcode_cannot_be_inherited() {
    assert_source_has_syntax_error(
        "open class IdentityBaseSpec {\n    final override fun equals(otherSpec: Any?): Boolean = this === otherSpec\n    final override fun hashCode(): Int = 0\n}\ndata object InvalidSpec : IdentityBaseSpec()\n",
    );
}

#[test]
fn ks_declarations_0079_data_object_obeys_regular_object_shape_restrictions() {
    assert_source_parses("data object ValidSpec\n");
    assert_source_has_syntax_error("data object GenericSpec<ValueSpec>\n");
    assert_source_has_syntax_error("data object ConstructedSpec()\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0080: kmp-lsp does not diagnose a data companion object"]
fn ks_declarations_0080_companion_object_cannot_be_data_object() {
    assert_source_parses("class HostSpec {\n    companion object RegistrySpec\n}\n");
    assert_source_has_syntax_error("class HostSpec {\n    data companion object RegistrySpec\n}\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0081: kmp-lsp does not diagnose the data object-literal form"]
fn ks_declarations_0081_object_literal_cannot_be_data_object() {
    assert_source_parses("val validSpec = object {}\n");
    assert_source_has_syntax_error("val invalidSpec = data object {}\n");
}

#[test]
fn ks_declarations_0082_enum_class_indexes_predefined_entry_values() {
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
#[ignore = "KS-DECLARATIONS-0083: kmp-lsp does not diagnose direct enum-class construction"]
fn ks_declarations_0083_enum_values_cannot_be_constructed_outside_entries() {
    assert_source_parses("enum class StateSpec { READY }\nval validSpec = StateSpec.READY\n");
    assert_source_has_syntax_error(
        "enum class StateSpec { READY }\nval invalidSpec = StateSpec()\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0085: kmp-lsp does not diagnose an enum with an explicit base class"]
fn ks_declarations_0085_enum_class_cannot_have_another_base_class() {
    assert_source_parses("interface ContractSpec\nenum class ValidSpec : ContractSpec { READY }\n");
    assert_source_has_syntax_error(
        "open class BaseSpec\nenum class InvalidSpec : BaseSpec() { READY }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0086: kmp-lsp does not diagnose inheritance from an enum class"]
fn ks_declarations_0086_enum_class_is_final_and_cannot_be_inherited() {
    assert_source_parses("enum class LeafSpec { READY }\n");
    assert_source_has_syntax_error(
        "enum class BaseSpec { READY }\nclass InvalidSpec : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0087: kmp-lsp does not diagnose enum-class type parameters"]
fn ks_declarations_0087_enum_class_cannot_have_type_parameters() {
    assert_source_parses("enum class ValidSpec { READY }\n");
    assert_source_has_syntax_error("enum class InvalidSpec<ValueSpec> { READY }\n");
}

#[test]
fn ks_declarations_0088_enum_entry_resolves_as_static_member_callable() {
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
#[ignore = "KS-DECLARATIONS-0089: kmp-lsp assigns enum-entry body members to the enum class container"]
fn ks_declarations_0089_enum_entry_body_accepts_entry_specific_declarations() {
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
fn ks_declarations_0090_enum_class_may_have_zero_entries() {
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
fn ks_declarations_0091_enum_entry_name_has_string_type() {
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
fn ks_declarations_0093_enum_entry_ordinal_has_int_type() {
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
#[ignore = "KS-DECLARATIONS-0096: kmp-lsp assigns entry-specific compareTo to the enum class container"]
fn ks_declarations_0096_compareto_may_be_overridden_in_enum_and_entry() {
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
#[ignore = "KS-DECLARATIONS-0098: kmp-lsp assigns entry-specific toString to the enum class container"]
fn ks_declarations_0098_tostring_may_be_overridden_in_enum_and_entry() {
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
fn ks_declarations_0099_enum_entries_property_has_bounded_list_type() {
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
fn ks_declarations_0101_enum_valueof_returns_enum_type() {
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
fn ks_declarations_0104_enum_values_returns_array_of_enum_type() {
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
fn ks_declarations_0107_annotation_class_introduces_indexed_classifier() {
    let source = "annotation class RouteSpec(val pathSpec: String)\n";
    assert_source_parses(source);
    let symbols = indexed_classifier_symbols(source);
    assert_eq!(symbols, vec![("RouteSpec".to_string(), SymbolKind::CLASS)]);
}

#[test]
#[ignore = "KS-DECLARATIONS-0108: kmp-lsp does not diagnose annotation secondary constructors"]
fn ks_declarations_0108_annotation_class_cannot_have_secondary_constructors() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    constructor() : this(0)\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0109: kmp-lsp does not diagnose non-property annotation parameters"]
fn ks_declarations_0109_annotation_constructor_parameters_require_property_syntax() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("annotation class InvalidSpec(valueSpec: Int)\n");
}

#[test]
fn ks_declarations_0110_annotation_constructor_properties_are_indexed() {
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
#[ignore = "KS-DECLARATIONS-0112: kmp-lsp does not diagnose additional annotation interfaces"]
fn ks_declarations_0112_annotation_class_cannot_implement_additional_interfaces() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error(
        "interface ContractSpec\nannotation class InvalidSpec : ContractSpec\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0113: kmp-lsp does not diagnose annotation base classes"]
fn ks_declarations_0113_annotation_class_cannot_specify_a_base_class() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error(
        "open class BaseSpec\nannotation class InvalidSpec : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0114: kmp-lsp does not diagnose inheritance from annotations"]
fn ks_declarations_0114_annotation_class_is_closed_to_inheritance() {
    assert_source_parses("annotation class LeafSpec\n");
    assert_source_has_syntax_error("annotation class BaseSpec\nclass InvalidSpec : BaseSpec()\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0115: kmp-lsp does not diagnose annotation member functions"]
fn ks_declarations_0115_annotation_class_cannot_declare_member_functions() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    fun helperSpec(): Int = valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0116: kmp-lsp does not diagnose extra annotation properties"]
fn ks_declarations_0116_annotation_class_cannot_declare_extra_properties() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    val extraSpec: Int = valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0117: kmp-lsp does not diagnose annotation overrides"]
fn ks_declarations_0117_annotation_class_cannot_declare_overrides() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec(val valueSpec: Int) {\n    override fun toString(): String = valueSpec.toString()\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0118: kmp-lsp does not diagnose annotation companion objects"]
fn ks_declarations_0118_annotation_class_cannot_have_companion_object() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec {\n    companion object RegistrySpec\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0119: kmp-lsp does not diagnose nested annotation classes"]
fn ks_declarations_0119_annotation_class_cannot_have_nested_class() {
    assert_source_parses("annotation class ValidSpec\n");
    assert_source_has_syntax_error("annotation class InvalidSpec {\n    class NestedSpec\n}\n");
}

#[test]
fn ks_declarations_0120_annotation_parameters_accept_allowed_scalar_types() {
    assert_source_parses(
        "import kotlin.reflect.KClass\nannotation class ScalarSpec(\n    val textSpec: String,\n    val classSpec: KClass<*>,\n    val byteSpec: Byte,\n    val shortSpec: Short,\n    val intSpec: Int,\n    val longSpec: Long,\n    val floatSpec: Float,\n    val doubleSpec: Double,\n    val charSpec: Char,\n    val booleanSpec: Boolean,\n)\n",
    );
}

#[test]
fn ks_declarations_0121_annotation_parameters_accept_annotations_and_arrays() {
    assert_source_parses(
        "annotation class NestedSpec(val valueSpec: Int)\nannotation class CompositeSpec(\n    val nestedSpec: NestedSpec,\n    val nestedArraySpec: Array<NestedSpec>,\n    val stringArraySpec: Array<String>,\n    val numberArraySpec: IntArray,\n)\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0122: kmp-lsp does not diagnose cyclic annotation types"]
fn ks_declarations_0122_annotation_types_cannot_reference_themselves_cyclically() {
    assert_source_parses("annotation class ValidSpec(val valueSpec: String)\n");
    assert_source_has_syntax_error("annotation class DirectSpec(val valueSpec: DirectSpec)\n");
    assert_source_has_syntax_error(
        "annotation class FirstSpec(val secondSpec: SecondSpec)\nannotation class SecondSpec(val firstSpec: Array<FirstSpec>)\n",
    );
}

#[test]
fn ks_declarations_0124_annotation_class_may_declare_type_parameters() {
    assert_source_parses("annotation class MarkerSpec<ElementSpec>\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0123: kmp-lsp does not diagnose annotation type-parameter properties"]
fn ks_declarations_0123_annotation_constructor_cannot_use_its_type_parameter() {
    assert_source_parses("annotation class ValidSpec<ElementSpec>(val valueSpec: String)\n");
    assert_source_has_syntax_error(
        "annotation class InvalidSpec<ElementSpec>(val valueSpec: ElementSpec)\n",
    );
}

#[tokio::test]
async fn ks_declarations_0125_annotation_class_can_be_instantiated_directly() {
    let source = "import kotlin.reflect.KClass\nannotation class RouteSpec<RouteType : Any>(val routeType: KClass<RouteType>)\nannotation class OtherSpec(val path: String)\nval routeSpec = RouteSpec(String::class)\nval otherSpec = OtherSpec(\"other\")\n";
    assert_source_parses(source);
    let locations = definition_locations(source, "RouteSpec", 1).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 1);
}

#[test]
fn ks_declarations_0126_annotation_class_may_have_no_parameters() {
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
fn ks_declarations_0127_annotation_constructor_supports_vararg_properties() {
    let source = "import kotlin.reflect.KClass\nannotation class TypesSpec(vararg val classesSpec: KClass<out Annotation>)\nannotation class RequiredSpec(val classSpec: KClass<out Annotation>)\nfun instantiateSpec() = TypesSpec()\n";
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
fn ks_declarations_0128_value_class_accepts_value_and_inline_declaration_modifiers() {
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
#[ignore = "KS-DECLARATIONS-0129: kmp-lsp does not diagnose inheritance from value classes"]
fn ks_declarations_0129_value_class_is_closed_to_inheritance() {
    assert_source_parses("value class LeafSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "value class BaseSpec(val valueSpec: Int)\nclass InvalidSpec(valueSpec: Int) : BaseSpec(valueSpec)\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0130: kmp-lsp does not diagnose incompatible value-class modifiers"]
fn ks_declarations_0130_value_class_rejects_inner_data_and_enum_forms() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "class HostSpec { inner value class InvalidSpec(val valueSpec: Int) }\n",
    );
    assert_source_has_syntax_error("data value class InvalidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("value enum class InvalidSpec { ENTRY_SPEC }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0131: kmp-lsp does not validate the value-class primary constructor"]
fn ks_declarations_0131_value_class_requires_one_constructor_property() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("value class MissingConstructorSpec\n");
    assert_source_has_syntax_error("value class EmptyConstructorSpec()\n");
    assert_source_has_syntax_error("value class BareParameterSpec(valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "value class MultiplePropertiesSpec(val firstSpec: Int, val secondSpec: Int)\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0132: kmp-lsp does not diagnose vararg value-class data properties"]
fn ks_declarations_0132_value_class_data_property_cannot_be_vararg() {
    assert_source_parses("value class ValidSpec(val valuesSpec: IntArray)\n");
    assert_source_has_syntax_error("value class InvalidSpec(vararg val valuesSpec: Int)\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0133: kmp-lsp does not diagnose non-public value-class data properties"]
fn ks_declarations_0133_value_class_data_property_must_be_public() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error("value class InvalidSpec(private val valueSpec: Int)\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0134: kmp-lsp does not diagnose value-class equals or hashCode overrides"]
fn ks_declarations_0134_value_class_cannot_override_equals_or_hashcode() {
    assert_source_parses("value class ValidSpec(val valueSpec: Int)\n");
    assert_source_has_syntax_error(
        "value class InvalidEqualsSpec(val valueSpec: Int) {\n    override fun equals(otherSpec: Any?): Boolean = false\n}\n",
    );
    assert_source_has_syntax_error(
        "value class InvalidHashSpec(val valueSpec: Int) {\n    override fun hashCode(): Int = valueSpec\n}\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0135: kmp-lsp does not diagnose value-class base classes"]
fn ks_declarations_0135_value_class_cannot_have_a_base_class_besides_any() {
    assert_source_parses(
        "interface ContractSpec\nvalue class ValidSpec(val valueSpec: Int) : ContractSpec\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec\nvalue class InvalidSpec(val valueSpec: Int) : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0136: kmp-lsp does not diagnose value-class backing fields"]
fn ks_declarations_0136_other_value_class_properties_cannot_have_backing_fields() {
    assert_source_parses(
        "value class ValidSpec(val valueSpec: Int) {\n    val doubledSpec: Int get() = valueSpec * 2\n}\n",
    );
    assert_source_has_syntax_error(
        "value class InvalidSpec(val valueSpec: Int) {\n    val storedSpec: Int = valueSpec * 2\n}\n",
    );
}

#[test]
fn ks_declarations_0137_value_class_accepts_computed_properties_without_backing_fields() {
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
fn ks_declarations_0138_inline_modifier_preserves_legacy_value_class_syntax() {
    assert_source_parses("inline class LegacyIdentifierSpec(val valueSpec: String)\n");
}

#[test]
fn ks_declarations_0142_value_class_may_override_tostring_explicitly() {
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

#[test]
fn ks_declarations_0147_interface_declares_a_contract_for_indexed_subtypes() {
    let source = "interface RenderableSpec { fun renderSpec(): String }\nclass ScreenSpec : RenderableSpec { override fun renderSpec(): String = \"screen\" }\nclass MisleadingSpec\n";
    let specification_uri = Url::parse("file:///kotlin-spec/InterfaceContract.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let interface_symbol = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "RenderableSpec")
        .expect("interface must be indexed");
    assert_eq!(interface_symbol.kind, SymbolKind::INTERFACE);
    let subtypes = indexer.subtypes_of("RenderableSpec");
    assert_eq!(subtypes.len(), 1);
    assert_eq!(subtypes[0].range.start.line, 1);
    assert!(indexer.subtypes_of("MisleadingSpec").is_empty());
}

#[test]
#[ignore = "KS-DECLARATIONS-0146: kmp-lsp does not diagnose direct interface construction"]
fn ks_declarations_0146_interface_cannot_be_instantiated_directly() {
    assert_source_parses(
        "interface RenderableSpec\nclass ScreenSpec : RenderableSpec\nval validSpec: RenderableSpec = ScreenSpec()\n",
    );
    assert_source_has_syntax_error("interface InvalidSpec\nval valueSpec = InvalidSpec()\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0148: kmp-lsp does not diagnose interfaces in statement or object-literal scopes"]
fn ks_declarations_0148_interface_is_limited_to_declaration_scopes() {
    assert_source_parses("interface TopLevelSpec\nclass HostSpec { interface NestedSpec; }\n");
    assert_source_has_syntax_error("fun invalidSpec() { interface LocalSpec; }\n");
    assert_source_has_syntax_error(
        "val invalidSpec = object { interface ObjectLiteralNestedSpec; }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0149: kmp-lsp does not diagnose class supertypes of interfaces"]
fn ks_declarations_0149_interface_cannot_have_a_class_supertype() {
    assert_source_parses("interface BaseContractSpec\ninterface ValidSpec : BaseContractSpec\n");
    assert_source_has_syntax_error("open class BaseSpec\ninterface InvalidSpec : BaseSpec\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0152: kmp-lsp does not diagnose interface constructors"]
fn ks_declarations_0152_interface_cannot_declare_a_constructor() {
    assert_source_parses("interface ValidSpec\n");
    assert_source_has_syntax_error("interface InvalidPrimarySpec()\n");
    assert_source_has_syntax_error("interface InvalidSecondarySpec { constructor(); }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0153: kmp-lsp does not diagnose initialized interface properties"]
fn ks_declarations_0153_interface_properties_cannot_have_initializers() {
    assert_source_parses("interface ValidSpec { val valueSpec: Int; }\n");
    assert_source_has_syntax_error("interface InvalidSpec { val valueSpec: Int = 1; }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0154: kmp-lsp does not diagnose delegated interface properties"]
fn ks_declarations_0154_interface_properties_cannot_be_delegated() {
    assert_source_parses("interface ValidSpec { val valueSpec: Int; }\n");
    assert_source_has_syntax_error("interface InvalidSpec { val valueSpec: Int by lazy { 1 }; }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0155: kmp-lsp does not diagnose inner classes in interfaces"]
fn ks_declarations_0155_interface_cannot_have_inner_classes() {
    assert_source_parses("interface ValidSpec { class NestedSpec; }\n");
    assert_source_has_syntax_error("interface InvalidSpec { inner class InnerSpec; }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0158: kmp-lsp does not diagnose non-public interface members"]
fn ks_declarations_0158_interface_members_cannot_be_non_public() {
    assert_source_parses("interface ValidSpec { val valueSpec: Int; fun renderSpec(): String; }\n");
    assert_source_has_syntax_error(
        "interface InvalidPropertySpec { private val valueSpec: Int; }\n",
    );
    assert_source_has_syntax_error(
        "interface InvalidFunctionSpec { protected fun renderSpec(): String; }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0160: tree-sitter-kotlin rejects fun interface declarations"]
fn ks_declarations_0160_functional_interface_uses_fun_interface_declaration() {
    assert_source_parses("fun interface ActionSpec { fun runSpec(valueSpec: Int): String }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0161: fun interface parsing blocks abstract-function count validation"]
fn ks_declarations_0161_functional_interface_has_only_one_abstract_function() {
    assert_source_parses("fun interface ValidSpec { fun runSpec(): Unit }\n");
    assert_source_has_syntax_error(
        "fun interface InvalidSpec { fun firstSpec(): Unit; fun secondSpec(): Unit }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0162: fun interface parsing blocks generic SAM validation"]
fn ks_declarations_0162_functional_interface_abstract_function_is_non_parameterized() {
    assert_source_parses("fun interface ValidSpec { fun runSpec(): Unit }\n");
    assert_source_has_syntax_error(
        "fun interface InvalidSpec { fun <ElementSpec> runSpec(valueSpec: ElementSpec): Unit }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0163: fun interface parsing blocks abstract-property validation"]
fn ks_declarations_0163_functional_interface_cannot_have_abstract_properties() {
    assert_source_parses("fun interface ValidSpec { fun runSpec(): Unit }\n");
    assert_source_has_syntax_error(
        "fun interface InvalidSpec { fun runSpec(): Unit; val valueSpec: Int }\n",
    );
}

#[test]
fn ks_declarations_0166_functional_contract_accepts_class_and_object_implementations() {
    let source = "interface ActionSpec { fun runSpec(valueSpec: Int): String; }\nclass ActionImplementationSpec : ActionSpec { override fun runSpec(valueSpec: Int): String = valueSpec.toString(); }\nval objectActionSpec = object : ActionSpec { override fun runSpec(valueSpec: Int): String = valueSpec.toString(); }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/FunctionalImplementations.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let subtypes = indexer.subtypes_of("ActionSpec");
    assert_eq!(subtypes.len(), 1);
    assert_eq!(subtypes[0].range.start.line, 1);
}

#[test]
fn ks_declarations_0169_object_declaration_introduces_type_and_single_value_symbol() {
    let source = "object RegistrySpec { val sizeSpec: Int = 1; }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/ObjectDeclaration.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let object_symbol = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "RegistrySpec")
        .expect("object declaration must be indexed");
    assert_eq!(object_symbol.kind, SymbolKind::OBJECT);
}

#[test]
#[ignore = "KS-DECLARATIONS-0170: kmp-lsp does not diagnose construction of additional object values"]
fn ks_declarations_0170_object_type_cannot_have_additional_constructed_values() {
    assert_source_parses("object RegistrySpec\nval validSpec = RegistrySpec\n");
    assert_source_has_syntax_error("object RegistrySpec\nval invalidSpec = RegistrySpec()\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0171: kmp-lsp does not diagnose named objects in statement or object-literal scopes"]
fn ks_declarations_0171_named_object_is_limited_to_declaration_scopes() {
    assert_source_parses("object TopLevelSpec\nclass HostSpec { object NestedSpec; }\n");
    assert_source_has_syntax_error("fun invalidSpec() { object LocalSpec; }\n");
    assert_source_has_syntax_error("val invalidSpec = object { object NestedSpec; }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0172: kmp-lsp does not diagnose object types used as supertypes"]
fn ks_declarations_0172_object_type_cannot_be_used_as_a_supertype() {
    assert_source_parses("open class BaseSpec\nobject ValidSpec : BaseSpec()\n");
    assert_source_has_syntax_error("object BaseObjectSpec\nclass InvalidSpec : BaseObjectSpec()\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0173: kmp-lsp does not diagnose object constructors"]
fn ks_declarations_0173_object_cannot_declare_constructors() {
    assert_source_parses("object ValidSpec\n");
    assert_source_has_syntax_error("object InvalidPrimarySpec()\n");
    assert_source_has_syntax_error("object InvalidSecondarySpec { constructor(); }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0174: kmp-lsp does not diagnose object companion objects"]
fn ks_declarations_0174_object_cannot_have_a_companion_object() {
    assert_source_parses("object ValidSpec\n");
    assert_source_has_syntax_error("object InvalidSpec { companion object RegistrySpec; }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0175: kmp-lsp does not diagnose inner classes in objects"]
fn ks_declarations_0175_object_cannot_have_inner_classes() {
    assert_source_parses("object ValidSpec { class NestedSpec; }\n");
    assert_source_has_syntax_error("object InvalidSpec { inner class InnerSpec; }\n");
}

#[test]
fn ks_declarations_0176_object_cannot_declare_type_parameters() {
    assert_source_parses("object ValidSpec\n");
    assert_source_has_syntax_error("object InvalidSpec<ElementSpec>\n");
}

#[test]
fn ks_declarations_0178_class_may_be_declared_in_a_function_statement_scope() {
    let source = "fun buildSpec(): Any {\n    class LocalSpec(val valueSpec: Int)\n    return LocalSpec(1)\n}\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/LocalClass.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let local_class = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "LocalSpec")
        .expect("local class must be indexed");
    assert_eq!(local_class.kind, SymbolKind::CLASS);
    assert_eq!(local_class.range.start.line, 1);
}

#[test]
#[ignore = "KS-DECLARATIONS-0179: kmp-lsp does not diagnose local interface or object declarations"]
fn ks_declarations_0179_interface_and_object_cannot_be_declared_locally() {
    assert_source_parses("fun validSpec() { class LocalSpec; }\n");
    assert_source_has_syntax_error("fun invalidInterfaceSpec() { interface LocalSpec; }\n");
    assert_source_has_syntax_error("fun invalidObjectSpec() { object LocalSpec; }\n");
}

#[tokio::test]
async fn ks_declarations_0180_local_class_may_capture_a_value_from_its_scope() {
    let source = "fun buildSpec(): Int {\n    val outerValueSpec = 2\n    class LocalSpec { val capturedSpec = outerValueSpec; }\n    return LocalSpec().capturedSpec\n}\n";
    assert_source_parses(source);
    let locations = definition_locations(source, "outerValueSpec", 1).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 1);
}

#[test]
#[ignore = "KS-DECLARATIONS-0181: kmp-lsp does not diagnose local enum or annotation classes"]
fn ks_declarations_0181_enum_and_annotation_classes_cannot_be_declared_locally() {
    assert_source_parses("fun validSpec() { class LocalSpec; }\n");
    assert_source_has_syntax_error(
        "fun invalidEnumSpec() { enum class LocalSpec { ENTRY_SPEC } }\n",
    );
    assert_source_has_syntax_error("fun invalidAnnotationSpec() { annotation class LocalSpec; }\n");
}

#[test]
fn ks_declarations_0197_functions_properties_and_inner_classifiers_use_actual_body_scope() {
    let source = "class HostSpec {\n    val valueSpec: Int = 1\n    fun renderSpec(): Int = valueSpec\n    inner class InnerSpec\n}\n";
    let specification_uri = Url::parse("file:///kotlin-spec/ActualClassifierScope.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for member_name in ["valueSpec", "renderSpec", "InnerSpec"] {
        let member = symbols
            .iter()
            .find(|symbol| symbol.name == member_name)
            .expect("actual-scope declaration must be indexed");
        assert_eq!(member.container.as_deref(), Some("HostSpec"));
    }
}

#[tokio::test]
#[ignore = "KS-DECLARATIONS-0202: kmp-lsp returns competing targets across the static-to-actual scope link"]
async fn ks_declarations_0202_static_scope_links_upward_to_actual_body_scope() {
    let source = "val valueSpec = 99\nclass HostSpec {\n    val valueSpec = 1\n    constructor(markerSpec: String) { println(valueSpec + markerSpec.length) }\n}\n";
    let locations = definition_locations(source, "valueSpec", 2).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 2);
}

#[test]
fn ks_declarations_0199_non_inner_nested_classifier_is_qualified_static_member() {
    let declaration_uri = Url::parse("file:///kotlin-spec/NestedStaticDeclaration.kt")
        .expect("specification fixture URI must be valid");
    let use_uri = Url::parse("file:///kotlin-spec/NestedStaticUse.kt")
        .expect("specification use-site URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package specification\nclass HostSpec { class NestedSpec }\nclass MisleadingNestedSpec\n",
    );
    indexer.index_content(
        &use_uri,
        "package specification\nval nestedSpec: HostSpec.NestedSpec? = null\n",
    );
    let locations = resolve_symbol(&indexer, "NestedSpec", Some("HostSpec"), &use_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, declaration_uri);
    assert_eq!(locations[0].range.start.line, 1);
}

#[test]
fn ks_declarations_0200_companion_object_is_qualified_static_member() {
    let source = "object RegistrySpec\nclass HostSpec { companion object RegistrySpec }\nval selectedSpec = HostSpec.RegistrySpec\n";
    let specification_uri = Url::parse("file:///kotlin-spec/CompanionStaticScope.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let locations = resolve_symbol(
        &indexer,
        "RegistrySpec",
        Some("HostSpec"),
        &specification_uri,
    );
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 1);
}

#[test]
fn ks_declarations_0201_enum_entry_is_qualified_static_member() {
    let source = "object READY\nenum class StateSpec { READY, STOPPED }\nval selectedSpec = StateSpec.READY\n";
    let specification_uri = Url::parse("file:///kotlin-spec/EnumStaticScope.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let locations = resolve_symbol(&indexer, "READY", Some("StateSpec"), &specification_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 1);
}

#[tokio::test]
#[ignore = "KS-DECLARATIONS-0203: kmp-lsp returns competing targets for object nested-member lookup"]
async fn ks_declarations_0203_object_static_and_actual_scopes_are_the_same() {
    let source = "val valueSpec = 99\nobject RegistrySpec {\n    val valueSpec = 1\n    class NestedSpec { fun readSpec(): Int = valueSpec; }\n}\n";
    let locations = definition_locations(source, "valueSpec", 2).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 2);
}

#[tokio::test]
#[ignore = "KS-DECLARATIONS-0204: kmp-lsp returns competing targets from classifier initializers"]
async fn ks_declarations_0204_initializers_link_to_actual_classifier_body_scope() {
    let source = "val baseSpec = 99\nclass HostSpec {\n    val baseSpec = 1\n    val derivedSpec = baseSpec + 1\n    init { println(baseSpec) }\n}\n";
    for occurrence in [2, 3] {
        let locations = definition_locations(source, "baseSpec", occurrence).await;
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start.line, 2);
    }
}

#[tokio::test]
#[ignore = "KS-DECLARATIONS-0205: kmp-lsp does not prioritize primary constructor parameter scope"]
async fn ks_declarations_0205_primary_constructor_parameters_bind_only_toward_initialization_scope()
{
    let source = "val parameterSpec = 99\nclass HostSpec(parameterSpec: Int) {\n    val copiedSpec = parameterSpec\n    fun readSpec(): Int = parameterSpec\n}\n";
    let initializer_locations = definition_locations(source, "parameterSpec", 2).await;
    assert_eq!(initializer_locations.len(), 1);
    assert_eq!(initializer_locations[0].range.start, Position::new(1, 15));

    let member_locations = definition_locations(source, "parameterSpec", 3).await;
    assert_eq!(member_locations.len(), 1);
    assert_eq!(member_locations[0].range.start.line, 0);
}

#[tokio::test]
async fn ks_declarations_0206_interface_delegate_uses_constructor_or_declaration_scope() {
    let source = "interface ContractSpec\nobject OuterDelegateSpec : ContractSpec\nclass HostSpec(delegateSpec: ContractSpec) : ContractSpec by delegateSpec\nobject RegistrySpec : ContractSpec by OuterDelegateSpec\n";
    let constructor_locations = definition_locations(source, "delegateSpec", 1).await;
    assert_eq!(constructor_locations.len(), 1);
    assert_eq!(constructor_locations[0].range.start, Position::new(2, 15));

    let outer_locations = definition_locations(source, "OuterDelegateSpec", 1).await;
    assert_eq!(outer_locations.len(), 1);
    assert_eq!(outer_locations[0].range.start.line, 1);
}

#[tokio::test]
async fn ks_declarations_0389_type_alias_introduces_simple_and_parameterized_alternative_names() {
    let source = "typealias IntListSpec = List<Int>\ntypealias IntMapSpec<ValueSpec> = Map<Int, ValueSpec>\nval listSpec: IntListSpec = emptyList()\nval mapSpec: IntMapSpec<String> = emptyMap()\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/TypeAliases.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for alias_name in ["IntListSpec", "IntMapSpec"] {
        let alias = symbols
            .iter()
            .find(|symbol| symbol.name == alias_name)
            .expect("type alias must be indexed");
        assert_eq!(alias.kind, SymbolKind::CLASS);
    }
    for (alias_name, use_occurrence) in [("IntListSpec", 1), ("IntMapSpec", 1)] {
        let locations = definition_locations(source, alias_name, use_occurrence).await;
        assert_eq!(locations.len(), 1);
        assert_eq!(
            locations[0].range.start,
            position_of_occurrence(source, alias_name, 0)
        );
    }
}

#[test]
#[ignore = "KS-DECLARATIONS-0391: kmp-lsp does not diagnose bounds or variance on type-alias parameters"]
fn ks_declarations_0391_type_alias_parameters_cannot_have_bounds_or_variance() {
    assert_source_parses("typealias ValidSpec<ValueSpec> = List<ValueSpec>\n");
    assert_source_has_syntax_error("typealias BoundedSpec<ValueSpec : Number> = List<ValueSpec>\n");
    assert_source_has_syntax_error("typealias CovariantSpec<out ValueSpec> = List<ValueSpec>\n");
    assert_source_has_syntax_error(
        "typealias ContravariantSpec<in ValueSpec> = Comparator<ValueSpec>\n",
    );
}

#[test]
fn ks_declarations_0392_type_alias_parameter_may_be_unreferenced() {
    assert_source_parses("typealias StrangeSpec<UnusedSpec> = String\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0395: kmp-lsp does not diagnose recursive type aliases"]
fn ks_declarations_0395_recursive_type_alias_is_forbidden() {
    assert_source_parses("typealias ValidSpec = List<Int>\n");
    assert_source_has_syntax_error("typealias DirectSpec = DirectSpec\n");
    assert_source_has_syntax_error(
        "typealias FirstSpec = SecondSpec\ntypealias SecondSpec = FirstSpec\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0396: kmp-lsp does not diagnose non-top-level type aliases"]
fn ks_declarations_0396_type_alias_must_be_top_level() {
    assert_source_parses("typealias TopLevelSpec = String\n");
    assert_source_has_syntax_error("class HostSpec { typealias MemberSpec = String }\n");
    assert_source_has_syntax_error("fun localSpec() { typealias LocalSpec = String }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0397: kmp-lsp resolves private type aliases across files"]
fn ks_declarations_0397_type_alias_accessibility_follows_visibility_modifier() {
    let declaration_uri = Url::parse("file:///kotlin-spec/aliases/Declarations.kt")
        .expect("declaration URI must be valid");
    let use_uri =
        Url::parse("file:///kotlin-spec/aliases/Usage.kt").expect("use URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package aliases\npublic typealias PublicAliasSpec = String\nprivate typealias PrivateAliasSpec = String\n",
    );
    indexer.index_content(
        &use_uri,
        "package aliases\nval publicSpec: PublicAliasSpec = \"value\"\nval privateSpec: PrivateAliasSpec = \"hidden\"\n",
    );
    let public_locations = resolve_symbol(&indexer, "PublicAliasSpec", None, &use_uri);
    assert_eq!(public_locations.len(), 1);
    assert_eq!(public_locations[0].uri, declaration_uri);
    let private_locations = resolve_symbol(&indexer, "PrivateAliasSpec", None, &use_uri);
    assert!(private_locations.is_empty());
}

#[test]
fn ks_declarations_0398_classes_functions_and_extension_properties_may_be_generic() {
    let source = "class BoxSpec<ValueSpec>(val valueSpec: ValueSpec)\nfun <ValueSpec> identitySpec(valueSpec: ValueSpec): ValueSpec = valueSpec\nval <ValueSpec> List<ValueSpec>.firstSpec: ValueSpec get() = first()\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/GenericDeclarations.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for declaration_name in ["BoxSpec", "identitySpec", "firstSpec"] {
        assert!(
            symbols.iter().any(|symbol| symbol.name == declaration_name),
            "generic declaration {declaration_name} must be indexed"
        );
    }
}

#[test]
fn ks_declarations_0399_type_parameter_may_be_used_as_type_in_declaration_scope() {
    let source = "class BoxSpec<ValueSpec>(val valueSpec: ValueSpec) { fun copySpec(replacementSpec: ValueSpec): BoxSpec<ValueSpec> = BoxSpec(replacementSpec); }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/GenericScope.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let box_symbol = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .find(|symbol| symbol.name == "BoxSpec")
        .expect("generic classifier must be indexed");
    assert!(box_symbol.detail.contains("ValueSpec"));
}

#[test]
#[ignore = "KS-DECLARATIONS-0401: kmp-lsp does not diagnose type parameters on non-extension properties"]
fn ks_declarations_0401_non_extension_property_cannot_have_type_parameters() {
    assert_source_parses("val <ValueSpec> List<ValueSpec>.firstSpec: ValueSpec get() = first()\n");
    assert_source_has_syntax_error("val <ValueSpec> invalidSpec: ValueSpec get() = TODO()\n");
}

#[test]
fn ks_declarations_0402_object_declaration_cannot_have_type_parameters() {
    assert_source_parses("object ValidSpec\n");
    assert_source_has_syntax_error("object InvalidSpec<ValueSpec>\n");
    assert_source_has_syntax_error(
        "class HostSpec { companion object InvalidCompanionSpec<ValueSpec> }\n",
    );
}

#[test]
fn ks_declarations_0403_constructor_declaration_cannot_have_type_parameters() {
    assert_source_parses("class ValidSpec<ValueSpec>(val valueSpec: ValueSpec)\n");
    assert_source_has_syntax_error("class InvalidSpec { constructor<ValueSpec>() }\n");
}

#[test]
fn ks_declarations_0404_property_accessors_cannot_have_type_parameters() {
    assert_source_parses("val validSpec: Int get() = 1\n");
    assert_source_has_syntax_error("val invalidGetterSpec: Int get<ValueSpec>() = 1\n");
    assert_source_has_syntax_error(
        "var invalidSetterSpec: Int = 1 set<ValueSpec>(newValueSpec) { field = newValueSpec }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0405: kmp-lsp does not diagnose generic enum classes"]
fn ks_declarations_0405_enum_class_cannot_have_type_parameters() {
    assert_source_parses("enum class ValidSpec { READY }\n");
    assert_source_has_syntax_error("enum class InvalidSpec<ValueSpec> { READY }\n");
}

#[test]
#[ignore = "KS-DECLARATIONS-0406: kmp-lsp does not diagnose generic Throwable classifiers"]
fn ks_declarations_0406_throwable_classifier_cannot_have_type_parameters() {
    assert_source_parses("class ValidSpec(messageSpec: String) : Throwable(messageSpec)\n");
    assert_source_has_syntax_error(
        "class InvalidSpec<ValueSpec>(messageSpec: String) : Throwable(messageSpec)\n",
    );
}

#[test]
fn ks_declarations_0407_type_parameter_bounds_accept_inline_and_where_forms() {
    assert_source_parses(
        "fun <ValueSpec : CharSequence> inlineBoundSpec(valueSpec: ValueSpec): Int = valueSpec.length\nfun <ValueSpec> whereBoundSpec(valueSpec: ValueSpec): Int where ValueSpec : CharSequence = valueSpec.length\n",
    );
}

#[test]
fn ks_declarations_0408_type_parameter_accepts_multiple_upper_bounds() {
    assert_source_parses(
        "fun <ValueSpec> inspectSpec(valueSpec: ValueSpec): Int where ValueSpec : CharSequence, ValueSpec : Comparable<ValueSpec> = valueSpec.length\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0409: kmp-lsp does not validate multiple type-parameter bounds"]
fn ks_declarations_0409_type_parameter_allows_only_one_bound_to_another_parameter() {
    assert_source_parses(
        "fun <ValueSpec, UpperSpec> validSpec(valueSpec: ValueSpec): ValueSpec where ValueSpec : UpperSpec = valueSpec\n",
    );
    assert_source_has_syntax_error(
        "fun <ValueSpec, FirstUpperSpec, SecondUpperSpec> invalidSpec(valueSpec: ValueSpec): ValueSpec where ValueSpec : FirstUpperSpec, ValueSpec : SecondUpperSpec = valueSpec\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0412: kmp-lsp does not diagnose reified parameters on non-inline functions"]
fn ks_declarations_0412_only_inline_declaration_type_parameters_may_be_reified() {
    assert_source_parses(
        "inline fun <reified ValueSpec> validSpec(valueSpec: ValueSpec): ValueSpec = valueSpec\n",
    );
    assert_source_has_syntax_error(
        "fun <reified ValueSpec> invalidSpec(valueSpec: ValueSpec): ValueSpec = valueSpec\n",
    );
}

#[test]
fn ks_declarations_0413_classifier_parameters_accept_in_out_and_invariant_forms() {
    assert_source_parses(
        "class ProducerSpec<out ValueSpec>\nclass ConsumerSpec<in ValueSpec>\nclass InvariantSpec<ValueSpec>\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0415: kmp-lsp does not diagnose direct covariant-position conflicts"]
fn ks_declarations_0415_covariant_parameter_rejects_explicit_input_positions() {
    assert_source_parses(
        "class ValidSpec<out ValueSpec>(val valueSpec: ValueSpec) { fun readSpec(): ValueSpec = valueSpec; }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidParameterSpec<out ValueSpec> { fun writeSpec(valueSpec: ValueSpec) {}; }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidPropertySpec<out ValueSpec>(var valueSpec: ValueSpec)\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0416: kmp-lsp does not diagnose direct contravariant-position conflicts"]
fn ks_declarations_0416_contravariant_parameter_rejects_explicit_output_positions() {
    assert_source_parses(
        "class ValidSpec<in ValueSpec> { fun writeSpec(valueSpec: ValueSpec) {}; }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidFunctionSpec<in ValueSpec> { fun readSpec(): ValueSpec = TODO(); }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidPropertySpec<in ValueSpec>(val valueSpec: ValueSpec)\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0417: kmp-lsp does not diagnose explicit invariant-position conflicts"]
fn ks_declarations_0417_variant_parameter_rejects_explicit_invariant_position() {
    assert_source_parses(
        "class ProducerSpec<out ValueSpec>\nclass ValidSpec<out ValueSpec> { fun readSpec(): ProducerSpec<ValueSpec> = TODO(); }\n",
    );
    assert_source_has_syntax_error(
        "class InvariantSpec<ValueSpec>\nclass InvalidSpec<out ValueSpec> { fun consumeSpec(valueSpec: InvariantSpec<ValueSpec>) {}; }\n",
    );
}

#[test]
fn ks_declarations_0418_private_member_may_lift_variance_conflict() {
    assert_source_parses(
        "class HostSpec<out ValueSpec>(private var valueSpec: ValueSpec) { private fun replaceSpec(newValueSpec: ValueSpec) { valueSpec = newValueSpec }; }\n",
    );
}

#[test]
fn ks_declarations_0420_extension_declaration_is_exempt_from_owner_variance_limit() {
    assert_source_parses(
        "class HostSpec<out ValueSpec>\nfun <ValueSpec> HostSpec<ValueSpec>.consumeSpec(valueSpec: ValueSpec) {}\n",
    );
}

#[test]
fn ks_declarations_0421_unsafe_variance_annotation_lifts_position_restriction() {
    assert_source_parses(
        "class HostSpec<out ValueSpec> { fun consumeSpec(valueSpec: @UnsafeVariance ValueSpec) {}; }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0436: kmp-lsp does not enforce private-to-this access"]
fn ks_declarations_0436_private_variance_conflict_is_private_to_this() {
    assert_source_parses(
        "class ValidSpec<out ValueSpec>(private var valueSpec: ValueSpec) { fun updateSpec(newValueSpec: @UnsafeVariance ValueSpec) { this.valueSpec = newValueSpec }; }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec<out ValueSpec>(private var valueSpec: ValueSpec) { fun copySpec(otherSpec: InvalidSpec<@UnsafeVariance ValueSpec>) { this.valueSpec = otherSpec.valueSpec }; }\n",
    );
}

#[test]
fn ks_declarations_0423_inline_function_and_property_parameters_may_be_reified() {
    assert_source_parses(
        "inline fun <reified ValueSpec> functionSpec(valueSpec: ValueSpec): ValueSpec = valueSpec\ninline val <reified ValueSpec> ValueSpec.propertySpec: ValueSpec get() = this\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0424: kmp-lsp does not diagnose runtime type checks with non-reified parameters"]
fn ks_declarations_0424_only_reified_parameter_is_runtime_available_for_type_check() {
    assert_source_parses(
        "inline fun <reified ValueSpec> validSpec(valueSpec: Any?): Boolean = valueSpec is ValueSpec\n",
    );
    assert_source_has_syntax_error(
        "fun <ValueSpec> invalidSpec(valueSpec: Any?): Boolean = valueSpec is ValueSpec\n",
    );
}

#[test]
fn ks_declarations_0427_underscore_type_argument_defers_selected_argument_inference() {
    assert_source_parses(
        "fun <FirstSpec, SecondSpec> pairSpec(firstSpec: FirstSpec, secondSpec: SecondSpec): Pair<FirstSpec, SecondSpec> = Pair(firstSpec, secondSpec)\nval resultSpec = pairSpec<String, _>(\"value\", 1)\n",
    );
}

#[test]
fn ks_declarations_0431_declarations_accept_default_and_explicit_visibility_modifiers() {
    let source = "val defaultPublicSpec = 1\npublic val explicitPublicSpec = 2\nprivate val privateSpec = 3\ninternal val internalSpec = 4\nopen class BaseSpec { protected val protectedSpec = 5; }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/VisibilityModifiers.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let symbols = indexer.file_symbols(&specification_uri);
    for declaration_name in [
        "defaultPublicSpec",
        "explicitPublicSpec",
        "privateSpec",
        "internalSpec",
        "protectedSpec",
    ] {
        assert!(symbols.iter().any(|symbol| symbol.name == declaration_name));
    }
}

#[test]
fn ks_declarations_0432_default_and_explicit_public_declarations_are_cross_file_accessible() {
    let declaration_uri = Url::parse("file:///kotlin-spec/public/Declarations.kt")
        .expect("declaration URI must be valid");
    let use_uri = Url::parse("file:///kotlin-spec/public/Usage.kt").expect("use URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package visibility\nval defaultPublicSpec = 1\npublic val explicitPublicSpec = 2\n",
    );
    indexer.index_content(
        &use_uri,
        "package visibility\nval firstUseSpec = defaultPublicSpec\nval secondUseSpec = explicitPublicSpec\n",
    );
    for symbol_name in ["defaultPublicSpec", "explicitPublicSpec"] {
        let locations = resolve_symbol(&indexer, symbol_name, None, &use_uri);
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].uri, declaration_uri);
    }
}

#[test]
#[ignore = "KS-DECLARATIONS-0435: kmp-lsp resolves private top-level declarations across files"]
fn ks_declarations_0435_private_top_level_declaration_is_file_scoped() {
    let declaration_uri = Url::parse("file:///kotlin-spec/private/Declarations.kt")
        .expect("declaration URI must be valid");
    let use_uri =
        Url::parse("file:///kotlin-spec/private/Usage.kt").expect("use URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package visibility\nprivate val privateSpec = 1\nval sameFileSpec = privateSpec\n",
    );
    indexer.index_content(
        &use_uri,
        "package visibility\nval otherFileSpec = privateSpec\n",
    );
    let same_file_locations = resolve_symbol(&indexer, "privateSpec", None, &declaration_uri);
    assert_eq!(same_file_locations.len(), 1);
    assert_eq!(same_file_locations[0].uri, declaration_uri);
    let other_file_locations = resolve_symbol(&indexer, "privateSpec", None, &use_uri);
    assert!(other_file_locations.is_empty());
}

#[tokio::test]
#[ignore = "KS-DECLARATIONS-0434: kmp-lsp resolves private members outside their owner scope"]
async fn ks_declarations_0434_private_member_is_accessible_only_in_its_declaration_scope() {
    let source = "class HostSpec {\n    private val secretSpec = 1\n    fun readSpec(): Int = secretSpec\n}\nval invalidSpec = HostSpec().secretSpec\n";
    let valid_locations = definition_locations(source, "secretSpec", 1).await;
    assert_eq!(valid_locations.len(), 1);
    assert_eq!(valid_locations[0].range.start.line, 1);
    let invalid_locations = definition_locations(source, "secretSpec", 2).await;
    assert!(invalid_locations.is_empty());
}

#[test]
fn ks_declarations_0437_internal_declaration_is_public_inside_same_module() {
    let declaration_uri = Url::parse("file:///kotlin-spec/module-a/source/Declarations.kt")
        .expect("declaration URI must be valid");
    let use_uri =
        Url::parse("file:///kotlin-spec/module-a/test/Usage.kt").expect("use URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package first\ninternal val internalSpec = 1\n",
    );
    indexer.index_content(
        &use_uri,
        "package second\nimport first.internalSpec\nval useSpec = internalSpec\n",
    );
    let locations = resolve_symbol(&indexer, "internalSpec", None, &use_uri);
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, declaration_uri);
}

#[test]
#[ignore = "KS-DECLARATIONS-0438: kmp-lsp does not model cross-module internal visibility"]
fn ks_declarations_0438_internal_declaration_is_private_outside_module() {
    let declaration_uri = Url::parse("file:///kotlin-spec/module-a/source/Declarations.kt")
        .expect("declaration URI must be valid");
    let use_uri =
        Url::parse("file:///kotlin-spec/module-b/source/Usage.kt").expect("use URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &declaration_uri,
        "package first\ninternal val internalSpec = 1\n",
    );
    indexer.index_content(
        &use_uri,
        "package second\nimport first.internalSpec\nval useSpec = internalSpec\n",
    );
    assert!(
        resolve_symbol(&indexer, "internalSpec", None, &use_uri).is_empty(),
        "module-b must not resolve module-a internal declaration"
    );
}

#[tokio::test]
#[ignore = "KS-DECLARATIONS-0439: kmp-lsp resolves protected members from unrelated classes"]
async fn ks_declarations_0439_protected_member_is_visible_to_owner_and_subtypes_only() {
    let source = "open class BaseSpec {\n    protected val protectedSpec = 1\n    fun ownerSpec(): Int = protectedSpec\n}\nclass DerivedSpec : BaseSpec() { fun inheritedSpec(): Int = protectedSpec; }\nclass OtherSpec { fun invalidSpec(baseSpec: BaseSpec): Int = baseSpec.protectedSpec; }\n";
    for valid_occurrence in [1, 2] {
        let locations = definition_locations(source, "protectedSpec", valid_occurrence).await;
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].range.start.line, 1);
    }
    let invalid_locations = definition_locations(source, "protectedSpec", 3).await;
    assert!(invalid_locations.is_empty());
}

#[test]
fn ks_declarations_0442_published_api_internal_declaration_is_available_to_public_inline_code() {
    assert_source_parses(
        "class HostSpec<ValueSpec>(@PublishedApi internal val valueSpec: ValueSpec) { inline fun readSpec(): ValueSpec = valueSpec; }\n",
    );
}

#[test]
#[ignore = "KS-DECLARATIONS-0441: kmp-lsp does not diagnose public inline access to stronger visibility"]
fn ks_declarations_0441_public_inline_declaration_cannot_access_stronger_visibility() {
    assert_source_parses(
        "class ValidSpec<ValueSpec>(@PublishedApi internal val valueSpec: ValueSpec) { inline fun readSpec(): ValueSpec = valueSpec; }\n",
    );
    assert_source_has_syntax_error(
        "class PrivateSpec<ValueSpec>(private val valueSpec: ValueSpec) { inline fun readSpec(): ValueSpec = valueSpec; }\n",
    );
    assert_source_has_syntax_error(
        "class InternalSpec<ValueSpec>(internal val valueSpec: ValueSpec) { inline fun readSpec(): ValueSpec = valueSpec; }\n",
    );
}
