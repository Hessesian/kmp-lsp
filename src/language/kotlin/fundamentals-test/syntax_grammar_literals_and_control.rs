use super::{assert_source_contains_node_kind, assert_source_parses};

#[test]
fn ks_syntax_0297_string_literal_accepts_line_with_multiline_forms() {
    assert_source_parses("val line = \"status\"\nval multiline = \"\"\"status\"\"\"\n");
}

#[test]
fn ks_syntax_0298_line_string_literal_accepts_content_with_expressions() {
    assert_source_parses(
        "fun render(name: String, count: Int) = \"Name: $name, count: ${count + 1}, newline: \\n\"\n",
    );
}

#[test]
fn ks_syntax_0299_multiline_string_literal_accepts_content_expressions_with_quotes() {
    assert_source_parses(
        "fun render(name: String) = \"\"\"Name: $name; expression: ${name.length}; quote: \"\"\"\n",
    );
}

#[test]
fn ks_syntax_0300_line_string_content_accepts_text_escape_with_reference() {
    assert_source_parses("fun render(name: String) = \"text \\t $name\"\n");
}

#[test]
fn ks_syntax_0301_line_string_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun render(count: Int) = \"count=${\ncount + 1\n}\"\n");
}

#[test]
fn ks_syntax_0302_multiline_string_content_accepts_text_quote_with_reference() {
    assert_source_parses("fun render(name: String) = \"\"\"text \" $name\"\"\"\n");
}

#[test]
fn ks_syntax_0303_multiline_string_expression_wraps_expression_with_newlines() {
    assert_source_parses("fun render(count: Int) = \"\"\"count=${\ncount + 1\n}\"\"\"\n");
}

#[test]
fn ks_syntax_0304_lambda_literal_accepts_parameters_arrow_with_statements() {
    assert_source_contains_node_kind(
        "val transform: (Int) -> Int = { count ->\nval offset = 1\ncount + offset\n}\nval action = { println(\"done\") }\n",
        crate::queries::KIND_LAMBDA_LIT,
    );
}

#[test]
#[ignore = "KS-SYNTAX-0305: tree-sitter-kotlin rejects a trailing comma in lambda parameters"]
fn ks_syntax_0305_lambda_parameters_accept_multiple_with_trailing_comma() {
    assert_source_parses("val combine = { first: Int, second: Int, -> first + second }\n");
}

#[test]
#[ignore = "KS-SYNTAX-0306: tree-sitter-kotlin rejects a typed destructuring lambda parameter"]
fn ks_syntax_0306_lambda_parameter_accepts_variable_with_typed_destructuring() {
    assert_source_parses(
        "val single = { count: Int -> count }\nval pair = { (count, title): Pair<Int, String> -> title + count }\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0307: tree-sitter-kotlin rejects a suspend anonymous receiver function with constraints"]
fn ks_syntax_0307_anonymous_function_accepts_suspend_receiver_constraints_with_body() {
    assert_source_parses(
        "fun <Element> build() = suspend fun List<Element>.(value: Element): Element where Element : Any = value\n",
    );
}

#[test]
fn ks_syntax_0308_function_literal_accepts_lambda_with_anonymous_function() {
    assert_source_parses(
        "val lambda = { count: Int -> count + 1 }\nval anonymous = fun(count: Int): Int { return count + 1 }\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0309: tree-sitter-kotlin rejects data on an object literal"]
fn ks_syntax_0309_object_literal_accepts_data_supertypes_with_body() {
    assert_source_parses(
        "interface Item { fun title(): String }\nval item = data object : Item { override fun title() = \"item\" }\n",
    );
}

#[test]
fn ks_syntax_0310_this_expression_accepts_plain_with_labeled_forms() {
    assert_source_parses(
        "class Holder {\nfun inspect() {\nval plain = this\nwith(this) named@ { val labeled = this@named }\n}\n}\n",
    );
}

#[test]
fn ks_syntax_0311_super_expression_accepts_type_with_label_qualifiers() {
    assert_source_parses(
        "interface Named {\nfun title(): String { return \"named\" }\n}\nopen class Base {\nopen fun title(): String { return \"base\" }\n}\nclass Child : Base(), Named {\noverride fun title(): String {\nval plain = super.title()\nreturn super<Base>.title() + super<Named>.title()\n}\n}\n",
    );
}

#[test]
fn ks_syntax_0312_if_expression_accepts_body_else_with_empty_forms() {
    assert_source_parses(
        "fun inspect(flag: Boolean) {\nif (flag) println(1)\nif (flag) { println(2) } else println(3)\nif (flag);\n}\n",
    );
}

#[test]
fn ks_syntax_0313_when_subject_accepts_expression_or_bound_variable() {
    assert_source_parses(
        "annotation class Marker\nfun inspect(value: Any) {\nwhen (value) { else -> println(value) }\nwhen (@Marker val subject = value) { else -> println(subject) }\n}\n",
    );
}

#[test]
fn ks_syntax_0314_when_expression_accepts_optional_subject_with_entries() {
    assert_source_parses(
        "fun inspect(value: Int) {\nval subject = when (value) { 1 -> \"one\"; else -> \"other\" }\nval subjectless = when { value > 0 -> \"positive\"; else -> \"other\" }\n}\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0315: tree-sitter-kotlin rejects a trailing comma in when conditions"]
fn ks_syntax_0315_when_entry_accepts_conditions_trailing_comma_with_else() {
    assert_source_parses(
        "fun inspect(value: Int) = when (value) {\n1, 2, -> \"small\"\nelse -> \"other\"\n}\n",
    );
}

#[test]
fn ks_syntax_0316_when_condition_accepts_expression_range_with_type_tests() {
    assert_source_parses(
        "fun inspect(value: Any) = when (value) {\n0 -> \"zero\";\nin 1..10 -> \"present\";\nis String -> \"text\";\nelse -> \"other\"\n}\n",
    );
}

#[test]
fn ks_syntax_0317_range_test_accepts_positive_with_negative_membership() {
    assert_source_parses(
        "fun inspect(value: Int) = when (value) {\nin 1..10 -> \"inside\"\n!in 20..30 -> \"outside\"\nelse -> \"other\"\n}\n",
    );
}

#[test]
fn ks_syntax_0318_type_test_accepts_positive_with_negative_checks() {
    assert_source_parses(
        "fun inspect(value: Any) = when (value) {\nis String -> \"text\"\n!is Number -> \"other\"\nelse -> \"number\"\n}\n",
    );
}

#[test]
fn ks_syntax_0319_try_expression_accepts_catches_with_finally() {
    assert_source_parses(
        "fun inspect() {\ntry { println(1) } catch (failure: IllegalStateException) { println(failure) } catch (failure: RuntimeException) { println(failure) } finally { println(2) }\ntry { println(3) } finally { println(4) }\n}\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0320: tree-sitter-kotlin rejects a trailing comma in a catch parameter"]
fn ks_syntax_0320_catch_block_accepts_annotation_type_trailing_comma_with_block() {
    assert_source_parses(
        "annotation class Marker\nfun inspect() { try { println(1) } catch (@Marker failure: RuntimeException,) { println(failure) } }\n",
    );
}

#[test]
fn ks_syntax_0321_finally_block_combines_keyword_with_block() {
    assert_source_parses(
        "fun inspect() { try { println(1) } finally { println(\"complete\") } }\n",
    );
}

#[test]
fn ks_syntax_0322_jump_expression_accepts_throw_return_continue_with_break_forms() {
    assert_source_parses(
        "fun inspect(values: List<Int>): Int {\nvalues.forEach named@ { if (it < 0) return@named; if (it == 0) throw IllegalStateException() }\nouter@ for (value in values) { if (value == 1) continue@outer; if (value == 2) break@outer }\nreturn values.size\n}\n",
    );
}

#[test]
fn ks_syntax_0323_callable_reference_accepts_receiver_name_with_class() {
    assert_source_parses(
        "class Item\nfun create() = Item()\nval constructor = ::Item\nval factory = ::create\nval length = String::length\nval type = String::class\n",
    );
}

#[test]
fn ks_syntax_0324_assignment_with_operator_accepts_every_compound_operator() {
    assert_source_parses(
        "fun update() { var count = 10; count += 1; count -= 1; count *= 2; count /= 2; count %= 3 }\n",
    );
}

#[test]
fn ks_syntax_0325_equality_operator_accepts_structural_with_referential_forms() {
    assert_source_parses(
        "fun compare(first: Any, second: Any) { val a = first == second; val b = first != second; val c = first === second; val d = first !== second }\n",
    );
}

#[test]
fn ks_syntax_0326_comparison_operator_accepts_all_ordering_forms() {
    assert_source_parses(
        "fun compare(first: Int, second: Int) { val a = first < second; val b = first > second; val c = first <= second; val d = first >= second }\n",
    );
}

#[test]
fn ks_syntax_0327_in_operator_accepts_positive_with_negative_forms() {
    assert_source_parses(
        "fun inspect(value: Int, values: List<Int>) { val present = value in values; val absent = value !in values }\n",
    );
}

#[test]
fn ks_syntax_0328_is_operator_accepts_positive_with_negative_forms() {
    assert_source_parses(
        "fun inspect(value: Any) { val text = value is String; val other = value !is String }\n",
    );
}

#[test]
fn ks_syntax_0329_additive_operator_accepts_plus_with_minus() {
    assert_source_parses("fun calculate(first: Int, second: Int) = first + second - 1\n");
}

#[test]
fn ks_syntax_0330_multiplicative_operator_accepts_multiply_divide_with_remainder() {
    assert_source_parses("fun calculate(first: Int, second: Int) = first * second / 2 % 3\n");
}

#[test]
fn ks_syntax_0331_as_operator_accepts_unsafe_with_safe_forms() {
    assert_source_parses(
        "fun inspect(value: Any) { val definite = value as String; val optional = value as? String }\n",
    );
}

#[test]
fn ks_syntax_0332_prefix_unary_operator_accepts_increment_decrement_sign_with_excl() {
    assert_source_parses(
        "fun update() { var count = 0; val flag = false; ++count; --count; val negative = -count; val positive = +count; val inverse = !flag }\n",
    );
}

#[test]
fn ks_syntax_0333_postfix_unary_operator_accepts_increment_decrement_with_not_null() {
    assert_source_parses(
        "fun update(value: String?) { var count = 0; count++; count--; val length = value!!.length }\n",
    );
}

#[test]
fn ks_syntax_0334_excl_accepts_adjacent_or_whitespace_followed_forms() {
    assert_source_parses("fun inspect(first: Boolean, second: Boolean) = !first || ! second\n");
}

#[test]
fn ks_syntax_0335_member_access_operator_accepts_dot_safe_navigation_with_reference() {
    assert_source_parses(
        "fun inspect(value: String?) { val direct = value\n?.length; val reference = String::length; val text = value\n.toString() }\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0336: tree-sitter-kotlin accepts whitespace inside the safe-navigation token"]
fn ks_syntax_0336_safe_nav_requires_no_whitespace_between_question_mark_with_dot() {
    assert_source_parses("fun inspect(value: String?) = value?.length\n");
    super::assert_source_has_syntax_error("fun inspect(value: String?) = value ? .length\n");
}

#[test]
fn ks_syntax_0337_modifiers_accept_annotations_with_repeated_modifiers() {
    assert_source_parses(
        "annotation class Marker\n@Marker public open class Holder { @Marker protected open fun render() {}; }\n",
    );
}

#[test]
fn ks_syntax_0338_parameter_modifiers_accept_annotation_with_parameter_modifiers() {
    assert_source_parses(
        "annotation class Marker\ninline fun inspect(@Marker crossinline action: () -> Unit, noinline fallback: () -> Unit, vararg values: Int) {}\n",
    );
}

#[test]
fn ks_syntax_0339_modifier_accepts_every_modifier_family() {
    assert_source_parses(
        "annotation class Marker\npublic open class Holder { override fun toString() = \"holder\"; lateinit var title: String; }\ninline fun inspect(vararg values: Int) {}\nconst val count = 1\nexpect class Expected\nactual class Expected\n",
    );
}

#[test]
fn ks_syntax_0340_type_modifiers_accept_repeated_type_modifiers() {
    assert_source_parses(
        "@Target(AnnotationTarget.TYPE) annotation class Marker\nval action: @Marker suspend () -> Unit = {}\n",
    );
}

#[test]
fn ks_syntax_0341_type_modifier_accepts_annotation_or_suspend() {
    assert_source_parses(
        "@Target(AnnotationTarget.TYPE) annotation class Marker\nval annotated: @Marker () -> Unit = {}\nval suspended: suspend () -> Unit = {}\n",
    );
}

#[test]
fn ks_syntax_0342_class_modifier_accepts_all_class_kinds() {
    assert_source_parses(
        "enum class Mode { FIRST }\nsealed class State\nannotation class Marker\ndata class Item(val count: Int)\nclass Outer { inner class Nested; }\nvalue class Identifier(val value: String)\n",
    );
}

#[test]
fn ks_syntax_0343_member_modifier_accepts_override_with_lateinit() {
    assert_source_parses(
        "open class Base { open fun render() {}; }\nclass Child : Base() { override fun render() {}; lateinit var title: String; }\n",
    );
}

#[test]
fn ks_syntax_0344_visibility_modifier_accepts_all_visibilities() {
    assert_source_parses(
        "public class PublicItem\nprivate class PrivateItem\ninternal class InternalItem\nopen class Base { protected fun inspect() {}; }\n",
    );
}

#[test]
fn ks_syntax_0345_variance_modifier_accepts_in_with_out() {
    assert_source_parses("class Consumer<in Input>\nclass Producer<out Output>\n");
}

#[test]
fn ks_syntax_0346_type_parameter_modifiers_accept_repeated_modifiers() {
    assert_source_parses(
        "annotation class Marker\ninline fun <@Marker reified out Element> inspect(value: Element) {}\n",
    );
}

#[test]
fn ks_syntax_0347_type_parameter_modifier_accepts_reified_variance_or_annotation() {
    assert_source_parses(
        "annotation class Marker\ninline fun <reified Element> inspect(value: Element) {}\nclass Producer<out Element>\nclass Consumer<@Marker in Element>\n",
    );
}

#[test]
fn ks_syntax_0348_function_modifier_accepts_every_function_modifier() {
    assert_source_parses(
        "tailrec fun repeat(count: Int): Int = if (count == 0) 0 else repeat(count - 1)\noperator fun Int.plus(other: String) = toString() + other\ninfix fun String.merge(other: String) = this + other\ninline fun apply(action: () -> Unit) = action()\nexternal fun nativeCall()\nsuspend fun load() {}\n",
    );
}

#[test]
fn ks_syntax_0349_property_modifier_accepts_const() {
    assert_source_parses("const val DEFAULT_COUNT = 1\n");
}

#[test]
fn ks_syntax_0350_inheritance_modifier_accepts_abstract_final_with_open() {
    assert_source_parses(
        "abstract class AbstractItem\nfinal class FinalItem\nopen class OpenItem\n",
    );
}

#[test]
fn ks_syntax_0351_parameter_modifier_accepts_vararg_noinline_with_crossinline() {
    assert_source_parses(
        "inline fun inspect(vararg values: Int, noinline fallback: () -> Unit, crossinline action: () -> Unit) {}\n",
    );
}

#[test]
fn ks_syntax_0352_reification_modifier_accepts_reified() {
    assert_source_parses("inline fun <reified Element> inspect(value: Element) {}\n");
}

#[test]
fn ks_syntax_0353_platform_modifier_accepts_expect_with_actual() {
    assert_source_parses("expect class PlatformItem\nactual class PlatformItem\n");
}

#[test]
fn ks_syntax_0354_annotation_accepts_single_or_multi_forms_with_newline() {
    assert_source_parses(
        "annotation class First\nannotation class Second\n@First\nclass Single\n@[First Second]\nclass Multiple\n",
    );
}

#[test]
fn ks_syntax_0355_single_annotation_accepts_use_site_with_at_token_forms() {
    assert_source_parses(
        "annotation class Marker\nclass Holder(@param:Marker val value: String) { @get:Marker val title = value; }\n",
    );
}

#[test]
fn ks_syntax_0356_multi_annotation_accepts_multiple_unescaped_annotations() {
    assert_source_parses(
        "annotation class First\nannotation class Second\n@[First Second]\nclass Holder\n",
    );
}

#[test]
fn ks_syntax_0357_annotation_use_site_target_accepts_every_target() {
    assert_source_parses(
        "@file:Marker\nannotation class Marker\nclass Holder(@param:Marker @property:Marker @field:Marker val value: String) {\n@get:Marker @delegate:Marker val title by lazy { value }\n@set:Marker @setparam:Marker var count = 0\nfun @receiver:Marker String.render() = this\n}\n",
    );
}

#[test]
fn ks_syntax_0358_unescaped_annotation_accepts_constructor_or_user_type() {
    assert_source_parses(
        "annotation class Named(val value: String)\nannotation class Marker\n@Named(\"holder\") @Marker class Holder\n",
    );
}

#[test]
#[ignore = "KS-SYNTAX-0359: tree-sitter-kotlin rejects dynamic as an unescaped simple identifier"]
fn ks_syntax_0359_simple_identifier_accepts_identifier_with_soft_keywords() {
    assert_source_parses(
        "fun inspect() { val ordinary = 0; val dynamic = ordinary; val field = dynamic; val property = field; val receiver = property; val param = receiver; val setparam = param; val delegate = setparam }\n",
    );
}

#[test]
fn ks_syntax_0360_identifier_accepts_dotted_simple_identifiers_with_newlines() {
    assert_source_parses("package neutral.\nfeature.\nsample\nclass Holder\n");
}
