use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::{live_tree::parse_live, Indexer};
use crate::semantic_tokens::{full_tokens, TOKEN_MODIFIERS};
use crate::Language;
use tower_lsp::lsp_types::{
    GotoDefinitionResponse, Position, SemanticTokenModifier, SemanticTokens, Url,
};

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
    let specification_uri =
        Url::parse("file:///kotlin-spec/Evolution.kt").expect("specification URI must be valid");
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

fn decode_semantic_tokens(tokens: &SemanticTokens) -> Vec<(Position, u32)> {
    let mut line = 0;
    let mut character = 0;
    tokens
        .data
        .iter()
        .map(|token| {
            line += token.delta_line;
            if token.delta_line > 0 {
                character = token.delta_start;
            } else {
                character += token.delta_start;
            }
            (Position::new(line, character), token.token_modifiers_bitset)
        })
        .collect()
}

fn semantic_token_modifiers_at(source: &str, needle: &str, occurrence: usize) -> u32 {
    let specification_uri = Url::parse("file:///kotlin-spec/EvolutionSemanticTokens.kt")
        .expect("specification URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let document = parse_live(source, tree_sitter_kotlin::language())
        .expect("evolution fixture must produce a live Kotlin document");
    let position = position_of_occurrence(source, needle, occurrence);
    decode_semantic_tokens(&full_tokens(
        &indexer,
        &specification_uri,
        &document,
        Language::Kotlin,
    ))
    .into_iter()
    .find_map(|(token_position, modifiers)| (token_position == position).then_some(modifiers))
    .expect("fixture identifier must receive a semantic token")
}

fn deprecated_modifier_bit() -> u32 {
    let modifier_index = TOKEN_MODIFIERS
        .iter()
        .position(|modifier| modifier == &SemanticTokenModifier::DEPRECATED)
        .expect("semantic-token legend must contain the deprecated modifier");
    1 << modifier_index
}

#[test]
#[ignore = "KL-1-9-0001: tree-sitter-kotlin accepts class literals without a left-hand side"]
fn kl_1_9_0001_class_literal_requires_a_left_hand_side() {
    assert_source_parses("val validSpec = String::class\n");
    assert_source_has_syntax_error("val invalidSpec = ::class\n");
}

#[tokio::test]
async fn kl_1_9_0002_callable_reference_keeps_target_when_expected_type_conflicts() {
    let source = "class ReferencedSpec\nclass ExpectedSpec\nval invalidSpec: ExpectedSpec = ::ReferencedSpec\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "ReferencedSpec", 1).await,
        Some(position_of_occurrence(source, "ReferencedSpec", 0))
    );
}

#[test]
#[ignore = "KL-1-9-0003: kmp-lsp does not diagnose callable references to enum entries"]
fn kl_1_9_0003_enum_entry_cannot_be_used_as_a_callable_reference() {
    assert_source_parses(
        "class ValidSpec {\n    fun memberSpec(): Unit {}\n}\nval validReferenceSpec = ValidSpec::memberSpec\n",
    );
    assert_source_has_syntax_error(
        "enum class StateSpec { ReadySpec }\nval invalidReferenceSpec = StateSpec::ReadySpec\n",
    );
}

#[tokio::test]
#[ignore = "KL-1-9-0008: kmp-lsp resolves synthetic enum entries to the competing companion property"]
async fn kl_1_9_0008_synthetic_enum_entries_precedes_companion_entries() {
    let source = "enum class StateSpec {\n    ReadySpec;\n    companion object {\n        val entries: String = \"companion\"\n    }\n}\nval selectedSpec = StateSpec.entries\nval companionSpec = StateSpec.Companion.entries\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "entries", 2).await,
        Some(position_of_occurrence(source, "entries", 0)),
        "the explicit companion path must resolve to the companion declaration"
    );
    assert_eq!(
        definition_position(source, "entries", 1).await,
        None,
        "the synthetic enum property must not resolve to the companion declaration"
    );
}

#[test]
#[ignore = "KL-1-9-0004: kmp-lsp does not propagate deprecation to enum-entry reference tokens"]
fn kl_1_9_0004_deprecated_enum_entry_reference_has_deprecated_semantic_token() {
    let source = "enum class StateSpec {\n    @Deprecated(\"legacy\") LegacySpec,\n    CurrentSpec\n}\nval legacySpec = StateSpec.LegacySpec\nval currentSpec = StateSpec.CurrentSpec\n";
    assert_source_parses(source);
    let deprecated_bit = deprecated_modifier_bit();
    let legacy_modifiers = semantic_token_modifiers_at(source, "LegacySpec", 1);
    let current_modifiers = semantic_token_modifiers_at(source, "CurrentSpec", 1);
    assert_ne!(legacy_modifiers & deprecated_bit, 0);
    assert_eq!(current_modifiers & deprecated_bit, 0);
}

#[test]
#[ignore = "KL-1-9-0005: kmp-lsp does not diagnose named arguments on function-type calls"]
fn kl_1_9_0005_function_type_call_forbids_named_arguments() {
    assert_source_parses(
        "fun validSpec(callbackSpec: (valueSpec: String) -> Unit) { callbackSpec(\"value\") }\n",
    );
    assert_source_has_syntax_error(
        "fun invalidSpec(callbackSpec: (valueSpec: String) -> Unit) { callbackSpec(valueSpec = \"value\") }\n",
    );
}

#[test]
fn kl_1_9_0006_extension_function_type_is_forbidden_as_a_supertype() {
    assert_source_parses("class ValidSpec : () -> Unit {\n    override fun invoke(): Unit {}\n}\n");
    assert_source_has_syntax_error(
        "class InvalidSpec : String.() -> Unit { override fun invoke(): Unit {} }\n",
    );
}

#[tokio::test]
#[ignore = "KL-1-9-0007: kmp-lsp does not resolve a companion property competing with a type parameter"]
async fn kl_1_9_0007_type_parameter_name_is_not_a_value_expression() {
    let source = "class OwnerSpec<valueSpec> {\n    companion object {\n        val valueSpec: Int = 1\n    }\n    inner class InnerSpec<valueSpec> {\n        val selectedSpec = valueSpec\n    }\n}\nval valueSpec: Int = 2\n";
    assert_source_parses(source);
    assert_eq!(
        definition_position(source, "valueSpec", 3).await,
        Some(position_of_occurrence(source, "valueSpec", 1))
    );
}
