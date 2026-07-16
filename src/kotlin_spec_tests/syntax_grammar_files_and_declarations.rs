use super::{assert_source_contains_node_kind, assert_source_parses, count_nodes_of_kind};

const KOTLIN_FILE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/fixtures/chapter_01/file_structure.kt"
));
const KOTLIN_SCRIPT_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/kotlin_spec/fixtures/chapter_01/script_structure.kts"
));

#[test]
fn ks_1_3_001_kotlin_file_orders_headers_imports_and_top_level_objects() {
    let tree = super::parse_kotlin_source(KOTLIN_FILE_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert_eq!(tree.root_node().kind(), crate::queries::KIND_SOURCE_FILE);
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PACKAGE_HEADER),
        1
    );
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_IMPORT_HEADER),
        2
    );
}

#[test]
fn ks_1_3_002_script_accepts_statements_after_headers() {
    let tree = super::parse_kotlin_source(KOTLIN_SCRIPT_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert_eq!(tree.root_node().kind(), crate::queries::KIND_SOURCE_FILE);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CALL_EXPR) > 0);
}

#[test]
fn ks_1_3_003_shebang_line_precedes_file_contents() {
    assert_source_contains_node_kind(KOTLIN_SCRIPT_FIXTURE, crate::queries::KIND_PROP_DECL);
}

#[test]
fn ks_1_3_004_file_annotation_precedes_package_header() {
    let tree = super::parse_kotlin_source(KOTLIN_FILE_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PACKAGE_HEADER),
        1
    );
}

#[test]
fn ks_1_3_005_package_header_accepts_dotted_identifier() {
    assert_source_contains_node_kind(
        "package sample.feature.ui\nclass Screen\n",
        crate::queries::KIND_PACKAGE_HEADER,
    );
}

#[test]
fn ks_1_3_006_import_list_accepts_multiple_import_headers() {
    let tree = super::parse_kotlin_source(KOTLIN_FILE_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_IMPORT_HEADER),
        2
    );
}

#[test]
fn ks_1_3_007_import_header_accepts_dotted_path() {
    assert_source_contains_node_kind(
        "package sample.feature\nimport sample.library.Widget\nclass Screen\n",
        crate::queries::KIND_IMPORT_HEADER,
    );
}

#[test]
fn ks_1_3_008_import_alias_follows_import_path() {
    assert_source_contains_node_kind(
        "package sample.feature\nimport sample.library.Renderer as ViewRenderer\nclass Screen\n",
        crate::queries::KIND_IMPORT_ALIAS,
    );
}

#[test]
fn ks_1_3_009_top_level_object_accepts_each_declaration_family() {
    let tree = super::parse_kotlin_source(KOTLIN_FILE_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_TYPE_ALIAS) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_DECL) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_OBJECT_DECL) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_FUN_DECL) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL) > 0);
}

#[test]
fn ks_1_3_010_type_alias_has_name_type_parameters_and_target_type() {
    assert_source_contains_node_kind(
        "typealias NamedItems<Element> = List<Pair<String, Element>>\n",
        crate::queries::KIND_TYPE_ALIAS,
    );
}

#[test]
fn ks_1_3_011_declaration_accepts_classifier_function_and_property_forms() {
    let source = "class Screen\nobject Registry\nfun render() = Unit\nval enabled = true\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_DECL),
        1
    );
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_OBJECT_DECL),
        1
    );
    assert_eq!(count_nodes_of_kind(&tree, crate::queries::KIND_FUN_DECL), 1);
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        1
    );
}

#[test]
fn ks_1_3_012_class_declaration_accepts_class_and_interface_forms() {
    let source = "class Screen\ninterface Renderer\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_DECL),
        2
    );
}

#[test]
fn ks_1_3_013_primary_constructor_accepts_modifiers_and_parameters() {
    assert_source_contains_node_kind(
        "class ScreenModel internal constructor(val title: String, enabled: Boolean)\n",
        crate::queries::KIND_PRIMARY_CTOR,
    );
}

#[test]
fn ks_1_3_014_class_body_contains_member_declarations() {
    assert_source_contains_node_kind(
        "class Screen {\nval title = \"neutral\"\nfun render() = title\n}\n",
        crate::queries::KIND_CLASS_BODY,
    );
}

#[test]
fn ks_1_3_015_class_parameters_allow_defaults_and_trailing_comma() {
    let source = "class Screen(\nval title: String,\nenabled: Boolean = true,\n)\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_PARAM),
        2
    );
}

#[test]
fn ks_1_3_016_class_parameter_allows_modifiers_property_and_default() {
    assert_source_contains_node_kind(
        "class Screen(private val title: String = \"neutral\")\n",
        crate::queries::KIND_CLASS_PARAM,
    );
}

#[test]
fn ks_1_3_017_delegation_specifiers_allow_comma_separated_supertypes() {
    let source = "open class Base\ninterface Renderer\nclass Screen : Base(), Renderer\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_DELEGATION_SPEC),
        2
    );
}

#[test]
#[ignore = "KS-1.3-018: tree-sitter-kotlin rejects a function type used directly as a supertype"]
fn ks_1_3_018_delegation_specifier_accepts_each_supertype_form() {
    for declaration in [
        "open class Base {}\nclass Screen : Base()\n",
        "interface Renderer {}\nclass Screen(delegate: Renderer) : Renderer by delegate\n",
        "interface Renderer {}\nclass Screen : Renderer\n",
        "interface Callback : () -> Unit\n",
        "interface AsyncCallback : suspend () -> Unit\n",
    ] {
        assert_source_contains_node_kind(declaration, crate::queries::KIND_DELEGATION_SPEC);
    }
}

#[test]
fn ks_1_3_019_constructor_invocation_combines_user_type_and_arguments() {
    assert_source_contains_node_kind(
        "open class Base(val count: Int)\nclass Screen : Base(2)\n",
        crate::queries::KIND_CONSTRUCTOR_INVOCATION,
    );
}

#[test]
#[ignore = "KS-1.3-020: tree-sitter-kotlin rejects an annotation before a delegation specifier"]
fn ks_1_3_020_annotated_delegation_specifier_precedes_supertype() {
    let source = "annotation class Marker\nopen class Base {}\nclass Screen : @Marker Base()\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_ANNOTATION) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_DELEGATION_SPEC) > 0);
}

#[test]
fn ks_1_3_021_explicit_delegation_uses_by_expression() {
    assert_source_contains_node_kind(
        "interface Renderer\nclass Screen(delegate: Renderer) : Renderer by delegate\n",
        crate::queries::KIND_EXPLICIT_DELEGATION,
    );
}

#[test]
#[ignore = "KS-1.3-022: tree-sitter-kotlin rejects the specification's trailing comma in type parameters"]
fn ks_1_3_022_type_parameters_allow_multiple_parameters_and_trailing_comma() {
    let source = "class Mapping<\nout Key,\nValue,\n>\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_TYPE_PARAM),
        2
    );
}

#[test]
fn ks_1_3_023_type_parameter_allows_modifiers_and_upper_bound() {
    assert_source_contains_node_kind(
        "class Items<out Element : CharSequence>\n",
        crate::queries::KIND_TYPE_PARAM,
    );
}

#[test]
fn ks_1_3_024_type_constraints_allow_comma_separated_where_clause() {
    assert_source_parses(
        "fun <Element> render(value: Element) where Element : CharSequence, Element : Comparable<Element> = value.toString()\n",
    );
}

#[test]
fn ks_1_3_025_type_constraint_allows_annotation_name_and_bound() {
    assert_source_parses(
        "annotation class Marker\nfun <Element> render(value: Element) where @Marker Element : CharSequence = value.toString()\n",
    );
}

#[test]
fn ks_1_3_026_class_member_declarations_accept_repeated_members_and_semicolons() {
    let source = "class Screen {\nval title = \"neutral\";\nfun render() = title\n}\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        1
    );
    assert_eq!(count_nodes_of_kind(&tree, crate::queries::KIND_FUN_DECL), 1);
}

#[test]
fn ks_1_3_027_class_member_declaration_accepts_all_member_families() {
    let source = r#"
class Screen private constructor() {
    val title = "neutral"
    companion object Named
    init { require(title.isNotEmpty()) }
    private constructor(title: String) : this()
}
"#;
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_COMPANION_OBJ) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_SECONDARY_CTOR) > 0);
}

#[test]
fn ks_1_3_028_anonymous_initializer_combines_init_and_block() {
    let source = "class Screen {\ninit { require(true) }\n}\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_BODY) > 0);
}

#[test]
fn ks_1_3_029_companion_object_accepts_name_supertypes_and_body() {
    assert_source_contains_node_kind(
        "interface Factory {}\nclass Screen {\ncompanion object Named : Factory {}\n}\n",
        crate::queries::KIND_COMPANION_OBJ,
    );
}

#[test]
fn ks_1_3_030_function_value_parameters_allow_defaults_and_trailing_comma() {
    let source = "fun render(\ntitle: String,\nenabled: Boolean = true,\n) = title\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PARAMETER),
        2
    );
}

#[test]
fn ks_1_3_031_function_value_parameter_accepts_modifiers_and_default() {
    assert_source_parses(
        "fun render(vararg labels: String, callback: () -> Unit = {}) = callback()\n",
    );
}

#[test]
fn ks_1_3_032_function_declaration_combines_generics_receiver_constraints_and_body() {
    assert_source_contains_node_kind(
        "suspend fun <Element> List<Element>.render(limit: Int): String where Element : CharSequence = first().take(limit).toString()\n",
        crate::queries::KIND_FUN_DECL,
    );
}

#[test]
fn ks_1_3_033_function_body_accepts_block_and_expression_forms() {
    for declaration in [
        "fun blockBody(): Int { return 1 }\n",
        "fun expressionBody(): Int = 1\n",
    ] {
        assert_source_contains_node_kind(declaration, crate::queries::KIND_FUN_BODY);
    }
}

#[test]
#[ignore = "KS-1.3-034: tree-sitter-kotlin rejects an annotation before a variable name"]
fn ks_1_3_034_variable_declaration_accepts_annotations_name_and_type() {
    assert_source_contains_node_kind(
        "annotation class Marker\nval @Marker title: String = \"neutral\"\n",
        crate::queries::KIND_VAR_DECL,
    );
}

#[test]
#[ignore = "KS-1.3-035: tree-sitter-kotlin treats a trailing destructuring comma as a missing variable"]
fn ks_1_3_035_multi_variable_declaration_allows_trailing_comma() {
    assert_source_contains_node_kind(
        "data class Pairing(val first: Int, val second: String)\nval (count, title,) = Pairing(1, \"neutral\")\n",
        crate::queries::KIND_MULTI_VAR_DECL,
    );
}

#[test]
fn ks_1_3_036_property_declaration_accepts_receiver_initializer_and_accessors() {
    assert_source_contains_node_kind(
        "var String.displayName: String\nget() = this\nset(value) { require(value.isNotEmpty()) }\n",
        crate::queries::KIND_PROP_DECL,
    );
}

#[test]
fn ks_1_3_037_property_delegate_uses_by_expression() {
    assert_source_contains_node_kind(
        "class Holder<out Value>(value: Value) {\noperator fun getValue(owner: Any?, property: Any?) = value\n}\nval title by Holder(\"neutral\")\n",
        crate::queries::KIND_PROP_DELEGATE,
    );
}

#[test]
fn ks_1_3_038_getter_accepts_return_type_and_function_body() {
    assert_source_parses("val title: String get(): String = \"neutral\"\n");
}

#[test]
#[ignore = "KS-1.3-039: tree-sitter-kotlin rejects a setter combining a trailing parameter comma with an explicit return type"]
fn ks_1_3_039_setter_accepts_parameter_trailing_comma_return_type_and_body() {
    assert_source_parses(
        "var title: String = \"neutral\"\nset(value: String,): Unit { field = value }\n",
    );
}

#[test]
#[ignore = "KS-1.3-040: tree-sitter-kotlin rejects an untyped anonymous-function parameter"]
fn ks_1_3_040_parameters_with_optional_type_allow_untyped_parameters_and_trailing_comma() {
    assert_source_parses(
        "val callback = fun(\ntitle: String,\ncount,\n) { println(title + count) }\n",
    );
}

#[test]
fn ks_1_3_041_function_value_parameter_with_optional_type_accepts_default() {
    assert_source_parses("val callback = fun(title: String = \"neutral\") { println(title) }\n");
}

#[test]
#[ignore = "KS-1.3-042: tree-sitter-kotlin rejects a parameter whose optional type is omitted"]
fn ks_1_3_042_parameter_with_optional_type_may_omit_type() {
    assert_source_parses("val callback = fun(value) { println(value) }\n");
}

#[test]
fn ks_1_3_043_parameter_requires_name_colon_and_type() {
    assert_source_contains_node_kind(
        "fun render(title: String) = title\n",
        crate::queries::KIND_PARAMETER,
    );
}

#[test]
fn ks_1_3_044_object_declaration_accepts_modifiers_supertypes_and_body() {
    assert_source_contains_node_kind(
        "interface Renderer {}\ninternal object ScreenRenderer : Renderer {\nval title = \"neutral\"\n}\n",
        crate::queries::KIND_OBJECT_DECL,
    );
}

#[test]
fn ks_1_3_045_secondary_constructor_accepts_modifiers_delegation_and_block() {
    assert_source_contains_node_kind(
        "open class Base(val title: String)\nclass Screen : Base {\nprivate constructor() : super(\"neutral\") {\nprintln(\"created\")\n}\n}\n",
        crate::queries::KIND_SECONDARY_CTOR,
    );
}

#[test]
fn ks_1_3_046_constructor_delegation_call_accepts_this_and_super() {
    for declaration in [
        "class Screen(val title: String) {\nconstructor() : this(\"neutral\")\n}\n",
        "open class Base(val title: String)\nclass Screen : Base {\nconstructor() : super(\"neutral\")\n}\n",
    ] {
        assert_source_contains_node_kind(declaration, crate::queries::KIND_SECONDARY_CTOR);
    }
}

#[test]
fn ks_1_3_047_enum_class_body_accepts_entries_semicolon_and_members() {
    assert_source_contains_node_kind(
        "enum class ScreenState {\nLoading, Content,;\nfun isReady() = this == Content\n}\n",
        crate::queries::KIND_ENUM_CLASS_BODY,
    );
}
