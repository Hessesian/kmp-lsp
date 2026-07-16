use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::features::implementation::find_implementation;
use crate::indexer::Indexer;
use crate::resolver::resolve_symbol;
use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Url};

async fn implementation_locations(
    indexer: &Indexer,
    symbol_name: &str,
    declaration_uri: &Url,
    declaration_line: u32,
) -> Vec<Location> {
    match find_implementation(symbol_name, indexer, declaration_uri, declaration_line).await {
        Some(GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(GotoDefinitionResponse::Array(locations)) => locations,
        Some(GotoDefinitionResponse::Link(_)) => {
            panic!("kmp-lsp implementation feature returns locations, not location links")
        }
        None => Vec::new(),
    }
}

#[test]
fn ks_5_1_001_class_has_one_superclass_and_multiple_interface_base_types() {
    let source = "open class BaseSpec\ninterface FirstSpec\ninterface SecondSpec\nclass DerivedSpec : BaseSpec(), FirstSpec, SecondSpec\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/Inheritance.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    for base_name in ["BaseSpec", "FirstSpec", "SecondSpec"] {
        let subtypes = indexer.subtypes_of(base_name);
        assert_eq!(subtypes.len(), 1);
        assert_eq!(subtypes[0].uri, specification_uri);
        assert_eq!(subtypes[0].range.start.line, 3);
    }
}

#[test]
#[ignore = "KS-5.1-002: kmp-lsp does not diagnose multiple class supertypes"]
fn ks_5_1_002_class_cannot_inherit_multiple_class_types() {
    assert_source_parses(
        "open class BaseSpec\ninterface ContractSpec\nclass ValidSpec : BaseSpec(), ContractSpec\n",
    );
    assert_source_has_syntax_error(
        "open class FirstBaseSpec\nopen class SecondBaseSpec\nclass InvalidSpec : FirstBaseSpec(), SecondBaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-5.1-004: kmp-lsp does not diagnose inheritance from closed classes"]
fn ks_5_1_004_closed_class_cannot_be_inherited() {
    assert_source_parses("open class OpenSpec\nabstract class AbstractSpec\nclass FirstSpec : OpenSpec()\nclass SecondSpec : AbstractSpec()\n");
    assert_source_has_syntax_error("class ClosedSpec\nclass InvalidSpec : ClosedSpec()\n");
}

#[test]
#[ignore = "KS-5.1-005: kmp-lsp does not validate openness of data, enum, and annotation classes"]
fn ks_5_1_005_data_enum_and_annotation_classes_are_always_closed() {
    assert_source_parses(
        "data class DataSpec(val valueSpec: Int)\nenum class EnumSpec { READY }\nannotation class AnnotationSpec\n",
    );
    for invalid_source in [
        "open data class OpenDataSpec(val valueSpec: Int)\n",
        "abstract data class AbstractDataSpec(val valueSpec: Int)\n",
        "open enum class OpenEnumSpec { READY }\n",
        "abstract enum class AbstractEnumSpec { READY }\n",
        "open annotation class OpenAnnotationSpec\n",
        "abstract annotation class AbstractAnnotationSpec\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
#[ignore = "KS-5.1-006: kmp-lsp does not diagnose exclusive sealed and abstract modifiers"]
fn ks_5_1_006_sealed_class_is_implicitly_abstract_and_modifiers_are_exclusive() {
    assert_source_parses("sealed class SealedSpec\nclass DerivedSpec : SealedSpec()\n");
    assert_source_has_syntax_error("sealed abstract class InvalidSpec\n");
}

#[test]
#[ignore = "KS-5.1-007: kmp-lsp does not diagnose class supertypes on interfaces"]
fn ks_5_1_007_interface_inherits_any_number_of_interfaces_only() {
    assert_source_parses(
        "interface FirstSpec\ninterface SecondSpec\ninterface DerivedSpec : FirstSpec, SecondSpec\n",
    );
    assert_source_has_syntax_error("open class BaseSpec\ninterface InvalidSpec : BaseSpec\n");
}

#[test]
#[ignore = "KS-5.1-008: kmp-lsp does not diagnose inheritance from object types"]
fn ks_5_1_008_object_type_cannot_be_inherited() {
    assert_source_parses("object RegistrySpec\n");
    assert_source_has_syntax_error("object RegistrySpec\nclass InvalidSpec : RegistrySpec()\n");
}

#[test]
#[ignore = "KS-5.1-009: kmp-lsp does not diagnose inheritance from data, enum, or annotation types"]
fn ks_5_1_009_data_enum_and_annotation_types_cannot_be_inherited() {
    assert_source_parses(
        "data class DataSpec(val valueSpec: Int)\nenum class EnumSpec { READY }\nannotation class AnnotationSpec\n",
    );
    assert_source_has_syntax_error(
        "data class DataSpec(val valueSpec: Int)\nclass InvalidDataSpec : DataSpec(1)\n",
    );
    assert_source_has_syntax_error(
        "enum class EnumSpec { READY }\nclass InvalidEnumSpec : EnumSpec()\n",
    );
    assert_source_has_syntax_error(
        "annotation class AnnotationSpec\nclass InvalidAnnotationSpec : AnnotationSpec\n",
    );
}

#[test]
#[ignore = "KS-5.1.1-001: kmp-lsp does not diagnose direct abstract-class construction"]
fn ks_5_1_1_001_abstract_class_cannot_be_instantiated_directly() {
    assert_source_parses(
        "abstract class BaseSpec\nclass DerivedSpec : BaseSpec()\nval validSpec = DerivedSpec()\n",
    );
    assert_source_has_syntax_error("abstract class BaseSpec\nval invalidSpec = BaseSpec()\n");
}

#[test]
fn ks_5_1_1_002_abstract_class_is_implicitly_open() {
    let source = "abstract class BaseSpec\nclass DerivedSpec : BaseSpec()\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/AbstractInheritance.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let subtypes = indexer.subtypes_of("BaseSpec");
    assert_eq!(subtypes.len(), 1);
    assert_eq!(subtypes[0].uri, specification_uri);
    assert_eq!(subtypes[0].range.start.line, 1);
}

#[test]
fn ks_5_1_1_003_abstract_class_accepts_abstract_properties_and_functions() {
    assert_source_parses(
        "abstract class BaseSpec { abstract val valueSpec: Int; abstract fun renderSpec(): String; }\n",
    );
}

#[test]
fn ks_5_1_2_001_class_and_interface_may_be_sealed() {
    assert_source_parses(
        "sealed class SealedClassSpec\nsealed interface SealedInterfaceSpec\nclass ClassLeafSpec : SealedClassSpec()\nclass InterfaceLeafSpec : SealedInterfaceSpec\n",
    );
}

#[test]
#[ignore = "KS-5.1.2-002: tree-sitter-kotlin cannot parse the baseline fun interface form"]
fn ks_5_1_2_002_functional_interface_cannot_be_sealed() {
    assert_source_parses("fun interface ValidSpec { fun invokeSpec(): String; }\n");
    assert_source_has_syntax_error(
        "sealed fun interface InvalidSpec { fun invokeSpec(): String; }\n",
    );
}

#[test]
fn ks_5_1_2_003_sealed_type_accepts_same_package_and_module_subtype() {
    let base_uri = Url::parse("file:///kotlin-spec/module-a/base/SealedSpec.kt")
        .expect("base URI must be valid");
    let subtype_uri = Url::parse("file:///kotlin-spec/module-a/leaf/LeafSpec.kt")
        .expect("subtype URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&base_uri, "package states\nsealed class SealedSpec\n");
    indexer.index_content(
        &subtype_uri,
        "package states\nclass LeafSpec : SealedSpec()\n",
    );
    let subtypes = indexer.subtypes_of("SealedSpec");
    assert_eq!(subtypes.len(), 1);
    assert_eq!(subtypes[0].uri, subtype_uri);
}

#[test]
#[ignore = "KS-5.1.2-004: kmp-lsp does not enforce sealed subtype package boundaries"]
fn ks_5_1_2_004_sealed_type_rejects_different_package_subtype() {
    let base_uri = Url::parse("file:///kotlin-spec/module-a/base/SealedSpec.kt")
        .expect("base URI must be valid");
    let subtype_uri = Url::parse("file:///kotlin-spec/module-a/leaf/LeafSpec.kt")
        .expect("subtype URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&base_uri, "package first\nsealed class SealedSpec\n");
    indexer.index_content(
        &subtype_uri,
        "package second\nimport first.SealedSpec\nclass LeafSpec : SealedSpec()\n",
    );
    assert!(indexer.subtypes_of("SealedSpec").is_empty());
}

#[test]
#[ignore = "KS-5.1.2-005: kmp-lsp does not enforce sealed subtype module boundaries"]
fn ks_5_1_2_005_sealed_type_rejects_different_module_subtype() {
    let base_uri = Url::parse("file:///kotlin-spec/module-a/source/SealedSpec.kt")
        .expect("base URI must be valid");
    let subtype_uri = Url::parse("file:///kotlin-spec/module-b/source/LeafSpec.kt")
        .expect("subtype URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&base_uri, "package states\nsealed class SealedSpec\n");
    indexer.index_content(
        &subtype_uri,
        "package states\nclass LeafSpec : SealedSpec()\n",
    );
    assert!(indexer.subtypes_of("SealedSpec").is_empty());
}

#[test]
#[ignore = "KS-5.1.2-006: kmp-lsp does not diagnose local or anonymous sealed subtypes"]
fn ks_5_1_2_006_sealed_type_rejects_local_and_anonymous_subtypes() {
    assert_source_parses("sealed class SealedSpec\nclass TopLevelSpec : SealedSpec()\n");
    assert_source_has_syntax_error(
        "sealed class SealedSpec\nfun invalidLocalSpec() { class LocalSpec : SealedSpec() }\n",
    );
    assert_source_has_syntax_error(
        "sealed class SealedSpec\nval anonymousSpec = object : SealedSpec() {}\n",
    );
}

#[test]
#[ignore = "KS-5.1.3-001: kmp-lsp does not diagnose inheritance from closed built-in types"]
fn ks_5_1_3_001_closed_builtin_class_types_cannot_be_inherited() {
    assert_source_parses("class ValidSpec\n");
    for invalid_source in [
        "class StringSpec : String()\n",
        "class IntSpec : Int()\n",
        "class BooleanSpec : Boolean()\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[test]
fn ks_5_1_3_002_function_type_is_inheritable_as_interface() {
    let source = "class HandlerSpec : (Int) -> String { override fun invoke(valueSpec: Int): String = valueSpec.toString(); }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/FunctionTypeInheritance.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    assert!(indexer
        .file_symbols(&specification_uri)
        .iter()
        .any(|symbol| symbol.name == "HandlerSpec"));
}

#[tokio::test]
async fn ks_5_2_001_matching_callable_requires_same_name() {
    let base_uri =
        Url::parse("file:///kotlin-spec/matching/BaseSpec.kt").expect("base URI must be valid");
    let derived_uri = Url::parse("file:///kotlin-spec/matching/DerivedSpec.kt")
        .expect("derived URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &base_uri,
        "package matching\ninterface BaseSpec {\n    fun renderSpec(): String\n    fun competingSpec(): String\n}\n",
    );
    indexer.index_content(
        &derived_uri,
        "package matching\nclass DerivedSpec : BaseSpec {\n    override fun renderSpec(): String = \"rendered\"\n    override fun competingSpec(): String = \"other\"\n}\n",
    );
    let locations = implementation_locations(&indexer, "renderSpec", &base_uri, 2).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, derived_uri);
}

#[tokio::test]
#[ignore = "KS-5.2-002: kmp-lsp implementation matching does not support property overrides by kind"]
async fn ks_5_2_002_matching_callable_requires_same_declaration_kind() {
    let base_uri = Url::parse("file:///kotlin-spec/matching/BasePropertySpec.kt")
        .expect("base URI must be valid");
    let derived_uri = Url::parse("file:///kotlin-spec/matching/DerivedPropertySpec.kt")
        .expect("derived URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &base_uri,
        "package matching\ninterface BaseSpec { val stateSpec: Int; }\n",
    );
    indexer.index_content(
        &derived_uri,
        "package matching\nclass DerivedSpec : BaseSpec { override val stateSpec: Int = 1; override fun stateSpec(): Int = 2; }\n",
    );
    let locations = implementation_locations(&indexer, "stateSpec", &base_uri, 1).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, derived_uri);
    assert_eq!(locations[0].range.start.character, 48);
}

#[tokio::test]
#[ignore = "KS-5.2-003: kmp-lsp implementation matching ignores overloaded function signatures"]
async fn ks_5_2_003_matching_functions_require_matching_signatures() {
    let base_uri = Url::parse("file:///kotlin-spec/matching/BaseOverloadSpec.kt")
        .expect("base URI must be valid");
    let derived_uri = Url::parse("file:///kotlin-spec/matching/DerivedOverloadSpec.kt")
        .expect("derived URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &base_uri,
        "package matching\ninterface BaseSpec {\n    fun selectSpec(valueSpec: Int): String\n    fun selectSpec(valueSpec: String): String\n}\n",
    );
    indexer.index_content(
        &derived_uri,
        "package matching\nclass DerivedSpec : BaseSpec {\n    override fun selectSpec(valueSpec: Int): String = \"int\"\n    override fun selectSpec(valueSpec: String): String = \"string\"\n}\n",
    );
    let locations = implementation_locations(&indexer, "selectSpec", &base_uri, 2).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, derived_uri);
    assert_eq!(locations[0].range.start.line, 2);
}

#[tokio::test]
async fn ks_5_2_004_derived_matching_declaration_subsumes_base_declaration() {
    let base_uri =
        Url::parse("file:///kotlin-spec/subsumption/BaseSpec.kt").expect("base URI must be valid");
    let derived_uri = Url::parse("file:///kotlin-spec/subsumption/DerivedSpec.kt")
        .expect("derived URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &base_uri,
        "package subsumption\nopen class BaseSpec {\n    open fun renderSpec(valueSpec: Int): String = valueSpec.toString()\n}\n",
    );
    indexer.index_content(
        &derived_uri,
        "package subsumption\nclass DerivedSpec : BaseSpec() {\n    override fun renderSpec(valueSpec: Int): String = \"derived\"\n}\n",
    );
    let locations = implementation_locations(&indexer, "renderSpec", &base_uri, 2).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, derived_uri);
}

#[test]
#[ignore = "KS-5.3-001: kmp-lsp resolves private callables as inherited members"]
fn ks_5_3_001_private_callable_is_not_inherited() {
    let specification_uri = Url::parse("file:///kotlin-spec/InheritedPrivate.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "open class BaseSpec { private fun hiddenSpec(): String = \"hidden\"; }\nclass DerivedSpec : BaseSpec()\n",
    );
    assert!(resolve_symbol(
        &indexer,
        "hiddenSpec",
        Some("DerivedSpec"),
        &specification_uri
    )
    .is_empty());
}

#[test]
fn ks_5_3_002_unopposed_inheritable_callable_is_inherited() {
    let specification_uri = Url::parse("file:///kotlin-spec/InheritedCallable.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "open class BaseSpec { open fun inheritedSpec(): String = \"base\"; }\nclass DerivedSpec : BaseSpec()\n",
    );
    let locations = resolve_symbol(
        &indexer,
        "inheritedSpec",
        Some("DerivedSpec"),
        &specification_uri,
    );
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 0);
}

#[test]
fn ks_5_3_003_superclass_concrete_callable_suppresses_interface_abstract_match() {
    let specification_uri = Url::parse("file:///kotlin-spec/SuperclassDominance.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &specification_uri,
        "open class BaseSpec { open fun renderSpec(): String = \"base\"; }\ninterface ContractSpec { fun renderSpec(): String; }\nclass DerivedSpec : BaseSpec(), ContractSpec\n",
    );
    let locations = resolve_symbol(
        &indexer,
        "renderSpec",
        Some("DerivedSpec"),
        &specification_uri,
    );
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].range.start.line, 0);
}

#[test]
#[ignore = "KS-5.3-004: kmp-lsp does not diagnose multiple inherited concrete implementations"]
fn ks_5_3_004_multiple_inherited_concrete_matches_require_override() {
    assert_source_parses(
        "interface FirstSpec { fun renderSpec(): String = \"first\"; }\ninterface SecondSpec { fun renderSpec(): String = \"second\"; }\nclass ValidSpec : FirstSpec, SecondSpec { override fun renderSpec(): String = super<FirstSpec>.renderSpec(); }\n",
    );
    assert_source_has_syntax_error(
        "interface FirstSpec { fun renderSpec(): String = \"first\"; }\ninterface SecondSpec { fun renderSpec(): String = \"second\"; }\nclass InvalidSpec : FirstSpec, SecondSpec\n",
    );
}

#[test]
#[ignore = "KS-5.3-005: kmp-lsp does not diagnose missing abstract implementations"]
fn ks_5_3_005_concrete_classifier_must_implement_inherited_abstract_callable() {
    assert_source_parses(
        "abstract class BaseSpec { abstract fun renderSpec(): String; }\nclass ValidSpec : BaseSpec() { override fun renderSpec(): String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "abstract class BaseSpec { abstract fun renderSpec(): String; }\nclass InvalidSpec : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-5.3-006: kmp-lsp does not diagnose mixed abstract and concrete interface inheritance"]
fn ks_5_3_006_abstract_and_concrete_interface_matches_require_override() {
    assert_source_parses(
        "interface AbstractSpec { fun renderSpec(): String; }\ninterface ConcreteSpec { fun renderSpec(): String = \"concrete\"; }\nclass ValidSpec : AbstractSpec, ConcreteSpec { override fun renderSpec(): String = super<ConcreteSpec>.renderSpec(); }\n",
    );
    assert_source_has_syntax_error(
        "interface AbstractSpec { fun renderSpec(): String; }\ninterface ConcreteSpec { fun renderSpec(): String = \"concrete\"; }\nclass InvalidSpec : AbstractSpec, ConcreteSpec\n",
    );
}
