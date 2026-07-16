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
