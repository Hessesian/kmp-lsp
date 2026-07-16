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
fn ks_11_1_1_001_implicit_receivers_are_available_in_nested_classifier_extension_and_lambda_scopes()
{
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
fn ks_11_1_2_001_functions_accept_fully_qualified_explicit_infix_operator_and_unqualified_calls() {
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
