use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{GotoDefinitionResponse, Position, Url};

fn position_of_occurrence(source: &str, needle: &str, occurrence: usize) -> Position {
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

async fn definition_position(source: &str, needle: &str, occurrence: usize) -> Option<Position> {
    let specification_uri = Url::parse("file:///kotlin-spec/OverloadResolution.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let position = position_of_occurrence(source, needle, occurrence);
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
fn ks_11_1_1_001_implicit_receivers_are_available_in_nested_receiver_scopes() {
    assert_source_parses(
        "class OuterSpec {\n    fun String.extensionSpec(blockSpec: String.() -> Unit) {\n        length\n        blockSpec()\n        run { this@OuterSpec }\n    }\n}\n",
    );
}

#[tokio::test]
#[ignore = "KS-11.1.1-002: kmp-lsp does not resolve unqualified implicit-receiver properties"]
async fn ks_11_1_1_002_innermost_implicit_receiver_has_higher_priority() {
    let source = "class OuterSpec {\n    val selectedSpec: Int = 1\n    inner class InnerSpec {\n        val selectedSpec: Int = 2\n        fun readSpec(): Int = selectedSpec\n    }\n}\n";
    assert_eq!(
        definition_position(source, "selectedSpec", 2).await,
        Some(Position::new(3, 12))
    );
}

#[test]
fn ks_11_1_2_001_functions_accept_all_specified_call_forms() {
    assert_source_parses(
        "infix fun Int.combineSpec(otherSpec: Int): Int = this + otherSpec\nfun callFormsSpec(valueSpec: Int) {\n    kotlin.io.println(valueSpec)\n    valueSpec.toString()\n    valueSpec combineSpec 2\n    valueSpec + 2\n    println(valueSpec)\n}\n",
    );
}

#[test]
fn ks_11_1_3_001_property_like_callable_uses_invoke_with_forwarded_arguments() {
    assert_source_parses(
        "class CallableSpec {\n    operator fun invoke(valueSpec: Int, blockSpec: () -> Unit): String { blockSpec(); return valueSpec.toString() }\n}\nval callableSpec = CallableSpec()\nfun invokeSpec() = callableSpec(1) { println(\"called\") }\n",
    );
}

#[test]
#[ignore = "KS-11.1.3-002: kmp-lsp does not require operator on invoke conventions"]
fn ks_11_1_3_002_invoke_convention_requires_the_operator_modifier() {
    assert_source_parses(
        "class ValidSpec {\n    operator fun invoke(): Unit {}\n}\nfun validSpec() { ValidSpec()() }\n",
    );
    assert_source_has_syntax_error(
        "class InvalidSpec {\n    fun invoke(): Unit {}\n}\nfun invalidSpec() { InvalidSpec()() }\n",
    );
}

#[tokio::test]
#[ignore = "KS-11.1.4-001: kmp-lsp does not resolve function-versus-property callable partitions"]
async fn ks_11_1_4_001_function_like_callable_precedes_property_like_callable() {
    let source = "class CallableSpec {\n    operator fun invoke(valueSpec: Int): String = valueSpec.toString()\n}\nfun chooseSpec(valueSpec: Int): String = \"function\"\nval chooseSpec = CallableSpec()\nfun useSpec(): String = chooseSpec(1)\n";
    assert_eq!(
        definition_position(source, "chooseSpec", 2).await,
        Some(Position::new(3, 4))
    );
}

#[tokio::test]
async fn ks_11_2_1_001_fully_qualified_call_resolves_top_level_callable() {
    let source = "package candidate.spec\nfun selectSpec(valueSpec: Int): Int = valueSpec\nval resultSpec = candidate.spec.selectSpec(1)\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "selectSpec", 1).await,
        Some(position_of_occurrence(source, "selectSpec", 0))
    );
}

#[tokio::test]
async fn ks_11_2_2_001_non_extension_member_precedes_extension_candidates() {
    let source = "class ReceiverSpec {\n    fun selectSpec(valueSpec: Int): String = \"member\"\n}\nfun ReceiverSpec.selectSpec(valueSpec: String): String = \"extension\"\nfun useSpec(receiverSpec: ReceiverSpec): String = receiverSpec.selectSpec(1)\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "selectSpec", 2).await,
        Some(position_of_occurrence(source, "selectSpec", 0))
    );
}

#[tokio::test]
#[ignore = "KS-11.2.2-002: kmp-lsp does not resolve local extension callables"]
async fn ks_11_2_2_002_local_extension_precedes_package_extension() {
    let source = "fun String.selectSpec(valueSpec: Any): String = \"package\"\nfun useSpec(): String {\n    fun String.selectSpec(valueSpec: Int): String = \"local\"\n    return \"receiver\".selectSpec(1)\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "selectSpec", 2).await,
        Some(position_of_occurrence(source, "selectSpec", 1))
    );
}

#[test]
fn ks_11_2_2_003_explicit_type_receiver_accepts_static_like_enum_calls() {
    assert_source_parses(
        "enum class StateSpec { ReadySpec, DoneSpec }\nval statesSpec = StateSpec.values()\nval readySpec = StateSpec.valueOf(\"ReadySpec\")\n",
    );
}

#[test]
fn ks_11_2_2_004_explicit_extended_super_receiver_is_accepted() {
    assert_source_parses(
        "interface FirstSpec { fun renderSpec(): String = \"first\"; }\ninterface SecondSpec { fun renderSpec(): String = \"second\"; }\nclass HostSpec : FirstSpec, SecondSpec {\n    override fun renderSpec(): String = super<FirstSpec>.renderSpec()\n}\n",
    );
}

#[test]
#[ignore = "KS-11.2.3-001: kmp-lsp does not require infix modifiers for infix calls"]
fn ks_11_2_3_001_infix_candidate_requires_infix_modifier() {
    assert_source_parses(
        "infix fun Int.combineSpec(otherSpec: Int): Int = this + otherSpec\nval validSpec = 1 combineSpec 2\n",
    );
    assert_source_has_syntax_error(
        "fun Int.combineSpec(otherSpec: Int): Int = this + otherSpec\nval invalidSpec = 1 combineSpec 2\n",
    );
}

#[test]
#[ignore = "KS-11.2.4-001: kmp-lsp does not require operator modifiers for operator calls"]
fn ks_11_2_4_001_operator_candidate_requires_operator_modifier() {
    assert_source_parses(
        "class NumberSpec { operator fun plus(otherSpec: NumberSpec): NumberSpec = this; }\nval validSpec = NumberSpec() + NumberSpec()\n",
    );
    assert_source_has_syntax_error(
        "class NumberSpec { fun plus(otherSpec: NumberSpec): NumberSpec = this; }\nval invalidSpec = NumberSpec() + NumberSpec()\n",
    );
}

#[tokio::test]
#[ignore = "KS-11.2.5-001: kmp-lsp does not resolve local callables at call sites"]
async fn ks_11_2_5_001_local_callable_precedes_top_level_callable() {
    let source = "fun selectSpec(valueSpec: Any): String = \"top-level\"\nfun useSpec(): String {\n    fun selectSpec(valueSpec: Int): String = \"local\"\n    return selectSpec(1)\n}\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "selectSpec", 2).await,
        Some(position_of_occurrence(source, "selectSpec", 1))
    );
}

#[tokio::test]
#[ignore = "KS-11.2.6-001: kmp-lsp does not filter overloads by named arguments"]
async fn ks_11_2_6_001_named_argument_filters_candidates_by_parameter_name() {
    let source = "fun selectSpec(numberSpec: Int): String = \"number\"\nfun selectSpec(textSpec: String): String = \"text\"\nval resultSpec = selectSpec(textSpec = \"value\")\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "selectSpec", 2).await,
        Some(position_of_occurrence(source, "selectSpec", 1))
    );
}

#[tokio::test]
async fn ks_11_2_7_001_trailing_lambda_keeps_callable_resolution() {
    let source = "fun applySpec(valueSpec: Int, blockSpec: () -> Unit): Unit = blockSpec()\nfun useSpec() {\n    applySpec(1, blockSpec = {})\n    applySpec(1) {}\n}\n";
    assert_source_parses(source);
    let declaration_position = Some(position_of_occurrence(source, "applySpec", 0));
    assert_eq!(
        definition_position(source, "applySpec", 1).await,
        declaration_position
    );
    assert_eq!(
        definition_position(source, "applySpec", 2).await,
        declaration_position
    );
}

#[tokio::test]
#[ignore = "KS-11.2.8-001: kmp-lsp does not filter overloads by explicit type-argument count"]
async fn ks_11_2_8_001_explicit_type_arguments_filter_by_type_parameter_count() {
    let source = "fun selectSpec(valueSpec: Int): String = \"plain\"\nfun <ValueSpec> selectSpec(valueSpec: ValueSpec): String = \"generic\"\nval resultSpec = selectSpec<Int>(1)\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "selectSpec", 2).await,
        Some(position_of_occurrence(source, "selectSpec", 1))
    );
}
