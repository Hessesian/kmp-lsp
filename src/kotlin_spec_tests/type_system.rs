use super::{assert_source_has_syntax_error, assert_source_parses};
use crate::backend::cursor::CursorContext;
use crate::features::definition::find_definition;
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{GotoDefinitionResponse, Location, Position, Url};

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

async fn definition_locations(source: &str, needle: &str, occurrence: usize) -> Vec<Location> {
    let specification_uri = Url::parse("file:///kotlin-spec/TypeContexts.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);
    let position = position_of_occurrence(source, needle, occurrence);
    let cursor_context = CursorContext::build(&indexer, &specification_uri, position)
        .expect("fixture cursor must select an identifier");

    match find_definition(&cursor_context, &indexer, &specification_uri, position).await {
        Some(GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(GotoDefinitionResponse::Array(locations)) => locations,
        Some(GotoDefinitionResponse::Link(_)) => {
            panic!("kmp-lsp definition feature returns locations, not location links")
        }
        None => Vec::new(),
    }
}

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

#[test]
fn ks_2_1_6_001_function_type_has_argument_and_return_types() {
    assert_source_parses(
        "val transform: (String, Int) -> Boolean = { value, count -> value.length == count }\n",
    );
}

#[test]
fn ks_2_1_6_004_function_type_with_receiver_has_receiver_arguments_and_return() {
    assert_source_parses("val render: String.(Int) -> Boolean = { count -> length == count }\n");
}

#[test]
fn ks_2_1_6_008_suspending_function_type_uses_suspend_modifier() {
    assert_source_parses("val load: suspend (String) -> Int = { value -> value.length }\n");
}

#[test]
fn ks_2_1_7_002_flexible_types_cannot_be_declared_explicitly() {
    assert_source_parses("val ordinary: String? = null\n");
    assert_source_has_syntax_error("val flexible: (String..String?) = null\n");
}

#[test]
fn ks_2_1_8_002_nullable_type_uses_question_marks_and_ignores_redundancy() {
    assert_source_parses("val once: String? = null\nval repeated: String?? = null\n");
}

#[test]
fn ks_2_1_8_006_definitely_non_nullable_type_uses_type_parameter_and_any() {
    assert_source_parses("fun <Element> require(value: Element?): Element & Any = value!!\n");
}

#[test]
#[ignore = "KS-2.1.9-001: kmp-lsp accepts arbitrary intersection types as definitely non-nullable syntax"]
fn ks_2_1_9_001_arbitrary_intersection_types_cannot_be_declared() {
    assert_source_parses("fun <Element> require(value: Element?): Element & Any = value!!\n");
    assert_source_has_syntax_error("val invalid: String & CharSequence = TODO()\n");
}

#[test]
fn ks_2_1_11_001_union_types_cannot_be_declared() {
    assert_source_parses("val ordinary: Any = TODO()\n");
    assert_source_has_syntax_error("val invalid: String | Int = TODO()\n");
}

#[tokio::test]
#[ignore = "KS-2.2.1-001: kmp-lsp does not index parent type parameters for inner-class definition lookup"]
async fn ks_2_2_1_001_inner_declaration_captures_parent_type_parameter() {
    let source = "class Envelope<EnvelopeElementSpec> {\n    inner class Content(val value: EnvelopeElementSpec)\n}\nclass EnvelopeElementSpec\n";
    let locations = definition_locations(source, "EnvelopeElementSpec", 1).await;

    assert_eq!(
        locations.len(),
        1,
        "the inner type use must have one target"
    );
    assert_eq!(locations[0].range.start, Position::new(0, 15));
}

#[tokio::test]
async fn ks_2_2_1_002_nested_declaration_does_not_capture_parent_type_parameter() {
    let source = "class Envelope<EnvelopeElementSpec> {\n    class Content(val value: EnvelopeElementSpec)\n}\nclass EnvelopeElementSpec\n";
    let locations = definition_locations(source, "EnvelopeElementSpec", 1).await;

    assert_eq!(
        locations.len(),
        1,
        "the nested type use must have one target"
    );
    assert_eq!(locations[0].range.start, Position::new(3, 6));
}

#[test]
fn ks_2_3_1_002_explicit_classifier_is_indexed_as_subtype_of_each_supertype() {
    let source = "interface RenderableSpec\ninterface MisleadingRenderableSpec\nclass ScreenSpec : RenderableSpec\n";
    let specification_uri = Url::parse("file:///kotlin-spec/ExplicitSubtyping.kt")
        .expect("specification fixture URI must be valid");
    let indexer = Indexer::new();
    indexer.index_content(&specification_uri, source);

    let locations = indexer.subtypes_of("RenderableSpec");
    assert_eq!(locations.len(), 1, "only the explicit subtype must match");
    assert_eq!(locations[0].uri, specification_uri);
    assert_eq!(locations[0].range.start, Position::new(2, 6));
    assert!(indexer.subtypes_of("MisleadingRenderableSpec").is_empty());
}
