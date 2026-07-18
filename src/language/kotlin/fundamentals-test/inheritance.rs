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
fn ks_inheritance_0001_class_has_one_superclass_and_multiple_interface_base_types() {
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
#[ignore = "KS-INHERITANCE-0002: kmp-lsp does not diagnose multiple class supertypes"]
fn ks_inheritance_0002_class_cannot_inherit_multiple_class_types() {
    assert_source_parses(
        "open class BaseSpec\ninterface ContractSpec\nclass ValidSpec : BaseSpec(), ContractSpec\n",
    );
    assert_source_has_syntax_error(
        "open class FirstBaseSpec\nopen class SecondBaseSpec\nclass InvalidSpec : FirstBaseSpec(), SecondBaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0004: kmp-lsp does not diagnose inheritance from closed classes"]
fn ks_inheritance_0004_closed_class_cannot_be_inherited() {
    assert_source_parses("open class OpenSpec\nabstract class AbstractSpec\nclass FirstSpec : OpenSpec()\nclass SecondSpec : AbstractSpec()\n");
    assert_source_has_syntax_error("class ClosedSpec\nclass InvalidSpec : ClosedSpec()\n");
}

#[test]
#[ignore = "KS-INHERITANCE-0005: kmp-lsp does not validate openness of data, enum, and annotation classes"]
fn ks_inheritance_0005_data_enum_and_annotation_classes_are_always_closed() {
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
#[ignore = "KS-INHERITANCE-0015: kmp-lsp does not diagnose exclusive sealed and abstract modifiers"]
fn ks_inheritance_0015_sealed_class_is_implicitly_abstract_and_modifiers_are_exclusive() {
    assert_source_parses("sealed class SealedSpec\nclass DerivedSpec : SealedSpec()\n");
    assert_source_has_syntax_error("sealed abstract class InvalidSpec\n");
}

#[test]
#[ignore = "KS-INHERITANCE-0007: kmp-lsp does not diagnose class supertypes on interfaces"]
fn ks_inheritance_0007_interface_inherits_any_number_of_interfaces_only() {
    assert_source_parses(
        "interface FirstSpec\ninterface SecondSpec\ninterface DerivedSpec : FirstSpec, SecondSpec\n",
    );
    assert_source_has_syntax_error("open class BaseSpec\ninterface InvalidSpec : BaseSpec\n");
}

#[test]
#[ignore = "KS-INHERITANCE-0008: kmp-lsp does not diagnose inheritance from object types"]
fn ks_inheritance_0008_object_type_cannot_be_inherited() {
    assert_source_parses("object RegistrySpec\n");
    assert_source_has_syntax_error("object RegistrySpec\nclass InvalidSpec : RegistrySpec()\n");
}

#[test]
#[ignore = "KS-INHERITANCE-0006: kmp-lsp does not diagnose inheritance from data, enum, or annotation types"]
fn ks_inheritance_0006_data_enum_and_annotation_types_cannot_be_inherited() {
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
#[ignore = "KS-INHERITANCE-0010: kmp-lsp does not diagnose direct abstract-class construction"]
fn ks_inheritance_0010_abstract_class_cannot_be_instantiated_directly() {
    assert_source_parses(
        "abstract class BaseSpec\nclass DerivedSpec : BaseSpec()\nval validSpec = DerivedSpec()\n",
    );
    assert_source_has_syntax_error("abstract class BaseSpec\nval invalidSpec = BaseSpec()\n");
}

#[test]
fn ks_inheritance_0011_abstract_class_is_implicitly_open() {
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
fn ks_inheritance_0012_abstract_class_accepts_abstract_properties_and_functions() {
    assert_source_parses(
        "abstract class BaseSpec { abstract val valueSpec: Int; abstract fun renderSpec(): String; }\n",
    );
}

#[test]
fn ks_inheritance_0013_class_and_interface_may_be_sealed() {
    assert_source_parses(
        "sealed class SealedClassSpec\nsealed interface SealedInterfaceSpec\nclass ClassLeafSpec : SealedClassSpec()\nclass InterfaceLeafSpec : SealedInterfaceSpec\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0014: tree-sitter-kotlin cannot parse the baseline fun interface form"]
fn ks_inheritance_0014_functional_interface_cannot_be_sealed() {
    assert_source_parses("fun interface ValidSpec { fun invokeSpec(): String; }\n");
    assert_source_has_syntax_error(
        "sealed fun interface InvalidSpec { fun invokeSpec(): String; }\n",
    );
}

#[test]
fn ks_inheritance_0016_sealed_type_accepts_same_package_and_module_subtype() {
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
#[ignore = "KS-INHERITANCE-0017: kmp-lsp does not enforce sealed subtype package boundaries"]
fn ks_inheritance_0017_sealed_type_rejects_different_package_subtype() {
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
#[ignore = "KS-INHERITANCE-0018: kmp-lsp does not enforce sealed subtype module boundaries"]
fn ks_inheritance_0018_sealed_type_rejects_different_module_subtype() {
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
#[ignore = "KS-INHERITANCE-0019: kmp-lsp does not diagnose local or anonymous sealed subtypes"]
fn ks_inheritance_0019_sealed_type_rejects_local_and_anonymous_subtypes() {
    assert_source_parses("sealed class SealedSpec\nclass TopLevelSpec : SealedSpec()\n");
    assert_source_has_syntax_error(
        "sealed class SealedSpec\nfun invalidLocalSpec() { class LocalSpec : SealedSpec() }\n",
    );
    assert_source_has_syntax_error(
        "sealed class SealedSpec\nval anonymousSpec = object : SealedSpec() {}\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0023: kmp-lsp does not diagnose inheritance from closed built-in types"]
fn ks_inheritance_0023_closed_builtin_class_types_cannot_be_inherited() {
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
fn ks_inheritance_0024_function_type_is_inheritable_as_interface() {
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
async fn ks_inheritance_0025_matching_callable_requires_same_name() {
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
#[ignore = "KS-INHERITANCE-0026: kmp-lsp implementation matching does not support property overrides by kind"]
async fn ks_inheritance_0026_matching_callable_requires_same_declaration_kind() {
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
#[ignore = "KS-INHERITANCE-0027: kmp-lsp implementation matching ignores overloaded function signatures"]
async fn ks_inheritance_0027_matching_functions_require_matching_signatures() {
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
async fn ks_inheritance_0029_derived_matching_declaration_subsumes_base_declaration() {
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
#[ignore = "KS-INHERITANCE-0031: kmp-lsp resolves private callables as inherited members"]
fn ks_inheritance_0031_private_callable_is_not_inherited() {
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
fn ks_inheritance_0034_unopposed_inheritable_callable_is_inherited() {
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
fn ks_inheritance_0035_superclass_concrete_callable_suppresses_interface_abstract_match() {
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
#[ignore = "KS-INHERITANCE-0036: kmp-lsp does not diagnose multiple inherited concrete implementations"]
fn ks_inheritance_0036_multiple_inherited_concrete_matches_require_override() {
    assert_source_parses(
        "interface FirstSpec { fun renderSpec(): String = \"first\"; }\ninterface SecondSpec { fun renderSpec(): String = \"second\"; }\nclass ValidSpec : FirstSpec, SecondSpec { override fun renderSpec(): String = super<FirstSpec>.renderSpec(); }\n",
    );
    assert_source_has_syntax_error(
        "interface FirstSpec { fun renderSpec(): String = \"first\"; }\ninterface SecondSpec { fun renderSpec(): String = \"second\"; }\nclass InvalidSpec : FirstSpec, SecondSpec\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0037: kmp-lsp does not diagnose missing abstract implementations"]
fn ks_inheritance_0037_concrete_classifier_must_implement_inherited_abstract_callable() {
    assert_source_parses(
        "abstract class BaseSpec { abstract fun renderSpec(): String; }\nclass ValidSpec : BaseSpec() { override fun renderSpec(): String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "abstract class BaseSpec { abstract fun renderSpec(): String; }\nclass InvalidSpec : BaseSpec()\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0038: kmp-lsp does not diagnose mixed abstract and concrete interface inheritance"]
fn ks_inheritance_0038_abstract_and_concrete_interface_matches_require_override() {
    assert_source_parses(
        "interface AbstractSpec { fun renderSpec(): String; }\ninterface ConcreteSpec { fun renderSpec(): String = \"concrete\"; }\nclass ValidSpec : AbstractSpec, ConcreteSpec { override fun renderSpec(): String = super<ConcreteSpec>.renderSpec(); }\n",
    );
    assert_source_has_syntax_error(
        "interface AbstractSpec { fun renderSpec(): String; }\ninterface ConcreteSpec { fun renderSpec(): String = \"concrete\"; }\nclass InvalidSpec : AbstractSpec, ConcreteSpec\n",
    );
}

#[tokio::test]
async fn ks_inheritance_0039_interface_callables_are_implicitly_abstract_or_open() {
    let base_uri =
        Url::parse("file:///kotlin-spec/override/ContractSpec.kt").expect("base URI must be valid");
    let derived_uri = Url::parse("file:///kotlin-spec/override/ImplementationSpec.kt")
        .expect("derived URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &base_uri,
        "package overridecontract\ninterface ContractSpec {\n    fun abstractSpec(): String\n    fun defaultSpec(): String = \"default\"\n}\n",
    );
    indexer.index_content(
        &derived_uri,
        "package overridecontract\nclass ImplementationSpec : ContractSpec {\n    override fun abstractSpec(): String = \"abstract\"\n    override fun defaultSpec(): String = \"override\"\n}\n",
    );
    for (symbol_name, declaration_line, implementation_line) in
        [("abstractSpec", 2, 2), ("defaultSpec", 3, 3)]
    {
        let locations =
            implementation_locations(&indexer, symbol_name, &base_uri, declaration_line).await;
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].uri, derived_uri);
        assert_eq!(locations[0].range.start.line, implementation_line);
    }
}

#[test]
#[ignore = "KS-INHERITANCE-0041: kmp-lsp does not diagnose private overridable callables"]
fn ks_inheritance_0041_private_callable_cannot_be_open_abstract_or_override() {
    assert_source_parses("class ValidSpec { private fun hiddenSpec() {}; }\n");
    for invalid_source in [
        "open class OpenHostSpec { private open fun invalidSpec() {}; }\n",
        "abstract class AbstractHostSpec { private abstract fun invalidSpec(); }\n",
        "open class BaseSpec { open fun valueSpec() {}; }\nclass OverrideHostSpec : BaseSpec() { private override fun valueSpec() {}; }\n",
    ] {
        assert_source_has_syntax_error(invalid_source);
    }
}

#[tokio::test]
async fn ks_inheritance_0042_override_modifier_marks_subsuming_derived_callable() {
    let base_uri =
        Url::parse("file:///kotlin-spec/override/BaseSpec.kt").expect("base URI must be valid");
    let derived_uri = Url::parse("file:///kotlin-spec/override/DerivedSpec.kt")
        .expect("derived URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(
        &base_uri,
        "package overridecontract\nopen class BaseSpec {\n    open fun renderSpec(valueSpec: Int): String = valueSpec.toString()\n}\n",
    );
    indexer.index_content(
        &derived_uri,
        "package overridecontract\nclass DerivedSpec : BaseSpec() {\n    override fun renderSpec(valueSpec: Int): String = \"derived\"\n}\n",
    );
    let locations = implementation_locations(&indexer, "renderSpec", &base_uri, 2).await;
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].uri, derived_uri);
    assert_eq!(locations[0].range.start.line, 2);
}

#[test]
#[ignore = "KS-INHERITANCE-0043: kmp-lsp does not validate overriding function return covariance"]
fn ks_inheritance_0043_overriding_function_return_type_must_be_subtype() {
    assert_source_parses(
        "open class BaseSpec { open fun valueSpec(): Any = 1; }\nclass ValidSpec : BaseSpec() { override fun valueSpec(): String = \"value\"; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { open fun valueSpec(): String = \"value\"; }\nclass InvalidSpec : BaseSpec() { override fun valueSpec(): Any = 1; }\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0044: kmp-lsp does not validate override suspendability"]
fn ks_inheritance_0044_overriding_function_suspendability_must_match() {
    assert_source_parses(
        "open class BaseSpec { open suspend fun loadSpec(): String = \"base\"; }\nclass ValidSpec : BaseSpec() { override suspend fun loadSpec(): String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { open suspend fun loadSpec(): String = \"base\"; }\nclass InvalidSpec : BaseSpec() { override fun loadSpec(): String = \"invalid\"; }\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0045: kmp-lsp does not validate overriding property mutability"]
fn ks_inheritance_0045_overriding_property_mutability_cannot_be_stronger() {
    assert_source_parses(
        "open class BaseSpec { open val valueSpec: String = \"base\"; }\nclass ValidSpec : BaseSpec() { override var valueSpec: String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { open var valueSpec: String = \"base\"; }\nclass InvalidSpec : BaseSpec() { override val valueSpec: String = \"invalid\"; }\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0046: kmp-lsp does not validate read-only override type covariance"]
fn ks_inheritance_0046_read_only_override_property_type_may_be_covariant() {
    assert_source_parses(
        "open class BaseSpec { open val valueSpec: Any = 1; }\nclass ValidSpec : BaseSpec() { override val valueSpec: String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { open val valueSpec: String = \"base\"; }\nclass InvalidSpec : BaseSpec() { override val valueSpec: Any = 1; }\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0047: kmp-lsp does not validate mutable override type equivalence"]
fn ks_inheritance_0047_mutable_override_property_type_must_be_equivalent() {
    assert_source_parses(
        "open class BaseSpec { open var valueSpec: String = \"base\"; }\nclass ValidSpec : BaseSpec() { override var valueSpec: String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { open var valueSpec: Any = 1; }\nclass InvalidSpec : BaseSpec() { override var valueSpec: String = \"invalid\"; }\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0048: kmp-lsp does not diagnose overrides of non-overridable bases"]
fn ks_inheritance_0048_non_overridable_base_callable_cannot_be_overridden() {
    assert_source_parses(
        "open class BaseSpec { open fun renderSpec(): String = \"base\"; }\nclass ValidSpec : BaseSpec() { override fun renderSpec(): String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { fun renderSpec(): String = \"base\"; }\nclass InvalidSpec : BaseSpec() { override fun renderSpec(): String = \"invalid\"; }\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0049: kmp-lsp does not require the override modifier"]
fn ks_inheritance_0049_overriding_callable_requires_override_modifier() {
    assert_source_parses(
        "open class BaseSpec { open fun renderSpec(): String = \"base\"; }\nclass ValidSpec : BaseSpec() { override fun renderSpec(): String = \"valid\"; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { open fun renderSpec(): String = \"base\"; }\nclass InvalidSpec : BaseSpec() { fun renderSpec(): String = \"invalid\"; }\n",
    );
}

#[test]
#[ignore = "KS-INHERITANCE-0051: kmp-lsp does not validate explicit override visibility"]
fn ks_inheritance_0051_explicit_override_visibility_cannot_be_stronger() {
    assert_source_parses(
        "open class BaseSpec { protected open fun renderSpec() {}; }\nclass ValidSpec : BaseSpec() { public override fun renderSpec() {}; }\n",
    );
    assert_source_has_syntax_error(
        "open class BaseSpec { public open fun renderSpec() {}; }\nclass InvalidSpec : BaseSpec() { protected override fun renderSpec() {}; }\n",
    );
}

#[test]
fn ks_inheritance_0054_same_name_non_subsuming_function_is_overload_not_override() {
    let source = "open class BaseSpec { open fun renderSpec(valueSpec: Int): String = valueSpec.toString(); }\nclass DerivedSpec : BaseSpec() { fun renderSpec(valueSpec: String): String = valueSpec; }\n";
    assert_source_parses(source);
    let specification_uri = Url::parse("file:///kotlin-spec/OverloadNotOverride.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let render_symbols: Vec<_> = indexer
        .file_symbols(&specification_uri)
        .into_iter()
        .filter(|symbol| symbol.name == "renderSpec")
        .collect();
    assert_eq!(render_symbols.len(), 2);
    assert!(render_symbols
        .iter()
        .any(|symbol| symbol.container.as_deref() == Some("BaseSpec")));
    assert!(render_symbols
        .iter()
        .any(|symbol| symbol.container.as_deref() == Some("DerivedSpec")));
}
