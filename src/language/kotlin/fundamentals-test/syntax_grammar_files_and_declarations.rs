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
fn ks_syntax_0187_kotlin_file_orders_headers_imports_with_top_level_objects() {
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
fn ks_syntax_0188_script_accepts_statements_after_headers() {
    let tree = super::parse_kotlin_source(KOTLIN_SCRIPT_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert_eq!(tree.root_node().kind(), crate::queries::KIND_SOURCE_FILE);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CALL_EXPR) > 0);
}

#[test]
fn ks_syntax_0189_shebang_line_precedes_file_contents() {
    assert_source_contains_node_kind(KOTLIN_SCRIPT_FIXTURE, crate::queries::KIND_PROP_DECL);
}

#[test]
fn ks_syntax_0190_file_annotation_precedes_package_header() {
    let tree = super::parse_kotlin_source(KOTLIN_FILE_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PACKAGE_HEADER),
        1
    );
}

#[test]
fn ks_syntax_0191_package_header_accepts_dotted_identifier() {
    assert_source_contains_node_kind(
        "package sample.feature.ui\nclass Screen\n",
        crate::queries::KIND_PACKAGE_HEADER,
    );
}

#[test]
fn ks_syntax_0192_import_list_accepts_multiple_import_headers() {
    let tree = super::parse_kotlin_source(KOTLIN_FILE_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_IMPORT_HEADER),
        2
    );
}

#[test]
fn ks_syntax_0193_import_header_accepts_dotted_path() {
    assert_source_contains_node_kind(
        "package sample.feature\nimport sample.library.Widget\nclass Screen\n",
        crate::queries::KIND_IMPORT_HEADER,
    );
}

#[test]
fn ks_syntax_0194_import_alias_follows_import_path() {
    assert_source_contains_node_kind(
        "package sample.feature\nimport sample.library.Renderer as ViewRenderer\nclass Screen\n",
        crate::queries::KIND_IMPORT_ALIAS,
    );
}

#[test]
fn ks_syntax_0195_top_level_object_accepts_each_declaration_family() {
    let tree = super::parse_kotlin_source(KOTLIN_FILE_FIXTURE);

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_TYPE_ALIAS) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_DECL) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_OBJECT_DECL) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_FUN_DECL) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL) > 0);
}

#[test]
fn ks_syntax_0196_type_alias_has_name_type_parameters_with_target_type() {
    assert_source_contains_node_kind(
        "typealias NamedItems<Element> = List<Pair<String, Element>>\n",
        crate::queries::KIND_TYPE_ALIAS,
    );
}

#[test]
fn ks_syntax_0197_declaration_accepts_classifier_function_with_property_forms() {
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
fn ks_syntax_0198_class_declaration_accepts_class_with_interface_forms() {
    let source = "class Screen\ninterface Renderer\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_DECL),
        2
    );
}

#[test]
fn ks_syntax_0199_primary_constructor_accepts_modifiers_with_parameters() {
    assert_source_contains_node_kind(
        "class ScreenModel internal constructor(val title: String, enabled: Boolean)\n",
        crate::queries::KIND_PRIMARY_CTOR,
    );
}

#[test]
fn ks_syntax_0200_class_body_contains_member_declarations() {
    assert_source_contains_node_kind(
        "class Screen {\nval title = \"neutral\"\nfun render() = title\n}\n",
        crate::queries::KIND_CLASS_BODY,
    );
}

#[test]
fn ks_syntax_0201_class_parameters_allow_defaults_with_trailing_comma() {
    let source = "class Screen(\nval title: String,\nenabled: Boolean = true,\n)\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_PARAM),
        2
    );
}

#[test]
fn ks_syntax_0202_class_parameter_allows_modifiers_property_with_default() {
    assert_source_contains_node_kind(
        "class Screen(private val title: String = \"neutral\")\n",
        crate::queries::KIND_CLASS_PARAM,
    );
}

#[test]
fn ks_syntax_0203_delegation_specifiers_allow_comma_separated_supertypes() {
    let source = "open class Base\ninterface Renderer\nclass Screen : Base(), Renderer\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_DELEGATION_SPEC),
        2
    );
}

#[test]
#[ignore = "KS-SYNTAX-0204: tree-sitter-kotlin rejects a function type used directly as a supertype"]
fn ks_syntax_0204_delegation_specifier_accepts_each_supertype_form() {
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
fn ks_syntax_0205_constructor_invocation_combines_user_type_with_arguments() {
    assert_source_contains_node_kind(
        "open class Base(val count: Int)\nclass Screen : Base(2)\n",
        crate::queries::KIND_CONSTRUCTOR_INVOCATION,
    );
}

#[test]
#[ignore = "KS-SYNTAX-0206: tree-sitter-kotlin rejects an annotation before a delegation specifier"]
fn ks_syntax_0206_annotated_delegation_specifier_precedes_supertype() {
    let source = "annotation class Marker\nopen class Base {}\nclass Screen : @Marker Base()\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_ANNOTATION) > 0);
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_DELEGATION_SPEC) > 0);
}

#[test]
fn ks_syntax_0207_explicit_delegation_uses_by_expression() {
    assert_source_contains_node_kind(
        "interface Renderer\nclass Screen(delegate: Renderer) : Renderer by delegate\n",
        crate::queries::KIND_EXPLICIT_DELEGATION,
    );
}

#[test]
#[ignore = "KS-SYNTAX-0208: tree-sitter-kotlin rejects the specification's trailing comma in type parameters"]
fn ks_syntax_0208_type_parameters_allow_multiple_parameters_with_trailing_comma() {
    let source = "class Mapping<\nout Key,\nValue,\n>\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_TYPE_PARAM),
        2
    );
}

#[test]
fn ks_syntax_0209_type_parameter_allows_modifiers_with_upper_bound() {
    assert_source_contains_node_kind(
        "class Items<out Element : CharSequence>\n",
        crate::queries::KIND_TYPE_PARAM,
    );
}

#[test]
fn ks_syntax_0210_type_constraints_allow_comma_separated_where_clause() {
    assert_source_parses(
        "fun <Element> render(value: Element) where Element : CharSequence, Element : Comparable<Element> = value.toString()\n",
    );
}

#[test]
fn ks_syntax_0211_type_constraint_allows_annotation_name_with_bound() {
    assert_source_parses(
        "annotation class Marker\nfun <Element> render(value: Element) where @Marker Element : CharSequence = value.toString()\n",
    );
}

#[test]
fn ks_syntax_0212_class_member_declarations_accept_repeated_members_with_semicolons() {
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
fn ks_syntax_0213_class_member_declaration_accepts_all_member_families() {
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
fn ks_syntax_0214_anonymous_initializer_combines_init_with_block() {
    let source = "class Screen {\ninit { require(true) }\n}\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert!(count_nodes_of_kind(&tree, crate::queries::KIND_CLASS_BODY) > 0);
}

#[test]
fn ks_syntax_0215_companion_object_accepts_name_supertypes_with_body() {
    assert_source_contains_node_kind(
        "interface Factory {}\nclass Screen {\ncompanion object Named : Factory {}\n}\n",
        crate::queries::KIND_COMPANION_OBJ,
    );
}

#[test]
fn ks_syntax_0216_function_value_parameters_allow_defaults_with_trailing_comma() {
    let source = "fun render(\ntitle: String,\nenabled: Boolean = true,\n) = title\n";
    let tree = super::parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PARAMETER),
        2
    );
}

#[test]
fn ks_syntax_0217_function_value_parameter_accepts_modifiers_with_default() {
    assert_source_parses(
        "fun render(vararg labels: String, callback: () -> Unit = {}) = callback()\n",
    );
}

#[test]
fn ks_syntax_0218_function_declaration_combines_generics_receiver_constraints_with_body() {
    assert_source_contains_node_kind(
        "suspend fun <Element> List<Element>.render(limit: Int): String where Element : CharSequence = first().take(limit).toString()\n",
        crate::queries::KIND_FUN_DECL,
    );
}

#[test]
fn ks_syntax_0219_function_body_accepts_block_with_expression_forms() {
    for declaration in [
        "fun blockBody(): Int { return 1 }\n",
        "fun expressionBody(): Int = 1\n",
    ] {
        assert_source_contains_node_kind(declaration, crate::queries::KIND_FUN_BODY);
    }
}

#[test]
#[ignore = "KS-SYNTAX-0220: tree-sitter-kotlin rejects an annotation before a variable name"]
fn ks_syntax_0220_variable_declaration_accepts_annotations_name_with_type() {
    assert_source_contains_node_kind(
        "annotation class Marker\nval @Marker title: String = \"neutral\"\n",
        crate::queries::KIND_VAR_DECL,
    );
}

#[test]
#[ignore = "KS-SYNTAX-0221: tree-sitter-kotlin treats a trailing destructuring comma as a missing variable"]
fn ks_syntax_0221_multi_variable_declaration_allows_trailing_comma() {
    assert_source_contains_node_kind(
        "data class Pairing(val first: Int, val second: String)\nval (count, title,) = Pairing(1, \"neutral\")\n",
        crate::queries::KIND_MULTI_VAR_DECL,
    );
}

#[test]
fn ks_syntax_0222_property_declaration_accepts_receiver_initializer_with_accessors() {
    assert_source_contains_node_kind(
        "var String.displayName: String\nget() = this\nset(value) { require(value.isNotEmpty()) }\n",
        crate::queries::KIND_PROP_DECL,
    );
}

#[test]
fn ks_syntax_0223_property_delegate_uses_by_expression() {
    assert_source_contains_node_kind(
        "class Holder<out Value>(value: Value) {\noperator fun getValue(owner: Any?, property: Any?) = value\n}\nval title by Holder(\"neutral\")\n",
        crate::queries::KIND_PROP_DELEGATE,
    );
}

#[test]
fn ks_syntax_0224_getter_accepts_return_type_with_function_body() {
    assert_source_parses("val title: String get(): String = \"neutral\"\n");
}

#[test]
#[ignore = "KS-SYNTAX-0225: tree-sitter-kotlin rejects a setter combining a trailing parameter comma with an explicit return type"]
fn ks_syntax_0225_setter_accepts_parameter_trailing_comma_return_type_with_body() {
    assert_source_parses(
        "var title: String = \"neutral\"\nset(value: String,): Unit { field = value }\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0226: tree-sitter-kotlin rejects an untyped anonymous-function parameter"]
fn ks_syntax_0226_parameters_with_optional_type_allow_untyped_parameters_with_trailing_comma() {
    assert_source_parses(
        "val callback = fun(\ntitle: String,\ncount,\n) { println(title + count) }\n",
    );
}

#[test]
fn ks_syntax_0227_function_value_parameter_with_optional_type_accepts_default() {
    assert_source_parses("val callback = fun(title: String = \"neutral\") { println(title) }\n");
}

#[test]
#[ignore = "KS-SYNTAX-0228: tree-sitter-kotlin rejects a parameter whose optional type is omitted"]
fn ks_syntax_0228_parameter_with_optional_type_may_omit_type() {
    assert_source_parses("val callback = fun(value) { println(value) }\n");
}

#[test]
fn ks_syntax_0229_parameter_requires_name_colon_with_type() {
    assert_source_contains_node_kind(
        "fun render(title: String) = title\n",
        crate::queries::KIND_PARAMETER,
    );
}

#[test]
fn ks_syntax_0230_object_declaration_accepts_modifiers_supertypes_with_body() {
    assert_source_contains_node_kind(
        "interface Renderer {}\ninternal object ScreenRenderer : Renderer {\nval title = \"neutral\"\n}\n",
        crate::queries::KIND_OBJECT_DECL,
    );
}

#[test]
fn ks_syntax_0231_secondary_constructor_accepts_modifiers_delegation_with_block() {
    assert_source_contains_node_kind(
        "open class Base(val title: String)\nclass Screen : Base {\nprivate constructor() : super(\"neutral\") {\nprintln(\"created\")\n}\n}\n",
        crate::queries::KIND_SECONDARY_CTOR,
    );
}

#[test]
fn ks_syntax_0232_constructor_delegation_call_accepts_this_with_super() {
    for declaration in [
        "class Screen(val title: String) {\nconstructor() : this(\"neutral\")\n}\n",
        "open class Base(val title: String)\nclass Screen : Base {\nconstructor() : super(\"neutral\")\n}\n",
    ] {
        assert_source_contains_node_kind(declaration, crate::queries::KIND_SECONDARY_CTOR);
    }
}

#[test]
fn ks_syntax_0233_enum_class_body_accepts_entries_semicolon_with_members() {
    assert_source_contains_node_kind(
        "enum class ScreenState {\nLoading, Content,;\nfun isReady() = this == Content\n}\n",
        crate::queries::KIND_ENUM_CLASS_BODY,
    );
}
