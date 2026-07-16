use super::{
    assert_source_contains_node_kind, assert_source_has_syntax_error, assert_source_parses,
};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::Indexer;
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
