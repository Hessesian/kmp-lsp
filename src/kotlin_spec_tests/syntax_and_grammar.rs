use super::{
    assert_source_contains_node_kind, assert_source_has_syntax_error, assert_source_lexes_token,
    assert_source_parses, count_nodes_of_kind, parse_kotlin_source,
};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{GotoDefinitionResponse, Position, Url};

fn syntax_position_of_occurrence(source: &str, needle: &str, occurrence: usize) -> Position {
    let byte_offset = source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(byte_offset, _)| byte_offset)
        .expect("fixture occurrence must exist");
    let preceding_source = &source[..byte_offset];
    let line = preceding_source.matches('\n').count() as u32;
    let character = preceding_source
        .rsplit('\n')
        .next()
        .expect("split always yields one segment")
        .chars()
        .count() as u32;
    Position::new(line, character)
}

async fn syntax_definition_position(
    source: &str,
    needle: &str,
    occurrence: usize,
) -> Option<Position> {
    let specification_uri =
        Url::parse("file:///kotlin-spec/Syntax.kt").expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let position = syntax_position_of_occurrence(source, needle, occurrence);
    let cursor_context = CursorContext::build(&indexer, &specification_uri, position)
        .expect("fixture cursor must select an identifier");

    match find_definition(&cursor_context, &indexer, &specification_uri, position).await {
        Some(GotoDefinitionResponse::Scalar(location)) => Some(location.range.start),
        Some(GotoDefinitionResponse::Array(locations)) if locations.len() == 1 => {
            Some(locations[0].range.start)
        }
        Some(GotoDefinitionResponse::Array(_)) | Some(GotoDefinitionResponse::Link(_)) | None => {
            None
        }
    }
}

#[test]
fn ks_syntax_0001_line_feed_is_u_000a() {
    let source = "val first = 1\nval second = 2\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        2
    );
}

#[test]
fn ks_syntax_0002_carriage_return_is_u_000d() {
    let source = "val first = 1\rval second = 2\r";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        2
    );
}

#[test]
fn ks_syntax_0003_shebang_extends_to_line_terminator() {
    let source = "#!/usr/bin/env kotlin\nval visible = 1\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        1
    );
}

#[test]
fn ks_syntax_0004_delimited_comment_allows_recursion() {
    assert_source_parses("/* outer /* nested */ outer */\nval visible = 1\n");
}

#[test]
fn ks_syntax_0005_line_comment_stops_before_line_terminator() {
    let source = "// first line\nval visible = 1\n";
    let tree = parse_kotlin_source(source);

    assert!(!tree.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
        1
    );
}

#[test]
fn ks_syntax_0006_whitespace_accepts_space_tab_form_feed() {
    assert_source_parses("val\tanswer\u{000c} =\t42\n");
}

#[test]
fn ks_syntax_0007_newline_accepts_lf_cr_crlf() {
    for source in [
        "val first = 1\nval second = 2\n",
        "val first = 1\rval second = 2\r",
        "val first = 1\r\nval second = 2\r\n",
    ] {
        let tree = parse_kotlin_source(source);
        assert!(!tree.root_node().has_error());
        assert_eq!(
            count_nodes_of_kind(&tree, crate::queries::KIND_PROP_DECL),
            2
        );
    }
}

#[test]
fn ks_syntax_0008_hidden_accepts_comments_whitespace() {
    for source in [
        "val value = 1\n",
        "val /* hidden */ value = 1\n",
        "val // hidden\n value = 1\n",
    ] {
        assert_source_parses(source);
    }
}

#[test]
#[ignore = "KS-SYNTAX-0009: tree-sitter-kotlin does not expose the reserved ellipsis as one lexical token"]
fn ks_syntax_0009_reserved_token() {
    assert_source_lexes_token("...", "...");
}

#[test]
fn ks_syntax_0010_dot_token() {
    assert_source_lexes_token(".", ".");
}

#[test]
fn ks_syntax_0011_comma_token() {
    assert_source_lexes_token(",", ",");
}

#[test]
fn ks_syntax_0012_lparen_token() {
    assert_source_lexes_token("(", "(");
}

#[test]
fn ks_syntax_0013_rparen_token() {
    assert_source_lexes_token(")", ")");
}

#[test]
fn ks_syntax_0014_lsquare_token() {
    assert_source_lexes_token("[", "[");
}

#[test]
fn ks_syntax_0015_rsquare_token() {
    assert_source_lexes_token("]", "]");
}

#[test]
fn ks_syntax_0016_lcurl_token() {
    assert_source_lexes_token("{", "{");
}

#[test]
fn ks_syntax_0017_rcurl_token() {
    assert_source_lexes_token("}", "}");
}

#[test]
fn ks_syntax_0018_mult_token() {
    assert_source_lexes_token("*", "*");
}

#[test]
fn ks_syntax_0019_mod_token() {
    assert_source_lexes_token("%", "%");
}

#[test]
fn ks_syntax_0020_div_token() {
    assert_source_lexes_token("/", "/");
}

#[test]
fn ks_syntax_0021_add_token() {
    assert_source_lexes_token("+", "+");
}

#[test]
fn ks_syntax_0022_sub_token() {
    assert_source_lexes_token("-", "-");
}

#[test]
fn ks_syntax_0023_incr_token() {
    assert_source_lexes_token("++", "++");
}

#[test]
fn ks_syntax_0024_decr_token() {
    assert_source_lexes_token("--", "--");
}

#[test]
fn ks_syntax_0025_conj_token() {
    assert_source_lexes_token("val result = true && false\n", "&&");
}

#[test]
fn ks_syntax_0026_disj_token() {
    assert_source_lexes_token("||", "||");
}

#[test]
fn ks_syntax_0027_excl_ws_token() {
    assert_source_lexes_token("! true", "!");
}

#[test]
fn ks_syntax_0028_excl_no_ws_token() {
    assert_source_lexes_token("!true", "!");
}

#[test]
fn ks_syntax_0029_colon_token() {
    assert_source_lexes_token(":", ":");
}

#[test]
fn ks_syntax_0030_semicolon_token() {
    assert_source_lexes_token(";", ";");
}

#[test]
fn ks_syntax_0031_assignment_token() {
    assert_source_lexes_token("=", "=");
}

#[test]
fn ks_syntax_0032_add_assignment_token() {
    assert_source_lexes_token("+=", "+=");
}

#[test]
fn ks_syntax_0033_sub_assignment_token() {
    assert_source_lexes_token("-=", "-=");
}

#[test]
fn ks_syntax_0034_mult_assignment_token() {
    assert_source_lexes_token("*=", "*=");
}

#[test]
fn ks_syntax_0035_div_assignment_token() {
    assert_source_lexes_token("/=", "/=");
}

#[test]
fn ks_syntax_0036_mod_assignment_token() {
    assert_source_lexes_token("%=", "%=");
}

#[test]
fn ks_syntax_0037_arrow_token() {
    assert_source_lexes_token("->", "->");
}

#[test]
#[ignore = "KS-SYNTAX-0038: tree-sitter-kotlin does not expose the double-arrow lexeme as one token"]
fn ks_syntax_0038_double_arrow_token() {
    assert_source_lexes_token("=>", "=>");
}

#[test]
fn ks_syntax_0039_range_token() {
    assert_source_lexes_token("..", "..");
}

#[test]
fn ks_syntax_0040_coloncolon_token() {
    assert_source_lexes_token("::", "::");
}

#[test]
#[ignore = "KS-SYNTAX-0041: tree-sitter-kotlin tokenizes neither the specified double-semicolon token nor a valid use"]
fn ks_syntax_0041_double_semicolon_token() {
    assert_source_lexes_token(";;", ";;");
}

#[test]
#[ignore = "KS-SYNTAX-0042: tree-sitter-kotlin reports standalone hash as an unexpected character"]
fn ks_syntax_0042_hash_token() {
    assert_source_lexes_token("#", "#");
}

#[test]
fn ks_syntax_0043_at_no_ws_token() {
    assert_source_lexes_token("@Target", "@");
}

#[test]
fn ks_syntax_0044_at_post_ws_token() {
    assert_source_lexes_token("@ Target", "@");
}

#[test]
fn ks_syntax_0045_at_pre_ws_token() {
    assert_source_lexes_token(" @Target", "@");
}

#[test]
fn ks_syntax_0046_at_both_ws_token() {
    assert_source_lexes_token(" @ Target", "@");
}

#[test]
fn ks_syntax_0047_quest_ws_token() {
    assert_source_parses("val value: String? = null\n");
}

#[test]
fn ks_syntax_0048_quest_no_ws_token() {
    assert_source_parses("val value: String?= null\n");
}

#[test]
fn ks_syntax_0049_langle_token() {
    assert_source_lexes_token("<", "<");
}

#[test]
fn ks_syntax_0050_rangle_token() {
    assert_source_lexes_token(">", ">");
}

#[test]
fn ks_syntax_0051_le_token() {
    assert_source_lexes_token("<=", "<=");
}

#[test]
fn ks_syntax_0052_ge_token() {
    assert_source_lexes_token(">=", ">=");
}

#[test]
fn ks_syntax_0053_excl_eq_token() {
    assert_source_lexes_token("val result = first != second\n", "!=");
}

#[test]
fn ks_syntax_0054_excl_eqeq_token() {
    assert_source_lexes_token("val result = first !== second\n", "!==");
}

#[test]
fn ks_syntax_0055_as_safe_token() {
    assert_source_lexes_token("val cast = value as? String\n", "as?");
}

#[test]
fn ks_syntax_0056_eqeq_token() {
    assert_source_lexes_token("val result = first == second\n", "==");
}

#[test]
fn ks_syntax_0057_eqeqeq_token() {
    assert_source_lexes_token("val result = first === second\n", "===");
}

#[test]
fn ks_syntax_0058_single_quote_token() {
    assert_source_lexes_token("'", "'");
}

#[test]
fn ks_syntax_0059_return_at_token() {
    assert_source_lexes_token("return@label", "return@");
}

#[test]
fn ks_syntax_0060_continue_at_token() {
    assert_source_lexes_token("continue@label", "continue@");
}

#[test]
fn ks_syntax_0061_break_at_token() {
    assert_source_lexes_token("break@label", "break@");
}

#[test]
fn ks_syntax_0062_this_at_token() {
    assert_source_lexes_token("this@label", "this@");
}

#[test]
fn ks_syntax_0063_super_at_token() {
    assert_source_lexes_token("super@label", "super@");
}

#[test]
fn ks_syntax_0064_file_token() {
    assert_source_parses("@file:Suppress(\"unused\")\nval value = 1\n");
}

#[test]
fn ks_syntax_0065_field_token() {
    assert_source_parses("@field:Marker val value = 1\n");
}

#[test]
fn ks_syntax_0066_property_token() {
    assert_source_parses("@property:Marker val value = 1\n");
}

#[test]
fn ks_syntax_0067_get_token() {
    assert_source_lexes_token("get", "get");
}

#[test]
fn ks_syntax_0068_set_token() {
    assert_source_lexes_token("set", "set");
}

#[test]
fn ks_syntax_0069_receiver_token() {
    assert_source_parses("fun @receiver:Marker String.render() = this\n");
}

#[test]
fn ks_syntax_0070_param_token() {
    assert_source_parses("fun render(@param:Marker value: String) = value\n");
}

#[test]
fn ks_syntax_0071_setparam_token() {
    assert_source_parses(
        "var value = 0\n    set(@setparam:Marker newValue) { field = newValue }\n",
    );
}

#[test]
fn ks_syntax_0072_delegate_token() {
    assert_source_parses("@delegate:Marker val value by lazy { 1 }\n");
}

#[test]
fn ks_syntax_0073_package_token() {
    assert_source_lexes_token("package", "package");
}

#[test]
fn ks_syntax_0074_import_token() {
    assert_source_lexes_token("import", "import");
}

#[test]
fn ks_syntax_0075_class_token() {
    assert_source_lexes_token("class", "class");
}

#[test]
fn ks_syntax_0076_interface_token() {
    assert_source_lexes_token("interface", "interface");
}

#[test]
fn ks_syntax_0077_fun_token() {
    assert_source_lexes_token("fun", "fun");
}

#[test]
fn ks_syntax_0078_object_token() {
    assert_source_lexes_token("object", "object");
}

#[test]
fn ks_syntax_0079_val_token() {
    assert_source_lexes_token("val", "val");
}

#[test]
fn ks_syntax_0080_var_token() {
    assert_source_lexes_token("var", "var");
}

#[test]
fn ks_syntax_0081_type_alias_token() {
    assert_source_lexes_token("typealias", "typealias");
}

#[test]
fn ks_syntax_0082_constructor_token() {
    assert_source_parses("class Box constructor(val value: Int)\n");
}

#[test]
fn ks_syntax_0083_by_token() {
    assert_source_parses("interface Item\nclass Box(item: Item) : Item by item\n");
}

#[test]
fn ks_syntax_0084_companion_token() {
    assert_source_parses("class Box {\n    companion object\n}\n");
}

#[test]
fn ks_syntax_0085_init_token() {
    assert_source_parses("class Box {\n    init { println(Unit) }\n}\n");
}

#[test]
fn ks_syntax_0086_this_token() {
    assert_source_lexes_token("this", "this");
}

#[test]
fn ks_syntax_0087_super_token() {
    assert_source_lexes_token("super", "super");
}

#[test]
#[ignore = "KS-SYNTAX-0088: tree-sitter-kotlin emits typeof as a simple identifier instead of the specified keyword token"]
fn ks_syntax_0088_typeof_token() {
    assert_source_lexes_token("typeof", "typeof");
}

#[test]
fn ks_syntax_0089_where_token() {
    assert_source_parses("fun <Value> render(value: Value) where Value : Any = value\n");
}

#[test]
fn ks_syntax_0090_if_token() {
    assert_source_lexes_token("if", "if");
}

#[test]
fn ks_syntax_0091_else_token() {
    assert_source_parses("val value = if (ready) 1 else 2\n");
}

#[test]
fn ks_syntax_0092_when_token() {
    assert_source_lexes_token("when", "when");
}

#[test]
fn ks_syntax_0093_try_token() {
    assert_source_lexes_token("try", "try");
}

#[test]
fn ks_syntax_0094_catch_token() {
    assert_source_parses("val value = try { 1 } catch (error: Throwable) { 2 }\n");
}

#[test]
fn ks_syntax_0095_finally_token() {
    assert_source_parses("val value = try { 1 } finally { println(Unit) }\n");
}

#[test]
fn ks_syntax_0096_for_token() {
    assert_source_lexes_token("for", "for");
}

#[test]
fn ks_syntax_0097_do_token() {
    assert_source_lexes_token("do", "do");
}

#[test]
fn ks_syntax_0098_while_token() {
    assert_source_lexes_token("while", "while");
}

#[test]
fn ks_syntax_0099_throw_token() {
    assert_source_lexes_token("throw", "throw");
}

#[test]
fn ks_syntax_0100_return_token() {
    assert_source_lexes_token("return", "return");
}

#[test]
fn ks_syntax_0101_continue_token() {
    assert_source_lexes_token("continue", "continue");
}

#[test]
fn ks_syntax_0102_break_token() {
    assert_source_lexes_token("break", "break");
}

#[test]
fn ks_syntax_0103_as_token() {
    assert_source_parses("val cast = value as String\n");
}

#[test]
fn ks_syntax_0104_is_token() {
    assert_source_parses("val result = value is String\n");
}

#[test]
fn ks_syntax_0105_in_token() {
    assert_source_parses("val result = value in values\n");
}

#[test]
fn ks_syntax_0106_not_is_token() {
    assert_source_lexes_token("value !is Type", "!is");
}

#[test]
fn ks_syntax_0107_not_in_token() {
    assert_source_lexes_token("value !in values", "!in");
}

#[test]
fn ks_syntax_0108_out_token() {
    assert_source_parses("interface Source<out Value>\n");
}

#[test]
fn ks_syntax_0109_dynamic_token() {
    assert_source_parses("val value: dynamic = source\n");
}

#[test]
fn ks_syntax_0110_public_token() {
    assert_source_lexes_token("public", "public");
}

#[test]
fn ks_syntax_0111_private_token() {
    assert_source_lexes_token("private", "private");
}

#[test]
fn ks_syntax_0112_protected_token() {
    assert_source_lexes_token("protected", "protected");
}

#[test]
fn ks_syntax_0113_internal_token() {
    assert_source_lexes_token("internal", "internal");
}

#[test]
fn ks_syntax_0114_enum_token() {
    assert_source_lexes_token("enum", "enum");
}

#[test]
fn ks_syntax_0115_sealed_token() {
    assert_source_lexes_token("sealed", "sealed");
}

#[test]
fn ks_syntax_0116_annotation_token() {
    assert_source_lexes_token("annotation", "annotation");
}

#[test]
fn ks_syntax_0117_data_token() {
    assert_source_lexes_token("data", "data");
}

#[test]
fn ks_syntax_0118_inner_token() {
    assert_source_lexes_token("inner", "inner");
}

#[test]
fn ks_syntax_0119_tailrec_token() {
    assert_source_lexes_token("tailrec", "tailrec");
}

#[test]
fn ks_syntax_0120_operator_token() {
    assert_source_lexes_token("operator", "operator");
}

#[test]
fn ks_syntax_0121_inline_token() {
    assert_source_lexes_token("inline", "inline");
}

#[test]
fn ks_syntax_0122_infix_token() {
    assert_source_lexes_token("infix", "infix");
}

#[test]
fn ks_syntax_0123_external_token() {
    assert_source_lexes_token("external", "external");
}

#[test]
fn ks_syntax_0124_suspend_token() {
    assert_source_lexes_token("suspend", "suspend");
}

#[test]
fn ks_syntax_0125_override_token() {
    assert_source_lexes_token("override", "override");
}

#[test]
fn ks_syntax_0126_abstract_token() {
    assert_source_lexes_token("abstract", "abstract");
}

#[test]
fn ks_syntax_0127_final_token() {
    assert_source_lexes_token("final", "final");
}

#[test]
fn ks_syntax_0128_open_token() {
    assert_source_lexes_token("open", "open");
}

#[test]
fn ks_syntax_0129_const_token() {
    assert_source_parses("const val value = 1\n");
}

#[test]
fn ks_syntax_0130_lateinit_token() {
    assert_source_lexes_token("lateinit", "lateinit");
}

#[test]
fn ks_syntax_0131_vararg_token() {
    assert_source_lexes_token("vararg", "vararg");
}

#[test]
fn ks_syntax_0132_noinline_token() {
    assert_source_lexes_token("noinline", "noinline");
}

#[test]
fn ks_syntax_0133_crossinline_token() {
    assert_source_lexes_token("crossinline", "crossinline");
}

#[test]
fn ks_syntax_0134_reified_token() {
    assert_source_parses("inline fun <reified Value> render() = Value::class\n");
}

#[test]
fn ks_syntax_0135_expect_token() {
    assert_source_lexes_token("expect", "expect");
}

#[test]
fn ks_syntax_0136_actual_token() {
    assert_source_lexes_token("actual", "actual");
}

#[test]
fn ks_syntax_0137_decimal_digit_no_zero_accepts_one_through_nine() {
    for digit in '1'..='9' {
        assert_source_parses(&format!("val value = {digit}\n"));
    }
}

#[test]
fn ks_syntax_0138_decimal_digit_accepts_zero_through_nine() {
    for digit in '0'..='9' {
        assert_source_parses(&format!("val value = {digit}\n"));
    }
}

#[test]
fn ks_syntax_0139_decimal_digit_or_separator_accepts_internal_underscore() {
    assert_source_parses("val value = 1_0\n");
    assert_source_has_syntax_error("val trailing = 10_\n");
}

#[test]
fn ks_syntax_0140_decimal_digits_allow_only_internal_separators() {
    assert_source_parses("val value = 1_000_000\n");
    assert_source_has_syntax_error("val trailing = 100_\n");
}

#[test]
fn ks_syntax_0141_double_exponent_accepts_marker_sign_digits() {
    for literal in ["1e9", "1E9", "1e+9", "1E-9"] {
        assert_source_parses(&format!("val value = {literal}\n"));
    }
}

#[test]
fn ks_syntax_0142_real_literal_accepts_float_or_double_forms() {
    for literal in ["0.5", "1e9", "0.5f", "1F"] {
        assert_source_parses(&format!("val value = {literal}\n"));
    }
}

#[test]
fn ks_syntax_0143_float_literal_accepts_double_or_integer_with_suffix() {
    for literal in ["0.5f", "0.5F", "1f", "1F"] {
        assert_source_parses(&format!("val value = {literal}\n"));
    }
}

#[test]
fn ks_syntax_0144_double_literal_accepts_fraction_or_exponent() {
    for literal in [".5", "0.5", "0.5e2", "1e2"] {
        assert_source_parses(&format!("val value = {literal}\n"));
    }
}

#[test]
#[ignore = "KS-SYNTAX-0145: tree-sitter-kotlin accepts the grammar-forbidden leading-zero literal 01"]
fn ks_syntax_0145_integer_literal_accepts_zero_or_nonzero_sequence() {
    for literal in ["0", "7", "42", "4_2"] {
        assert_source_parses(&format!("val value = {literal}\n"));
    }
    assert_source_has_syntax_error("val value = 01\n");
}

#[test]
fn ks_syntax_0146_hex_digit_accepts_decimal_a_through_f() {
    for literal in ["0x0", "0x9", "0xA", "0xF", "0xa", "0xf"] {
        assert_source_parses(&format!("val value = {literal}\n"));
    }
}

#[test]
fn ks_syntax_0147_hex_digit_or_separator_accepts_internal_underscore() {
    assert_source_parses("val value = 0xCA_FE\n");
    assert_source_has_syntax_error("val value = 0xCA_\n");
}

#[test]
fn ks_syntax_0148_hex_literal_accepts_both_prefix_cases() {
    assert_source_parses("val lower = 0xCAFE\nval upper = 0X10\n");
    assert_source_has_syntax_error("val missing = 0x\n");
}

#[test]
fn ks_syntax_0149_binary_digit_accepts_zero_or_one() {
    assert_source_parses("val zero = 0b0\nval one = 0b1\n");
    assert_source_has_syntax_error("val invalid = 0b2\n");
}

#[test]
#[ignore = "KS-SYNTAX-0150: tree-sitter-kotlin rejects a valid binary literal with an internal underscore"]
fn ks_syntax_0150_binary_digit_or_separator_accepts_internal_underscore() {
    assert_source_parses("val value = 0b10_01\n");
    assert_source_has_syntax_error("val value = 0b10_\n");
}

#[test]
#[ignore = "KS-SYNTAX-0151: tree-sitter-kotlin rejects a separated binary literal and uppercase B prefix"]
fn ks_syntax_0151_binary_literal_accepts_both_prefix_cases() {
    assert_source_parses("val separated = 0b1010_0011\nval upper = 0B10\n");
    assert_source_has_syntax_error("val invalid = 0b102\n");
}

#[test]
#[ignore = "KS-SYNTAX-0152: tree-sitter-kotlin misparses the valid binary unsigned literal 0b10U"]
fn ks_syntax_0152_unsigned_literal_accepts_u_optional_l() {
    assert_source_parses("val decimal = 42u\nval hex = 0xFFUL\nval binary = 0b10U\n");
}

#[test]
#[ignore = "KS-SYNTAX-0153: tree-sitter-kotlin misparses the valid binary long literal 0b10L"]
fn ks_syntax_0153_long_literal_accepts_uppercase_l() {
    assert_source_parses("val decimal = 42L\nval hex = 0xFFL\nval binary = 0b10L\n");
    assert_source_has_syntax_error("val lowercase = 42l\n");
}

#[test]
fn ks_syntax_0154_boolean_literal_accepts_true_or_false() {
    let tree = parse_kotlin_source("val enabled = true\nval disabled = false\n");
    assert_eq!(
        count_nodes_of_kind(&tree, crate::queries::KIND_BOOLEAN_LITERAL),
        2
    );
}

#[test]
fn ks_syntax_0155_null_literal_recognizes_null() {
    assert_source_contains_node_kind("val absent = null\n", crate::queries::KIND_NULL_LITERAL);
}

#[test]
fn ks_syntax_0156_character_literal_accepts_one_plain_or_escape() {
    assert_source_parses("val plain = 'K'\nval escaped = '\\t'\nval unicode = '\\u004b'\n");
    assert_source_has_syntax_error("val tooMany = 'KT'\n");
}

#[test]
fn ks_syntax_0157_unicode_character_literal_requires_four_hex_digits() {
    assert_source_parses("val unicode = '\\u004b'\n");
    assert_source_has_syntax_error("val short = '\\u04b'\nval nonHex = '\\u00G0'\n");
}

#[test]
fn ks_syntax_0158_escaped_identifier_accepts_enumerated_escape_codes() {
    for escape_code in ["\\t", "\\b", "\\r", "\\n", "\\'", "\\\"", "\\\\", "\\$"] {
        assert_source_parses(&format!("val value = '{escape_code}'\n"));
    }
}

#[test]
fn ks_syntax_0159_escape_sequence_accepts_unicode_or_named_escape() {
    assert_source_parses("val unicode = '\\u004b'\nval named = '\\n'\n");
    assert_source_has_syntax_error("val invalid = '\\q'\n");
}

#[test]
#[ignore = "KS-SYNTAX-0160: tree-sitter-kotlin rejects a valid Unicode Lo letter in an identifier"]
fn ks_syntax_0160_letter_accepts_unicode_letter_categories() {
    assert_source_parses(
        "val Alpha = 1\nval lower = 2\nval ǅelta = 3\nval ʰvalue = 4\nval 名称 = 5\n",
    );
}

#[test]
fn ks_syntax_0161_quoted_symbol_excludes_terminators() {
    assert_source_parses("val `@# name-with spaces` = 1\n");
    assert_source_has_syntax_error("val `` = 1\nval `line\nbreak` = 2\n");
}

#[test]
fn ks_syntax_0162_unicode_digit_accepts_nd_after_letter() {
    assert_source_parses("val value١ = 1\nval value१ = 2\n");
    assert_source_has_syntax_error("val ١value = 1\n");
}

#[test]
fn ks_syntax_0163_identifier_accepts_grammar_alternatives() {
    assert_source_parses(
        "val _count2 = 2\nval Δelta3 = 3\nval данные4 = 4\nval `quoted name` = 5\n",
    );
    assert_source_has_syntax_error("val 2count = 2\n");
}

#[test]
fn ks_syntax_0164_escaped_identifier_accepts_keyword_symbols() {
    assert_source_parses("val `when` = 1\nfun `render-screen#`() = `when`\n");
}

#[tokio::test]
#[ignore = "KS-SYNTAX-0166: kmp-lsp cannot resolve an unescaped use to its escaped declaration"]
async fn ks_syntax_0166_escaped_plain_identifier_share_entity() {
    let source = "val foo = 1\nval escapedUse = `foo`\nval `bar` = 2\nval plainUse = bar\n";
    assert_source_parses(source);

    let foo_declaration = syntax_position_of_occurrence(source, "foo", 0);
    let escaped_foo_definition = syntax_definition_position(source, "foo", 1).await;
    assert_eq!(escaped_foo_definition, Some(foo_declaration));

    let bar_declaration = syntax_position_of_occurrence(source, "`bar`", 0);
    let plain_bar_definition = syntax_definition_position(source, "bar", 1).await;
    assert_eq!(plain_bar_definition, Some(bar_declaration));
}

#[test]
#[ignore = "KS-SYNTAX-0167: tree-sitter-kotlin rejects the specification-listed soft keyword `dynamic` as a property name"]
fn ks_syntax_0167_identifier_or_soft_key_accepts_complete_list() {
    let soft_keywords = [
        "abstract",
        "annotation",
        "by",
        "catch",
        "companion",
        "constructor",
        "crossinline",
        "data",
        "dynamic",
        "enum",
        "external",
        "final",
        "finally",
        "import",
        "infix",
        "init",
        "inline",
        "inner",
        "internal",
        "lateinit",
        "noinline",
        "open",
        "operator",
        "out",
        "override",
        "private",
        "protected",
        "public",
        "reified",
        "sealed",
        "tailrec",
        "vararg",
        "where",
        "get",
        "set",
        "field",
        "property",
        "receiver",
        "param",
        "setparam",
        "delegate",
        "file",
        "expect",
        "actual",
        "const",
        "suspend",
    ];

    for soft_keyword in soft_keywords {
        let source = format!("val {soft_keyword} = 1\n");
        let tree = parse_kotlin_source(&source);
        assert!(
            !tree.root_node().has_error(),
            "soft keyword {soft_keyword} should parse as an identifier, got: {}",
            tree.root_node().to_sexp()
        );
    }
}

#[test]
#[ignore = "KS-SYNTAX-0168: tree-sitter-kotlin accepts the unescaped hard keyword `if` as a simple identifier"]
fn ks_syntax_0168_hard_keyword_requires_escaped_identifier() {
    assert_source_has_syntax_error("val if = 1\n");
    assert_source_parses("val `if` = 1\n");
}

#[test]
fn ks_syntax_0169_quote_open_recognizes_double_quote() {
    assert_source_parses("val text = \"value\"\n");
}

#[test]
fn ks_syntax_0170_triple_quote_open_recognizes_three_quotes() {
    assert_source_parses(
        r#"val text = """value"""
"#,
    );
}

#[test]
fn ks_syntax_0171_field_identifier_accepts_soft_key() {
    assert_source_parses("val field = 1\nval text = \"$field\"\n");
}

#[test]
fn ks_syntax_0172_quote_switches_line_string_mode() {
    assert_source_parses(
        r#"val name = "sample"
val message = "Hello, $name: ${name.length}\n"
"#,
    );
}

#[test]
fn ks_syntax_0173_quote_close_terminates_line_string() {
    assert_source_parses("val closed = \"text\"\n");
    assert_source_has_syntax_error("val open = \"text\n");
}

#[test]
fn ks_syntax_0174_line_string_reference_accepts_field_identifier() {
    assert_source_parses("val name = \"sample\"\nval text = \"hello $name\"\n");
}

#[test]
fn ks_syntax_0175_line_string_text_accepts_ordinary_or_dollar() {
    assert_source_parses("val ordinary = \"letters 123 !\"\nval dollar = \"cost $\"\n");
}

#[test]
#[ignore = "KS-SYNTAX-0176: tree-sitter-kotlin accepts the invalid line-string escape \\q"]
fn ks_syntax_0176_line_string_escaped_char_accepts_escape_families() {
    assert_source_parses("val text = \"tab=\\t unicode=\\u004b\"\n");
    assert_source_has_syntax_error("val invalid = \"\\q\"\n");
}

#[test]
fn ks_syntax_0177_line_string_expression_start_recognizes_dollar_brace() {
    assert_source_parses("val name = \"sample\"\nval text = \"${name.length}\"\n");
}

#[test]
fn ks_syntax_0178_triple_quote_switches_multiline_mode() {
    assert_source_parses(
        r##"val name = "sample"
val message = """path\segment
Hello, $name: ${name.length}"""
"##,
    );
}

#[test]
fn ks_syntax_0179_triple_quote_close_accepts_preceding_quote_sequence() {
    assert_source_parses(
        r####"val text = """value"""""
"####,
    );
}

#[test]
fn ks_syntax_0180_multiline_string_quote_accepts_quote_run() {
    assert_source_parses(
        r####"val text = """a "" quote run"""
"####,
    );
}

#[test]
fn ks_syntax_0181_multiline_string_reference_accepts_field_identifier() {
    assert_source_parses(
        r##"val name = "sample"
val text = """hello $name"""
"##,
    );
}

#[test]
fn ks_syntax_0182_multiline_string_text_preserves_backslash_newline_dollar() {
    assert_source_parses(
        r##"val text = """path\segment
price $"""
"##,
    );
}

#[test]
fn ks_syntax_0183_multiline_expression_start_recognizes_dollar_brace() {
    assert_source_parses(
        r##"val name = "sample"
val text = """${name.length}"""
"##,
    );
}

#[test]
fn ks_syntax_0184_syntax_grammar_ignores_hidden_tokens() {
    let compact = parse_kotlin_source("val answer=42\n");
    let separated = parse_kotlin_source("/* lead */ val\tanswer /* type */ = // value\n42\n");

    assert!(!compact.root_node().has_error());
    assert!(!separated.root_node().has_error());
    assert_eq!(
        count_nodes_of_kind(&compact, crate::queries::KIND_PROP_DECL),
        count_nodes_of_kind(&separated, crate::queries::KIND_PROP_DECL)
    );
}

#[test]
fn ks_syntax_0185_kotlin_token_covers_representative_families() {
    assert_source_parses(
        r#"#!/usr/bin/env kotlin
package sample.tokens

/* comment */
class Box<T>(val value: T?) {
    fun render(input: Any?): String = when (input) {
        null -> "none"
        is String -> "text: $input"
        else -> "${value ?: input}"
    }
}
"#,
    );
}

#[test]
fn ks_syntax_0186_eof_recognizes_input_end() {
    assert_source_parses("");
    assert_source_parses("val finalDeclaration = 1");
}

#[test]
fn ks_syntax_0361_kdoc_comment_uses_documentation_delimiters() {
    assert_source_parses(
        "/**\n * Renders a neutral item.\n * @param value item value\n */\nfun render(value: String) = value\n",
    );
    assert_source_has_syntax_error("/** unterminated documentation\nfun hidden() = Unit\n");
}
