use super::{
    assert_source_contains_node_kind, assert_source_has_syntax_error, assert_source_parses,
};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{SymbolKind, Url};

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
