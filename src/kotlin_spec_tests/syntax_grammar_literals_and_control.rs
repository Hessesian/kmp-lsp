use super::{assert_source_contains_node_kind, assert_source_parses};

#[test]
fn ks_1_3_111_string_literal_accepts_line_and_multiline_forms() {
    assert_source_parses("val line = \"status\"\nval multiline = \"\"\"status\"\"\"\n");
}

#[test]
fn ks_1_3_112_line_string_literal_accepts_content_and_expressions() {
    assert_source_parses(
        "fun render(name: String, count: Int) = \"Name: $name, count: ${count + 1}, newline: \\n\"\n",
    );
}

#[test]
fn ks_1_3_113_multiline_string_literal_accepts_content_expressions_and_quotes() {
    assert_source_parses(
        "fun render(name: String) = \"\"\"Name: $name; expression: ${name.length}; quote: \"\"\"\n",
    );
}

#[test]
fn ks_1_3_114_line_string_content_accepts_text_escape_and_reference() {
    assert_source_parses("fun render(name: String) = \"text \\t $name\"\n");
}

#[test]
fn ks_1_3_115_line_string_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun render(count: Int) = \"count=${\ncount + 1\n}\"\n");
}

#[test]
fn ks_1_3_116_multiline_string_content_accepts_text_quote_and_reference() {
    assert_source_parses("fun render(name: String) = \"\"\"text \" $name\"\"\"\n");
}

#[test]
fn ks_1_3_117_multiline_string_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun render(count: Int) = \"\"\"count=${\ncount + 1\n}\"\"\"\n");
}

#[test]
fn ks_1_3_118_lambda_literal_accepts_parameters_arrow_and_statements() {
    assert_source_contains_node_kind(
        "val transform: (Int) -> Int = { count ->\nval offset = 1\ncount + offset\n}\nval action = { println(\"done\") }\n",
        crate::queries::KIND_LAMBDA_LIT,
    );
}

#[test]
#[ignore = "KS-1.3-119: tree-sitter-kotlin rejects a trailing comma in lambda parameters"]
fn ks_1_3_119_lambda_parameters_accept_multiple_and_trailing_comma() {
    assert_source_parses("val combine = { first: Int, second: Int, -> first + second }\n");
}

#[test]
#[ignore = "KS-1.3-120: tree-sitter-kotlin rejects a typed destructuring lambda parameter"]
fn ks_1_3_120_lambda_parameter_accepts_variable_and_typed_destructuring() {
    assert_source_parses(
        "val single = { count: Int -> count }\nval pair = { (count, title): Pair<Int, String> -> title + count }\n",
    );
}

#[test]
#[ignore = "KS-1.3-121: tree-sitter-kotlin rejects a suspend anonymous receiver function with constraints"]
fn ks_1_3_121_anonymous_function_accepts_suspend_receiver_constraints_and_body() {
    assert_source_parses(
        "fun <Element> build() = suspend fun List<Element>.(value: Element): Element where Element : Any = value\n",
    );
}

#[test]
fn ks_1_3_122_function_literal_accepts_lambda_and_anonymous_function() {
    assert_source_parses(
        "val lambda = { count: Int -> count + 1 }\nval anonymous = fun(count: Int): Int { return count + 1 }\n",
    );
}

#[test]
#[ignore = "KS-1.3-123: tree-sitter-kotlin rejects data on an object literal"]
fn ks_1_3_123_object_literal_accepts_data_supertypes_and_body() {
    assert_source_parses(
        "interface Item { fun title(): String }\nval item = data object : Item { override fun title() = \"item\" }\n",
    );
}

#[test]
fn ks_1_3_124_this_expression_accepts_plain_and_labeled_forms() {
    assert_source_parses(
        "class Holder {\nfun inspect() {\nval plain = this\nwith(this) named@ { val labeled = this@named }\n}\n}\n",
    );
}

#[test]
fn ks_1_3_125_super_expression_accepts_type_and_label_qualifiers() {
    assert_source_parses(
        "interface Named {\nfun title(): String { return \"named\" }\n}\nopen class Base {\nopen fun title(): String { return \"base\" }\n}\nclass Child : Base(), Named {\noverride fun title(): String {\nval plain = super.title()\nreturn super<Base>.title() + super<Named>.title()\n}\n}\n",
    );
}

#[test]
fn ks_1_3_126_if_expression_accepts_body_else_and_empty_forms() {
    assert_source_parses(
        "fun inspect(flag: Boolean) {\nif (flag) println(1)\nif (flag) { println(2) } else println(3)\nif (flag);\n}\n",
    );
}

#[test]
fn ks_1_3_127_when_subject_accepts_expression_or_bound_variable() {
    assert_source_parses(
        "annotation class Marker\nfun inspect(value: Any) {\nwhen (value) { else -> println(value) }\nwhen (@Marker val subject = value) { else -> println(subject) }\n}\n",
    );
}

#[test]
fn ks_1_3_128_when_expression_accepts_optional_subject_and_entries() {
    assert_source_parses(
        "fun inspect(value: Int) {\nval subject = when (value) { 1 -> \"one\"; else -> \"other\" }\nval subjectless = when { value > 0 -> \"positive\"; else -> \"other\" }\n}\n",
    );
}

#[test]
#[ignore = "KS-1.3-129: tree-sitter-kotlin rejects a trailing comma in when conditions"]
fn ks_1_3_129_when_entry_accepts_conditions_trailing_comma_and_else() {
    assert_source_parses(
        "fun inspect(value: Int) = when (value) {\n1, 2, -> \"small\"\nelse -> \"other\"\n}\n",
    );
}

#[test]
fn ks_1_3_130_when_condition_accepts_expression_range_and_type_tests() {
    assert_source_parses(
        "fun inspect(value: Any) = when (value) {\n0 -> \"zero\";\nin 1..10 -> \"present\";\nis String -> \"text\";\nelse -> \"other\"\n}\n",
    );
}

#[test]
fn ks_1_3_131_range_test_accepts_positive_and_negative_membership() {
    assert_source_parses(
        "fun inspect(value: Int) = when (value) {\nin 1..10 -> \"inside\"\n!in 20..30 -> \"outside\"\nelse -> \"other\"\n}\n",
    );
}

#[test]
fn ks_1_3_132_type_test_accepts_positive_and_negative_checks() {
    assert_source_parses(
        "fun inspect(value: Any) = when (value) {\nis String -> \"text\"\n!is Number -> \"other\"\nelse -> \"number\"\n}\n",
    );
}

#[test]
fn ks_1_3_133_try_expression_accepts_catches_and_finally() {
    assert_source_parses(
        "fun inspect() {\ntry { println(1) } catch (failure: IllegalStateException) { println(failure) } catch (failure: RuntimeException) { println(failure) } finally { println(2) }\ntry { println(3) } finally { println(4) }\n}\n",
    );
}

#[test]
#[ignore = "KS-1.3-134: tree-sitter-kotlin rejects a trailing comma in a catch parameter"]
fn ks_1_3_134_catch_block_accepts_annotation_type_trailing_comma_and_block() {
    assert_source_parses(
        "annotation class Marker\nfun inspect() { try { println(1) } catch (@Marker failure: RuntimeException,) { println(failure) } }\n",
    );
}

#[test]
fn ks_1_3_135_finally_block_combines_keyword_and_block() {
    assert_source_parses(
        "fun inspect() { try { println(1) } finally { println(\"complete\") } }\n",
    );
}

#[test]
fn ks_1_3_136_jump_expression_accepts_throw_return_continue_and_break_forms() {
    assert_source_parses(
        "fun inspect(values: List<Int>): Int {\nvalues.forEach named@ { if (it < 0) return@named; if (it == 0) throw IllegalStateException() }\nouter@ for (value in values) { if (value == 1) continue@outer; if (value == 2) break@outer }\nreturn values.size\n}\n",
    );
}

#[test]
fn ks_1_3_137_callable_reference_accepts_receiver_name_and_class() {
    assert_source_parses(
        "class Item\nfun create() = Item()\nval constructor = ::Item\nval factory = ::create\nval length = String::length\nval type = String::class\n",
    );
}

#[test]
fn ks_1_3_138_assignment_and_operator_accepts_every_compound_operator() {
    assert_source_parses(
        "fun update() { var count = 10; count += 1; count -= 1; count *= 2; count /= 2; count %= 3 }\n",
    );
}

#[test]
fn ks_1_3_139_equality_operator_accepts_structural_and_referential_forms() {
    assert_source_parses(
        "fun compare(first: Any, second: Any) { val a = first == second; val b = first != second; val c = first === second; val d = first !== second }\n",
    );
}

#[test]
fn ks_1_3_140_comparison_operator_accepts_all_ordering_forms() {
    assert_source_parses(
        "fun compare(first: Int, second: Int) { val a = first < second; val b = first > second; val c = first <= second; val d = first >= second }\n",
    );
}

#[test]
fn ks_1_3_141_in_operator_accepts_positive_and_negative_forms() {
    assert_source_parses(
        "fun inspect(value: Int, values: List<Int>) { val present = value in values; val absent = value !in values }\n",
    );
}

#[test]
fn ks_1_3_142_is_operator_accepts_positive_and_negative_forms() {
    assert_source_parses(
        "fun inspect(value: Any) { val text = value is String; val other = value !is String }\n",
    );
}

#[test]
fn ks_1_3_143_additive_operator_accepts_plus_and_minus() {
    assert_source_parses("fun calculate(first: Int, second: Int) = first + second - 1\n");
}

#[test]
fn ks_1_3_144_multiplicative_operator_accepts_multiply_divide_and_remainder() {
    assert_source_parses("fun calculate(first: Int, second: Int) = first * second / 2 % 3\n");
}

#[test]
fn ks_1_3_145_as_operator_accepts_unsafe_and_safe_forms() {
    assert_source_parses(
        "fun inspect(value: Any) { val definite = value as String; val optional = value as? String }\n",
    );
}

#[test]
fn ks_1_3_146_prefix_unary_operator_accepts_increment_decrement_sign_and_excl() {
    assert_source_parses(
        "fun update() { var count = 0; val flag = false; ++count; --count; val negative = -count; val positive = +count; val inverse = !flag }\n",
    );
}

#[test]
fn ks_1_3_147_postfix_unary_operator_accepts_increment_decrement_and_not_null() {
    assert_source_parses(
        "fun update(value: String?) { var count = 0; count++; count--; val length = value!!.length }\n",
    );
}

#[test]
fn ks_1_3_148_excl_accepts_adjacent_or_whitespace_followed_forms() {
    assert_source_parses("fun inspect(first: Boolean, second: Boolean) = !first || ! second\n");
}

#[test]
fn ks_1_3_149_member_access_operator_accepts_dot_safe_navigation_and_reference() {
    assert_source_parses(
        "fun inspect(value: String?) { val direct = value\n?.length; val reference = String::length; val text = value\n.toString() }\n",
    );
}

#[test]
#[ignore = "KS-1.3-150: tree-sitter-kotlin accepts whitespace inside the safe-navigation token"]
fn ks_1_3_150_safe_nav_requires_no_whitespace_between_question_mark_and_dot() {
    assert_source_parses("fun inspect(value: String?) = value?.length\n");
    super::assert_source_has_syntax_error("fun inspect(value: String?) = value ? .length\n");
}

#[test]
fn ks_1_3_151_modifiers_accept_annotations_and_repeated_modifiers() {
    assert_source_parses(
        "annotation class Marker\n@Marker public open class Holder { @Marker protected open fun render() {}; }\n",
    );
}

#[test]
fn ks_1_3_152_parameter_modifiers_accept_annotation_and_parameter_modifiers() {
    assert_source_parses(
        "annotation class Marker\ninline fun inspect(@Marker crossinline action: () -> Unit, noinline fallback: () -> Unit, vararg values: Int) {}\n",
    );
}

#[test]
fn ks_1_3_153_modifier_accepts_every_modifier_family() {
    assert_source_parses(
        "annotation class Marker\npublic open class Holder { override fun toString() = \"holder\"; lateinit var title: String; }\ninline fun inspect(vararg values: Int) {}\nconst val count = 1\nexpect class Expected\nactual class Expected\n",
    );
}

#[test]
fn ks_1_3_154_type_modifiers_accept_repeated_type_modifiers() {
    assert_source_parses(
        "@Target(AnnotationTarget.TYPE) annotation class Marker\nval action: @Marker suspend () -> Unit = {}\n",
    );
}

#[test]
fn ks_1_3_155_type_modifier_accepts_annotation_or_suspend() {
    assert_source_parses(
        "@Target(AnnotationTarget.TYPE) annotation class Marker\nval annotated: @Marker () -> Unit = {}\nval suspended: suspend () -> Unit = {}\n",
    );
}

#[test]
fn ks_1_3_156_class_modifier_accepts_all_class_kinds() {
    assert_source_parses(
        "enum class Mode { FIRST }\nsealed class State\nannotation class Marker\ndata class Item(val count: Int)\nclass Outer { inner class Nested; }\nvalue class Identifier(val value: String)\n",
    );
}

#[test]
fn ks_1_3_157_member_modifier_accepts_override_and_lateinit() {
    assert_source_parses(
        "open class Base { open fun render() {}; }\nclass Child : Base() { override fun render() {}; lateinit var title: String; }\n",
    );
}

#[test]
fn ks_1_3_158_visibility_modifier_accepts_all_visibilities() {
    assert_source_parses(
        "public class PublicItem\nprivate class PrivateItem\ninternal class InternalItem\nopen class Base { protected fun inspect() {}; }\n",
    );
}

#[test]
fn ks_1_3_159_variance_modifier_accepts_in_and_out() {
    assert_source_parses("class Consumer<in Input>\nclass Producer<out Output>\n");
}

#[test]
fn ks_1_3_160_type_parameter_modifiers_accept_repeated_modifiers() {
    assert_source_parses(
        "annotation class Marker\ninline fun <@Marker reified out Element> inspect(value: Element) {}\n",
    );
}

#[test]
fn ks_1_3_161_type_parameter_modifier_accepts_reified_variance_or_annotation() {
    assert_source_parses(
        "annotation class Marker\ninline fun <reified Element> inspect(value: Element) {}\nclass Producer<out Element>\nclass Consumer<@Marker in Element>\n",
    );
}

#[test]
fn ks_1_3_162_function_modifier_accepts_every_function_modifier() {
    assert_source_parses(
        "tailrec fun repeat(count: Int): Int = if (count == 0) 0 else repeat(count - 1)\noperator fun Int.plus(other: String) = toString() + other\ninfix fun String.merge(other: String) = this + other\ninline fun apply(action: () -> Unit) = action()\nexternal fun nativeCall()\nsuspend fun load() {}\n",
    );
}

#[test]
fn ks_1_3_163_property_modifier_accepts_const() {
    assert_source_parses("const val DEFAULT_COUNT = 1\n");
}

#[test]
fn ks_1_3_164_inheritance_modifier_accepts_abstract_final_and_open() {
    assert_source_parses(
        "abstract class AbstractItem\nfinal class FinalItem\nopen class OpenItem\n",
    );
}

#[test]
fn ks_1_3_165_parameter_modifier_accepts_vararg_noinline_and_crossinline() {
    assert_source_parses(
        "inline fun inspect(vararg values: Int, noinline fallback: () -> Unit, crossinline action: () -> Unit) {}\n",
    );
}

#[test]
fn ks_1_3_166_reification_modifier_accepts_reified() {
    assert_source_parses("inline fun <reified Element> inspect(value: Element) {}\n");
}

#[test]
fn ks_1_3_167_platform_modifier_accepts_expect_and_actual() {
    assert_source_parses("expect class PlatformItem\nactual class PlatformItem\n");
}

#[test]
fn ks_1_3_168_annotation_accepts_single_or_multi_forms_and_newline() {
    assert_source_parses(
        "annotation class First\nannotation class Second\n@First\nclass Single\n@[First Second]\nclass Multiple\n",
    );
}

#[test]
fn ks_1_3_169_single_annotation_accepts_use_site_and_at_token_forms() {
    assert_source_parses(
        "annotation class Marker\nclass Holder(@param:Marker val value: String) { @get:Marker val title = value; }\n",
    );
}

#[test]
fn ks_1_3_170_multi_annotation_accepts_multiple_unescaped_annotations() {
    assert_source_parses(
        "annotation class First\nannotation class Second\n@[First Second]\nclass Holder\n",
    );
}

#[test]
fn ks_1_3_171_annotation_use_site_target_accepts_every_target() {
    assert_source_parses(
        "@file:Marker\nannotation class Marker\nclass Holder(@param:Marker @property:Marker @field:Marker val value: String) {\n@get:Marker @delegate:Marker val title by lazy { value }\n@set:Marker @setparam:Marker var count = 0\nfun @receiver:Marker String.render() = this\n}\n",
    );
}

#[test]
fn ks_1_3_172_unescaped_annotation_accepts_constructor_or_user_type() {
    assert_source_parses(
        "annotation class Named(val value: String)\nannotation class Marker\n@Named(\"holder\") @Marker class Holder\n",
    );
}

#[test]
#[ignore = "KS-1.3-173: tree-sitter-kotlin rejects dynamic as an unescaped simple identifier"]
fn ks_1_3_173_simple_identifier_accepts_identifier_and_soft_keywords() {
    assert_source_parses(
        "fun inspect() { val ordinary = 0; val dynamic = ordinary; val field = dynamic; val property = field; val receiver = property; val param = receiver; val setparam = param; val delegate = setparam }\n",
    );
}

#[test]
fn ks_1_3_174_identifier_accepts_dotted_simple_identifiers_with_newlines() {
    assert_source_parses("package neutral.\nfeature.\nsample\nclass Holder\n");
}
