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

#[test]
fn ks_2_1_3_004_bounded_type_parameter_accepts_multiple_upper_bounds() {
    assert_source_parses(
        "fun <Element> inspect(value: Element) where Element : CharSequence, Element : Comparable<Element> = value.length\n",
    );
}

#[test]
#[ignore = "KS-2.1.3-009: kmp-lsp does not diagnose variance on function type parameters"]
fn ks_2_1_3_009_function_type_parameters_cannot_declare_variance() {
    assert_source_has_syntax_error("fun <out Element> inspect(value: Element) = value\n");
}

#[test]
fn ks_2_1_3_012_declaration_site_variance_accepts_in_and_out() {
    assert_source_parses("interface Consumer<in Element>\ninterface Producer<out Element>\n");
}

#[test]
fn ks_2_1_3_013_use_site_variance_accepts_in_and_out_projections() {
    assert_source_parses(
        "fun inspect(input: List<out CharSequence>, output: Comparator<in String>) {}\n",
    );
}

#[test]
#[ignore = "KS-2.1.3-014: kmp-lsp does not diagnose use-site variance in a supertype argument"]
fn ks_2_1_3_014_supertype_top_level_argument_cannot_use_site_variance() {
    assert_source_parses("interface Box<Element>\ninterface Valid : Box<String>\n");
    assert_source_has_syntax_error("interface Box<Element>\ninterface Invalid : Box<out String>\n");
}
