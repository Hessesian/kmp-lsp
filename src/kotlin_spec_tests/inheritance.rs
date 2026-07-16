use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::Url;

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
