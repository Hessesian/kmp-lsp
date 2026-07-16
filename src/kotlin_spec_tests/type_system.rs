use super::{assert_source_has_syntax_error, assert_source_parses};

#[test]
fn ks_2_1_2_001_classifier_types_have_simple_and_parameterized_forms() {
    assert_source_parses(
        "class Simple\nclass Box<Element>\ninterface Contract\nobject Singleton\n",
    );
}

#[test]
fn ks_2_1_2_002_simple_classifier_has_name_and_optional_supertypes() {
    assert_source_parses(
        "interface First\ninterface Second\ninterface Derived : First, Second\nclass Plain\n",
    );
}

#[test]
fn ks_2_1_2_003_classifier_supertypes_must_be_non_nullable() {
    assert_source_parses("interface Base\ninterface Derived : Base\n");
    assert_source_has_syntax_error("interface Base\ninterface Invalid : Base?\n");
}

#[test]
fn ks_2_1_2_005_type_constructor_has_name_parameters_and_supertypes() {
    assert_source_parses("interface Base\ninterface Generic<First, Second> : Base\n");
}

#[test]
#[ignore = "KS-2.1.2-007: kmp-lsp does not diagnose an uninstantiated generic supertype"]
fn ks_2_1_2_007_parameterized_supertype_requires_type_arguments() {
    assert_source_parses("interface Generic<Element>\ninterface Concrete : Generic<String>\n");
    assert_source_has_syntax_error("interface Generic<Element>\ninterface Invalid : Generic\n");
}
